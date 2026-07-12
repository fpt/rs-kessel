.PHONY: help build install uninstall run run-text run-openai run-openai-text build-win run-win clean test integration-test testsuite testsuite-local gen-uniffi install-deps zip

# Install location (override with: make install PREFIX=/usr/local)
PREFIX ?= $(HOME)
BINDIR := $(PREFIX)/bin

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
	@echo "  make run-verbose     - Run in Voice Mode (verbose)"
	@echo "  make run-text-verbose- Run in Text Mode (verbose)"
	@echo "  make build-win       - Build the Windows C# CLI (Rust cdylib + .NET)"
	@echo "  make run-win         - Build & run the Windows C# CLI (WIN_CONFIG=configs/foo.yaml)"
	@echo ""
	@echo "  make clean           - Clean build artifacts"
	@echo "  make test            - Run tests"
	@echo "  make integration-test- Run Rust ReAct tool-calling tests"
	@echo "  make testsuite       - Run CLI capability matrix (all backends; see testsuite/)"
	@echo "  make testsuite-local - Run matrix for local backends only (gemma4,gpt-oss)"
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

# Install the release binary to $(BINDIR). The executable links
# libkessel_core.dylib by absolute path inside this repo's
# crates/target/release, so keep the repo in place after installing (and re-run
# `make install` after pulling/rebuilding so $(BINDIR) tracks the latest code).
install: build
	@mkdir -p "$(BINDIR)"
	@cp swift/.build/release/kessel-cli "$(BINDIR)/kessel-cli"
	@echo "✅ Installed: $(BINDIR)/kessel-cli"
	@echo "   Links dylib from: $(CURDIR)/crates/target/release (keep this repo in place)"
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "   ⚠️  $(BINDIR) is not on your PATH — add it to use 'kessel-cli' directly." ;; esac

uninstall:
	@rm -f "$(BINDIR)/kessel-cli"
	@echo "Removed $(BINDIR)/kessel-cli"

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
	@echo "Running Kessel with Qwen3 (local)..."
	@cd swift && swift run kessel-cli --config ../configs/qwen3.yaml

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

# Windows C# CLI. WIN_CONFIG selects the config (Windows-only; independent of the
# other run targets). e.g. make run-win WIN_CONFIG=configs/local-lfm2.yaml
WIN_CLI    := win/KesselCli/bin/Release/net8.0-windows/kessel-cli.exe
WIN_CONFIG ?= configs/default.yaml

# Full Windows build: Rust cdylib (local llama.cpp, via the MSVC build script) +
# the .NET CLI. For a CUDA build, run scripts\build-win-cuda.bat instead.
build-win:
	@cmd //C "scripts\\build-win-local.bat"
	@dotnet build win/KesselCli/KesselCli.csproj -c Release --nologo
	@echo "Built $(WIN_CLI)"

# Build the .NET CLI (copies the latest Rust cdylib) then run it.
run-win:
	@dotnet build win/KesselCli/KesselCli.csproj -c Release --nologo
	@echo "Running Windows CLI with $(WIN_CONFIG)..."
	@"$(WIN_CLI)" --config "$(WIN_CONFIG)"

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
	@bash testsuite/matrix_runner.sh

testsuite-local:
	@BACKENDS="gemma4,gpt-oss" bash testsuite/matrix_runner.sh

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
