using YamlDotNet.Serialization;

namespace KesselCli;

/// Mirror of the kessel-cli YAML config (configs/*.yaml). Only the fields the
/// Windows CLI needs are mapped; unknown keys are ignored.
public sealed class AppConfig
{
    [YamlMember(Alias = "llm")]
    public LlmSection Llm { get; set; } = new();

    [YamlMember(Alias = "agent")]
    public AgentSection Agent { get; set; } = new();

    [YamlMember(Alias = "stt")]
    public SttSection? Stt { get; set; }

    [YamlMember(Alias = "mcpServers")]
    public List<McpServerEntry>? McpServers { get; set; }

    public sealed class SttSection
    {
        [YamlMember(Alias = "locale")]
        public string? Locale { get; set; }
    }

    /// BCP-47 culture for speech recognition / synthesis: explicit stt.locale,
    /// else derived from agent.language (ja -> ja-JP, otherwise en-US).
    public string SpeechCulture =>
        !string.IsNullOrWhiteSpace(Stt?.Locale) ? Stt!.Locale!
        : Agent.Language == "ja" ? "ja-JP"
        : "en-US";

    public sealed class LlmSection
    {
        [YamlMember(Alias = "modelPath")]
        public string? ModelPath { get; set; }

        [YamlMember(Alias = "baseURL")]
        public string BaseUrl { get; set; } = "https://api.openai.com/v1";

        [YamlMember(Alias = "model")]
        public string Model { get; set; } = "gpt-5.6-luna";

        [YamlMember(Alias = "apiKey")]
        public string? ApiKey { get; set; }

        [YamlMember(Alias = "harmonyTemplate")]
        public bool HarmonyTemplate { get; set; }

        [YamlMember(Alias = "temperature")]
        public float? Temperature { get; set; }

        [YamlMember(Alias = "maxTokens")]
        public int MaxTokens { get; set; } = 2048;

        [YamlMember(Alias = "contextWindow")]
        public int? ContextWindow { get; set; }

        [YamlMember(Alias = "reasoningEffort")]
        public string? ReasoningEffort { get; set; }

        // Local inference backend for modelPath: "llamacpp" (default) or
        // "gallium". Overridable at runtime by the INFERENCE_ENGINE env var.
        [YamlMember(Alias = "inferenceEngine")]
        public string? InferenceEngine { get; set; }
    }

    public sealed class AgentSection
    {
        [YamlMember(Alias = "systemPromptPath")]
        public string? SystemPromptPath { get; set; }

        [YamlMember(Alias = "maxTurns")]
        public int MaxTurns { get; set; } = 50;

        [YamlMember(Alias = "language")]
        public string Language { get; set; } = "en";
    }

    public sealed class McpServerEntry
    {
        [YamlMember(Alias = "command")]
        public string Command { get; set; } = "";

        [YamlMember(Alias = "args")]
        public List<string> Args { get; set; } = [];

        /// If set, connect over Streamable HTTP to this URL instead of spawning command.
        [YamlMember(Alias = "url")]
        public string? Url { get; set; }
    }

    /// The default user config: ~/.cache/kessel/config.yml.
    public static string UserConfigPath()
    {
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        return Path.Combine(home, ".cache", "kessel", "config.yml");
    }

    /// Load config from an explicit path, or the first existing default candidate.
    /// If nothing is found, a starter config is written to the user config path.
    public static (AppConfig Config, string? Path) Load(string? explicitPath)
    {
        var userCfg = UserConfigPath();
        var candidates = new[]
        {
            explicitPath,
            Environment.GetEnvironmentVariable("KESSEL_CONFIG"),
            userCfg,
            Path.ChangeExtension(userCfg, ".yaml"),    // accept .yaml too
            Path.Combine(Directory.GetCurrentDirectory(), "configs", "default.yaml"),
            FindInAncestors(AppContext.BaseDirectory, Path.Combine("configs", "default.yaml")),
        };

        foreach (var path in candidates)
        {
            if (path is not null && File.Exists(path))
            {
                return (Deserialize(File.ReadAllText(path)), Path.GetFullPath(path));
            }
        }

        // Nothing found anywhere: scaffold a starter config at the user path so the
        // user has a template to edit (LLM, TTS/STT, and MCP servers).
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(userCfg)!);
            File.WriteAllText(userCfg, DefaultConfigYaml);
            Console.Error.WriteLine($"[info] Wrote a starter config to {userCfg}");
            return (Deserialize(DefaultConfigYaml), Path.GetFullPath(userCfg));
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[warn] Could not create {userCfg}: {ex.Message}");
            return (new AppConfig(), null);
        }
    }

    private static AppConfig Deserialize(string yaml) =>
        new DeserializerBuilder()
            .IgnoreUnmatchedProperties()
            .Build()
            .Deserialize<AppConfig>(yaml) ?? new AppConfig();

    /// Starter config written to ~/.cache/kessel/config.yml on first run.
    private const string DefaultConfigYaml =
        """
        # Kessel configuration (~/.cache/kessel/config.yml)
        # API key: fill apiKey below, set OPENAI_API_KEY in the environment, or put
        # it in a local .env file (project dir, the exe's dir, or ~/.cache/kessel/.env).

        llm:
          baseURL: "https://api.openai.com/v1"
          model: "gpt-5.6-luna"
          apiKey: ""
          maxTokens: 8192
          reasoningEffort: "high"   # reasoning models: low | medium | high
          # For a local GGUF model instead, set modelPath (a local path or an
          # hf: spec that auto-downloads) and remove baseURL, e.g.:
          # modelPath: "hf:LiquidAI/LFM2.5-8B-A1B-GGUF/LFM2.5-8B-A1B-Q4_K_M.gguf"

        agent:
          maxTurns: 50
          language: "en"            # "en" or "ja"

        tts:
          enabled: false

        stt:
          enabled: false
          locale: "en-US"           # BCP-47 locale for speech recognition / TTS

        # MCP servers to spawn and expose as tools (stdio JSON-RPC).
        # Uncomment and edit; each server's tools become available to the agent.
        # mcpServers:
        #   - command: "godevmcp"
        #     args: ["serve"]
        """;

    private static string? FindInAncestors(string start, string relative)
    {
        for (var dir = new DirectoryInfo(start); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, relative);
            if (File.Exists(candidate)) return candidate;
        }
        return null;
    }
}
