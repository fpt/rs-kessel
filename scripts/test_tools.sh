#!/bin/bash
# Integration tests for ReAct tool calling
#
# Usage:
#   # With local llama.cpp server running:
#   bash scripts/test_tools.sh
#
#   # With OpenAI:
#   OPENAI_API_KEY=sk-... bash scripts/test_tools.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CRATES_DIR="$PROJECT_DIR/crates"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

PASS=0
FAIL=0
SKIP=0

# Build the text agent
echo "Building kessel-cli..."
cd "$CRATES_DIR"
cargo build -p kessel-cli --release 2>/dev/null
TEXT_AGENT="$CRATES_DIR/target/release/kessel-cli"

# Set working dir to project root for tool operations
export WORKING_DIR="$PROJECT_DIR"
export RUST_LOG="${RUST_LOG:-warn}"

run_test() {
    local name="$1"
    local prompt="$2"
    local expect_pattern="$3"

    echo -n "  $name ... "

    # Run agent with prompt, capture output
    local output
    output=$(echo "$prompt" | gtimeout 60 "$TEXT_AGENT" 2>/dev/null) || {
        echo -e "${RED}FAIL${NC} (timeout or error)"
        FAIL=$((FAIL + 1))
        return
    }

    # Check if output matches expected pattern (case-insensitive grep)
    if echo "$output" | grep -qi "$expect_pattern"; then
        echo -e "${GREEN}PASS${NC}"
        PASS=$((PASS + 1))
    else
        echo -e "${RED}FAIL${NC}"
        echo "    Expected pattern: $expect_pattern"
        echo "    Got: $(echo "$output" | head -5)"
        FAIL=$((FAIL + 1))
    fi
}

run_test_multi() {
    local name="$1"
    local prompt="$2"
    shift 2
    local patterns=("$@")

    echo -n "  $name ... "

    local output
    output=$(echo "$prompt" | gtimeout 60 "$TEXT_AGENT" 2>/dev/null) || {
        echo -e "${RED}FAIL${NC} (timeout or error)"
        FAIL=$((FAIL + 1))
        return
    }

    local all_match=true
    local missing=""
    for pattern in "${patterns[@]}"; do
        if ! echo "$output" | grep -qi "$pattern"; then
            all_match=false
            missing="$pattern"
            break
        fi
    done

    if $all_match; then
        echo -e "${GREEN}PASS${NC}"
        PASS=$((PASS + 1))
    else
        echo -e "${RED}FAIL${NC}"
        echo "    Missing pattern: $missing"
        echo "    Got: $(echo "$output" | head -5)"
        FAIL=$((FAIL + 1))
    fi
}

# Check if LLM is available
echo "Checking LLM availability..."
# MAX_REACT_ITERATIONS is left unset on purpose: the CLI falls back to
# react::DEFAULT_MAX_ITERATIONS. Hardcoding a copy here just gives it somewhere
# to drift. Export it yourself to override.

if [ -n "${MODEL_PATH:-}" ]; then
    echo "  Using local model (FFI): $MODEL_PATH"
    if [ ! -f "$MODEL_PATH" ]; then
        echo -e "${RED}ERROR: Model file not found: $MODEL_PATH${NC}"
        exit 1
    fi
elif [ -n "${OPENAI_API_KEY:-}" ]; then
    echo "  Using OpenAI API"
    export LLM_MODEL="${LLM_MODEL:-gpt-5.6-luna}"
else
    echo "  Using local llama.cpp server"
    export LLM_BASE_URL="${LLM_BASE_URL:-http://127.0.0.1:8080/v1}"
    export LLM_MODEL="${LLM_MODEL:-gpt-5.2}"

    # Quick check if server is running
    if ! curl -s --max-time 3 "$LLM_BASE_URL/models" >/dev/null 2>&1; then
        echo -e "${YELLOW}WARNING: llama.cpp server not running at $LLM_BASE_URL${NC}"
        echo "  Set MODEL_PATH, OPENAI_API_KEY, or start llama.cpp server first"
        echo "  Skipping integration tests."
        exit 0
    fi
fi

echo ""
echo "=== Read Tool Tests ==="

run_test "Read a known file" \
    "Read the file configs/default.yaml and tell me what LLM model is configured" \
    "model"

run_test "Read with line numbers" \
    "Read the first 5 lines of configs/default.yaml" \
    "llm"

run_test "Read non-existent file" \
    "Read the file does_not_exist.txt" \
    "error\|not found\|failed\|exist\|unable\|can't\|cannot\|sorry\|no such"

echo ""
echo "=== Glob Tool Tests ==="

run_test "Find Rust files" \
    "Find all .rs files in the crates/lib/src/ directory" \
    "lib.rs"

run_test "Find YAML configs" \
    "List all yaml files in the configs directory" \
    "default.yaml"

run_test "No match glob" \
    "Find all .xyz files in the project" \
    "no.*found\|not found\|none\|found 0\|couldn't find\|no.*files\|no.*match"

echo ""
echo "=== Tasks Tool Tests ==="

run_test "Create a task" \
    "Create a task called 'Fix audio bug' with description 'The audio buffer is not clearing properly'" \
    "fix audio\|created\|task"

run_test "List tasks" \
    "Create a task called 'Test task' and then list all tasks" \
    "test task"

echo ""
echo "=== Multi-step ReAct Tests ==="

run_test "Read and summarize" \
    "Read configs/default.yaml and tell me what model name is configured for the LLM" \
    "gpt-oss"

run_test_multi "Glob then Read" \
    "Find all yaml files in configs/ directory and read the first one you find" \
    "yaml" "llm\|model\|config"

echo ""
echo "=== Results ==="
TOTAL=$((PASS + FAIL + SKIP))
echo -e "  ${GREEN}Passed: $PASS${NC}"
echo -e "  ${RED}Failed: $FAIL${NC}"
if [ $SKIP -gt 0 ]; then
    echo -e "  ${YELLOW}Skipped: $SKIP${NC}"
fi
echo "  Total: $TOTAL"

if [ $FAIL -gt 0 ]; then
    exit 1
fi
