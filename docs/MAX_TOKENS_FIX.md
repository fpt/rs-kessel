# maxTokens Configuration Fix

## Problem

When asking complex questions, the LLM response would end in the middle of the analysis phase without producing a final answer. The response would show only the `<|channel|>analysis` content but not reach `<|channel|>final`.

## Root Cause

The gpt-oss model uses the Harmony chat template which generates responses in two phases:

1. **Analysis phase** (`<|channel|>analysis`): Internal reasoning and thinking
2. **Final phase** (`<|channel|>final`): The actual user-facing response

For complex questions, the analysis can consume many tokens. If `maxTokens` is set too low (e.g., 2048), the model might finish analyzing but run out of tokens before generating the final response.

## Solution

Increased the default `maxTokens` from 2048 to 4096:

### Changed Files

1. **configs/default.yaml**:
```yaml
llm:
  maxTokens: 4096  # Increased from 2048
  # Added comments explaining the Harmony template needs
```

2. **swift/Sources/Util/Config.swift**:
```swift
maxTokens: 4096  // Increased default from 2048
```

3. **scripts/start_llm.sh**:
```bash
llama-server -c 8192  # Changed from -c 0 (unlimited)
# Added comments explaining context window
```

## Recommended Settings

### For Different Use Cases

| Use Case | maxTokens | Context (-c) |
|----------|-----------|--------------|
| Simple Q&A | 2048 | 4096 |
| **Normal use** | **4096** | **8192** |
| Complex questions | 8192 | 16384 |
| Research/Deep analysis | 16384 | 32768 |

### Current Defaults

- **maxTokens**: 4096 (good for most questions)
- **Context window**: 8192 (in llama-server)

## How to Adjust

### For Specific Complex Questions

Edit `configs/default.yaml`:
```yaml
llm:
  maxTokens: 8192  # Or even 16384 for very complex questions
```

### For Different Context Needs

Edit `scripts/start_llm.sh`:
```bash
llama-server -m model.gguf -c 16384  # Increase context window
```

## Verification

Test with a complex question that requires analysis:

```bash
swift run kessel-cli --config ../configs/default.yaml

You: Explain the differences between various sorting algorithms and when to use each one.

# Should now produce complete response with final answer
# Not just analysis that cuts off
```

## Documentation

Full configuration guide available in:
- `docs/CONFIGURATION.md` - Complete reference
- Sections on maxTokens, context windows, and troubleshooting

## Summary

- ✅ Increased maxTokens from 2048 → 4096
- ✅ Set reasonable context window (8192)
- ✅ Added comprehensive documentation
- ✅ Explained Harmony template requirements
- ✅ Provided troubleshooting guide

Your complex questions should now produce complete responses with final answers!
