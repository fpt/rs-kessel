.PHONY: help build install uninstall run run-text run-openai run-openai-text build-win run-win clean test integration-test testsuite testsuite-local gen-uniffi install-deps zip

# Install location (override with: make install PREFIX=/usr/local)
PREFIX ?= $(HOME)
BINDIR := $(PREFIX)/bin

# Testsuite CLI. Defaults to the Rust kessel-cli via the yq->env adapter
# (Swift-free, statically linked). Override with e.g.:
#   make testsuite CLI=swift/.build/release/kessel-cli
#   make testsuite CLI=win/KesselCli/bin/Release/net8.0-windows/kessel.exe
RUST_TESTSUITE_CLI := $(CURDIR)/testsuite/rust_cli.sh
CLI ?= $(RUST_TESTSUITE_CLI)

help:
	@echo "Kessel - Makefile"
	@echo ""
	@echo "Available targets:"
	@echo "  make build           - Build Rust and Swift"
	@echo "  make install         - Build (release) and install kessel-cli to \$$PREFIX/bin (default ~/bin)"
	@echo "  make uninstall       - Remove the installed kessel-cli"
	@echo "  make run             - Run in Auto-Listen Voice Mode (local)"
	@echo "  make run-text        - Run in Text Mode (local)"
	@echo "  make run-openai      - Run with OpenAI in voice mode (set OPENAI_API_KEY)"
	@echo "  make run-openai-text - Run with OpenAI in text mode (set OPENAI_API_KEY)"
	@echo "  make run-qwen3       - Run with local Qwen3.5-9B (auto-downloads model)"
	@echo "  make run-gemma4      - Run with local Gemma 4 26B-A4B (auto-downloads model)"
	@echo "  make run-lfm2        - Run with local LFM2.5-8B (auto-downloads model)"
	@echo "  make run-verbose     - Run in Voice Mode (verbose)"
	@echo "  make run-text-verbose- Run in Text Mode (verbose)"
	@echo "  make build-win       - Build Windows kessel.exe (C# frontend) + kessel-cli.exe (Rust core)"
	@echo "  make run-win         - Build & run the Windows frontend (WIN_CONFIG=configs/foo.yaml)"
	@echo ""
	@echo "  make clean           - Clean build artifacts"
	@echo "  make test            - Run tests"
	@echo "  make integration-test- Run Rust ReAct tool-calling tests"
	@echo "  make testsuite       - Run CLI capability matrix on the Rust CLI (all backends; CLI= to override)"
	@echo "  make testsuite-local - Run matrix on the Rust CLI, local backends only (gemma4,gpt-oss)"
	@echo "  make gen-uniffi      - Generate UniFFI Swift bindings"
	@echo "  make install-deps    - Install development dependencies"
	@echo "  make zip             - Create source archive (excludes models/build artifacts)"
	@echo ""
	@echo "Note: Provider selection is automatic:"
	@echo "  - With OPENAI_API_KEY → OpenAI"
	@echo "  - Without API key → Local llama.cpp"
	@echo ""

install-deps:
	@echo "Installing Rust dependencies..."
	@cd crates && cargo fetch
	@echo "Installing Swift dependencies..."
	@cd swift && swift package resolve
	@echo "Dependencies installed!"

build:
	@echo "Building Rust library..."
	@cd crates && cargo build --release
	@echo "Building Swift CLI..."
	@cd swift && swift build -c release
	@echo "Build complete!"

# Install both binaries to $(BINDIR):
#
#   kessel-cli  the Rust core — text REPL plus `app-server` (the JSON-RPC
#               whole-turn backend klein drives). Statically linked, so it is
#               self-contained and does not care where this repo lives.
#   kessel      the Swift app — voice (TTS/STT) + the Claude Code watcher. It
#               links libkessel_core.dylib by ABSOLUTE path into this repo's
#               crates/target/release, so this repo must stay put for it to run.
#
# The two names are not interchangeable: only kessel-cli understands
# `app-server`, and klein's kessel backend spawns `kessel-cli app-server` by
# default. Re-run `make install` after pulling so $(BINDIR) tracks the latest.
install: build
	@mkdir -p "$(BINDIR)"
	@cp crates/target/release/kessel-cli "$(BINDIR)/kessel-cli"
	@cp swift/.build/release/kessel-cli "$(BINDIR)/kessel"
	@echo "✅ Installed:"
	@echo "   $(BINDIR)/kessel-cli  — Rust core (REPL + app-server; used by klein). Self-contained."
	@echo "   $(BINDIR)/kessel      — Swift voice app. Links dylib from $(CURDIR)/crates/target/release (keep this repo in place)."
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "   ⚠️  $(BINDIR) is not on your PATH — add it to use 'kessel' / 'kessel-cli' directly." ;; esac

