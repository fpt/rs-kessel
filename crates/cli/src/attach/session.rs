//! Session files: how `kessel play` finds a running `kessel mcp`.
//!
//! Each server writes one small JSON file into the user's cache directory,
//! naming the loopback port it listens on. Discovery prefers a session whose
//! root matches where you are, then falls back to the only live one.
//!
//! Liveness is decided by *connecting*, not by a pid check: a pid can be reused
//! and a crashed server leaves its file behind, so the socket is the only honest
//! answer. Stale files are removed when we find them.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A running `kessel mcp`, as advertised on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Loopback port the attach listener accepts on.
    pub port: u16,
    /// Absolute working directory the console is rooted at.
    pub root: String,
    pub pid: u32,
    pub version: String,
}

/// Directory holding session files. Honours `KESSEL_SESSION_DIR` (tests use it),
/// then the platform cache dir, then a temp-dir fallback so discovery still
/// works on a system with no `HOME`.
pub fn session_dir() -> PathBuf {
    // An empty value means "unset" — an exported-but-empty variable would
    // otherwise resolve to the relative path "", where nothing is ever found.
    if let Some(dir) = std::env::var("KESSEL_SESSION_DIR")
        .ok()
        .filter(|d| !d.is_empty())
    {
        return PathBuf::from(dir);
    }
    let base = if cfg!(windows) {
        std::env::var("LOCALAPPDATA").ok().map(PathBuf::from)
    } else {
        std::env::var("XDG_CACHE_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".cache"))
            })
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("kessel")
        .join("sessions")
}

