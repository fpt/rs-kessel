//! Persistent, on-disk game **projects** — the workspace the agent develops in.
//!
//! The `vm_*` tools are per-call primitives with no memory: sources and ROMs
//! lived in `HashMap`s that vanished on restart, invisible to `kessel --play`,
//! to a human editor, and to the backend's own file tools. A project puts the
//! durable half of game development on disk instead:
//!
//! ```text
//! <root>/
//!   kessel-project.json    name + creation time
//!   game.lua               the working source (edited by the backend's file tools)
//!   design.md              concept + current spec
//!   tasks.json             open / closed tasks
//!   playtest.jsonl         append-only journal: user feedback, playtest runs
//!   assets/ tests/ revisions/ snapshots/
//! ```
//!
//! **The filesystem is the source of truth for game source.** The backend edits
//! `game.lua` with its own write/edit tools and then asks the VM to run it, so
//! opening a project points [`VmConsole`] at the same directory (see
//! [`VmConsole::set_root`]) rather than keeping a private copy that can drift.
//!
//! A project is opened explicitly — `KESSEL_PROJECT` at startup, or the
//! `project_new`/`project_open` tools mid-session. With none open the VM keeps
//! its in-memory behaviour, so `VmPlayer` and the test suites are unaffected.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::vm::VmConsole;

pub mod tools;

/// Project metadata file, at the project root.
pub const META_FILE: &str = "kessel-project.json";
/// Concept + current spec, in Markdown.
pub const DESIGN_FILE: &str = "design.md";
/// Open/closed task list.
pub const TASKS_FILE: &str = "tasks.json";
/// Append-only journal of development events (one JSON object per line).
pub const JOURNAL_FILE: &str = "playtest.jsonl";
/// The source a project builds unless told otherwise.
pub const DEFAULT_SOURCE: &str = "game.lua";
/// Sub-directories every project has (created on open, empty is fine).
pub const SUBDIRS: [&str; 4] = ["assets", "tests", "revisions", "snapshots"];

/// Seconds since the Unix epoch (0 if the clock is before it).
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Format a Unix timestamp as an ISO-8601 UTC string, so journal lines and
/// metadata stay readable to a human opening the file (no date-time dependency
/// in this crate).
pub fn iso_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Days since 1970-01-01 → civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_civil = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m_civil <= 2);
    format!("{y:04}-{m_civil:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Resolve a caller-supplied relative path against `root`, refusing anything
/// that could escape it (absolute paths, `..`, Windows prefixes). Project tools
/// take paths from the model, so containment is checked, not assumed.
pub fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.trim().is_empty() {
        return Err("path is empty".to_string());
    }
    if p.is_absolute() {
        return Err(format!("path '{rel}' must be relative to the project root"));
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("path '{rel}' must stay inside the project")),
        }
    }
    Ok(root.join(p))
}

/// The default parent directory for projects created by name:
/// `$KESSEL_PROJECTS_DIR`, else `~/kessel/projects`.
pub fn default_projects_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KESSEL_PROJECTS_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join("kessel").join("projects")
}

/// Expand a leading `~` and make the path absolute, so every project root the
/// tools report back is unambiguous for the backend's own file tools.
pub fn expand_path(input: &str) -> PathBuf {
    let trimmed = input.trim();
    let expanded = if trimmed == "~" || trimmed.starts_with("~/") {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "~".to_string());
        PathBuf::from(home).join(trimmed.trim_start_matches("~").trim_start_matches('/'))
    } else {
        PathBuf::from(trimmed)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or(expanded)
    }
}

// ============================================================================
// Metadata, tasks
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub created_unix: u64,
    /// The source `vm_assemble` builds unless another path is given.
    #[serde(default = "default_main_source")]
    pub main_source: String,
}

fn default_main_source() -> String {
    DEFAULT_SOURCE.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: u32,
    pub text: String,
    pub done: bool,
    pub created_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_unix: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskList {
    #[serde(default)]
    pub next_id: u32,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

impl TaskList {
    pub fn open_tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(|t| !t.done)
    }
}

// ============================================================================
// Project
// ============================================================================

/// One opened project. Cheap to clone the handle (`Arc<Project>`); all state
/// beyond the metadata lives in files, so concurrent readers see whatever is on
/// disk rather than a cached snapshot that could drift from the editor.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    meta: ProjectMeta,
}

