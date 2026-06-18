import Foundation
import AgentKit
import AgentBridge

/// Ambient `/loop` mode: periodically runs a read-only observation turn and
/// reports what you're doing, connecting it to your task board. Two cadences:
/// fixed interval, or self-paced (the model picks the next delay each tick).
///
/// Quiet by default — the model is told to reply `SILENT` when nothing material
/// changed, and those ticks produce no output. Read-only tools only, so the loop
/// can observe and report but never mutate the board or files on its own.
@MainActor
final class AmbientLoop {
    enum Mode {
        case fixed(TimeInterval)
        case selfPaced
    }

    /// Read-only tools the observation turn may use (no mutations).
    static let readOnlyTools = [
        "list_windows", "find_window", "capture_screen", "apply_ocr",
        "github_list_tasks", "read_situation_messages",
        "read", "glob", "grep", "suggest_next_check",
    ]

    static let defaultPrompt = """
    You are doing a periodic background check of my desktop. Use the desk-activity \
    skill: look at my open windows, read the most relevant one if useful, and tell me \
    in one short sentence what I'm working on (and the matching task if there is one). \
    Do not change anything.
    """

    private let session: AgentSession
    private let gate: TurnGate
    private let defaultDelay: TimeInterval

    /// How a non-silent summary is delivered (print / speak). Reassignable so
    /// voice mode can wrap it with mic muting for half-duplex.
    var emit: @MainActor (String) async -> Void

    private var mode: Mode = .fixed(300)
    private var prompt: String = AmbientLoop.defaultPrompt
    private var task: Task<Void, Never>?
    private var lastSummary: String = ""
    private var lastRun: Date?

    init(
        session: AgentSession,
        gate: TurnGate,
        defaultDelay: TimeInterval,
        emit: @escaping @MainActor (String) async -> Void
    ) {
        self.session = session
        self.gate = gate
        self.defaultDelay = max(30, defaultDelay)
        self.emit = emit
    }

    var isRunning: Bool { task != nil }

    /// Start (or restart) the loop. `intervalSeconds == nil` → self-paced.
    func start(intervalSeconds: TimeInterval?, prompt: String?) {
        stop()
        if let p = prompt?.trimmingCharacters(in: .whitespacesAndNewlines), !p.isEmpty {
            self.prompt = p
        } else {
            self.prompt = Self.defaultPrompt
        }
        self.mode = intervalSeconds.map { .fixed(max(30, $0)) } ?? .selfPaced
        print("\u{1B}[90m[ambient] started (\(cadenceLabel))\u{1B}[0m")
        task = Task { @MainActor in await self.runLoop() }
    }

    func stop() {
        guard task != nil else { return }
        task?.cancel()
        task = nil
        print("\u{1B}[90m[ambient] stopped\u{1B}[0m")
    }

    /// Run one tick immediately (waits for the gate rather than skipping).
    func triggerNow() {
        Task { @MainActor in _ = await self.tick(force: true) }
    }

    func status() {
        guard isRunning else {
            print("[ambient] not running. Start with: /loop <interval> [prompt]")
            return
        }
        let last = lastRun.map { "last run \(Int(-$0.timeIntervalSinceNow))s ago" } ?? "no run yet"
        print("[ambient] running, \(cadenceLabel), \(last)")
    }

    // MARK: - Internals

    private var cadenceLabel: String {
        switch mode {
        case .fixed(let s): return "every \(Int(s))s"
        case .selfPaced: return "self-paced"
        }
    }

    private func runLoop() async {
        // Brief settle so the starting `/loop` turn finishes and the MainActor
        // frees, then fire an initial check (immediate confirmation) and settle
        // into the chosen cadence.
        try? await Task.sleep(for: .seconds(2))
        while !Task.isCancelled {
            let delay = await tick(force: false)
            if Task.isCancelled { break }
            try? await Task.sleep(for: .seconds(delay))
        }
    }

    /// Run one observation; returns the delay until the next tick.
    private func tick(force: Bool) async -> TimeInterval {
        // Don't barge into a user turn: forced ticks wait, scheduled ticks skip.
        if force {
            await gate.lock()
        } else if !(await gate.tryLock()) {
            return defaultDelay
        }

        var nextDelay = defaultDelay
        do {
            let p = buildPrompt()
            let tools = Self.readOnlyTools
            // Off the MainActor so the @MainActor capture poller can service
            // screen-capture tool requests during the observation.
            let response = try await Task.detached { [session] in
                try session.observe(p, allowedTools: tools)
            }.value

            lastRun = Date()
            let summary = session.formatResponse(response.content)
                .trimmingCharacters(in: .whitespacesAndNewlines)

            if !isSilent(summary) {
                lastSummary = summary
                session.agent.pushSituationMessage(
                    text: "[ambient] \(summary)", source: "ambient", sessionId: "")
                await emit(summary)
            }

            switch mode {
            case .fixed(let s):
                nextDelay = s
            case .selfPaced:
                nextDelay = response.suggestedNextCheckSeconds
                    .map { TimeInterval(min(3600, max(30, $0))) } ?? defaultDelay
            }
        } catch {
            print("\u{1B}[90m[ambient] check failed: \(error)\u{1B}[0m")
            nextDelay = max(defaultDelay, 60)
        }
        await gate.unlock()
        return nextDelay
    }

    private func isSilent(_ s: String) -> Bool {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        return t.isEmpty || t == "SILENT" || t.hasPrefix("SILENT")
    }

    private func buildPrompt() -> String {
        var p = prompt
        if !lastSummary.isEmpty {
            p += "\n\nYour previous report was: \"\(lastSummary)\". "
                + "If nothing has materially changed since then, reply with exactly: SILENT"
        } else {
            p += "\n\nIf there is nothing noteworthy on screen, reply with exactly: SILENT"
        }
        if case .selfPaced = mode {
            p += "\n\nThis is a recurring background check with no fixed schedule. "
                + "Call suggest_next_check to set how many seconds until the next check "
                + "(shorter if activity is changing, longer if quiet)."
        }
        return p
    }
}
