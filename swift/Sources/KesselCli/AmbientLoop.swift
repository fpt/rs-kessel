import Foundation
import AgentBridge

/// Recurring `/loop` mode, modelled on Claude Code's `/loop`: run a prompt (or a
/// slash command) on a recurring cadence as a **normal agent turn** — full tools,
/// persisted to conversation memory. Two cadences: a fixed interval, or
/// self-paced (the model picks the next delay each tick via `suggest_next_check`).
///
/// The turn itself is executed by the injected `runTurn` closure so the loop
/// reuses the exact path a typed line takes (printing, TTS, mic muting in voice
/// mode). The loop owns scheduling and turn serialization; `runTurn` must not
/// touch the gate (the loop already holds it while a tick runs).
@MainActor
final class AmbientLoop {
    enum Mode {
        case fixed(TimeInterval)
        case selfPaced
    }

    /// Default prompt when `/loop` is invoked with no prompt: a quiet desk check.
    static let defaultPrompt = """
    You are doing a periodic background check of my desktop. Look at my open \
    windows, read the most relevant one if useful, and tell me in one short \
    sentence what I'm working on (and the matching task if there is one).
    """

    /// Minimum cadence, in seconds. Matches Claude Code's 1-minute floor.
    static let minInterval: TimeInterval = 60

    private let gate: TurnGate
    private let defaultDelay: TimeInterval

    /// Runs one turn for the given prompt and returns the response (or `nil` for
    /// a slash command / error). Reassignable so voice mode can wrap TTS with mic
    /// muting for half-duplex. Must NOT acquire the turn gate.
    var runTurn: @MainActor (String) async -> AgentResponse?

    private var mode: Mode = .fixed(300)
    private var prompt: String = AmbientLoop.defaultPrompt
    private var task: Task<Void, Never>?
    private var lastRun: Date?

    init(
        gate: TurnGate,
        defaultDelay: TimeInterval,
        runTurn: @escaping @MainActor (String) async -> AgentResponse?
    ) {
        self.gate = gate
        self.defaultDelay = max(Self.minInterval, defaultDelay)
        self.runTurn = runTurn
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
        self.mode = intervalSeconds.map { .fixed(max(Self.minInterval, $0)) } ?? .selfPaced
        print("\u{1B}[90m[loop] started (\(cadenceLabel))\u{1B}[0m")
        task = Task { @MainActor in await self.runLoop() }
    }

    func stop() {
        guard task != nil else { return }
        task?.cancel()
        task = nil
        print("\u{1B}[90m[loop] stopped\u{1B}[0m")
    }

    /// Run one tick immediately (waits for the gate rather than skipping).
    func triggerNow() {
        Task { @MainActor in _ = await self.tick(force: true) }
    }

    func status() {
        guard isRunning else {
            print("[loop] not running. Start with: /loop [interval] <prompt>")
            return
        }
        let last = lastRun.map { "last run \(Int(-$0.timeIntervalSinceNow))s ago" } ?? "no run yet"
        print("[loop] running, \(cadenceLabel), \(last)")
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
        // frees, then fire an initial check (immediate, like Claude Code) and
        // settle into the chosen cadence.
        try? await Task.sleep(for: .seconds(2))
        while !Task.isCancelled {
            let delay = await tick(force: false)
            if Task.isCancelled { break }
            try? await Task.sleep(for: .seconds(delay))
        }
    }

    /// Run one turn; returns the delay until the next tick.
    private func tick(force: Bool) async -> TimeInterval {
        // Don't barge into a user turn: forced ticks wait, scheduled ticks skip.
        if force {
            await gate.lock()
        } else if !(await gate.tryLock()) {
            return defaultDelay
        }

        let response = await runTurn(buildPrompt())
        lastRun = Date()
        await gate.unlock()

        switch mode {
        case .fixed(let s):
            return s
        case .selfPaced:
            return response?.suggestedNextCheckSeconds
                .map { TimeInterval(min(3600, max(Int(Self.minInterval), Int($0)))) }
                ?? defaultDelay
        }
    }

    private func buildPrompt() -> String {
        // Slash commands are passed through verbatim (like Claude Code).
        if prompt.hasPrefix("/") { return prompt }
        var p = prompt
        if case .selfPaced = mode {
            p += "\n\nThis is a recurring background check with no fixed schedule. "
                + "Call suggest_next_check to set how many seconds until the next check "
                + "(shorter if activity is changing, longer if quiet)."
        }
        return p
    }
}