impl Project {
    /// Open `root`, creating the layout if it is missing. An existing directory
    /// without a `kessel-project.json` is **adopted** (its basename becomes the
    /// project name) so an ordinary folder of `.lua` files can be picked up as
    /// a project without moving anything.
    pub fn open_or_create(root: &Path, name: Option<&str>) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| format!("create '{}': {e}", root.display()))?;
        for dir in SUBDIRS {
            let p = root.join(dir);
            fs::create_dir_all(&p).map_err(|e| format!("create '{}': {e}", p.display()))?;
        }

        let meta_path = root.join(META_FILE);
        let meta = match fs::read_to_string(&meta_path) {
            Ok(text) => {
                let mut meta: ProjectMeta = serde_json::from_str(&text)
                    .map_err(|e| format!("parse '{}': {e}", meta_path.display()))?;
                if let Some(n) = name {
                    meta.name = n.to_string();
                }
                meta
            }
            Err(_) => ProjectMeta {
                name: name
                    .map(String::from)
                    .or_else(|| {
                        root.file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .filter(|s| !s.is_empty())
                    })
                    .unwrap_or_else(|| "game".to_string()),
                created_unix: now_unix(),
                main_source: default_main_source(),
            },
        };

        let project = Project {
            root: root.to_path_buf(),
            meta,
        };
        project.write_meta()?;
        if !project.design_path().exists() {
            project.write_design(&starter_design(&project.meta.name))?;
        }
        Ok(project)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> &str {
        &self.meta.name
    }

    /// The source the project builds by default (`game.lua` unless overridden).
    pub fn main_source(&self) -> &str {
        &self.meta.main_source
    }

    fn write_meta(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&self.meta)
            .map_err(|e| format!("serialize metadata: {e}"))?;
        write_file(&self.root.join(META_FILE), &format!("{text}\n"))
    }

    // ---- design ----

    pub fn design_path(&self) -> PathBuf {
        self.root.join(DESIGN_FILE)
    }

    pub fn read_design(&self) -> Result<String, String> {
        match fs::read_to_string(self.design_path()) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(format!("read '{}': {e}", self.design_path().display())),
        }
    }

    pub fn write_design(&self, content: &str) -> Result<(), String> {
        write_file(&self.design_path(), content)
    }

    // ---- tasks ----

    pub fn tasks_path(&self) -> PathBuf {
        self.root.join(TASKS_FILE)
    }

    pub fn read_tasks(&self) -> Result<TaskList, String> {
        match fs::read_to_string(self.tasks_path()) {
            Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text)
                .map_err(|e| format!("parse '{}': {e}", self.tasks_path().display())),
            Ok(_) | Err(_) => Ok(TaskList::default()),
        }
    }

    fn write_tasks(&self, list: &TaskList) -> Result<(), String> {
        let text =
            serde_json::to_string_pretty(list).map_err(|e| format!("serialize tasks: {e}"))?;
        write_file(&self.tasks_path(), &format!("{text}\n"))
    }

    /// Append a task and return it.
    pub fn add_task(&self, text: &str) -> Result<Task, String> {
        let mut list = self.read_tasks()?;
        let id = list
            .next_id
            .max(list.tasks.iter().map(|t| t.id).max().unwrap_or(0))
            + 1;
        let task = Task {
            id,
            text: text.to_string(),
            done: false,
            created_unix: now_unix(),
            closed_unix: None,
        };
        list.next_id = id;
        list.tasks.push(task.clone());
        self.write_tasks(&list)?;
        Ok(task)
    }

    /// Mark a task done (`done = true`) or reopen it. Errors if the id is unknown.
    pub fn set_task_done(&self, id: u32, done: bool) -> Result<Task, String> {
        let mut list = self.read_tasks()?;
        let task = list
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| format!("no task with id {id}"))?;
        task.done = done;
        task.closed_unix = done.then(now_unix);
        let updated = task.clone();
        self.write_tasks(&list)?;
        Ok(updated)
    }

    // ---- journal ----

    pub fn journal_path(&self) -> PathBuf {
        self.root.join(JOURNAL_FILE)
    }

    /// Append one development event. `entry` is stamped with the current time
    /// and written as a single JSON line, so the journal stays append-only and
    /// greppable.
    pub fn append_journal(&self, kind: &str, mut entry: Value) -> Result<Value, String> {
        let secs = now_unix();
        let obj = entry
            .as_object_mut()
            .ok_or_else(|| "journal entry must be a JSON object".to_string())?;
        obj.insert("kind".to_string(), json!(kind));
        obj.insert("time_unix".to_string(), json!(secs));
        obj.insert("time".to_string(), json!(iso_utc(secs)));

        let line = format!("{entry}\n");
        let path = self.journal_path();
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open '{}': {e}", path.display()))?;
        file.write_all(line.as_bytes())
            .map_err(|e| format!("append '{}': {e}", path.display()))?;
        Ok(entry)
    }

    /// The most recent `limit` journal events, oldest first. Unparsable lines
    /// are skipped rather than failing the read — the journal is a log, and a
    /// half-written line must not make the whole history unreadable.
    pub fn journal(&self, limit: usize) -> Vec<Value> {
        let text = match fs::read_to_string(self.journal_path()) {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        let mut events: Vec<Value> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if events.len() > limit {
            events.drain(..events.len() - limit);
        }
        events
    }

    pub fn journal_len(&self) -> usize {
        fs::read_to_string(self.journal_path())
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    // ---- status ----

    /// A snapshot of the project for the model: where it is, what source and
    /// assets exist, how much is outstanding, and what happened recently.
    pub fn status(&self, recent: usize) -> Value {
        let tasks = self.read_tasks().unwrap_or_default();
        let (open, done): (Vec<_>, Vec<_>) = tasks.tasks.iter().partition(|t| !t.done);
        let main = self.root.join(&self.meta.main_source);
        let design = self.read_design().unwrap_or_default();

        json!({
            "name": self.meta.name,
            "root": self.root.display().to_string(),
            "created": iso_utc(self.meta.created_unix),
            "main_source": self.meta.main_source,
            "main_source_exists": main.is_file(),
            "main_source_bytes": fs::metadata(&main).map(|m| m.len()).unwrap_or(0),
            "design_lines": design.lines().count(),
            "tasks": { "open": open.len(), "done": done.len() },
            "open_tasks": open.iter().take(10).map(|t| json!({ "id": t.id, "text": t.text })).collect::<Vec<_>>(),
            "journal_events": self.journal_len(),
            "recent_events": self.journal(recent),
            "files": self.list_files(60),
        })
    }

    /// Project-relative paths of the files a game is made of, capped at `max`.
    /// The top level plus `assets/` and `tests/` — enough for the model to see
    /// what exists without walking build output.
    pub fn list_files(&self, max: usize) -> Vec<String> {
        let mut out = Vec::new();
        let push_dir = |dir: &Path, prefix: &str, out: &mut Vec<String>| {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .map(|e| format!("{prefix}{}", e.file_name().to_string_lossy()))
                .filter(|n| !n.ends_with(META_FILE))
                .collect();
            names.sort();
            out.extend(names);
        };
        push_dir(&self.root, "", &mut out);
        push_dir(&self.root.join("assets"), "assets/", &mut out);
        push_dir(&self.root.join("tests"), "tests/", &mut out);
        out.truncate(max);
        out
    }
}

/// Write `content` to `path`, creating parent directories.
fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create '{}': {e}", parent.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("write '{}': {e}", path.display()))
}

