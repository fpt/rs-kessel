.PHONY: help build install uninstall run run-verbose run-text run-codex build-win run-win clean test gen-uniffi install-deps dev-rust dev-swift fmt fmt-fix zip

# Install location (override with: make install PREFIX=/usr/local)
PREFIX ?= $(HOME)
BINDIR := $(PREFIX)/bin

help:
	@echo "Kessel - Makefile"
	@echo ""
	@echo "kessel is a voice/VM frontend and ACP client. It spawns a backend agent"
	@echo "(the standalone 'gallium' binary by default; 'codex' via KESSEL_ACP_BACKEND)"
	@echo "and drives it over JSON-RPC."
	@echo ""
	@echo "Available targets:"
	@echo "  make build           - Build the Rust core (cdylib) and the Swift app"
	@echo "  make install         - Build (release) and install 'kessel' to \$$PREFIX/bin (default ~/bin)"
	@echo "  make uninstall       - Remove the installed 'kessel'"
	@echo "  make run             - Run in voice mode against the local gallium backend"
	@echo "  make run-text        - Run in text mode against the local gallium backend"
	@echo "  make run-codex       - Run against a cloud backend (KESSEL_ACP_BACKEND=codex, needs OPENAI_API_KEY)"
	@echo "  make run-verbose     - Run in voice mode (verbose)"
	@echo "  make build-win       - Build Windows kessel.exe (C# frontend) + kessel_core.dll"
	@echo "  make run-win         - Build & run the Windows frontend (WIN_CONFIG=configs/foo.yaml)"
	@echo ""
	@echo "  make clean           - Clean build artifacts"
	@echo "  make test            - Run tests"
	@echo "  make gen-uniffi      - Generate UniFFI Swift bindings"
	@echo "  make install-deps    - Install development dependencies"
	@echo "  make zip             - Create source archive (excludes models/build artifacts)"
	@echo ""
	@echo "Note: the backend must be on PATH. Install 'gallium' from ../rs-gallium,"
	@echo "or set KESSEL_ACP_BACKEND to another codex-app-server binary."
	@echo ""

install-deps:
	@echo "Installing Rust dependencies..."
	@cd crates && cargo fetch
	@echo "Installing Swift dependencies..."
	@cd swift && swift package resolve
	@echo "Dependencies installed!"

build:
	@echo "Building Rust core (cdylib)..."
	@cd crates && cargo build --release
	@echo "Building Swift app..."
	@cd swift && swift build -c release
	@echo "Build complete!"

# Install the Swift voice app as `kessel`. It links libkessel_core.dylib by
# ABSOLUTE path into this repo's crates/target/release, so this repo must stay
# put for it to run. The agent backend ('gallium' etc.) is a SEPARATE binary,
# installed from its own repo and found on PATH at runtime.
install: build
	@mkdir -p "$(BINDIR)"
	@cp swift/.build/release/kessel-cli "$(BINDIR)/kessel"
	@echo "✅ Installed:"
	@echo "   $(BINDIR)/kessel  — Swift voice app + ACP client. Links the dylib from $(CURDIR)/crates/target/release (keep this repo in place)."
	@echo "   Backend: install 'gallium' (or another codex-app-server) on PATH; override with KESSEL_ACP_BACKEND."
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "   ⚠️  $(BINDIR) is not on your PATH — add it to use 'kessel' directly." ;; esac

uninstall:
	@rm -f "$(BINDIR)/kessel"
	@echo "Removed $(BINDIR)/kessel"

run:
	@echo "Running Kessel (voice, local gallium backend)..."
	@cd swift && swift run kessel-cli --config ../configs/gallium.yaml

run-verbose:
	@echo "Running Kessel (voice, verbose)..."
	@cd swift && swift run kessel-cli --config ../configs/gallium.yaml --verbose

run-text:
	@echo "Running Kessel (text, local gallium backend)..."
	@cd swift && swift run kessel-cli --config ../configs/gallium.yaml --text

run-codex:
	@if [ -z "$$OPENAI_API_KEY" ]; then \
		echo "❌ Error: OPENAI_API_KEY environment variable not set"; \
		echo ""; \
		echo "  export OPENAI_API_KEY=sk-...   # or run inline: OPENAI_API_KEY=sk-... make run-codex"; \
		exit 1; \
	fi
	@echo "Running Kessel against the cloud backend (KESSEL_ACP_BACKEND=codex)..."
	@echo "Using API key: $${OPENAI_API_KEY:0:8}..."
	@cd swift && KESSEL_ACP_BACKEND=codex swift run kessel-cli --config ../configs/codex.yaml

# Windows. Two artifacts:
#
#   kessel.exe        the C# frontend (voice/REPL). Needs uniffi_kessel_core.dll
#                     beside it — the csproj copies the Rust cdylib under that name.
#   kessel_core.dll   the Rust cdylib the frontend links (no in-process inference).
#
# WIN_CONFIG selects the config for run-win. e.g. make run-win WIN_CONFIG=configs/gallium.yaml
WIN_EXE    := win/KesselCli/bin/Release/net8.0-windows/kessel.exe
WIN_CONFIG ?= configs/gallium.yaml

build-win:
	@cmd //C "scripts\\build-win-local.bat"
	@dotnet build win/KesselCli/KesselCli.csproj -c Release --nologo
	@echo "Built:"
	@echo "   $(WIN_EXE)  — C# frontend (voice/REPL)"

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
	ARCHIVE_NAME="kessel-$$TIMESTAMP.zip"; \
	zip -r "$$ARCHIVE_NAME" \
		README.md \
		CLAUDE.md \
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
