# Configuration Guide

This guide explains all configuration options for the Kessel.

## Configuration File

The main configuration file is `configs/default.yaml`. You can create custom configs and load them with:

```bash
swift run kessel-cli --config path/to/config.yaml
```

## LLM Configuration

### maxTokens

**What it controls**: Maximum number of tokens in the LLM's response.

**Important for Harmony Template**: The gpt-oss model uses the Harmony template which produces two outputs:
1. `<|channel|>analysis` - Internal reasoning (can be long)
2. `<|channel|>final` - The actual response to the user

For complex questions, the analysis can be quite long. If `maxTokens` is too small, the model might finish the analysis but not reach the `final` channel, resulting in no visible response.

**Recommended values**:
- **Simple questions**: 2048 tokens (default for many systems)
- **Complex questions**: 4096 tokens (current default)
- **Very complex questions**: 8192 tokens
- **Research/analysis tasks**: 16384 tokens (if your GPU has enough VRAM)

**Example symptoms of too-small maxTokens**:
- Response ends abruptly during analysis
- No final response shown
- Only see analysis output without conclusion

**Configuration**:
```yaml
llm:
  maxTokens: 4096  # Increase to 8192 or 16384 for complex questions
```

### Context Window (llama-server)

The context window is set when starting llama-server (`scripts/start_llm.sh`):

```bash
llama-server -m model.gguf -c 8192
```

The `-c` parameter sets the context window size. This affects:
- How much conversation history can be kept
- How long prompts can be
- Memory usage

**Recommended values**:
- **Standard**: 8192 (current default, good for most use)
- **Long conversations**: 16384
- **Maximum**: 32768 (requires significant VRAM)

**To change**: Edit `scripts/start_llm.sh` and modify the `-c` value.

## Complete Configuration Reference

### LLM Settings

```yaml
llm:
  baseURL: "http://127.0.0.1:8080/v1"
  # The llama.cpp server endpoint
  # Change if running on different host/port

  model: "gpt-oss-20b"
  # Model name passed to the API
  # Must match the model loaded in llama-server

  apiKey: ""
  # Optional API key for authentication
  # Leave empty for local llama.cpp server

  harmonyTemplate: true
  # Enable Harmony template parsing
  # Set to true for gpt-oss models
  # Set to false for standard models

  temperature: 0.7
  # Sampling temperature (0.0-2.0)
  # Lower = more focused/deterministic
  # Higher = more creative/random
  # Recommended: 0.7 for conversation, 0.3 for factual

  maxTokens: 4096
  # Maximum tokens in response
  # See detailed explanation above
```

### Agent Settings

```yaml
agent:
  systemPromptPath: null
  # Path to custom system prompt file
  # null = use default prompt
  # Example: "prompts/custom_system.txt"

  maxTurns: 50
  # Maximum conversation turns before exit
  # One turn = user input + assistant response
  # Set to -1 for unlimited
```

### TTS (Text-to-Speech) Settings

```yaml
tts:
  enabled: true
  # Enable/disable voice output

  voice: null
  # Voice identifier from /voices command
  # null = default en-US voice
  # Example: "com.apple.voice.compact.en-US.Samantha"

  rate: 0.5
  # Speech rate (0.0-1.0)
  # 0.5 = normal speed
  # Lower = slower, Higher = faster

  pitchMultiplier: 1.0
  # Pitch adjustment (0.5-2.0)
  # 1.0 = normal pitch
  # Lower = deeper, Higher = higher pitch

  volume: 1.0
  # Volume level (0.0-1.0)
  # 1.0 = maximum volume
```

### STT (Speech-to-Text) Settings

```yaml
stt:
  enabled: true
  # Enable/disable voice input

  model: "base"
  # Whisper model size
  # Options: tiny, base, small, medium, large-v2, large-v3
  # See STT_SUCCESS.md for model comparison

  language: "en"
  # Language code for transcription
  # null = auto-detect
  # Examples: "en", "ja", "es", "fr"

  silenceThreshold: -40.0
  # Audio level threshold in dB
  # Lower = more sensitive (picks up quieter sounds)
  # Higher = less sensitive (requires louder speech)
  # Recommended: -40.0 for normal environments
  # Adjust: -50.0 for quiet speech, -30.0 for noisy

  silenceDuration: 1.5
  # Seconds of silence before auto-stop
  # Longer = more forgiving pauses
  # Shorter = faster response but may cut off
```

