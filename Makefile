.PHONY: help build run run-text run-openai run-openai-text run-ministral3 run-win clean test integration-test testsuite testsuite-local gen-uniffi install-deps zip

help:
	@echo "Voice Agent - Makefile"
	@echo ""
	@echo "Available targets:"
	@echo "  make build           - Build Rust and Swift"
	@echo "  make run             - Run in Auto-Listen Voice Mode (local)"
	@echo "  make run-text        - Run in Text Mode (local)"
	@echo "  make run-openai      - Run with OpenAI in voice mode (set OPENAI_API_KEY)"
	@echo "  make run-openai-text - Run with OpenAI in text mode (set OPENAI_API_KEY)"
	@echo "  make run-ministral3  - Run with local Ministral-3B (auto-downloads model)"
	@echo "  make run-verbose     - Run in Voice Mode (verbose)"
	@echo "  make run-text-verbose- Run in Text Mode (verbose)"
	@echo "  make run-win         - Build & run the Windows C# CLI (CONFIG=configs/foo.yaml)"
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

run:
	@echo "Running Voice Agent in Default Mode..."
	@cd swift && swift run voice-agent --config ../configs/default.yaml

run-verbose:
	@echo "Running Voice Agent in Auto-Listen Voice Mode (verbose)..."
	@cd swift && swift run voice-agent --config ../configs/default.yaml --verbose

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
	@echo "Running Voice Agent with OpenAI (voice mode)..."
	@echo "Using API key: $${OPENAI_API_KEY:0:8}..."
	@cd swift && swift run voice-agent --config ../configs/openai.yaml

run-qwen3:
	@echo "Running Voice Agent with Qwen3 (local)..."
	@cd swift && swift run voice-agent --config ../configs/qwen3.yaml

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
	@echo "Running Voice Agent with OpenAI (ja mode)..."
	@echo "Using API key: $${OPENAI_API_KEY:0:8}..."
	@cd swift && swift run voice-agent --config ../configs/openai-ja.yaml

# Windows C# CLI: build then run. Override the config with CONFIG=...
# e.g. make run-win CONFIG=configs/local-lfm2.yaml
WIN_CLI := win/VoiceAgentCLI/bin/Release/net8.0-windows/voice-agent.exe
CONFIG  ?= configs/default.yaml
run-win:
	@dotnet build win/VoiceAgentCLI/VoiceAgentCLI.csproj -c Release --nologo
	@echo "Running Windows CLI with $(CONFIG)..."
	@"$(WIN_CLI)" --config "$(CONFIG)"

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
# (local llama.cpp) and gpt-5.4-mini (cloud; needs OPENAI_API_KEY or .env).
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
		cargo run --bin uniffi-bindgen-swift -- --swift-sources ../target/release/libagent_core.dylib ../../vendor/uniffi-swift && \
		cargo run --bin uniffi-bindgen-swift -- --headers ../target/release/libagent_core.dylib ../../vendor/uniffi-swift && \
		cargo run --bin uniffi-bindgen-swift -- --modulemap ../target/release/libagent_core.dylib ../../vendor/uniffi-swift
	@echo ""
	@echo "✅ UniFFI bindings generated!"
	@echo ""
	@echo "Generated files in vendor/uniffi-swift/:"
	@ls -lh vendor/uniffi-swift/
	@echo ""
	@echo "📝 Next steps:"
	@echo "  1. Copy agent_core.swift to swift/Sources/AgentBridge/"
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
	ARCHIVE_NAME="voice-agent-$$TIMESTAMP.zip"; \
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
