#!/bin/bash
set -e

# Generate UniFFI C# bindings for the Windows CLI (win/VoiceAgentCLI).
#
# Prereq (install once):
#   cargo install uniffi-bindgen-cs \
#     --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.9.0+v0.28.3
#
# The Rust crate must be built first so the cdylib exists:
#   cd crates && cargo build --release --no-default-features   # cloud-only
#   (add --features local for in-process llama.cpp; needs a C/C++ toolchain)

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT/win/vendor"

mkdir -p "$OUT_DIR"

echo "🔧 Generating UniFFI C# bindings from agent.udl..."
# UDL mode (not --library): avoids a full `cargo metadata` resolve. Run from the
# crate dir so uniffi can locate the surrounding Cargo.toml.
cd "$ROOT/crates/lib"
uniffi-bindgen-cs --out-dir "$OUT_DIR" src/agent.udl

echo "✅ Generated $OUT_DIR/agent_core.cs"
echo "   DllImport target: uniffi_agent_core (agent_core.dll copied at build time)"
