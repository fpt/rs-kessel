using uniffi.kessel_core;
using KesselCli;

// Render Unicode (Japanese, emoji, …) instead of '?'. The default console code
// page can't represent non-ASCII; UTF-8 (no BOM) fixes both input and output.
try { Console.OutputEncoding = new System.Text.UTF8Encoding(false); } catch { /* redirected */ }
try { Console.InputEncoding = new System.Text.UTF8Encoding(false); } catch { /* redirected input */ }

// Load a local .env (project root / exe dir / ~/.cache/kessel) so keys like
// OPENAI_API_KEY can live in a file. Real env vars are not overridden. Done
// before config load so .env may also supply KESSEL_CONFIG.
foreach (var envFile in DotEnv.Load())
    Console.Error.WriteLine($"[info] Loaded environment from {envFile}");

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
    .Select(s => new McpServerConfig(s.Command, s.Args, s.Url))
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
    @inferenceEngine: cfg.Llm.InferenceEngine,
    @mcpServers: mcpServers);

Agent agent;
try
{
    agent = KesselCoreMethods.AgentNew(agentConfig);
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

// ── Banner ──────────────────────────────────────────────────────────────────
var (modelLine, endpointLine) = string.IsNullOrEmpty(cfg.Llm.ModelPath)
    ? ($"Model: {cfg.Llm.Model}", $"Endpoint: {cfg.Llm.BaseUrl}")
    : ($"Model: {Path.GetFileName(cfg.Llm.ModelPath)}", "Endpoint: local (in-process llama.cpp)");

Console.WriteLine($"""

===========================================
  Kessel - Windows CLI
===========================================

{modelLine}
{endpointLine}

Shift+Tab cycles mode:  text ⇄ listen
  text    type messages; replies are printed
  listen  speak (STT); replies are printed AND spoken (TTS)

Commands: /listen (one phrase)  /reset  /history  /help  /quit

===========================================

""");

int turnCount = 0;
int maxTurns = cfg.Agent.MaxTurns;
VoiceOutput? voice = null;
var speechCulture = cfg.SpeechCulture; // BCP-47 for STT + TTS (e.g. "en-US")

// Piped/redirected stdin (e.g. the testsuite) can't use ReadKey, so it gets the
// simple line loop with no mode switching. An interactive terminal gets the
// key-level loop with Shift+Tab toggling.
if (Console.IsInputRedirected)
    RunPiped();
else
    RunInteractive();

voice?.Dispose();
return 0;

// ── Loops ─────────────────────────────────────────────────────────────────────

void RunPiped()
{
    while (turnCount < maxTurns)
    {
        Console.Write("You: ");
        var line = Console.ReadLine();
        if (line is null) break; // EOF
        var input = line.TrimStart('﻿').Trim(); // strip leading BOM on first piped line
        if (input.Length == 0) continue;

        var res = HandleCommand(input, speak: false);
        if (res == CommandResult.Quit) { Console.WriteLine("Goodbye!"); return; }
        if (res == CommandResult.Handled) continue;
        ProcessMessage(input, speak: false);
    }
}

void RunInteractive()
{
    bool listening = false;
    while (turnCount < maxTurns)
    {
        if (!listening)
        {
            var (line, toggled) = ReadLineOrToggle("You: ");
            if (toggled)
            {
                listening = true;
                Console.WriteLine("\n🎧 listen mode — speak; press Shift+Tab to return to text.\n");
                continue;
            }
            if (line is null) break;
            var input = line.Trim();
            if (input.Length == 0) continue;

            var res = HandleCommand(input, speak: false);
            if (res == CommandResult.Quit) { Console.WriteLine("Goodbye!"); return; }
            if (res == CommandResult.Handled) continue;
            ProcessMessage(input, speak: false);
        }
        else
        {
            var (outcome, heard) = SpeechInput.Listen(speechCulture, TogglePressed);
            if (outcome != ListenOutcome.Recognized)
            {
                listening = false;
                Console.WriteLine(outcome == ListenOutcome.Unavailable
                    ? "\n⌨️  back to text mode (speech unavailable).\n"
                    : "\n⌨️  text mode.\n");
                continue;
            }
            if (string.IsNullOrWhiteSpace(heard)) continue;
            Console.WriteLine($"You (voice): {heard}\n");
            ProcessMessage(heard!, speak: true);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

// Run one user turn through the agent; print the reply and optionally speak it.
void ProcessMessage(string text, bool speak)
{
    try
    {
        var resp = agent.Step(text);
        if (!string.IsNullOrEmpty(resp.reasoning))
            Console.WriteLine($"[90m💭 {resp.reasoning}[0m\n");
        Console.WriteLine($"Assistant: {resp.content}");
        Console.WriteLine($"[90m[{(int)resp.contextPercent}% context][0m\n");
        if (speak)
        {
            voice ??= new VoiceOutput(speechCulture);
            voice.Speak(resp.content);
        }
        turnCount++;
    }
    catch (Exception ex)
    {
        Console.Error.WriteLine($"[error] {ex.Message}\n");
    }
}

// Returns true if Shift+Tab is the next available key (consumes one key if any).
bool TogglePressed()
{
    if (!Console.KeyAvailable) return false;
    var k = Console.ReadKey(intercept: true);
    return k.Key == ConsoleKey.Tab && k.Modifiers.HasFlag(ConsoleModifiers.Shift);
}

// Minimal line editor that also reports a Shift+Tab toggle.
(string? Line, bool Toggled) ReadLineOrToggle(string prompt)
{
    Console.Write(prompt);
    var sb = new System.Text.StringBuilder();
    while (true)
    {
        var key = Console.ReadKey(intercept: true);
        if (key.Key == ConsoleKey.Tab && key.Modifiers.HasFlag(ConsoleModifiers.Shift))
        {
            Console.WriteLine();
            return (sb.ToString(), true);
        }
        switch (key.Key)
        {
            case ConsoleKey.Enter:
                Console.WriteLine();
                return (sb.ToString(), false);
            case ConsoleKey.Backspace:
                if (sb.Length > 0) { sb.Length--; Console.Write("\b \b"); }
                continue;
        }
        if (!char.IsControl(key.KeyChar))
        {
            sb.Append(key.KeyChar);
            Console.Write(key.KeyChar);
        }
    }
}

// Handle a slash command. `speak` controls TTS for the one-shot /listen reply.
CommandResult HandleCommand(string input, bool speak)
{
    if (!input.StartsWith('/')) return CommandResult.NotCommand;
    switch (input)
    {
        case "/quit" or "/exit":
            return CommandResult.Quit;
        case "/help":
            PrintHelp();
            return CommandResult.Handled;
        case "/reset":
            agent.Reset();
            Console.WriteLine("Conversation history cleared.\n");
            return CommandResult.Handled;
        case "/history":
            Console.WriteLine("Conversation History:");
            Console.WriteLine(agent.GetConversationHistory());
            Console.WriteLine();
            return CommandResult.Handled;
        case "/listen":
            if (Console.IsInputRedirected)
            {
                Console.WriteLine("/listen needs an interactive terminal.\n");
                return CommandResult.Handled;
            }
            Console.WriteLine("🎤 Listening… (speak now)");
            var oneShot = SpeechInput.RecognizeOnce(speechCulture);
            if (string.IsNullOrWhiteSpace(oneShot))
            {
                Console.WriteLine("(no speech detected)\n");
                return CommandResult.Handled;
            }
            Console.WriteLine($"You (voice): {oneShot}\n");
            ProcessMessage(oneShot, speak);
            return CommandResult.Handled;
        default:
            Console.WriteLine($"Unknown command: {input}");
            Console.WriteLine("Type /help for available commands.\n");
            return CommandResult.Handled;
    }
}

static void PrintHelp()
{
    Console.WriteLine("""
    Kessel - Windows CLI

    Usage: kessel-cli [OPTIONS]

    Options:
        --config PATH      Path to configuration file (default: configs/default.yaml)
        --help, -h         Show this help message

    Modes (interactive only): Shift+Tab cycles text ⇄ listen.
        text    type messages; replies printed
        listen  speak (STT in); replies printed and spoken (TTS out)

    REPL commands:
        /listen   Speak one phrase (one-shot, stays in current mode)
        /reset    Clear conversation history
        /history  Show conversation history
        /help     Show this help
        /quit     Exit
    """);
}

enum CommandResult
{
    NotCommand,
    Handled,
    Quit,
}
