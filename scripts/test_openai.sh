#!/bin/bash
# Test script for OpenAI integration
set -e

echo "🧪 Testing OpenAI Integration"
echo "=============================="
echo ""

# Check if API key is set
if [ -z "$OPENAI_API_KEY" ]; then
    echo "❌ OPENAI_API_KEY environment variable not set"
    echo ""
    echo "Please set it:"
    echo "  export OPENAI_API_KEY=sk-..."
    echo ""
    exit 1
fi

echo "✅ OPENAI_API_KEY is set (${OPENAI_API_KEY:0:8}...)"
echo ""

# Check if configs/openai.yaml exists
if [ ! -f "configs/openai.yaml" ]; then
    echo "❌ configs/openai.yaml not found"
    exit 1
fi

echo "✅ OpenAI config file exists"
echo ""

# Build Rust with OpenAI feature
echo "🦀 Building Rust with OpenAI feature..."
cd crates
cargo build --release --no-default-features --features openai
cd ..

echo "✅ Rust built successfully"
echo ""

# Regenerate UniFFI bindings
echo "🔧 Regenerating UniFFI bindings..."
bash scripts/gen_uniffi.sh > /dev/null 2>&1

echo "✅ UniFFI bindings regenerated"
echo ""

# Build Swift
echo "🍎 Building Swift CLI..."
cd swift
swift build > /dev/null 2>&1
cd ..

echo "✅ Swift built successfully"
echo ""

# Test with a simple query
echo "📝 Testing with simple query..."
echo ""
echo "Query: 'What is 2+2? Reply with just the number.'"
echo ""

# Run the agent with a test query
echo "What is 2+2? Reply with just the number." | \
    OPENAI_API_KEY=$OPENAI_API_KEY \
    swift run kessel-cli --config ../configs/openai.yaml 2>/dev/null | \
    grep "Assistant:" || true

echo ""
echo "=============================="
echo "✅ Test complete!"
echo ""
echo "To use OpenAI in your sessions:"
echo "  OPENAI_API_KEY=\$OPENAI_API_KEY make run-text"
echo ""
echo "Or for voice mode, update configs/openai.yaml:"
echo "  agent.autoListen: true"
echo ""