/// The `design.md` a new project starts from — headings the agent is expected
/// to keep filled in, so "what is this game" survives between sessions.
fn starter_design(name: &str) -> String {
    format!(
        "# {name}\n\n\
         ## Concept\n\n\
         (one paragraph: what the game is and what makes it fun)\n\n\
         ## Controls\n\n\
         | button | action |\n\
         |--------|--------|\n\n\
         ## Current spec\n\n\
         - \n\n\
         ## Known issues\n\n\
         - \n"
    )
}

// ============================================================================
// ProjectStore — the one open project, shared by the tools
// ============================================================================

/// Holds the currently open project (at most one) and keeps the resident
/// [`VmConsole`] pointed at it, so `vm_assemble` compiles the same file the
/// backend's file tools just edited.
pub struct ProjectStore {
    current: Mutex<Option<Arc<Project>>>,
    console: Arc<Mutex<VmConsole>>,
}

impl ProjectStore {
    pub fn new(console: Arc<Mutex<VmConsole>>) -> Self {
        Self {
            current: Mutex::new(None),
            console,
        }
    }

    pub fn current(&self) -> Option<Arc<Project>> {
        self.current.lock().clone()
    }

    /// The open project, or an error worded for the model (it can act on it).
    pub fn require(&self) -> Result<Arc<Project>, String> {
        self.current().ok_or_else(|| {
            "no project is open — call project_open with a directory, or project_new with a name"
                .to_string()
        })
    }