## Configuration Examples

### For Complex Technical Questions

```yaml
llm:
  maxTokens: 8192  # Large response capacity
  temperature: 0.5  # More focused responses

agent:
  maxTurns: 100     # Longer conversations
```

Also update llama-server context:
```bash
# In scripts/start_llm.sh
llama-server -m model.gguf -c 16384
```

### For Fast, Simple Conversations

```yaml
llm:
  maxTokens: 2048   # Shorter responses
  temperature: 0.7   # Natural conversation

tts:
  rate: 0.6          # Slightly faster speech
```

### For Noisy Environments

```yaml
stt:
  silenceThreshold: -35.0  # Less sensitive
  silenceDuration: 2.0      # More forgiving pauses

tts:
  volume: 1.0               # Maximum volume
```

### For Quiet/Private Use

```yaml
stt:
  silenceThreshold: -45.0  # More sensitive

tts:
  volume: 0.5              # Quieter output
  rate: 0.45               # Slower for better understanding
```

### For Multilingual Use

```yaml
stt:
  language: null      # Auto-detect language
  model: "medium"     # Better multilingual support

llm:
  maxTokens: 6144     # More space for translations
```

## Performance vs Quality Trade-offs

### Fast Response (Lower Resource)

- STT model: tiny or base
- LLM maxTokens: 2048
- llama-server context: 4096

### Balanced (Recommended)

- STT model: base or small
- LLM maxTokens: 4096
- llama-server context: 8192

### High Quality (Higher Resource)

- STT model: small or medium
- LLM maxTokens: 8192
- llama-server context: 16384

## Troubleshooting Configuration Issues

### "Response ends without final answer"

**Cause**: maxTokens too small for Harmony template
**Fix**: Increase `llm.maxTokens` to 4096 or 8192

### "Out of memory" errors

**Cause**: Context window too large
**Fix**: Reduce `-c` value in start_llm.sh (try 4096 or 2048)

### "Speech cuts off too early"

**Cause**: Silence detection too aggressive
**Fix**:
- Increase `stt.silenceDuration` to 2.0 or 2.5
- Decrease `stt.silenceThreshold` to -45.0

### "Speech doesn't stop automatically"

**Cause**: Silence detection not sensitive enough
**Fix**:
- Decrease `stt.silenceDuration` to 1.0
- Increase `stt.silenceThreshold` to -35.0

### "Transcription accuracy poor"

**Fix**:
- Upgrade `stt.model` to "small" or "medium"
- Specify `stt.language` explicitly (e.g., "en")
- Check microphone quality and background noise

### "TTS voice sounds unnatural"

**Fix**:
- Adjust `tts.rate` (try 0.4-0.6 range)
- Try different voices with `/voices` command
- Adjust `tts.pitchMultiplier` slightly (0.9-1.1)

## Environment-Specific Recommendations

### Car Use (Original Intent)

```yaml
llm:
  maxTokens: 3072    # Moderate length responses

tts:
  volume: 1.0        # Maximum volume
  rate: 0.55         # Slightly faster

stt:
  silenceThreshold: -30.0  # Less sensitive (road noise)
  silenceDuration: 2.0     # Forgiving (car movement)
  model: "small"           # Better accuracy in noise
```

### Office/Quiet Room

```yaml
stt:
  silenceThreshold: -45.0  # More sensitive
  silenceDuration: 1.2     # Quick response

tts:
  volume: 0.6              # Moderate volume
```

### Research/Study

```yaml
llm:
  maxTokens: 8192         # Long, detailed responses
  temperature: 0.4        # More focused

agent:
  maxTurns: 200          # Extended sessions
```

## See Also

- [UNIFFI_SUCCESS.md](../UNIFFI_SUCCESS.md) - LLM integration details
- [TTS_SUCCESS.md](../TTS_SUCCESS.md) - TTS configuration
- [STT_SUCCESS.md](../STT_SUCCESS.md) - STT configuration
- [CLAUDE.md](../CLAUDE.md) - Overall project architecture
