#!/usr/bin/env bash
#
# Adapter that lets the testsuite drive the **Rust** kessel-cli (crates/app —
# the same binary klein spawns) instead of the Swift build. The Rust REPL is
# env-driven and deliberately does NOT parse the YAML config (which carries
# Swift-only fields: tts/stt/watcher/ambient/skillPaths/…). This shim reads only
# the fields the Rust CLI understands out of the `--config` YAML with `yq` and
# passes them through as environment variables.
#
# Use it as the testsuite's CLI:
#   CLI="$PWD/testsuite/rust_cli.sh" BACKENDS=gemma4 bash testsuite/matrix_runner.sh
#
# Override the binary with KESSEL_RUST_BIN (default: crates/target/release/kessel-cli).
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="${KESSEL_RUST_BIN:-$script_dir/../crates/target/release/kessel-cli}"

# The runner invokes us as: rust_cli.sh --config <backend.yaml>  (prompt on stdin)
config=""
while [ $# -gt 0 ]; do
    case "$1" in
        --config) config="${2:-}"; shift 2 ;;
        *) shift ;;
    esac
done

if [ -z "$config" ] || [ ! -f "$config" ]; then
    echo "rust_cli.sh: --config <file> required (got '$config')" >&2
    exit 2
fi
if [ ! -x "$BIN" ]; then
    echo "rust_cli.sh: Rust binary not found: $BIN (build: cd crates && cargo build --release)" >&2
    exit 2
fi

# Pull the fields the Rust CLI reads. `// ""` yields empty for null/missing.
y() { yq "$1 // \"\"" "$config"; }

model_path="$(y '.llm.modelPath')"
base_url="$(y '.llm.baseURL')"
model="$(y '.llm.model')"
api_key="$(y '.llm.apiKey')"
temperature="$(y '.llm.temperature')"
max_tokens="$(y '.llm.maxTokens')"
reasoning="$(y '.llm.reasoningEffort')"
inference_engine="$(y '.llm.inferenceEngine')"
max_turns="$(y '.agent.maxTurns')"
# stdio MCP servers only: "cmd arg1 arg2,cmd2 ..." (matches MCP_SERVERS format).
mcp="$(yq '[.mcpServers[]? | select(.command != null and .command != "") | (.command + " " + ((.args // []) | join(" ")))] | join(",")' "$config")"

# Export only non-empty values. In particular MODEL_PATH must stay UNSET for
# cloud backends — an empty MODEL_PATH would make the Rust provider try to load
# a local model from "".
[ -n "$model_path" ] && export MODEL_PATH="$model_path"
[ -n "$base_url" ]   && export LLM_BASE_URL="$base_url"
[ -n "$model" ]      && export LLM_MODEL="$model"
[ -n "$api_key" ]    && export OPENAI_API_KEY="$api_key"   # else inherit ambient key
[ -n "$temperature" ] && export LLM_TEMPERATURE="$temperature"
[ -n "$max_tokens" ] && export MAX_TOKENS="$max_tokens"
[ -n "$reasoning" ]  && export REASONING_EFFORT="$reasoning"
# Local backend selector: "llamacpp" (default) or "gallium". If the yaml omits
# it, any ambient INFERENCE_ENGINE (e.g. set to run the matrix per engine) flows
# through untouched.
[ -n "$inference_engine" ] && export INFERENCE_ENGINE="$inference_engine"
[ -n "$max_turns" ]  && export MAX_REACT_ITERATIONS="$max_turns"
[ -n "$mcp" ]        && export MCP_SERVERS="$mcp"

exec "$BIN"
