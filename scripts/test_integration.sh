#!/bin/bash
set -e

# Test script for Swift → Rust → llama.cpp integration

GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}Testing Kessel Integration${NC}"
echo ""

# Check server
if ! curl -s http://127.0.0.1:8080/health > /dev/null 2>&1; then
    echo "❌ llama.cpp server not running"
    exit 1
fi
echo "✅ llama.cpp server running"

# Build
cd swift
echo "Building..."
swift build > /dev/null 2>&1

# Run with a simple conversation using echo
echo ""
echo -e "${GREEN}Starting conversation...${NC}"
echo ""

# Use echo to send a test message
echo "What is 2+2?" | .build/debug/kessel-cli 2>&1 | grep -A 2 "Assistant:"

echo ""
echo -e "${GREEN}✅ Integration test complete!${NC}"