uninstall:
	@rm -f "$(BINDIR)/kessel-cli" "$(BINDIR)/kessel"
	@echo "Removed $(BINDIR)/kessel-cli and $(BINDIR)/kessel"

run:
	@echo "Running Kessel in Default Mode..."
	@cd swift && swift run kessel-cli --config ../configs/default.yaml

run-verbose:
	@echo "Running Kessel in Auto-Listen Voice Mode (verbose)..."
	@cd swift && swift run kessel-cli --config ../configs/default.yaml --verbose

run-openai:
	@if [ -z "$$OPENAI_API_KEY" ]; then \
		echo "❌ Error: OPENAI_API_KEY environment variable not set"; \
		echo ""; \
		echo "Set it with:"; \
		echo "  export OPENAI_API_KEY=sk-..."; \
		echo ""; \
		echo "Or run inline:"; \
		echo "  OPENAI_API_KEY=sk-... make run-openai"; \
		echo ""; \
		exit 1; \
	fi
	@echo "Running Kessel with OpenAI (voice mode)..."
	@echo "Using API key: $${OPENAI_API_KEY:0:8}..."
	@cd swift && swift run kessel-cli --config ../configs/openai.yaml

run-lfm2:
	@echo "Running Kessel with LFM2 (local)..."
	@cd swift && swift run kessel-cli --config ../configs/lfm2.yaml

run-qwen3:
	@echo "Running Kessel with Qwen3.5 (local)..."
	@cd swift && swift run kessel-cli --config ../configs/qwen3.yaml

run-gemma4:
	@echo "Running Kessel with Gemma 4 26B-A4B (local)..."
	@cd swift && swift run kessel-cli --config ../configs/gemma4.yaml

run-openai-ja:
	@if [ -z "$$OPENAI_API_KEY" ]; then \
		echo "❌ Error: OPENAI_API_KEY environment variable not set"; \
		echo ""; \
		echo "Set it with:"; \
		echo "  export OPENAI_API_KEY=sk-..."; \
		echo ""; \
		echo "Or run inline:"; \
		echo "  OPENAI_API_KEY=sk-... make run-openai-text"; \
		echo ""; \
		exit 1; \
	fi
	@echo "Running Kessel with OpenAI (ja mode)..."
	@echo "Using API key: $${OPENAI_API_KEY:0:8}..."
	@cd swift && swift run kessel-cli --config ../configs/openai-ja.yaml

# Windows. Two binaries, mirroring the macOS split (see `make install`):
#
#   kessel.exe      the C# frontend (voice/REPL). Needs uniffi_kessel_core.dll
#                   beside it — the csproj copies the Rust cdylib under that name.
#   kessel-cli.exe  the Rust core: REPL + `app-server`, the JSON-RPC backend klein
#                   spawns. Statically links kessel_core, so it stands alone.
#
# WIN_CONFIG selects the config for run-win (independent of the other run
# targets). e.g. make run-win WIN_CONFIG=configs/gemma4.yaml
WIN_EXE    := win/KesselCli/bin/Release/net8.0-windows/kessel.exe
WIN_CLI    := crates/target/release/kessel-cli.exe
WIN_CONFIG ?= configs/default.yaml

# Full Windows build. build-win-local.bat builds BOTH Rust artifacts (cdylib +
# kessel-cli.exe) in one cargo invocation; dotnet then builds kessel.exe.
# For a CUDA build, run scripts\build-win-cuda.bat instead, then dotnet build.
build-win:
	@cmd //C "scripts\\build-win-local.bat"
	@dotnet build win/KesselCli/KesselCli.csproj -c Release --nologo
	@echo "Built:"
	@echo "   $(WIN_EXE)  — C# frontend (voice/REPL)"
	@echo "   $(WIN_CLI)  — Rust core (REPL + app-server; used by klein)"

