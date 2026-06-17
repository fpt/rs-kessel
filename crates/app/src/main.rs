//! Simple text-mode REPL for testing tool calling without Swift/STT/TTS.
//!
//! Usage:
//!   # With local model (no server needed):
//!   MODEL_PATH=/path/to/model.gguf cargo run -p kessel-cli
//!
//!   # With OpenAI:
//!   OPENAI_API_KEY=sk-... cargo run -p kessel-cli
//!
//!   # One-shot mode (for integration tests):
//!   echo "Read the file configs/default.yaml" | MODEL_PATH=... cargo run -p kessel-cli

use kessel_core::{ChatMessage, create_provider};
use kessel_core::tool::ToolAccess;

use std::io::{self, BufRead};

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Configuration from environment
    let model_path = std::env::var("MODEL_PATH").ok();
    let base_url = std::env::var("LLM_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("LLM_MODEL")
        .unwrap_or_else(|_| "gpt-5.4-mini".to_string());
    let api_key = std::env::var("OPENAI_API_KEY").ok();
    let working_dir = std::env::var("WORKING_DIR")
        .unwrap_or_else(|_| std::env::current_dir().unwrap().to_string_lossy().to_string());
    let max_tokens: u32 = std::env::var("MAX_TOKENS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2048);
    let max_react_iterations: u32 = std::env::var("MAX_REACT_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    // Create provider (no temperature for models like gpt-5.4-mini that don't support it)
    let temperature: Option<f32> = std::env::var("LLM_TEMPERATURE")
        .ok()
        .and_then(|s| s.parse().ok());
    let reasoning_effort = std::env::var("REASONING_EFFORT").ok();
    let client = create_provider(
        model_path.clone(),
        base_url.clone(),
        model.clone(),
        api_key.clone(),
        temperature,
        max_tokens,
        reasoning_effort,
    )
    .expect("Failed to create LLM provider");

    // Create tool registry
    let skill_registry = std::sync::Arc::new(kessel_core::skill::SkillRegistry::new());
    let situation = std::sync::Arc::new(kessel_core::situation::SituationMessages::default());
    let mut tool_registry = kessel_core::tool::create_default_registry(
        std::path::PathBuf::from(&working_dir),
        skill_registry,
        situation,
    );

    // Connect MCP servers from MCP_SERVERS env (comma-separated "command arg1 arg2,...")
    if let Ok(mcp_spec) = std::env::var("MCP_SERVERS") {
        for entry in mcp_spec.split(',') {
            let parts: Vec<&str> = entry.trim().split_whitespace().collect();
            if let Some((cmd, args)) = parts.split_first() {
                match kessel_core::mcp_client::McpClient::connect(cmd, args) {
                    Ok(client) => {
                        for handler in client.tool_handlers() {
                            tool_registry.register(handler);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to connect MCP server '{}': {}", cmd, e);
                    }
                }
            }
        }
    }

    let provider_name = if model_path.is_some() {
        "Local (FFI)"
    } else if api_key.is_some() {
        "OpenAI"
    } else {
        "Unknown"
    };

    // Check if stdin is a pipe (one-shot mode) or terminal (interactive)
    let is_interactive = atty::is(atty::Stream::Stdin);

    if is_interactive {
        eprintln!("=== Text Agent (ReAct Tool Calling) ===");
        eprintln!("Provider: {} ({})", provider_name, model);
        eprintln!("Working dir: {}", working_dir);
        eprintln!("Tools: {:?}", tool_registry.get_definitions().iter().map(|t| &t.name).collect::<Vec<_>>());
        eprintln!("Type /quit to exit\n");
    }

    let mut messages: Vec<ChatMessage> = vec![
        ChatMessage::system(
            "You are a helpful assistant with access to tools. \
             Use tools when the user asks you to read files, find files, or manage tasks. \
             Be concise in your responses."
                .to_string(),
        ),
    ];

    let stdin = io::stdin();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let input = line.trim().to_string();

        if input.is_empty() {
            continue;
        }

        if input == "/quit" || input == "/exit" {
            break;
        }

        if input == "/reset" {
            messages.truncate(1); // Keep system prompt
            eprintln!("Conversation reset.");
            continue;
        }

        // Add user message
        messages.push(ChatMessage::user(input.clone()));

        if is_interactive {
            eprint!("Thinking...");
        }

        // Run ReAct loop
        let mut react_messages = messages.clone();

        let result = kessel_core::react::run(
            client.as_ref(),
            &mut react_messages,
            &tool_registry,
            Some(max_react_iterations),
        );

        if is_interactive {
            eprint!("\r            \r"); // Clear "Thinking..."
        }

        match result {
            Ok((response, reasoning, usage)) => {
                if let Some(ref thinking) = reasoning {
                    eprintln!("\x1b[90m💭 {}\x1b[0m", thinking);
                }
                println!("{}", response);
                if usage.total_tokens > 0 {
                    eprintln!(
                        "\x1b[90m📊 tokens: in={}, out={}, total={}\x1b[0m",
                        usage.input_tokens, usage.output_tokens, usage.total_tokens
                    );
                }

                // Add assistant response to conversation history
                messages.push(ChatMessage::assistant(response));
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }

        if is_interactive {
            println!();
        }
    }

    if is_interactive {
        eprintln!("Goodbye!");
    }
}
