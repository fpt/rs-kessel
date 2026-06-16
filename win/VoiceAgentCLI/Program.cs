using uniffi.agent_core;
using VoiceAgentCLI;

// ── Parse arguments ───────────────────────────────────────────────────────────
string? configPath = null;
for (int i = 0; i < args.Length; i++)
{
    switch (args[i])
    {
        case "--config" when i + 1 < args.Length:
            configPath = args[++i];
            break;
        case "--help" or "-h":
            PrintHelp();
            return 0;
    }
}

// ── Load configuration ────────────────────────────────────────────────────────
var (cfg, resolvedConfigPath) = AppConfig.Load(configPath);
if (resolvedConfigPath is not null)
    Console.Error.WriteLine($"[info] Loaded configuration from {resolvedConfigPath}");
else
    Console.Error.WriteLine("[warn] Config file not found, using defaults");

// API key falls back to the OPENAI_API_KEY environment variable.
var apiKey = cfg.Llm.ApiKey;
if (string.IsNullOrWhiteSpace(apiKey))
    apiKey = Environment.GetEnvironmentVariable("OPENAI_API_KEY");

var mcpServers = (cfg.McpServers ?? [])
    .Select(s => new McpServerConfig(s.Command, s.Args))
    .ToList();

var agentConfig = new AgentConfig(
    @modelPath: cfg.Llm.ModelPath,
    @baseUrl: cfg.Llm.BaseUrl,
    @model: cfg.Llm.Model,
    @apiKey: apiKey,
    @useHarmonyTemplate: cfg.Llm.HarmonyTemplate,
    @temperature: cfg.Llm.Temperature,
    @maxTokens: (uint)cfg.Llm.MaxTokens,
    @contextWindow: (uint)(cfg.Llm.ContextWindow ?? 128_000),
    @language: cfg.Agent.Language,
    @workingDir: Directory.GetCurrentDirectory(),
    @reasoningEffort: cfg.Llm.ReasoningEffort,
    @mcpServers: mcpServers);

Agent agent;
try
{
    agent = AgentCoreMethods.AgentNew(agentConfig);
}
catch (Exception ex)
{
    Console.Error.WriteLine($"[error] Failed to initialize agent: {ex.Message}");
    return 1;
}

// ── Load system prompt (with {language} template substitution) ─────────────────
if (cfg.Agent.SystemPromptPath is { } promptPath && resolvedConfigPath is not null)
{
    var resolved = Path.IsPathRooted(promptPath)
        ? promptPath
        : Path.Combine(Path.GetDirectoryName(resolvedConfigPath)!, promptPath);
    try
    {
        var prompt = File.ReadAllText(resolved);
        var languagePrompt = cfg.Agent.Language switch
        {
            "ja" => "日本語で回答してください。",
            "en" => "",
            var l => $"Respond in {l}.",
        };
        prompt = prompt.Replace("{language}", languagePrompt);
        agent.SetSystemPrompt(prompt);
        Console.Error.WriteLine($"[info] Loaded system prompt from {resolved}");
    }
    catch (Exception ex)
    {
        Console.Error.WriteLine($"[warn] Failed to load system prompt: {ex.Message}");
    }
}

// ── Text REPL ─────────────────────────────────────────────────────────────────
Console.WriteLine($"""

===========================================
  Voice Agent - Windows CLI (text mode)
===========================================

Model: {cfg.Llm.Model}
Endpoint: {cfg.Llm.BaseUrl}

Type your messages below. Commands:
  /reset    - Clear conversation history
  /quit     - Exit the program
  /help     - Show this help
  /history  - Show conversation history

===========================================

""");

int turnCount = 0;
int maxTurns = cfg.Agent.MaxTurns;

while (turnCount < maxTurns)
{
    Console.Write("You: ");
    var line = Console.ReadLine();
    if (line is null) break; // EOF
    // Strip a leading UTF-8 BOM (can appear on the first piped line) before trimming.
    var input = line.TrimStart('﻿').Trim();
    if (input.Length == 0) continue;

    if (input.StartsWith('/'))
    {
        switch (input)
        {
            case "/quit" or "/exit":
                Console.WriteLine("Goodbye!");
                return 0;
            case "/help":
                PrintHelp();
                break;
            case "/reset":
                agent.Reset();
                Console.WriteLine("Conversation history cleared.\n");
                break;
            case "/history":
                Console.WriteLine("Conversation History:");
                Console.WriteLine(agent.GetConversationHistory());
                Console.WriteLine();
                break;
            default:
                Console.WriteLine($"Unknown command: {input}");
                Console.WriteLine("Type /help for available commands.\n");
                break;
        }
        continue;
    }

    try
    {
        var resp = agent.Step(input);
        if (!string.IsNullOrEmpty(resp.reasoning))
            Console.WriteLine($"[90m💭 {resp.reasoning}[0m\n");
        Console.WriteLine($"Assistant: {resp.content}");
        Console.WriteLine($"[90m[{(int)resp.contextPercent}% context][0m\n");
        turnCount++;
    }
    catch (Exception ex)
    {
        Console.Error.WriteLine($"[error] {ex.Message}\n");
    }
}

return 0;

static void PrintHelp()
{
    Console.WriteLine("""
    Voice Agent - Windows CLI

    Usage: voice-agent [OPTIONS]

    Options:
        --config PATH      Path to configuration file (default: configs/default.yaml)
        --help, -h         Show this help message

    REPL commands:
        /reset    Clear conversation history
        /history  Show conversation history
        /help     Show this help
        /quit     Exit
    """);
}