# Build the .NET frontend (copies the latest Rust cdylib) then run it.
run-win:
	@dotnet build win/KesselCli/KesselCli.csproj -c Release --nologo
	@echo "Running Windows frontend with $(WIN_CONFIG)..."
	@"$(WIN_EXE)" --config "$(WIN_CONFIG)"

clean:
	@echo "Cleaning build artifacts..."
	@cd crates && cargo clean
	@cd swift && swift package clean
	@rm -rf vendor/uniffi-swift
	@echo "Clean complete!"

test:
	@echo "Running Rust tests..."
	@cd crates && cargo test
	@echo "Running Swift tests..."
	@cd swift && swift test
	@echo "Tests complete!"

integration-test:
	./scripts/test_tools.sh

# CLI capability matrix (see testsuite/README.md). Backends: gemma4, gpt-oss
# (local llama.cpp) and gpt-5.6-luna (cloud; needs OPENAI_API_KEY or .env).
# Filter with TESTS=... / BACKENDS=...; override the binary with CLI=...
testsuite:
	@if [ "$(CLI)" = "$(RUST_TESTSUITE_CLI)" ]; then cd crates && cargo build --release -p kessel-cli; fi
	@CLI="$(CLI)" bash testsuite/matrix_runner.sh

testsuite-local:
	@if [ "$(CLI)" = "$(RUST_TESTSUITE_CLI)" ]; then cd crates && cargo build --release -p kessel-cli; fi
	@CLI="$(CLI)" BACKENDS="gemma4,gpt-oss" bash testsuite/matrix_runner.sh

gen-uniffi:
	@echo "🦀 Building Rust library..."
	@cd crates && cargo build --release
	@echo "🔧 Generating UniFFI Swift bindings..."
	@mkdir -p vendor/uniffi-swift
	@cd crates/lib && \
		cargo run --bin uniffi-bindgen-swift -- --swift-sources ../target/release/libkessel_core.dylib ../../vendor/uniffi-swift && \
		cargo run --bin uniffi-bindgen-swift -- --headers ../target/release/libkessel_core.dylib ../../vendor/uniffi-swift && \
		cargo run --bin uniffi-bindgen-swift -- --modulemap ../target/release/libkessel_core.dylib ../../vendor/uniffi-swift
	@echo ""
	@echo "✅ UniFFI bindings generated!"
	@echo ""
	@echo "Generated files in vendor/uniffi-swift/:"
	@ls -lh vendor/uniffi-swift/
	@echo ""
	@echo "📝 Next steps:"
	@echo "  1. Copy kessel_core.swift to swift/Sources/AgentBridge/"
	@echo "  2. Verify swift/Package.swift links the dylib"

# Development shortcuts
dev-rust:
	@cd crates && cargo watch -x "check" -x "test"

dev-swift:
	@cd swift && swift build

# Check formatting
fmt:
	@cd crates && cargo fmt --all -- --check
	@cd swift && swift format --recursive Sources/

# Apply formatting
fmt-fix:
	@cd crates && cargo fmt --all
	@cd swift && swift format --in-place --recursive Sources/

# Create source archive
zip:
	@echo "Creating source archive..."
	@TIMESTAMP=$$(date +%Y%m%d-%H%M%S); \
	ARCHIVE_NAME="kessel-cli-$$TIMESTAMP.zip"; \
	zip -r "$$ARCHIVE_NAME" \
		README.md \
		CLAUDE.md \
		UNIFFI_SUCCESS.md \
		TTS_SUCCESS.md \
		STT_SUCCESS.md \
		Makefile \
		.gitignore \
		configs/ \
		scripts/ \
		docs/ \
		crates/Cargo.toml \
		crates/lib/Cargo.toml \
		crates/lib/build.rs \
		crates/lib/uniffi-bindgen.rs \
		crates/lib/uniffi-bindgen-swift.rs \
		crates/lib/src/ \
		crates/app/Cargo.toml \
		crates/app/src/ \
		swift/Package.swift \
		swift/Sources/ \
		-x "*.DS_Store" \
		-x "**/target/*" \
		-x "**/.build/*" \
		-x "**/models/*" \
		-x "**/vendor/*" \
		-x "**/*.gguf" \
		-x "**/*.bin" \
		-x "**/*.dylib" \
		-x "**/*.so" \
		-x "**/.git/*"; \
	echo ""; \
	echo "✅ Archive created: $$ARCHIVE_NAME"; \
	ls -lh "$$ARCHIVE_NAME"
