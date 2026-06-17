#!/bin/bash
set -e

# Development run script

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}🚀 Kessel Development Run${NC}"
echo ""

# Check if llama.cpp server is running
echo "Checking llama.cpp server..."
if ! curl -s http://127.0.0.1:8080/health > /dev/null 2>&1; then
    echo -e "${RED}❌ llama.cpp server is not running!${NC}"
    echo ""
    echo "Please start the server first:"
    echo "  llama-server -m models/gpt-oss-20b-mxfp4.gguf -c 0 -fa --jinja"
    echo ""
    exit 1
fi
echo -e "${GREEN}✅ llama.cpp server is running${NC}"
echo ""

# Build Rust library
echo "Building Rust library..."
cd crates
cargo build
cd ..

# Run Swift CLI
echo ""
echo -e "${GREEN}🎯 Starting Kessel CLI...${NC}"
echo ""
cd swift
swift run kessel-cli --config ../configs/default.yaml --verbose
