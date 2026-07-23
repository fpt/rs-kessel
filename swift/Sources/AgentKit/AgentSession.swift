import Foundation
import AgentBridge
import Util
import TTS

/// Shared agent lifecycle — usable from CLI, iOS, or any other frontend.
public class AgentSession: @unchecked Sendable {

    // MARK: - Public properties

    public let agent: Agent
    public let tts: TextToSpeech
    public let config: Config
    public let language: String
    public let configPath: String

    // MARK: - Private state

    private let logger = Logger("AgentSession")

    // MARK: - Init

    /// Initialize agent, TTS, and load skills. `approver` gates the backend's
    /// mutation requests (file writes / shell commands); pass `nil` to let the
    /// backend run autonomously with no approval gate.
    public init(config: Config, configPath: String, approver: MutationApprover? = nil) async throws {
        self.config = config
        self.configPath = configPath
        self.language = config.agent.language ?? "en"

        // Resolve API key
        let apiKey: String? = {
            if let envKey = ProcessInfo.processInfo.environment["OPENAI_API_KEY"], !envKey.isEmpty {
                return envKey
            } else if let configKey = config.llm.apiKey, !configKey.isEmpty {
                return configKey
            }
            return nil
        }()

        // Resolve model path. `hf:ORG/REPO[@REV]/file.gguf` specs and absolute
        // paths pass through untouched (the Rust model downloader resolves `hf:`,
        // downloading into the HF cache); only bare relative paths are resolved
        // against the config dir.
        var modelPath: String? = nil
        if let cfgModelPath = config.llm.modelPath {
            if cfgModelPath.hasPrefix("hf:") || cfgModelPath.hasPrefix("/") {
                modelPath = cfgModelPath
            } else {
                let configDir = URL(fileURLWithPath: configPath).deletingLastPathComponent()
                modelPath = configDir.appendingPathComponent(cfgModelPath).path
            }
        }

        let mcpServers = (config.mcpServers ?? []).map {
            McpServerConfig(command: $0.command ?? "", args: $0.args ?? [], url: $0.url)
        }
        let contextWindow = config.llm.contextWindow.map { UInt32($0) } ?? 128_000
        let agentConfig = AgentConfig(
            modelPath: modelPath,
            baseUrl: config.llm.baseURL ?? "",
            model: config.llm.model ?? "",
            apiKey: apiKey,
            useHarmonyTemplate: config.llm.harmonyTemplate,
            temperature: config.llm.temperature,
            maxTokens: UInt32(config.llm.maxTokens),
            contextWindow: contextWindow,
            language: language,
            workingDir: FileManager.default.currentDirectoryPath,
            reasoningEffort: config.llm.reasoningEffort,
            inferenceEngine: config.llm.inferenceEngine,
            mcpServers: mcpServers
        )

        self.agent = try agentNew(config: agentConfig, approver: approver)
        logger.info("Agent initialized")

        // TTS
        let ttsConfig = config.tts ?? Config.TTSConfig(
            enabled: false, voice: nil, rate: 0.5, pitchMultiplier: 1.0, volume: 1.0
        )
        let ttsVoice: String?
        if let v = ttsConfig.voice {
            ttsVoice = v
        } else {
            switch language {
            case "ja": ttsVoice = "com.apple.voice.enhanced.ja-JP.Kyoko"
            default: ttsVoice = "com.apple.voice.enhanced.en-US.Samantha"
            }
        }
        self.tts = TextToSpeech(config: TextToSpeech.Config(
            enabled: ttsConfig.enabled,
            voice: ttsVoice,
            rate: ttsConfig.rate,
            pitchMultiplier: ttsConfig.pitchMultiplier,
            volume: ttsConfig.volume
        ))

        // --- Post-init setup ---

        // Load system prompt with {language} template
        if let systemPromptPath = config.agent.systemPromptPath {
            var resolvedPath = systemPromptPath
            if !systemPromptPath.hasPrefix("/") {
                let configDir = URL(fileURLWithPath: configPath).deletingLastPathComponent()
                resolvedPath = configDir.appendingPathComponent(systemPromptPath).path
            }
            do {
                var systemPrompt = try String(contentsOfFile: resolvedPath, encoding: .utf8)
                let languagePrompt: String = {
                    switch language {
                    case "ja": return "日本語で回答してください。"
                    case "en": return ""
                    default: return "Respond in \(language)."
                    }
                }()
                systemPrompt = systemPrompt.replacingOccurrences(of: "{language}", with: languagePrompt)
                agent.setSystemPrompt(prompt: systemPrompt)
                logger.info("Loaded system prompt from \(resolvedPath)")
            } catch {
                logger.warning("Failed to load system prompt: \(error)")
            }
        }

        // Load skills from configured paths (relative to config dir)
        let configDir = URL(fileURLWithPath: configPath).deletingLastPathComponent().path
        let skillPaths = config.agent.skillPaths ?? ["skills"]
        let discoveredSkills = SkillLoader.loadAll(paths: skillPaths, baseDir: configDir)
        for skill in discoveredSkills {
            agent.addSkill(name: skill.name, description: skill.description, prompt: skill.prompt)
        }
        logger.info("Skills registered (\(discoveredSkills.count) from \(skillPaths))")
    }

    // MARK: - Lifecycle

    /// Start background event sources. Currently a no-op — the Claude Code
    /// watcher was removed; ambient context is fed via `pushSituationMessage`
    /// from the frontend's window-list poller.
    public func start() {}

    /// Stop background resources. Currently a no-op (see `start()`).
    public func stop() {}

    // MARK: - Agent calls

    /// Run one conversation turn.
    public func step(_ text: String) throws -> AgentResponse {
        try agent.step(userInput: text)
    }

    /// Run a non-persisting, read-only observation turn (ambient `/loop`).
    /// Does not touch conversation memory; scoped to `allowedTools`.
    public func observe(_ prompt: String, allowedTools: [String]) throws -> AgentResponse {
        try agent.observe(prompt: prompt, allowedTools: allowedTools)
    }

    /// Reset conversation history.
    public func reset() {
        agent.reset()
    }

    /// Process a slash command. Returns true if handled.
    public func handleCommand(_ command: String) -> Bool {
        switch command {
        case "/reset":
            agent.reset()
            return true
        case "/voices":
            TextToSpeech.printAvailableVoices()
            return true
        case "/stop":
            tts.stop()
            return true
        default:
            return false
        }
    }

    /// Format response text (strip Harmony wrapper if needed).
    public func formatResponse(_ text: String) -> String {
        config.llm.harmonyTemplate
            ? HarmonyParser.extractFinalResponse(text)
            : text
    }

}