    /// Open (or create) the project at `root` and make it current. Points the
    /// VM at the new root, discarding ROMs built from the previous one.
    pub fn open(&self, root: &Path, name: Option<&str>) -> Result<Arc<Project>, String> {
        let project = Arc::new(Project::open_or_create(root, name)?);
        self.console
            .lock()
            .set_root(Some(project.root().to_path_buf()));
        *self.current.lock() = Some(project.clone());
        tracing::info!(
            "project '{}' open at {}",
            project.name(),
            project.root().display()
        );
        Ok(project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (ProjectStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        (
            ProjectStore::new(Arc::new(Mutex::new(VmConsole::new()))),
            tmp,
        )
    }

    #[test]
    fn open_creates_the_layout_and_is_idempotent() {
        let (store, tmp) = store();
        let root = tmp.path().join("dodger");
        let p = store.open(&root, Some("dodger")).unwrap();

        assert_eq!(p.name(), "dodger");
        assert!(root.join(META_FILE).is_file());
        assert!(root.join(DESIGN_FILE).is_file());
        for dir in SUBDIRS {
            assert!(root.join(dir).is_dir(), "missing {dir}/");
        }

        // Reopening keeps the same identity and does not clobber edits.
        p.write_design("# dodger\n\nkeep me\n").unwrap();
        let again = store.open(&root, None).unwrap();
        assert_eq!(again.name(), "dodger");
        assert!(again.read_design().unwrap().contains("keep me"));
    }

    #[test]
    fn an_existing_directory_is_adopted_by_basename() {
        let (store, tmp) = store();
        let root = tmp.path().join("my-game");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("game.lua"), "function draw() cls(0) end").unwrap();

        let p = store.open(&root, None).unwrap();
        assert_eq!(p.name(), "my-game");
        assert!(p.list_files(20).contains(&"game.lua".to_string()));
        // Adoption must not disturb the source that was already there.
        assert!(fs::read_to_string(root.join("game.lua"))
            .unwrap()
            .contains("cls(0)"));
    }

    #[test]
    fn tasks_add_close_and_reopen() {
        let (store, tmp) = store();
        let p = store.open(&tmp.path().join("t"), None).unwrap();

        let a = p.add_task("make the enemies slower").unwrap();
        let b = p.add_task("add a title screen").unwrap();
        assert_ne!(a.id, b.id);

        p.set_task_done(a.id, true).unwrap();
        let list = p.read_tasks().unwrap();
        assert_eq!(list.open_tasks().count(), 1);
        assert!(list.tasks.iter().find(|t| t.id == a.id).unwrap().done);

        p.set_task_done(a.id, false).unwrap();
        assert_eq!(p.read_tasks().unwrap().open_tasks().count(), 2);
        assert!(p.set_task_done(999, true).is_err());
    }

    #[test]
    fn journal_appends_and_reads_back_the_tail() {
        let (store, tmp) = store();
        let p = store.open(&tmp.path().join("j"), None).unwrap();

        for i in 0..5 {
            p.append_journal(
                "playtest_feedback",
                json!({ "target": "difficulty", "n": i }),
            )
            .unwrap();
        }
        assert_eq!(p.journal_len(), 5);

        let tail = p.journal(2);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[1]["n"], 4);
        assert_eq!(tail[0]["kind"], "playtest_feedback");
        assert!(tail[0]["time"].as_str().unwrap().ends_with('Z'));

        // A corrupt line must not hide the rest of the history.
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(p.journal_path())
            .unwrap();
        f.write_all(b"{ truncated\n").unwrap();
        assert_eq!(p.journal(10).len(), 5);
    }

    #[test]
    fn state_survives_a_restart() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("persist");
        {
            let (store, _t) = (
                ProjectStore::new(Arc::new(Mutex::new(VmConsole::new()))),
                &tmp,
            );
            let p = store.open(&root, Some("persist")).unwrap();
            p.write_design("# persist\n\nA dodging game.\n").unwrap();
            p.add_task("tune difficulty").unwrap();
            p.append_journal("playtest_feedback", json!({ "sentiment": "too_hard" }))
                .unwrap();
        }
        // A fresh store — as after restarting kessel — sees all of it.
        let (store, _t) = (
            ProjectStore::new(Arc::new(Mutex::new(VmConsole::new()))),
            &tmp,
        );
        let p = store.open(&root, None).unwrap();
        assert!(p.read_design().unwrap().contains("A dodging game."));
        assert_eq!(p.read_tasks().unwrap().open_tasks().count(), 1);
        assert_eq!(p.journal(10)[0]["sentiment"], "too_hard");
    }

    #[test]
    fn opening_points_the_vm_at_the_project() {
        let console = Arc::new(Mutex::new(VmConsole::new()));
        let store = ProjectStore::new(console.clone());
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("vm");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("game.lua"),
            "function update() end\nfunction draw() cls(3) end\n",
        )
        .unwrap();

        store.open(&root, None).unwrap();
        // The file on disk is what the VM compiles — nothing was written
        // through kessel's own tools.
        let built = console.lock().assemble("game.lua").unwrap();
        assert!(built.ok(), "diagnostics: {:?}", built.diagnostics);
    }

    #[test]
    fn require_reports_no_open_project() {
        let (store, _tmp) = store();
        let err = store.require().unwrap_err();
        assert!(err.contains("project_open"), "got: {err}");
    }

    #[test]
    fn paths_cannot_escape_the_root() {
        let root = Path::new("/tmp/proj");
        assert!(resolve_in_root(root, "tests/dodge.yaml").is_ok());
        assert!(resolve_in_root(root, "../../etc/passwd").is_err());
        assert!(resolve_in_root(root, "/etc/passwd").is_err());
        assert!(resolve_in_root(root, "").is_err());
    }

    #[test]
    fn iso_utc_formats_known_instants() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    }
}
