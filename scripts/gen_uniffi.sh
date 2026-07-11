#!/bin/bash
set -e

# Generate UniFFI bindings for Swift

echo "🦀 Building Rust library..."
cd crates
cargo build --release

echo "🔧 Generating UniFFI Swift bindings..."
cd lib

LIBRARY_PATH="../target/release/libkessel_core.dylib"
OUT_DIR="../../vendor/uniffi-swift"

mkdir -p $OUT_DIR

# Generate Swift sources
cargo run --bin uniffi-bindgen-swift -- --swift-sources $LIBRARY_PATH $OUT_DIR

# Generate headers
cargo run --bin uniffi-bindgen-swift -- --headers $LIBRARY_PATH $OUT_DIR

# Generate modulemap
cargo run --bin uniffi-bindgen-swift -- --modulemap $LIBRARY_PATH $OUT_DIR

echo "✅ UniFFI bindings generated!"

# Copy the generated outputs into the tracked Swift tree. `vendor/uniffi-swift/`
# is gitignored, so the build must NOT depend on it — the FFI header is committed
# self-contained under AgentBridgeFFI, and the Swift bindings under AgentBridge.
# Doing the copies here (rather than as a manual "next step") keeps a clean clone
# buildable and stops the committed bindings from silently going stale after a
# .udl change — which is exactly what broke the McpServerConfig.url field.
cd ..
echo "📋 Copying generated bindings into the tracked Swift tree..."
cp vendor/uniffi-swift/kessel_core.swift   swift/Sources/AgentBridge/kessel_core.swift
cp vendor/uniffi-swift/kessel_coreFFI.h    swift/Sources/AgentBridgeFFI/kessel_coreFFI.h
cp vendor/uniffi-swift/kessel_coreFFI.h    swift/Sources/AgentBridge/include/kessel_coreFFI.h

echo "✅ Bindings copied. Review & commit:"
echo "     swift/Sources/AgentBridge/kessel_core.swift"
echo "     swift/Sources/AgentBridgeFFI/kessel_coreFFI.h"
echo "     swift/Sources/AgentBridge/include/kessel_coreFFI.h"