/// FNV-1a of the canonical root path — a short, stable, filesystem-safe name.
/// Deep project paths would blow past filename limits if used verbatim, and the
/// root is stored inside the file anyway for verification.
fn root_key(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in canonical.to_string_lossy().as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Session {
    /// Advertise a server in `dir`. Returns the path written so it can be
    /// cleaned up. The directory is a parameter rather than read from the
    /// environment so tests can each use their own without racing on a
    /// process-wide variable; callers pass [`session_dir`].
    pub fn publish_in(dir: &Path, root: &Path, port: u16) -> io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let session = Session {
            port,
            root: canonical.display().to_string(),
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
        };
        let key = root_key(root);
        let path = dir.join(format!("{key}.json"));

        // Write-then-rename rather than writing in place: `discover` reads these
        // concurrently, and a reader that caught a half-written file would see
        // the session as absent. Rename within one directory is atomic.
        //
        // This replaces an existing file on every platform we support, Windows
        // included: `std::fs::rename` is documented to replace the destination,
        // and uses `MoveFileExW`/`SetFileInformationByHandle` — not the Win32
        // `MoveFile` that refuses an existing target, which is where the
        // folklore comes from. A concurrent reader doesn't block it either,
        // since Rust opens files with `FILE_SHARE_DELETE` by default.
        let tmp = dir.join(format!("{key}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(&session)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Remove a published session, but only if the file still describes *this*
    /// server.
    ///
    /// Two servers rooted at the same directory share a filename, so the second
    /// to start overwrites the first's advertisement. Without this check the
    /// first one's exit would then delete the *second's* live entry, and
    /// discovery would lose a session whose listener is still up.
    pub fn unpublish(path: &Path, port: u16) -> io::Result<()> {
        match Session::load(path) {
            Some(s) if s.port == port && s.pid == std::process::id() => std::fs::remove_file(path),
            // Someone else's now (or already gone) — leave it alone.
            _ => Ok(()),
        }
    }

    fn load(path: &Path) -> Option<Session> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Every session file in `dir`, live or not.
    #[cfg(any(feature = "play", test))]
    pub fn list_in(dir: &Path) -> Vec<(PathBuf, Session)> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(s) = Session::load(&path) {
                out.push((path, s));
            }
        }
        out
    }
}

/// Why discovery failed, phrased for someone at a terminal.
///
/// Discovery is the *player's* half of the feature — a headless `kessel mcp`
/// publishes a session but never looks for one — so it compiles with the window.
#[cfg(any(feature = "play", test))]
pub enum Discovery {
    Found(Session),
    None,
    /// More than one live server and no way to tell which was meant.
    Ambiguous(Vec<Session>),
}

/// Find the server to attach to.
///
/// `root` (when given) picks a specific one. Otherwise prefer a session rooted
/// at the current directory — the overwhelmingly common case, since you run
/// `kessel play` from the project you're working in — and fall back to the only
/// live session if there is exactly one.
#[cfg(feature = "play")]
pub fn discover(root: Option<&Path>, is_live: impl Fn(&Session) -> bool) -> Discovery {
    discover_in(&session_dir(), root, is_live)
}

/// As [`discover`], over an explicit session directory.
#[cfg(any(feature = "play", test))]
pub fn discover_in(
    dir: &Path,
    root: Option<&Path>,
    is_live: impl Fn(&Session) -> bool,
) -> Discovery {
    let mut live = Vec::new();
    for (path, session) in Session::list_in(dir) {
        if is_live(&session) {
            live.push(session);
        } else {
            // A crashed server's leftovers would otherwise shadow a real one.
            let _ = std::fs::remove_file(&path);
        }
    }

    if let Some(root) = root {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let target = canonical.display().to_string();
        return match live.into_iter().find(|s| s.root == target) {
            Some(s) => Discovery::Found(s),
            None => Discovery::None,
        };
    }

    if let Ok(cwd) = std::env::current_dir() {
        let canonical = cwd.canonicalize().unwrap_or(cwd);
        let target = canonical.display().to_string();
        if let Some(s) = live.iter().find(|s| s.root == target) {
            return Discovery::Found(s.clone());
        }
    }

    match live.len() {
        0 => Discovery::None,
        1 => Discovery::Found(live.remove(0)),
        _ => Discovery::Ambiguous(live),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private session directory per test. Explicit rather than via
    /// `KESSEL_SESSION_DIR`, because that variable is process-wide and these
    /// tests run in parallel.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kessel-sess-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn publish_then_discover_by_root() {
        let dir = scratch("by-root");
        let root = dir.join("proj");
        std::fs::create_dir_all(&root).unwrap();

        Session::publish_in(&dir, &root, 4321).unwrap();
        match discover_in(&dir, Some(&root), |_| true) {
            Discovery::Found(s) => {
                assert_eq!(s.port, 4321);
                assert_eq!(s.pid, std::process::id());
            }
            _ => panic!("should have found the published session"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Asking for a root that has no server must not silently hand back some
    /// other project's session.
    #[test]
    fn a_root_with_no_server_finds_nothing() {
        let dir = scratch("wrong-root");
        let mine = dir.join("mine");
        let theirs = dir.join("theirs");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();

        Session::publish_in(&dir, &theirs, 4321).unwrap();
        assert!(matches!(
            discover_in(&dir, Some(&mine), |_| true),
            Discovery::None
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A server that died leaves its file behind; discovery must treat it as
    /// absent *and* clean it up, or it shadows the next real one forever.
    #[test]
    fn dead_sessions_are_ignored_and_removed() {
        let dir = scratch("dead");
        let root = dir.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let path = Session::publish_in(&dir, &root, 4321).unwrap();

        assert!(matches!(
            discover_in(&dir, Some(&root), |_| false),
            Discovery::None
        ));
        assert!(!path.exists(), "stale session file should be deleted");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_single_live_session_is_used_without_a_root() {
        let dir = scratch("single");
        let root = dir.join("only");
        std::fs::create_dir_all(&root).unwrap();
        Session::publish_in(&dir, &root, 5555).unwrap();

        match discover_in(&dir, None, |_| true) {
            Discovery::Found(s) => assert_eq!(s.port, 5555),
            _ => panic!("the only live session should be chosen"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two servers and no root given: refuse rather than guess, since attaching
    /// to the wrong agent's game is confusing and silent.
    #[test]
    fn two_live_sessions_are_ambiguous() {
        let dir = scratch("ambiguous");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        Session::publish_in(&dir, &a, 6001).unwrap();
        Session::publish_in(&dir, &b, 6002).unwrap();

        match discover_in(&dir, None, |_| true) {
            Discovery::Ambiguous(v) => assert_eq!(v.len(), 2),
            Discovery::Found(_) => panic!("neither session is the cwd, so this is ambiguous"),
            Discovery::None => panic!("both sessions are live"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Ambiguity is resolved in favour of where you are, which is why running
    /// `kessel play` from your project just works.
    #[test]
    fn the_cwd_session_wins_over_another_live_one() {
        let dir = scratch("cwd-wins");
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let cwd = std::env::current_dir().unwrap();

        Session::publish_in(&dir, &other, 7001).unwrap();
        Session::publish_in(&dir, &cwd, 7002).unwrap();

        match discover_in(&dir, None, |_| true) {
            Discovery::Found(s) => assert_eq!(s.port, 7002, "should prefer the cwd's session"),
            _ => panic!("the cwd session should have been chosen"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An exported-but-empty override must fall back, not resolve to the
    /// relative path "" where nothing is ever found.
    #[test]
    fn an_empty_override_falls_back() {
        let prev = std::env::var("KESSEL_SESSION_DIR").ok();
        std::env::set_var("KESSEL_SESSION_DIR", "");
        let dir = session_dir();
        assert!(
            dir.is_absolute() || dir.components().count() > 1,
            "got {dir:?}"
        );
        assert!(dir.ends_with("kessel/sessions"), "got {dir:?}");
        match prev {
            Some(v) => std::env::set_var("KESSEL_SESSION_DIR", v),
            None => std::env::remove_var("KESSEL_SESSION_DIR"),
        }
    }

    #[test]
    fn distinct_roots_get_distinct_files() {
        let dir = scratch("keys");
        let a = dir.join("one");
        let b = dir.join("two");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_ne!(root_key(&a), root_key(&b));
        // ...and the same root is stable across calls.
        assert_eq!(root_key(&a), root_key(&a));
        std::fs::remove_dir_all(&dir).ok();
    }
}
