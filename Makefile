.PHONY: help build build-headless install uninstall test fmt fmt-fix clean play mcp \
        android android-install android-test android-deps

# Install location (override with: make install PREFIX=/usr/local)
PREFIX ?= $(HOME)
BINDIR := $(PREFIX)/bin

help:
	@echo "Kessel - a tiny fantasy console for agents and humans"
	@echo ""
	@echo "  make build           - Build 'kessel' (release, with the play window)"
	@echo "  make build-headless  - Build without the window (MCP only; for servers/containers)"
	@echo "  make install         - Build and install 'kessel' to \$$PREFIX/bin (default ~/bin)"
	@echo "  make uninstall       - Remove the installed 'kessel'"
	@echo "  make test            - Run the test suite"
	@echo "  make fmt / fmt-fix   - Check / apply rustfmt"
	@echo "  make clean           - Remove build artifacts"
	@echo ""
	@echo "  make play GAME=games/tetris.lua   - Build and play a game"
	@echo "  make mcp  ROOT=.                  - Run the MCP server on stdio"
	@echo ""
	@echo "  make android-deps    - Install the Rust Android targets and cargo-ndk"
	@echo "  make android         - Build the Android APK (debug)"
	@echo "  make android-install - Build and install onto a connected device"
	@echo "  make android-test    - Run the Android unit tests"
	@echo ""

build:
	@cd crates && cargo build --release
	@echo "Built crates/target/release/kessel"

# No winit/softbuffer: `kessel mcp` has no business linking a window system on a
# headless box.
build-headless:
	@cd crates && cargo build --release --no-default-features
	@echo "Built crates/target/release/kessel (MCP only, no player)"

install: build
	@mkdir -p "$(BINDIR)"
	@cp crates/target/release/kessel "$(BINDIR)/kessel"
	@echo "✅ Installed $(BINDIR)/kessel"
	@echo "   kessel mcp   — serve the console to an agent over MCP"
	@echo "   kessel run    — open a game window"
	@echo "   kessel attach — join a running mcp session"
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "   ⚠️  $(BINDIR) is not on your PATH — add it to use 'kessel' directly." ;; esac

uninstall:
	@rm -f "$(BINDIR)/kessel"
	@echo "Removed $(BINDIR)/kessel"

test:
	@cd crates && cargo test

fmt:
	@cd crates && cargo fmt --check

fmt-fix:
	@cd crates && cargo fmt

GAME ?= games/bounce.lua
play: build
	@./crates/target/release/kessel run "$(GAME)"

ROOT ?= .
mcp: build
	@./crates/target/release/kessel mcp --root "$(ROOT)"

# --- Android ---------------------------------------------------------------
#
# The APK bundles the Rust console as a .so and this repo's games/ directory as
# its assets, so `make android` is the only step — Gradle drives cargo-ndk
# itself. Requires a JDK 17+ and the NDK named in android/app/build.gradle.kts.

android-deps:
	@rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
	@cargo install cargo-ndk
	@echo "✅ Android Rust toolchain ready"

android:
	@cd android && ./gradlew assembleDebug
	@echo "Built android/app/build/outputs/apk/debug/app-debug.apk"

android-install:
	@cd android && ./gradlew installDebug
	@echo "✅ Installed; launch 'Kessel' on the device"

android-test:
	@cd android && ./gradlew testDebugUnitTest

clean:
	@cd crates && cargo clean
	@cd android && ./gradlew clean 2>/dev/null || true
	@echo "Cleaned."
