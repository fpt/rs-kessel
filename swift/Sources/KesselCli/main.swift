import Foundation
import CoreGraphics
import Util
import AgentBridge
import AgentKit
import CEditline
import TTS
import Audio
import ScreenCapture

// Nonisolated so the @Sendable watcher/detached closures can log. As top-level
// code (main.swift), an unannotated global would be inferred @MainActor.
nonisolated(unsafe) let logger = Logger("Main")

// readline (libedit) callback globals — C callback can't capture Swift context
nonisolated(unsafe) var _rlCompletedLine: UnsafeMutablePointer<CChar>? = nil
nonisolated(unsafe) var _rlLineReady = false
nonisolated(unsafe) var _rlGotEOF = false

private func rlLineCallback(_ line: UnsafeMutablePointer<CChar>?) {
    if line != nil {
        _rlCompletedLine = line
        _rlLineReady = true
    } else {
        _rlGotEOF = true
    }
}

// Thread-safe voice queue (voice callback on MainActor -> readline thread)
final class VoiceQueue: @unchecked Sendable {
    private var queue: [String] = []
    private var lock = os_unfair_lock()

    func enqueue(_ text: String) {
        os_unfair_lock_lock(&lock)
        queue.append(text)
        os_unfair_lock_unlock(&lock)
    }

    func dequeue() -> String? {
        os_unfair_lock_lock(&lock)
        let v = queue.isEmpty ? nil : queue.removeFirst()
        os_unfair_lock_unlock(&lock)
        return v
    }
}

/// Read a line of stdin WITHOUT blocking the MainActor. `readLine()` is a
/// blocking syscall; calling it directly from the @MainActor text loop pins the
/// main thread while waiting for input, which starves the @MainActor ambient
/// loop and capture poller. Running it on a detached thread lets the `await`
/// free the MainActor so those background tasks keep ticking.
func readLineAsync() async -> String? {
    await Task.detached { readLine() }.value
}

// Run async main. This file is named `main.swift`, so Swift treats it as
// top-level code and the entry point is the top-level statement below; `@main`
// can't be used here (it conflicts with top-level code).
await runMain()

