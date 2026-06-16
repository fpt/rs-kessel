using YamlDotNet.Serialization;

namespace VoiceAgentCLI;

/// Mirror of the voice-agent YAML config (configs/*.yaml). Only the fields the
/// Windows CLI needs are mapped; unknown keys are ignored.
public sealed class AppConfig
{
    [YamlMember(Alias = "llm")]
    public LlmSection Llm { get; set; } = new();

    [YamlMember(Alias = "agent")]
    public AgentSection Agent { get; set; } = new();

    [YamlMember(Alias = "mcpServers")]
    public List<McpServerEntry>? McpServers { get; set; }

    public sealed class LlmSection
    {
        [YamlMember(Alias = "modelPath")]
        public string? ModelPath { get; set; }

        [YamlMember(Alias = "baseURL")]
        public string BaseUrl { get; set; } = "https://api.openai.com/v1";

        [YamlMember(Alias = "model")]
        public string Model { get; set; } = "gpt-5.4-mini";

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
    }

    /// Load config from an explicit path, or the first existing default candidate.
    public static (AppConfig Config, string? Path) Load(string? explicitPath)
    {
        var candidates = new[]
        {
            explicitPath,
            Environment.GetEnvironmentVariable("VOICE_AGENT_CONFIG"),
            Path.Combine(Directory.GetCurrentDirectory(), "configs", "default.yaml"),
            FindInAncestors(AppContext.BaseDirectory, Path.Combine("configs", "default.yaml")),
        };

        foreach (var path in candidates)
        {
            if (path is not null && File.Exists(path))
            {
                var yaml = File.ReadAllText(path);
                var cfg = new DeserializerBuilder()
                    .IgnoreUnmatchedProperties()
                    .Build()
                    .Deserialize<AppConfig>(yaml) ?? new AppConfig();
                return (cfg, Path.GetFullPath(path));
            }
        }

        return (new AppConfig(), null);
    }

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
