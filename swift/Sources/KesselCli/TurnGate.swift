import Foundation

/// Serializes agent turns. A user turn and an ambient observation must not run
/// concurrently — they share the capture bridge and per-tool caches. User turns
/// acquire the gate and wait (`lock`); ambient ticks try and skip if a turn is
/// already running (`tryLock`).
actor TurnGate {
    private var locked = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    /// Blocking acquire — suspends until the gate is free.
    func lock() async {
        while locked {
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                waiters.append(c)
            }
        }
        locked = true
    }

    /// Non-blocking acquire — returns false if a turn is already running.
    func tryLock() -> Bool {
        if locked { return false }
        locked = true
        return true
    }

    /// Release the gate and wake any waiters (they re-check `locked` and race;
    /// exactly one wins). Contention is tiny (ticks are minutes apart), so the
    /// wake-all approach is fine.
    func unlock() {
        locked = false
        let resume = waiters
        waiters.removeAll()
        for w in resume { w.resume() }
    }
}