@MainActor
func runMain() async {

// Parse command line arguments
let arguments = CommandLine.arguments
var configPath = "configs/default.yaml"

for (index, arg) in arguments.enumerated() {
    if arg == "--config" && index + 1 < arguments.count {
        configPath = arguments[index + 1]
    } else if arg == "--help" || arg == "-h" {
        printHelp()
        exit(0)
    } else if arg == "--verbose" || arg == "-v" {
        Logger.setLevel(.debug)
    }
}

func printHelp() {
    // Derived from argv[0]: this is installed as `kessel`, but still runs as
    // `kessel-cli` in-repo via `swift run kessel-cli`. (The Rust core owns the
    // installed `kessel-cli` name — it is the one with the `app-server` mode.)
    let name = (CommandLine.arguments.first as NSString?)?.lastPathComponent ?? "kessel"
    print("""
    Kessel - Local Voice Assistant

    Usage: \(name) [OPTIONS]

    Options:
        --config PATH      Path to configuration file (default: configs/default.yaml)
        --verbose, -v      Enable verbose logging
        --help, -h         Show this help message

    Examples:
        \(name)
        \(name) --config custom.yaml
        \(name) --verbose
    """)
}

// Load configuration
let config: Config
do {
    if FileManager.default.fileExists(atPath: configPath) {
        config = try Config.load(from: configPath)
        logger.info("Loaded configuration from \(configPath)")
    } else {
        config = Config.default()
        logger.warning("Config file not found, using defaults")
    }
} catch {
    logger.error("Failed to load configuration: \(error)")
    config = Config.default()
    logger.info("Using default configuration")
}

// Initialize AgentSession (agent + TTS + skills)
let session: AgentSession
do {
    session = try await AgentSession(config: config, configPath: configPath)
} catch {
    logger.error("Failed to initialize agent: \(error)")
    exit(1)
}

// Mutable: `/listen` turns on spoken replies when switching into voice mode, so
// voice mode always speaks even if the config started with TTS off.
var ttsEnabled = config.tts?.enabled ?? false

// Initialize STT with SpeechTranscriber
let sttConfig = config.stt ?? Config.STTConfig(enabled: false)

let locale: Locale = {
    if let id = sttConfig.locale {
        return Locale(identifier: id)
    }
    return Locale.current
}()

// Always STT-capable so voice can be entered at runtime via `/listen` even when
// starting in text mode. Initialization (and the mic-permission prompt it leads
// to) is deferred until we actually enter voice mode.
let audioCapture = AudioCapture(config: AudioCapture.Config(
    enabled: true,
    locale: locale,
    censor: sttConfig.censor ?? false
))

// Voice-mode state, shared by the text loop, `/listen`, and the routing below.
var voiceInitialized = false
var switchToVoice = false

if sttConfig.enabled {
    logger.info("Initializing SpeechTranscriber...")
    do {
        try await audioCapture.initialize()
        voiceInitialized = true
        logger.info("SpeechTranscriber initialized successfully")
    } catch {
        logger.error("Failed to initialize SpeechTranscriber: \(error)")
        logger.info("Continuing without STT")
    }
}

// Watcher events are pushed to situation context — just log them.
session.onWatcherEvent = { @Sendable json in
    logger.debug("[Watcher] \(json)")
}

// Start watcher + summary poller
session.start()

// Periodic window list (every 30s) -> situation message
let wm = WindowManager()
let windowListPoller = Task { @MainActor in
    while !Task.isCancelled {
        if let list = try? await wm.listWindows() {
            let text = list.map { $0.summary }.joined(separator: "\n")
            session.agent.pushSituationMessage(
                text: "[screen] Windows:\n\(text)", source: "screen", sessionId: ""
            )
        }
        try? await Task.sleep(for: .seconds(30))
    }
}

// Capture request fulfillment (100ms polling)
let capturePoller = Task { @MainActor in
    var lastCapturedImage: CGImage? = nil
    var lastCaptureInfo: WindowInfo? = nil

    while !Task.isCancelled {
        try? await Task.sleep(for: .milliseconds(100))
        let requests = session.agent.drainCaptureRequests()
        for req in requests {
            // Window query. searchKeywords present = find_window/list_windows.
            // Empty keywords is the list_windows sentinel (list filtered windows);
            // non-empty does keyword matching.
            if let keywords = req.searchKeywords {
                do {
                    let text: String
                    if keywords.isEmpty {
                        // list_windows: the "what am I doing" set (noise filtered out).
                        let windows = try await wm.listWindows(excludeNoise: true)
                        if windows.isEmpty {
                            text = "No user windows found."
                        } else {
                            let lines = windows.map { $0.findWindowDescription }.joined(separator: "\n  ")
                            text = "Open windows (\(windows.count)):\n  \(lines)"
                        }
                    } else {
                        // find_window: keyword search across all windows.
                        let allWindows = try await wm.listWindows()
                        let kws = keywords.lowercased().split(separator: " ").map(String.init)
                        let matched = allWindows.filter { win in
                            let haystack = "\(win.title ?? "") \(win.appName ?? "")".lowercased()
                            return kws.allSatisfy { haystack.contains($0) }
                        }
                        if matched.isEmpty {
                            let all = allWindows.map { $0.findWindowDescription }.joined(separator: "\n  ")
                            text = "No windows matched keywords: \(keywords)\n\nAll windows:\n  \(all)"
                        } else {
                            let lines = matched.map { $0.findWindowDescription }.joined(separator: "\n  ")
                            text = "Found \(matched.count) window(s):\n  \(lines)"
                        }
                    }
                    session.agent.submitCaptureResult(id: req.id, imageBase64: "", metadataJson: text)
                } catch {
                    session.agent.submitCaptureResult(
                        id: req.id, imageBase64: "", metadataJson: "Error: \(error)"
                    )
                }
                continue
            }

            // apply_ocr: run OCR on cached image with optional crop
            if req.applyOcr == true {
                do {
                    guard let cached = lastCapturedImage, let cachedInfo = lastCaptureInfo else {
                        session.agent.submitCaptureResult(
                            id: req.id, imageBase64: "",
                            metadataJson: "Error: no cached image. Capture a window first with capture_screen."
                        )
                        continue
                    }
                    var image = cached
                    var cropLabel = ""
                    let hasCrop = req.cropX != nil || req.cropY != nil
                        || req.cropW != nil || req.cropH != nil
                    if hasCrop {
                        let cx = req.cropX ?? 0.0
                        let cy = req.cropY ?? 0.0
                        let cw = req.cropW ?? 1.0
                        let ch = req.cropH ?? 1.0
                        if let cropped = WindowManager.cropCGImage(image, x: cx, y: cy, w: cw, h: ch) {
                            image = cropped
                            cropLabel = ", Cropped: \(cx),\(cy) \(Int(cw * 100))%x\(Int(ch * 100))%"
                        }
                    }
                    let entries = try performOCR(on: image)
                    let header = "Window: \(cachedInfo.title ?? "?"), App: \(cachedInfo.appName ?? "?")\(cropLabel)"
                    // Grouped blocks read like a human sees the window — far more
                    // legible to the model than a flat per-line bbox list.
                    let metadata = header + "\n" + formatOCRResultsGrouped(entries)
                    session.agent.submitCaptureResult(id: req.id, imageBase64: "", metadataJson: metadata)
                } catch {
                    session.agent.submitCaptureResult(
                        id: req.id, imageBase64: "", metadataJson: "Error: \(error)"
                    )
                }
                continue
            }

            // capture_screen: capture by window_id, optional detect
            do {
                guard let windowId = req.windowId else {
                    session.agent.submitCaptureResult(
                        id: req.id, imageBase64: "",
                        metadataJson: "Error: window_id is required. Use find_window first."
                    )
                    continue
                }

                let (image, info) = try await wm.captureWindow(windowId: windowId)
                lastCapturedImage = image
                lastCaptureInfo = info

                if req.detect == true {
                    let objects = try performObjectDetection(on: image)
                    let header = "Window: \(info.title ?? "?"), App: \(info.appName ?? "?")"
                    let metadata = header + "\n" + formatDetectionResults(objects)
                    session.agent.submitCaptureResult(id: req.id, imageBase64: "", metadataJson: metadata)
                } else {
                    let base64 = WindowManager.cgImageToBase64(image) ?? ""
                    let metadata = "Window: \(info.title ?? "?"), App: \(info.appName ?? "?"), Size: \(Int(info.frame.width))x\(Int(info.frame.height))"
                    session.agent.submitCaptureResult(id: req.id, imageBase64: base64, metadataJson: metadata)
                }
            } catch {
                session.agent.submitCaptureResult(
                    id: req.id, imageBase64: "", metadataJson: "Error: \(error)"
                )
            }
        }
    }
}

// /loop coordination: one agent turn at a time. Loop ticks run a full turn OFF
// the MainActor and skip if a user turn is active. The text-mode `runTurn` below
// is reused by voice mode with mic-muting wrapped around TTS.
let turnGate = TurnGate()
// Both loops are constructed with placeholder handlers; the real `runTurn`
// closures are assigned below, once both exist — they call `runLoopTurn`, which
// transitively captures both `ambientLoop` and `goalDriver` (via handleCommand).
let ambientLoop = AmbientLoop(
    gate: turnGate,
    defaultDelay: TimeInterval(config.ambient?.intervalSeconds ?? 300),
    runTurn: { _ in nil }
)
// /goal driver: keeps running turns until an evaluator confirms the condition
// (or the maxTurns cap is hit). Shares the turn gate and the runTurn path.
let goalDriver = GoalDriver(
    agent: session.agent,
    gate: turnGate,
    maxTurns: config.agent.maxTurns,
    runTurn: { _ in nil }
)
// Text-mode default: run each turn fully (no mic muting). Voice mode reassigns
// these with mic-muting around TTS.
ambientLoop.runTurn = { prompt in await runLoopTurn(prompt, muteMic: false) }
goalDriver.runTurn = { prompt in await runLoopTurn(prompt, muteMic: false) }

// Route to voice or text mode. From text mode, `/listen` sets `switchToVoice`
// and breaks the loop, then we hand off to continuous voice mode.
if sttConfig.enabled {
    await runContinuousVoiceMode()
} else {
    await runTextMode()
    if switchToVoice {
        await runContinuousVoiceMode()
    }
}

// Cleanup
session.stop()
windowListPoller.cancel()
capturePoller.cancel()

// Skip C++ static destructors to avoid ggml Metal device assertion crash.
// Flush first — _exit bypasses stdio flushing, which would drop buffered
// stdout when it's a pipe/file (e.g. the testsuite capturing responses).
fflush(stdout)
fflush(stderr)
_exit(0)

// MARK: - Text Mode

/// Lazily initialize the speech transcriber for voice mode. Returns true if voice
/// is ready (already initialized, or initialized successfully now). On failure
/// (e.g. SpeechTranscriber unavailable) it prints why and returns false so the
/// caller stays in text mode.
func prepareVoiceMode() async -> Bool {
    // Voice mode speaks responses: turn on TTS even if the config started with it
    // off (e.g. a text-first config). The configured/default voice still applies.
    if !ttsEnabled {
        ttsEnabled = true
        session.tts.setEnabled(true)
        print("\u{1B}[90m[voice] enabling spoken replies (TTS)\u{1B}[0m")
    }
    if voiceInitialized { return true }
    print("Initializing voice mode…")
    do {
        try await audioCapture.initialize()
        voiceInitialized = true
        return true
    } catch {
        logger.error("Cannot start voice mode: \(error)")
        print("Cannot start voice mode: \(error)\n")
        return false
    }
}

func runTextMode() async {
    print("""

===========================================
  Kessel - Text Mode
===========================================

Model: \(config.llm.model ?? config.llm.modelPath ?? "(local)")
Endpoint: \(config.llm.baseURL ?? "in-process llama.cpp")

Type your messages below. Commands:
  /reset    - Clear conversation history
  /quit     - Exit the program
  /help     - Show this help
  /history  - Show conversation history
  /voices   - List available TTS voices
  /stop     - Stop current TTS playback
  /loop     - Run a prompt/command on a recurring cadence as a full turn:
              /loop [interval] <prompt> (e.g. /loop 5m /reset, /loop check email
              every 30m); /loop alone = self-paced desk check; /loop now|stop|status
  /goal     - Keep working until a condition is met: /goal <condition>
              (e.g. /goal all tests pass); /goal = status; /goal clear = stop
  /listen   - Switch to continuous voice mode (speak instead of type)

===========================================

""")

    if config.ambient?.enabled == true {
        ambientLoop.start(
            intervalSeconds: config.ambient?.intervalSeconds.map(TimeInterval.init),
            prompt: config.ambient?.prompt
        )
    }

    var turnCount = 0
    let maxTurns = config.agent.maxTurns

    while turnCount < maxTurns {
        print("You: ", terminator: "")
        fflush(stdout)

        guard let line = await readLineAsync() else {
            logger.info("EOF reached, exiting")
            break
        }

        let userInput = line.trimmingCharacters(in: .whitespacesAndNewlines)
        if userInput.isEmpty { continue }

        if userInput.hasPrefix("/") {
            // `/listen` switches into continuous voice mode (init on demand).
            if userInput == "/listen" {
                if await prepareVoiceMode() {
                    switchToVoice = true
                    break
                }
                continue
            }
            handleCommand(userInput)
            continue
        }

        do {
            // Run the agent OFF the MainActor so the @MainActor capture poller
            // can service screen-capture tool requests (find_window / list_windows
            // / capture_screen / apply_ocr) while the ReAct loop runs. Calling
            // step() directly here would block the MainActor for the whole loop
            // and those tools would time out. Voice mode does the same via detach.
            // The turn gate serializes against ambient observation ticks.
            await turnGate.lock()
            let response: AgentResponse
            do {
                response = try await Task.detached { try session.step(userInput) }.value
            } catch {
                await turnGate.unlock()
                throw error
            }
            await turnGate.unlock()
            let finalResponse = session.formatResponse(response.content)

            if let reasoning = response.reasoning {
                print("\u{1B}[90m💭 \(reasoning)\u{1B}[0m\n")
            }
            print("Assistant: \(finalResponse)")
            print("\u{1B}[90m[\(Int(response.contextPercent))% context]\u{1B}[0m\n")

            if ttsEnabled {
                await session.tts.speakAsync(finalResponse)
            }

            turnCount += 1
        } catch {
            logger.error("Agent error: \(error)")
            print("Error: \(error)\n")
        }
    }
}

func handleCommand(_ command: String) {
    // /loop and /goal take arguments, so handle them by prefix before the switch.
    if command == "/loop" || command.hasPrefix("/loop ") {
        handleLoopCommand(String(command.dropFirst("/loop".count)).trimmingCharacters(in: .whitespaces))
        return
    }
    if command == "/goal" || command.hasPrefix("/goal ") {
        handleGoalCommand(String(command.dropFirst("/goal".count)).trimmingCharacters(in: .whitespaces))
        return
    }
    switch command {
    case "/quit", "/exit":
        session.tts.stop()
        print("Goodbye!")
        fflush(stdout)
        _exit(0)
    case "/help":
        printHelp()
    case "/history":
        print("Conversation History:")
        print(session.agent.getConversationHistory())
        print()
    case "/reset":
        session.reset()
        print("Conversation history cleared.\n")
    case "/voices":
        TextToSpeech.printAvailableVoices()
    case "/stop":
        if session.tts.speaking { session.tts.stop(); print("TTS stopped.\n") }
        else { print("TTS is not currently speaking.\n") }
    default:
        if !session.handleCommand(command) {
            print("Unknown command: \(command)")
            print("Type /help for available commands.\n")
        }
    }
}

/// Run one `/loop` turn exactly like a typed line: a slash command is dispatched
/// via handleCommand; anything else runs as a full agent turn (off the MainActor
/// so the capture poller keeps servicing screen tools). `muteMic` brackets TTS
/// with mic muting for half-duplex in voice mode. The caller (AmbientLoop) holds
/// the turn gate, so this must not touch it.
@MainActor
func runLoopTurn(_ prompt: String, muteMic: Bool) async -> AgentResponse? {
    if prompt.hasPrefix("/") {
        handleCommand(prompt)
        return nil
    }
    let response: AgentResponse
    do {
        response = try await Task.detached { try session.step(prompt) }.value
    } catch {
        logger.error("Loop turn error: \(error)")
        print("\u{1B}[90m[loop] turn failed: \(error)\u{1B}[0m")
        return nil
    }
    let text = session.formatResponse(response.content)
    if let reasoning = response.reasoning {
        print("\n\u{1B}[90m💭 \(reasoning)\u{1B}[0m")
    }
    print("\n\u{1F501} \(text)")
    print("\u{1B}[90m[\(Int(response.contextPercent))% context]\u{1B}[0m\n")
    fflush(stdout)
    if ttsEnabled {
        if muteMic { audioCapture.mute() }
        await session.tts.speakAsync(text)
        if muteMic { audioCapture.unmute() }
    }
    return response
}

/// Handle `/loop`, modelled on Claude Code's `/loop`. Subcommands first, then
/// parse `[interval] <prompt>`:
///   /loop                          → self-paced, default desk-activity check
///   /loop <interval> <prompt>       → fixed interval (e.g. /loop 5m /babysit-prs)
///   /loop <prompt> every <N><unit>  → fixed interval from a trailing "every" clause
///   /loop <prompt>                  → self-paced with a custom prompt
///   /loop now | stop | status
func handleLoopCommand(_ args: String) {
    let trimmed = args.trimmingCharacters(in: .whitespaces)
    switch trimmed {
    case "":
        ambientLoop.start(intervalSeconds: nil, prompt: nil)
        return
    case "stop", "off":
        ambientLoop.stop()
        return
    case "now":
        ambientLoop.triggerNow()
        return
    case "status":
        ambientLoop.status()
        return
    default:
        break
    }
    let (interval, prompt) = parseLoopArgs(trimmed)
    ambientLoop.start(intervalSeconds: interval, prompt: prompt.isEmpty ? nil : prompt)
}

/// Handle `/goal`, modelled on Claude Code's `/goal`:
///   /goal <condition>  → set a completion condition and work toward it
///   /goal              → show status of the active goal
///   /goal clear        → clear it early (aliases: stop, off, reset, none, cancel)
func handleGoalCommand(_ args: String) {
    let trimmed = args.trimmingCharacters(in: .whitespaces)
    switch trimmed.lowercased() {
    case "":
        goalDriver.status()
    case "clear", "stop", "off", "reset", "none", "cancel":
        goalDriver.clear()
    default:
        goalDriver.set(trimmed)
    }
}

/// Parse `[interval] <prompt>` like Claude Code's /loop:
/// 1. leading interval token (`5m`, `2h`, `1d`) → interval + rest as prompt
/// 2. trailing `every <N><unit>` / `every <N> <word>` → interval, stripped prompt
/// 3. otherwise → no interval (self-paced), whole input is the prompt
func parseLoopArgs(_ input: String) -> (TimeInterval?, String) {
    let parts = input.split(separator: " ", maxSplits: 1).map(String.init)
    if let head = parts.first, let secs = parseDuration(head) {
        return (secs, parts.count > 1 ? parts[1] : "")
    }
    if let (secs, stripped) = parseTrailingEvery(input) {
        return (secs, stripped)
    }
    return (nil, input)
}

/// Match a trailing `every <N><unit>` / `every <N> <unit-word>` clause — but only
/// when "every" is followed by a time expression (`check every PR` has none).
func parseTrailingEvery(_ input: String) -> (TimeInterval, String)? {
    let pattern = #"(?i)\bevery\s+(\d+)\s*([a-z]+)\s*$"#
    guard let re = try? NSRegularExpression(pattern: pattern) else { return nil }
    let ns = input as NSString
    guard let m = re.firstMatch(in: input, range: NSRange(location: 0, length: ns.length)),
          let value = Double(ns.substring(with: m.range(at: 1))),
          let secs = durationUnitToSeconds(value, ns.substring(with: m.range(at: 2))) else { return nil }
    let stripped = ns.substring(to: m.range.location).trimmingCharacters(in: .whitespaces)
    return (secs, stripped)
}

/// Parse a duration token like "30s", "5m", "2h", "1d" (a unit is required) into seconds.
func parseDuration(_ s: String) -> TimeInterval? {
    guard let unit = s.last, let value = Double(s.dropLast()), value > 0 else { return nil }
    return durationUnitToSeconds(value, String(unit))
}

/// Map a numeric value + unit word to seconds. Accepts short and long forms.
func durationUnitToSeconds(_ value: Double, _ unit: String) -> TimeInterval? {
    switch unit.lowercased() {
    case "s", "sec", "secs", "second", "seconds": return value
    case "m", "min", "mins", "minute", "minutes": return value * 60
    case "h", "hr", "hrs", "hour", "hours": return value * 3600
    case "d", "day", "days": return value * 86400
    default: return nil
    }
}

// MARK: - Continuous Voice Mode

func runContinuousVoiceMode() async {
    let combineWindowMs = 500
    let micMuteDurationSecs: Double = 3.0
    var bufferedVoice: String? = nil
    var combineTimer: Task<Void, Never>? = nil
    var micUnmuteTask: Task<Void, Never>? = nil

    let voiceQueue = VoiceQueue()

    func muteMicWithTimer() {
        audioCapture.mute()
        micUnmuteTask?.cancel()
        micUnmuteTask = Task { @MainActor in
            try? await Task.sleep(for: .seconds(micMuteDurationSecs))
            guard !Task.isCancelled else { return }
            if !session.tts.speaking {
                audioCapture.unmute()
            }
        }
    }

    func feedInput(voiceText: String?, typedText: String?) {
        var parts: [String] = []
        if let v = voiceText { parts.append(v) }
        if let t = typedText { parts.append("----text: \(t)") }
        let combined = parts.joined(separator: "\n")
        guard !combined.isEmpty else { return }
        Task.detached {
            await turnGate.lock()
            do {
                let response = try session.step(combined)
                await turnGate.unlock()
                let text = session.formatResponse(response.content)
                await MainActor.run {
                    if let reasoning = response.reasoning {
                        print("\u{1B}[90m💭 \(reasoning)\u{1B}[0m\n")
                    }
                    print("Assistant: \(text)")
                    print("\u{1B}[90m[\(Int(response.contextPercent))% context]\u{1B}[0m\n")
                }
                if ttsEnabled {
                    await MainActor.run { audioCapture.mute() }
                    await session.tts.speakAsync(text)
                    await MainActor.run { audioCapture.unmute() }
                }
            } catch {
                await turnGate.unlock()
                await MainActor.run { logger.error("Agent error: \(error)") }
            }
        }
    }

    // In voice mode, loop and goal turns mute the mic while speaking (half-duplex).
    ambientLoop.runTurn = { prompt in await runLoopTurn(prompt, muteMic: true) }
    goalDriver.runTurn = { prompt in await runLoopTurn(prompt, muteMic: true) }
    if config.ambient?.enabled == true {
        ambientLoop.start(
            intervalSeconds: config.ambient?.intervalSeconds.map(TimeInterval.init),
            prompt: config.ambient?.prompt
        )
    }

    audioCapture.onVolatileResult = { text in
        print("\r\u{1B}[K  \(text)", terminator: "")
        fflush(stdout)
    }

    audioCapture.onFinalResult = { text in
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        print("\r\u{1B}[KYou (voice): \(trimmed)\n")
        voiceQueue.enqueue(trimmed)
    }

    do {
        try await audioCapture.start()

        print("""

===========================================
  Kessel - Continuous Voice Mode
===========================================

Model: \(config.llm.model ?? config.llm.modelPath ?? "(local)")
Endpoint: \(config.llm.baseURL ?? "in-process llama.cpp")
STT: Apple SpeechTranscriber (\(locale.identifier))

Start speaking or type below. Press Ctrl+C to exit.
Commands: /reset /quit /help /history /voices /stop /loop /goal

===========================================

""")

        _rlLineReady = false
        _rlCompletedLine = nil
        _rlGotEOF = false

        let stdinReader = Task.detached {
            rl_callback_handler_install("> ", rlLineCallback)

            while !Task.isCancelled && !_rlGotEOF {
                var fds = [pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)]
                let ret = poll(&fds, 1, 50)

                if ret > 0 && (fds[0].revents & Int16(POLLIN)) != 0 {
                    await MainActor.run { muteMicWithTimer() }
                    rl_callback_read_char()
                }

                if _rlLineReady {
                    _rlLineReady = false
                    if let cStr = _rlCompletedLine {
                        let line = String(cString: cStr)
                        if !line.isEmpty { add_history(cStr) }
                        free(cStr)
                        _rlCompletedLine = nil

                        await MainActor.run {
                            let text = line.trimmingCharacters(in: .whitespacesAndNewlines)

                            if text.hasPrefix("/") {
                                handleCommand(text)
                                return
                            }

                            guard !text.isEmpty else { return }

                            let voice = bufferedVoice
                            bufferedVoice = nil
                            combineTimer?.cancel()
                            combineTimer = nil

                            if let voice = voice {
                                print("You: \(voice) + text: \(text)\n")
                            } else {
                                print("You (text): \(text)\n")
                            }
                            feedInput(voiceText: voice, typedText: text)
                        }
                    }
                }

                if let voice = voiceQueue.dequeue() {
                    let partial: String? = {
                        guard let buf = rl_line_buffer else { return nil }
                        let s = String(cString: buf)
                        return s.isEmpty ? nil : s
                    }()

                    if let partial = partial {
                        rl_kill_text(0, rl_end)
                        rl_point = 0
                        rl_redisplay()

                        await MainActor.run {
                            print("  [+ text: \(partial)]")
                            feedInput(voiceText: voice, typedText: partial)
                        }
                    } else {
                        await MainActor.run {
                            if let existing = bufferedVoice {
                                bufferedVoice = existing + " " + voice
                            } else {
                                bufferedVoice = voice
                            }
                            combineTimer?.cancel()
                            combineTimer = Task { @MainActor in
                                try? await Task.sleep(for: .milliseconds(combineWindowMs))
                                guard !Task.isCancelled else { return }
                                if let voice = bufferedVoice {
                                    feedInput(voiceText: voice, typedText: nil)
                                    bufferedVoice = nil
                                }
                                combineTimer = nil
                            }
                        }
                    }
                }
            }

            rl_callback_handler_remove()
        }

        let signalSource = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
        signal(SIGINT, SIG_IGN)

        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            signalSource.setEventHandler {
                signalSource.cancel()
                continuation.resume()
            }
            signalSource.resume()
        }

        stdinReader.cancel()
        micUnmuteTask?.cancel()
        combineTimer?.cancel()
        print("\nGoodbye!")
        await audioCapture.stop()

    } catch {
        logger.error("Failed to start voice mode: \(error)")
        print("Error: \(error)\n")
    }
}

} // end runMain
