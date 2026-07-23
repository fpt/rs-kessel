import Foundation
import CEditline

/// Blocking libedit-backed line reader for the text REPL.
///
/// `readLine()` reads stdin raw: no cursor movement, no history, no editing —
/// arrow keys arrive as literal escape sequences. This routes the prompt through
/// libedit's readline emulation instead, which gives emacs-style editing
/// (left/right, Ctrl+A/E/W/K/U, Esc-b/Esc-f word jumps), ↑/↓ history persisted
/// across runs, UTF-8-aware cursor movement, and TAB completion of slash
/// commands.
///
/// Note: macOS libedit does **not** implement Ctrl+R incremental search — the
/// pattern is inserted literally. Binding it explicitly via `rl_parse_and_bind`
/// reports success but changes nothing, and rebinding the arrow-key escape
/// sequences corrupts them, so the stock bindings are left alone.
///
/// libedit is **not** thread-safe and owns the terminal while reading, so
/// `read(prompt:)` blocks: call it from a detached task, never on the MainActor.
/// Voice mode drives the same library through the callback API
/// (`rl_callback_handler_install`) and inherits the history + completion set up
/// by `configure(historyPath:)`.
enum LineEditor {

    /// Slash commands offered by TAB completion. Kept in sync with
    /// `handleCommand` in main.swift.
    static let commands = [
        "/goal", "/help", "/history", "/listen", "/loop",
        "/quit", "/reset", "/stop", "/voices",
    ]

    /// Where history is persisted across runs. Empty until `configure` runs.
    private nonisolated(unsafe) static var historyPath = ""
    private nonisolated(unsafe) static var configured = false

    /// True when stdin is a terminal. Piped/redirected input (the testsuite,
    /// `echo … | kessel`) gets plain `readLine()` — libedit would otherwise
    /// echo prompts and control sequences into the captured output.
    static var isInteractive: Bool { isatty(STDIN_FILENO) == 1 }

    /// Install history and completion. Safe to call more than once; only the
    /// first call does anything. `historyPath` is created if its directory is
    /// missing, and is written back by `saveHistory()`.
    static func configure(historyPath path: String) {
        guard !configured else { return }
        configured = true
        historyPath = path

        // Wide-character input: libedit only decodes UTF-8 (Japanese input, and
        // any non-ASCII paste) once the locale is initialized from the env.
        setlocale(LC_ALL, "")

        guard isInteractive else { return }

        rl_readline_name = strdup("kessel")
        rl_initialize()
        using_history()
        stifle_history(1000)

        try? FileManager.default.createDirectory(
            at: URL(fileURLWithPath: path).deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        read_history(path)

        rl_attempted_completion_function = kesselAttemptedCompletion
    }

    /// Read one line, showing `prompt`. Returns nil at EOF (Ctrl+D). Blocking —
    /// must not be called on the MainActor. Non-blank lines that differ from the
    /// previous entry are appended to history.
    static func read(prompt: String) -> String? {
        guard isInteractive else {
            print(prompt, terminator: "")
            fflush(stdout)
            return readLine()
        }
        guard let cLine = readline(prompt) else { return nil }
        defer { free(cLine) }
        let line = String(cString: cLine)
        if !line.trimmingCharacters(in: .whitespaces).isEmpty && line != lastHistoryEntry() {
            add_history(cLine)
        }
        return line
    }

    /// Put the terminal back into cooked mode. libedit switches it to raw mode
    /// for the duration of `readline()`, so any exit taken while a read is in
    /// flight — notably the Ctrl+C handler — must undo that or it leaves the
    /// user's shell without echo.
    static func restoreTerminal() {
        guard configured, isInteractive else { return }
        rl_deprep_terminal()
    }

    /// Persist history. Called before every exit path, including `_exit(0)`,
    /// which bypasses atexit handlers.
    static func saveHistory() {
        guard configured, isInteractive, !historyPath.isEmpty else { return }
        write_history(historyPath)
    }

    /// Most recent history entry, used to suppress consecutive duplicates.
    private static func lastHistoryEntry() -> String? {
        guard history_length > 0,
              let entry = history_get(history_base + history_length - 1),
              let line = entry.pointee.line else { return nil }
        return String(cString: line)
    }
}

// MARK: - Completion callbacks
//
// These are C callbacks: top-level functions with no captured context, so they
// convert to @convention(c) pointers. State is shared through globals because
// libedit's generator protocol is inherently stateful (state == 0 means "new
// completion, rebuild the candidate list").

private nonisolated(unsafe) var completionCandidates: [String] = []
private nonisolated(unsafe) var completionIndex = 0

/// libedit generator: return successive matches for `text`, then NULL. The
/// returned string must be malloc'd — libedit frees it.
private func kesselCompletionGenerator(
    _ text: UnsafePointer<CChar>?, _ state: Int32
) -> UnsafeMutablePointer<CChar>? {
    if state == 0 {
        completionIndex = 0
        let prefix = text.map { String(cString: $0) } ?? ""
        completionCandidates = LineEditor.commands.filter { $0.hasPrefix(prefix) }
    }
    guard completionIndex < completionCandidates.count else { return nil }
    let match = completionCandidates[completionIndex]
    completionIndex += 1
    return strdup(match)
}

/// Complete slash commands, and only as the first word — `/re<TAB>` → `/reset`.
/// Anything else completes to nothing rather than falling back to filenames,
/// which are almost never what you want at this prompt.
private func kesselAttemptedCompletion(
    _ text: UnsafePointer<CChar>?, _ start: Int32, _ end: Int32
) -> UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>? {
    // Suppress libedit's filename-completion fallback in both branches below.
    rl_attempted_completion_over = 1
    guard start == 0, let text, text.pointee == CChar(UInt8(ascii: "/")) else { return nil }
    return rl_completion_matches(text, kesselCompletionGenerator)
}
