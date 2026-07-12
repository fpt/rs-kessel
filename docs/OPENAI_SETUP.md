# Using OpenAI API with Kessel

This guide explains how to use OpenAI's cloud API instead of local llama.cpp.

## Overview

The kessel supports two LLM backends via Rust feature flags:
- **llamacpp** (default): Local llama.cpp server
- **openai**: OpenAI cloud API

## Prerequisites

1. OpenAI API key from https://platform.openai.com/api-keys
2. Rust toolchain
3. Swift toolchain

## Setup Steps

### 1. Build with OpenAI Feature

```bash
# Navigate to Rust workspace
cd crates

# Build with OpenAI feature (disable default llamacpp)
cargo build --release --no-default-features --features openai

# Go back to root
cd ..
```

### 2. Regenerate UniFFI Bindings

```bash
# Regenerate Swift bindings for the new library
bash scripts/gen_uniffi.sh
```

### 3. Build Swift CLI

```bash
cd swift
swift build
```

### 4. Set Environment Variable

The agent reads the API key from the `OPENAI_API_KEY` environment variable:

```bash
# Export in your shell
export OPENAI_API_KEY=sk-proj-...your-key-here...

# Or set inline when running
OPENAI_API_KEY=sk-... make run-text
```

**Priority order**:
1. `OPENAI_API_KEY` environment variable (highest priority)
2. `apiKey` field in config.yaml
3. `null` (for local provider)

### 5. Use OpenAI Configuration

```bash
# Text mode
OPENAI_API_KEY=sk-... swift run kessel-cli --config ../configs/openai.yaml

# Or use the Makefile target
OPENAI_API_KEY=sk-... make run-text
```

## Configuration

### configs/openai.yaml

```yaml
llm:
  baseURL: "https://api.openai.com/v1"
  model: "gpt-5.6-luna"  # or gpt-5.6-sol, gpt-5.6-terra
  apiKey: ""  # Leave empty - uses OPENAI_API_KEY env var
  harmonyTemplate: false  # OpenAI doesn't use Harmony
  temperature: 0.7
  maxTokens: 2048

agent:
  maxTurns: 50
  autoListen: false  # Set true for voice mode

tts:
  enabled: true

stt:
  enabled: true
  model: "base"
```

