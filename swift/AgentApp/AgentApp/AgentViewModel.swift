import Foundation
import AVFoundation
import Speech
import Observation

/// Message in the chat history
struct ChatMessage: Identifiable {
    let id = UUID()
    let role: Role
    let text: String
    let timestamp = Date()

    enum Role {
        case user, assistant
    }
}

enum LLMProvider: String, CaseIterable {
    case openai = "openai"
    case llamacpp = "llamacpp"

    var displayName: String {
        switch self {
        case .openai: return "OpenAI"
        case .llamacpp: return "Llama.cpp"
        }
    }
}

enum ModelDownloadState: Equatable {
    case notDownloaded
    case downloading(progress: Double)
    case downloaded(path: String)
    case failed(message: String)

    static func == (lhs: ModelDownloadState, rhs: ModelDownloadState) -> Bool {
        switch (lhs, rhs) {
        case (.notDownloaded, .notDownloaded): return true
        case (.downloading(let a), .downloading(let b)): return a == b
        case (.downloaded(let a), .downloaded(let b)): return a == b
        case (.failed(let a), .failed(let b)): return a == b
        default: return false
        }
    }
}

/// Main view model for the agent chat interface
@Observable
final class AgentViewModel {
    var messages: [ChatMessage] = []
    var inputText = ""
    var isLoading = false
    var errorMessage: String?
    var isSpeaking = false
    var isListening = false
    var liveTranscript = ""
    var modelDownloadState: ModelDownloadState = .notDownloaded
    var contextPercent: Int = 0

    private var agent: Agent?
    private let synthesizer = AVSpeechSynthesizer()
    private var ttsDelegate: TTSDelegate?
    private var ttsVoice: AVSpeechSynthesisVoice?
    private var ttsRate: Float = 0.5

    // Speech recognition
    private let speechRecognizer = SFSpeechRecognizer()
    private let audioEngine = AVAudioEngine()
    private var recognitionRequest: SFSpeechAudioBufferRecognitionRequest?
    private var recognitionTask: SFSpeechRecognitionTask?

    // Download
    private var downloadTask: URLSessionDownloadTask?

    // Model info
    static let modelFileName = "Qwen3-1.7B-Q8_0.gguf"
    static let modelDownloadURL = "https://huggingface.co/Qwen/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q8_0.gguf"
    static let modelSizeBytes: Int64 = 1_834_426_016

