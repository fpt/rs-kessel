import Foundation
import AgentBridge

/// `/goal` driver, modelled on Claude Code's `/goal`: set a completion condition
/// and the agent keeps running turns on its own until an evaluator (a tool-less
/// LLM call, in Rust) confirms the condition is met, or a safety cap is hit.
///
/// Each turn is run by the injected `runTurn` closure (the same path a typed line
/// takes — prints/speaks/persists); this driver owns scheduling, evaluation, and
/// turn serialization via the gate. `runTurn` must not touch the gate.
@MainActor
final class GoalDriver {
    private let agent: Agent
    private let gate: TurnGate
    private let maxTurns: Int

    /// Runs one turn for a directive. Reassignable so voice mode can wrap TTS with
    /// mic muting. Must NOT acquire the turn gate (the driver holds it per turn).
    var runTurn: @MainActor (String) async -> AgentResponse?

    private var task: Task<Void, Never>?

    init(
        agent: Agent,
        gate: TurnGate,
        maxTurns: Int,
        runTurn: @escaping @MainActor (String) async -> AgentResponse?
    ) {
        self.agent = agent
        self.gate = gate
        self.maxTurns = max(1, maxTurns)
        self.runTurn = runTurn
    }

    var isActive: Bool { task != nil }

    /// Set (or replace) the goal and start driving turns toward it.
    func set(_ condition: String) {
        stop(clearAgent: true)
        agent.setGoal(condition: condition)
        print("\u{25CE} goal set: \(condition)")
        print("\u{1B}[90m[goal] working toward it; /goal clear to stop, /goal for status\u{1B}[0m")
        task = Task { @MainActor in await self.drive(condition) }
    }

    /// Clear the active goal (and stop the driver) before the condition is met.
    func clear() {
        guard isActive || agent.goalStatus() != nil else {
            print("[goal] no active goal.")
            return
        }
        stop(clearAgent: true)
        print("\u{25CE} goal cleared")
    }

    /// Print the current goal status (Claude Code-style).
    func status() {
        guard let s = agent.goalStatus() else {
            print("[goal] no active goal. Set one with: /goal <condition>")
            return
        }
        print("\u{25CE} goal active — \(s.turnsEvaluated) turn(s) evaluated, \(s.elapsedSeconds)s elapsed")
        print("   condition: \(s.condition)")
        if let reason = s.lastReason {
            print("   latest: \(reason)")
        }
    }

    // MARK: - Internals

    private func stop(clearAgent: Bool) {
        task?.cancel()
        task = nil
        if clearAgent { agent.clearGoal() }
    }

    private func drive(_ condition: String) async {
        var directive = condition
        var turn = 0
        while !Task.isCancelled {
            if turn >= maxTurns {
                print("\n\u{25CE} goal stopped: reached the \(maxTurns)-turn cap without meeting the condition.\n")
                agent.clearGoal()
                break
            }

            // Foreground work: wait for the gate (don't barge a user turn).
            await gate.lock()
            if Task.isCancelled { await gate.unlock(); break }
            _ = await runTurn(directive)
            // Evaluate off the MainActor (it's an LLM call) while holding the gate
            // so an ambient tick can't interleave between turn and evaluation.
            let eval = await Task.detached { [agent] in try? agent.evaluateGoal() }.value
            await gate.unlock()
            turn += 1

            if Task.isCancelled { break }
            guard let eval else {
                print("\n\u{25CE} goal stopped: evaluation failed.\n")
                agent.clearGoal()
                break
            }
            if eval.met {
                print("\n\u{2705} goal achieved (\(turn) turn(s)): \(eval.reason)\n")
                break  // the agent already cleared the goal on a met evaluation
            }
            print("\n\u{1B}[90m\u{25CE} not yet (turn \(turn)/\(maxTurns)): \(eval.reason)\u{1B}[0m\n")
            directive = """
            Continue working toward this goal:
            \(condition)

            It is not satisfied yet — \(eval.reason) Take the next concrete step.
            """
        }
        task = nil
    }
}