**Important differences from llama.cpp**:
- `baseURL`: Points to OpenAI API
- `harmonyTemplate`: Set to `false` (OpenAI models don't use Harmony)
- `apiKey`: Leave empty (read from environment)

## Available Models

Current OpenAI models:
- `gpt-5.6-luna` — the default used by `configs/openai.yaml`
- `gpt-5.6-sol`
- `gpt-5.6-terra`

See https://platform.openai.com/docs/models for the full list, current
capabilities, and pricing.

## Usage Examples

### Text Mode

```bash
export OPENAI_API_KEY=sk-...

# Run with OpenAI config
swift run kessel-cli --config ../configs/openai.yaml

# Example conversation
You: What is 2+2?
Assistant: 2+2 equals 4.

You: /quit
Goodbye!
```

### Voice Mode

Update `configs/openai.yaml`:
```yaml
agent:
  autoListen: true
```

Then run:
```bash
export OPENAI_API_KEY=sk-...
swift run kessel-cli --config ../configs/openai.yaml

# Now speak naturally
# Agent will listen -> transcribe -> call OpenAI -> speak response
```

## Switching Back to Local llama.cpp

To switch back to local mode:

```bash
# 1. Rebuild with llamacpp feature
cd crates
cargo build --release --features llamacpp

# 2. Regenerate bindings
cd ..
bash scripts/gen_uniffi.sh

# 3. Rebuild Swift
cd swift
swift build

# 4. Run with default config
make run
```

## Architecture

### How it Works

```
Swift CLI (main.swift)
    ↓
1. Read OPENAI_API_KEY from environment
    ↓
2. Pass to Rust via AgentConfig
    ↓
Rust Agent (lib.rs)
    ↓
3. Create provider based on feature flag
    ↓
OpenAiProvider (llm.rs)
    ↓
4. HTTP POST to api.openai.com/v1/chat/completions
    ↓
5. Return response to Swift
```

### Code Flow

**Swift** (`swift/Sources/KesselCli/main.swift`):
```swift
// Check environment variable first
let apiKey: String? = {
    if let envKey = ProcessInfo.processInfo.environment["OPENAI_API_KEY"] {
        return envKey
    } else {
        return config.llm.apiKey
    }
}()

let agentConfig = AgentConfig(
    baseUrl: config.llm.baseURL,
    model: config.llm.model,
    apiKey: apiKey,  // Passed to Rust
    //...
)
```

**Rust** (`crates/agent-core/src/lib.rs`):
```rust
pub fn agent_new(config: AgentConfig) -> Result<Arc<Agent>> {
    // Factory chooses provider based on feature flag
    let client = create_provider(
        config.base_url,
        config.model,
        config.api_key,  // API key from Swift
        config.temperature,
        config.max_tokens,
    );
    // ...
}
```

**Rust** (`crates/agent-core/src/llm.rs`):
```rust
#[cfg(feature = "openai")]
pub fn create_provider(...) -> Box<dyn LlmProvider> {
    let api_key = api_key.expect("OpenAI requires API key");
    Box::new(OpenAiProvider::new(api_key, model, ...))
}

impl LlmProvider for OpenAiProvider {
    fn chat(&self, messages: &[ChatMessage]) -> Result<String> {
        let response = ureq::post("https://api.openai.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(&request)?
            .into_json()?;
        Ok(response.choices.first().unwrap().message.content)
    }
}
```

## Troubleshooting

### "OpenAI provider requires API key"

**Problem**: Rust panics with this error at startup.

**Solution**: Set the `OPENAI_API_KEY` environment variable:
```bash
export OPENAI_API_KEY=sk-...
```

### "No API key provided (using local provider)"

**Problem**: This log message appears but you expect OpenAI.

**Solution**:
1. Check environment variable is set: `echo $OPENAI_API_KEY`
2. Verify it's not empty
3. Rebuild Swift if you just set it: `swift build`

### "Failed to chat: Network error"

**Problem**: Can't connect to OpenAI API.

**Possible causes**:
1. Invalid API key - check on platform.openai.com
2. Network connectivity issues
3. Rate limit exceeded
4. Wrong baseURL in config

### "library 'kessel_core' not found"

**Problem**: Swift can't find the Rust library.

**Solution**: You forgot to rebuild Rust with the OpenAI feature:
```bash
cd crates
cargo build --release --no-default-features --features openai
bash ../scripts/gen_uniffi.sh
```

## Cost Considerations

OpenAI charges per token. Rates change and differ per model, so check
https://openai.com/api/pricing for current numbers rather than trusting a table
here.

What drives cost in this app:
- Each voice interaction = ~200-500 tokens
- Backchannel triggers do NOT call the LLM (state-only)

**Tip**: Use `maxTokens` to control response length and costs.

## Security

### Best Practices

1. **Never commit API keys**:
   ```bash
   # Add to .gitignore
   echo "*.key" >> .gitignore
   echo ".env" >> .gitignore
   ```

2. **Use environment variables**:
   ```bash
   # In your shell profile (~/.zshrc or ~/.bashrc)
   export OPENAI_API_KEY=sk-...
   ```

3. **Use project-specific keys**:
   - Create separate keys for different projects
   - Revoke keys when done testing

4. **Monitor usage**:
   - Check https://platform.openai.com/usage
   - Set spending limits

## FAQ

**Q: Can I use both llamacpp and openai?**
A: Not simultaneously. You must rebuild Rust with the desired feature flag.

**Q: Does backchannel work with OpenAI?**
A: Yes! Backchannel responses are handled by the rule-based state updater, which doesn't call the LLM. Only the final user utterance calls OpenAI.

**Q: Can I use other OpenAI-compatible APIs?**
A: Yes! Just change the `baseURL` in your config:
```yaml
llm:
  baseURL: "https://your-api.com/v1"
  model: "your-model"
```

**Q: How do I use GPT-4 with vision?**
A: The current implementation only supports text. Vision support would require extending the `ChatMessage` struct to support image content.

**Q: Can I use Azure OpenAI?**
A: Not currently. Azure uses a different authentication method (api-key header instead of Bearer token). Would need code changes.

## Next Steps

- [Configuration Reference](./CONFIGURATION.md)
- [Backchannel Responses](./BACKCHANNEL_SUCCESS.md)
- [Development Guide](../CLAUDE.md)