    // Config
    var provider: LLMProvider {
        get { LLMProvider(rawValue: UserDefaults.standard.string(forKey: "llm_provider") ?? "openai") ?? .openai }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "llm_provider") }
    }
    var apiKey: String {
        get { UserDefaults.standard.string(forKey: "openai_api_key") ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: "openai_api_key") }
    }
    var model: String {
        get { UserDefaults.standard.string(forKey: "openai_model") ?? "gpt-5.4-mini" }
        set { UserDefaults.standard.set(newValue, forKey: "openai_model") }
    }
    var baseURL: String {
        get { UserDefaults.standard.string(forKey: "openai_base_url") ?? "https://api.openai.com/v1" }
        set { UserDefaults.standard.set(newValue, forKey: "openai_base_url") }
    }
    var language: String {
        get { UserDefaults.standard.string(forKey: "agent_language") ?? "en" }
        set { UserDefaults.standard.set(newValue, forKey: "agent_language") }
    }
    var ttsEnabled: Bool {
        get { UserDefaults.standard.object(forKey: "tts_enabled") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "tts_enabled") }
    }

    var isConfigured: Bool {
        switch provider {
        case .openai:
            return !apiKey.isEmpty
        case .llamacpp:
            if case .downloaded = modelDownloadState { return true }
            return false
        }
    }

    var modelFilePath: String {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first!
        return docs.appendingPathComponent(Self.modelFileName).path
    }

    init() {
        ttsDelegate = TTSDelegate { [weak self] in
            self?.isSpeaking = false
        }
        synthesizer.delegate = ttsDelegate

        if let voice = AVSpeechSynthesisVoice(language: language == "ja" ? "ja-JP" : "en-US") {
            ttsVoice = voice
        }

        // Check if model already downloaded
        if FileManager.default.fileExists(atPath: modelFilePath) {
            modelDownloadState = .downloaded(path: modelFilePath)
        }
    }

    func initializeAgent() {
        do {
            let languagePrompt: String = {
                switch language {
                case "ja": return "日本語で回答してください。"
                case "en": return ""
                default: return "Respond in \(language)."
                }
            }()

            var systemPrompt = "You are a kind voice agent.\nGive clear and concise response."
            if !languagePrompt.isEmpty {
                systemPrompt += "\n\(languagePrompt)"
            }

            let config: AgentConfig
            switch provider {
            case .openai:
                guard !apiKey.isEmpty else {
                    errorMessage = "Please set your OpenAI API key in Settings."
                    return
                }
                config = AgentConfig(
                    modelPath: nil,
                    baseUrl: baseURL,
                    model: model,
                    apiKey: apiKey,
                    useHarmonyTemplate: false,
                    temperature: nil,
                    maxTokens: 8192,
                    contextWindow: 128_000,
                    language: language,
                    workingDir: NSHomeDirectory(),
                    reasoningEffort: "medium",
                    mcpServers: []
                )
            case .llamacpp:
                guard case .downloaded(let path) = modelDownloadState else {
                    errorMessage = "Please download a model first."
                    return
                }
                config = AgentConfig(
                    modelPath: path,
                    baseUrl: "",
                    model: "",
                    apiKey: nil,
                    useHarmonyTemplate: false,
                    temperature: 0.7,
                    maxTokens: 2048,
                    contextWindow: 32_000,
                    language: language,
                    workingDir: NSHomeDirectory(),
                    reasoningEffort: nil,
                    mcpServers: []
                )
            }

            agent = try agentNew(config: config)
            agent?.setSystemPrompt(prompt: systemPrompt)
            errorMessage = nil
        } catch {
            errorMessage = "Failed to initialize agent: \(error.localizedDescription)"
        }
    }

    // MARK: - Model Download

    func downloadModel() {
        guard case .notDownloaded = modelDownloadState else { return }
        guard let url = URL(string: Self.modelDownloadURL) else { return }

        modelDownloadState = .downloading(progress: 0)

        let session = URLSession(configuration: .default, delegate: nil, delegateQueue: nil)
        let task = session.downloadTask(with: url) { [weak self] tempURL, response, error in
            Task { @MainActor in
                guard let self else { return }
                if let error {
                    self.modelDownloadState = .failed(message: error.localizedDescription)
                    return
                }
                guard let tempURL else {
                    self.modelDownloadState = .failed(message: "No file received")
                    return
                }
                do {
                    let dest = URL(fileURLWithPath: self.modelFilePath)
                    if FileManager.default.fileExists(atPath: dest.path) {
                        try FileManager.default.removeItem(at: dest)
                    }
                    try FileManager.default.moveItem(at: tempURL, to: dest)
                    self.modelDownloadState = .downloaded(path: dest.path)
                } catch {
                    self.modelDownloadState = .failed(message: error.localizedDescription)
                }
            }
        }

        // Observe progress
        let observation = task.progress.observe(\.fractionCompleted) { [weak self] progress, _ in
            Task { @MainActor in
                guard let self else { return }
                self.modelDownloadState = .downloading(progress: progress.fractionCompleted)
            }
        }
        // Keep observation alive until task completes
        Task {
            while !task.progress.isFinished && !task.progress.isCancelled {
                try? await Task.sleep(for: .milliseconds(100))
            }
            withExtendedLifetime(observation) {}
        }

        downloadTask = task
        task.resume()
    }

    func cancelDownload() {
        downloadTask?.cancel()
        downloadTask = nil
        modelDownloadState = .notDownloaded
    }

    func deleteModel() {
        try? FileManager.default.removeItem(atPath: modelFilePath)
        modelDownloadState = .notDownloaded
        if provider == .llamacpp {
            agent = nil
        }
    }

    // MARK: - Chat

    func send() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let agent else { return }

        messages.append(ChatMessage(role: .user, text: text))
        inputText = ""
        isLoading = true
        errorMessage = nil

        Task.detached { [weak self] in
            do {
                let response = try agent.step(userInput: text)
                await MainActor.run {
                    guard let self else { return }
                    self.messages.append(ChatMessage(role: .assistant, text: response.content))
                    self.contextPercent = Int(response.contextPercent)
                    self.isLoading = false
                    if self.ttsEnabled {
                        self.speak(response.content)
                    }
                }
            } catch {
                await MainActor.run {
                    guard let self else { return }
                    self.errorMessage = error.localizedDescription
                    self.isLoading = false
                }
            }
        }
    }

    func reset() {
        agent?.reset()
        messages.removeAll()
    }

    func stopSpeaking() {
        synthesizer.stopSpeaking(at: .immediate)
        isSpeaking = false
    }

    // MARK: - Speech Recognition

    func toggleListening() {
        if isListening {
            stopListening()
        } else {
            startListening()
        }
    }

    private func startListening() {
        if isSpeaking { stopSpeaking() }

        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            Task { @MainActor in
                guard let self else { return }
                guard status == .authorized else {
                    self.errorMessage = "Speech recognition not authorized."
                    return
                }
                self.beginRecording()
            }
        }
    }

    private func beginRecording() {
        recognitionTask?.cancel()
        recognitionTask = nil

        let locale = language == "ja" ? Locale(identifier: "ja-JP") : Locale(identifier: "en-US")
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
            errorMessage = "Speech recognizer not available for \(language)."
            return
        }

        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        recognitionRequest = request

        let audioSession = AVAudioSession.sharedInstance()
        do {
            try audioSession.setCategory(.record, mode: .measurement, options: .duckOthers)
            try audioSession.setActive(true, options: .notifyOthersOnDeactivation)
        } catch {
            errorMessage = "Audio session error: \(error.localizedDescription)"
            return
        }

        let inputNode = audioEngine.inputNode
        let recordingFormat = inputNode.outputFormat(forBus: 0)
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: recordingFormat) { buffer, _ in
            request.append(buffer)
        }

        audioEngine.prepare()
        do {
            try audioEngine.start()
            isListening = true
            liveTranscript = ""
        } catch {
            errorMessage = "Audio engine error: \(error.localizedDescription)"
            return
        }

        recognitionTask = recognizer.recognitionTask(with: request) { [weak self] result, error in
            Task { @MainActor in
                guard let self else { return }
                if let result {
                    self.liveTranscript = result.bestTranscription.formattedString
                }
            }
        }
    }

    func stopListening() {
        audioEngine.stop()
        audioEngine.inputNode.removeTap(onBus: 0)
        recognitionRequest?.endAudio()
        recognitionRequest = nil
        recognitionTask?.cancel()
        recognitionTask = nil
        isListening = false

        let text = liveTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty {
            inputText = text
            liveTranscript = ""
            send()
        } else {
            liveTranscript = ""
        }
    }

    private func speak(_ text: String) {
        guard ttsEnabled, !text.isEmpty else { return }

        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = ttsVoice
        utterance.rate = ttsRate

        isSpeaking = true
        synthesizer.speak(utterance)
    }
}

/// Delegate to track TTS completion
private class TTSDelegate: NSObject, AVSpeechSynthesizerDelegate {
    let onFinish: () -> Void
    init(onFinish: @escaping () -> Void) { self.onFinish = onFinish }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        Task { @MainActor in onFinish() }
    }
    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        Task { @MainActor in onFinish() }
    }
}
