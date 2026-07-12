# Environment Variable Setup - Implementation Summary

**Date**: 2025-10-15
**Status**: ✅ Complete

## Overview

Implemented environment variable support for passing the OpenAI API key from Swift to Rust. The `OPENAI_API_KEY` environment variable takes precedence over configuration files.

## Implementation

### 1. Swift: Environment Variable Reading

**File**: `swift/Sources/KesselCli/main.swift`

```swift
// Check for OPENAI_API_KEY environment variable (takes precedence over config)
let apiKey: String? = {
    if let envKey = ProcessInfo.processInfo.environment["OPENAI_API_KEY"], !envKey.isEmpty {
        logger.info("Using OPENAI_API_KEY from environment variable")
        return envKey
    } else if let configKey = config.llm.apiKey, !configKey.isEmpty {
        logger.info("Using API key from configuration file")
        return configKey
    } else {
        logger.info("No API key provided (using local provider)")
        return nil
    }
}()

let agentConfig = AgentConfig(
    baseUrl: config.llm.baseURL,
    model: config.llm.model,
    apiKey: apiKey,  // Passes to Rust via UniFFI
    useHarmonyTemplate: config.llm.harmonyTemplate,
    temperature: config.llm.temperature,
    maxTokens: UInt32(config.llm.maxTokens)
)
```

### 2. Rust: API Key Reception

**File**: `crates/agent-core/src/lib.rs`

The API key flows through the existing `AgentConfig` struct:

```rust
pub struct AgentConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,  // Received from Swift
    pub use_harmony_template: bool,
    pub temperature: f32,
    pub max_tokens: u32,
}

pub fn agent_new(config: AgentConfig) -> Result<Arc<Agent>, AgentError> {
    // Create provider based on feature flag
    let client = create_provider(
        config.base_url.clone(),
        config.model.clone(),
        config.api_key.clone(),  // Passed to provider
        config.temperature,
        config.max_tokens,
    );
    // ...
}
```

### 3. Provider Creation

**File**: `crates/agent-core/src/llm.rs`

```rust
#[cfg(feature = "openai")]
pub fn create_provider(
    base_url: String,
    model: String,
    api_key: Option<String>,
    temperature: f32,
    max_tokens: u32,
) -> Box<dyn LlmProvider> {
    let api_key = api_key.expect("OpenAI provider requires API key");
    Box::new(OpenAiProvider::new(
        api_key,
        model,
        temperature,
        max_tokens,
    ))
}

impl LlmProvider for OpenAiProvider {
    fn chat(&self, messages: &[ChatMessage]) -> Result<String> {
        let response = ureq::post("https://api.openai.com/v1/chat/completions")
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {}", self.api_key))  // Uses API key
            .send_json(&request)?
            .into_json()?;
        // ...
    }
}
```

## Priority Order

The API key is resolved in this order (highest to lowest priority):

1. **`OPENAI_API_KEY` environment variable** ← Highest priority
2. `apiKey` field in YAML config file
3. `null` (for local llama.cpp provider)

## Usage

### Setting the Environment Variable

**Option 1: Export in shell**
```bash
export OPENAI_API_KEY=sk-proj-...your-key...
swift run kessel-cli --config ../configs/openai.yaml
```

**Option 2: Inline**
```bash
OPENAI_API_KEY=sk-... swift run kessel-cli --config ../configs/openai.yaml
```

**Option 3: Shell profile** (~/.zshrc or ~/.bashrc)
```bash
# Add to ~/.zshrc
export OPENAI_API_KEY=sk-...

# Reload shell
source ~/.zshrc
```

**Option 4: .env file** (manual sourcing)
```bash
# Create .env file (gitignored)
echo "export OPENAI_API_KEY=sk-..." > .env

# Source before running
source .env
make run-text
```

### Configuration File

**configs/openai.yaml:**
```yaml
llm:
  baseURL: "https://api.openai.com/v1"
  model: "gpt-5.6-luna"
  apiKey: ""  # Leave empty - uses OPENAI_API_KEY env var
  harmonyTemplate: false
  temperature: 0.7
  maxTokens: 2048
```

## Testing

### Manual Test

```bash
# 1. Set API key
export OPENAI_API_KEY=sk-...

# 2. Build with OpenAI feature
cd crates
cargo build --release --no-default-features --features openai
cd ..

# 3. Regenerate bindings
bash scripts/gen_uniffi.sh

# 4. Build Swift
cd swift
swift build
cd ..

# 5. Test
echo "What is 2+2?" | swift run kessel-cli --config ../configs/openai.yaml
```

