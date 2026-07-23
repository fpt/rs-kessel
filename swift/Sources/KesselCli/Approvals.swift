import Foundation
import AgentBridge

/// Answers the backend's mutation-approval requests (file writes, shell
/// commands). Kessel passes one of these to `agentNew`; without it the backend
/// is told to run autonomously and edits files with no gate — which is the bug
/// this fixes.
///
/// The same object serves every REPL mode; `mode` is flipped as the CLI moves
/// between them:
///
/// - `.prompt` (text REPL): print the request and block on a `y/a/n` answer read
///   from stdin. Safe here because a turn runs while the REPL loop is parked at
///   `await session.step`, so nothing else is reading stdin.
/// - `.deny` (voice REPL): refuse every mutation. Voice has no safe way to
///   confirm a file write by speech, and its libedit char-reader owns stdin, so
///   prompting would corrupt the input line. The user switches to text mode to
///   allow edits.
/// - `.allow` (one-shot `--prompt`): non-interactive, so grant automatically —
///   `--prompt` is the scripting path and there's no one to ask.
///
/// `approve` is invoked synchronously on a backend-servicing thread, so it must
/// be thread-safe and is expected to block until it returns a decision.
final class ReplApprover: MutationApprover, @unchecked Sendable {

    enum Mode {
        case prompt
        case deny
        case allow
    }

    private var _mode: Mode
    private var lock = os_unfair_lock()

    init(mode: Mode) {
        self._mode = mode
    }

    var mode: Mode {
        get { os_unfair_lock_lock(&lock); defer { os_unfair_lock_unlock(&lock) }; return _mode }
        set { os_unfair_lock_lock(&lock); _mode = newValue; os_unfair_lock_unlock(&lock) }
    }

    func approve(action: String, target: String) -> ApprovalDecision {
        switch mode {
        case .allow:
            return .allowOnce
        case .deny:
            // \r\u{1B}[K clears any partial prompt line the voice reader drew.
            print("\r\u{1B}[K\u{1B}[33m[approval] denied \(action): \(target)")
            print("  (voice mode can't confirm edits — switch to text mode to allow)\u{1B}[0m")
            fflush(stdout)
            return .deny
        case .prompt:
            return promptOnStdin(action: action, target: target)
        }
    }

    /// Blocking `y/a/n` prompt. Anything other than `y`/`a` (including EOF) denies
    /// — the safe default. `a` grants for the rest of the backend session.
    private func promptOnStdin(action: String, target: String) -> ApprovalDecision {
        print("\n\u{1B}[33m⚠️  The assistant wants to \(action):\u{1B}[0m")
        print("    \(target)")
        print("  [y] allow once   [a] allow all this session   [n] deny (default)")
        print("approve> ", terminator: "")
        fflush(stdout)

        guard let raw = readLine() else { return .deny }
        switch raw.trimmingCharacters(in: .whitespaces).lowercased() {
        case "y", "yes":
            return .allowOnce
        case "a", "all":
            return .allowSession
        default:
            print("\u{1B}[90m[approval] denied\u{1B}[0m")
            return .deny
        }
    }
}
