#!/bin/bash
# Build Rust kessel_core for iOS (device + simulator).
# Produces an XCFramework at swift/AgentApp/kessel_core.xcframework
#
# kessel is a pure ACP client (no in-process inference), so the core has no
# feature flags — one build covers every target.
#
# Usage: bash scripts/build-ios.sh
# Prerequisites: rustup target add aarch64-apple-ios aarch64-apple-ios-sim

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CRATES_DIR="$ROOT_DIR/crates"
OUTPUT_DIR="$ROOT_DIR/swift/AgentApp"
CARGO_TOML="$CRATES_DIR/lib/Cargo.toml"

echo "Building kessel_core for iOS..."

# Temporarily remove cdylib from crate-type (iOS cross-link fails for dylibs
# due to libc++ tbd stub mismatch in Xcode 26 beta).
cp "$CARGO_TOML" "$CARGO_TOML.bak"
sed -i '' 's/crate-type = \["lib", "staticlib", "cdylib"\]/crate-type = ["lib", "staticlib"]/' "$CARGO_TOML"
trap 'mv "$CARGO_TOML.bak" "$CARGO_TOML"' EXIT

export SDKROOT
export IPHONEOS_DEPLOYMENT_TARGET=26.2

# Build for device (aarch64-apple-ios)
echo "  [1/3] Building for iOS device (aarch64-apple-ios)..."
cd "$CRATES_DIR"
SDKROOT=$(xcrun --sdk iphoneos --show-sdk-path) \
    cargo build -p kessel-core --release \
    --target aarch64-apple-ios 2>&1 | tail -3

# Build for simulator (aarch64-apple-ios-sim)
echo "  [2/3] Building for iOS simulator (aarch64-apple-ios-sim)..."
SDKROOT=$(xcrun --sdk iphonesimulator --show-sdk-path) \
    cargo build -p kessel-core --release \
    --target aarch64-apple-ios-sim 2>&1 | tail -3

# Create XCFramework
echo "  [3/3] Creating XCFramework..."

DEVICE_LIB="$CRATES_DIR/target/aarch64-apple-ios/release/libkessel_core.a"
SIM_LIB="$CRATES_DIR/target/aarch64-apple-ios-sim/release/libkessel_core.a"

# Use the UniFFI-generated header
HEADER_DIR="$ROOT_DIR/vendor/uniffi-swift"
HEADER="$HEADER_DIR/kessel_coreFFI.h"
MODULEMAP="$HEADER_DIR/kessel_core.modulemap"

if [ ! -f "$HEADER" ]; then
    echo "Error: $HEADER not found. Run: bash scripts/gen_uniffi.sh"
    exit 1
fi

# Prepare header directories for each platform
DEVICE_HEADERS="$CRATES_DIR/target/ios-headers-device"
SIM_HEADERS="$CRATES_DIR/target/ios-headers-sim"
rm -rf "$DEVICE_HEADERS" "$SIM_HEADERS"
mkdir -p "$DEVICE_HEADERS" "$SIM_HEADERS"
cp "$HEADER" "$DEVICE_HEADERS/"
cp "$MODULEMAP" "$DEVICE_HEADERS/"
cp "$HEADER" "$SIM_HEADERS/"
cp "$MODULEMAP" "$SIM_HEADERS/"

XCFW_PATH="$OUTPUT_DIR/kessel_core.xcframework"
rm -rf "$XCFW_PATH"

xcodebuild -create-xcframework \
    -library "$DEVICE_LIB" -headers "$DEVICE_HEADERS" \
    -library "$SIM_LIB" -headers "$SIM_HEADERS" \
    -output "$XCFW_PATH" 2>&1 | tail -3

# Copy Bridge header
cp "$HEADER" "$OUTPUT_DIR/Bridge/"

echo ""
echo "Done! XCFramework: $XCFW_PATH"
echo ""
echo "Next steps:"
echo "  1. Open swift/AgentApp/AgentApp.xcodeproj in Xcode"
echo "  2. Build and run"