### Automated Test Script

```bash
# Run test script
OPENAI_API_KEY=sk-... bash scripts/test_openai.sh
```

## Logs

When running, you'll see one of these log messages:

```
✅ Using OPENAI_API_KEY from environment variable
✅ Using API key from configuration file
ℹ️  No API key provided (using local provider)
```

## Security Considerations

### ✅ Good Practices

1. **Use environment variables** (not config files):
   ```bash
   export OPENAI_API_KEY=sk-...
   ```

2. **Add to .gitignore**:
   ```bash
   echo ".env" >> .gitignore
   echo "*.key" >> .gitignore
   ```

3. **Never commit keys**:
   - Don't put keys in YAML files in version control
   - Use environment variables for sensitive data

4. **Use shell profiles for convenience**:
   ```bash
   # ~/.zshrc
   export OPENAI_API_KEY=sk-...
   ```

### ❌ Avoid

1. **Don't hardcode in config files**:
   ```yaml
   # BAD - will be committed to git
   apiKey: "sk-proj-actual-key-here"
   ```

2. **Don't pass as command-line argument**:
   ```bash
   # BAD - visible in process list
   swift run --api-key sk-...
   ```

3. **Don't echo or log API keys**:
   ```bash
   # BAD
   echo "Using key: $OPENAI_API_KEY"
   ```

## Architecture Flow

```
┌─────────────────────────────────────────────────┐
│ Environment / Shell                             │
│ export OPENAI_API_KEY=sk-...                    │
└─────────────────┬───────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────┐
│ Swift (main.swift)                              │
│ ProcessInfo.processInfo.environment[]           │
│   → Reads OPENAI_API_KEY                        │
│   → Priority: env var > config > null           │
│   → Logs which source is used                   │
└─────────────────┬───────────────────────────────┘
                  │
                  ↓ (via UniFFI)
┌─────────────────────────────────────────────────┐
│ Rust (lib.rs)                                   │
│ pub struct AgentConfig {                        │
│     api_key: Option<String>  ← Received         │
│ }                                                │
└─────────────────┬───────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────┐
│ Rust (llm.rs)                                   │
│ create_provider(api_key) → OpenAiProvider       │
│   → Validates api_key exists                    │
│   → Panics if missing for OpenAI                │
└─────────────────┬───────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────┐
│ HTTP Request                                    │
│ POST https://api.openai.com/v1/chat/completions │
│ Authorization: Bearer <api_key>                 │
└─────────────────────────────────────────────────┘
```

## Files Modified

1. **Swift CLI** - Environment variable reading:
   - `swift/Sources/KesselCli/main.swift`

2. **Rust Core** - Already supported via AgentConfig:
   - `crates/agent-core/src/lib.rs` (no changes needed)
   - `crates/agent-core/src/llm.rs` (no changes needed)

3. **Configuration**:
   - `configs/openai.yaml` (new)

4. **Documentation**:
   - `docs/OPENAI_SETUP.md` (new)
   - `docs/ENV_VAR_SETUP.md` (this file)

5. **Scripts**:
   - `scripts/test_openai.sh` (new)

## Benefits

1. **Security**: API keys never committed to version control
2. **Flexibility**: Easy to switch between different keys/environments
3. **Standard Practice**: Follows 12-factor app methodology
4. **CI/CD Friendly**: Can inject secrets at runtime
5. **Developer Experience**: No need to edit config files

## Troubleshooting

### "OpenAI provider requires API key"

```bash
# Check if env var is set
echo $OPENAI_API_KEY

# If empty, set it
export OPENAI_API_KEY=sk-...

# Verify
echo $OPENAI_API_KEY | cut -c1-8
# Should show: sk-proj-
```

### "No API key provided (using local provider)"

This is expected if:
1. Running with llamacpp feature (default)
2. No env var or config apiKey set
3. Using local llama.cpp server

Not an error for local mode!

### Environment variable not being read

```bash
# Check the variable is exported
export OPENAI_API_KEY=sk-...

# Run in same shell session
swift run kessel-cli ...

# Don't run in a new shell without exporting again
```

## Related Documentation

- [OpenAI Setup Guide](./OPENAI_SETUP.md)
- [Configuration Reference](./CONFIGURATION.md)
- [Development Guide](../CLAUDE.md)
