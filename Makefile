.PHONY: help build build-headless install uninstall test fmt fmt-fix clean play mcp

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
	@echo "   kessel play  — open a game window"
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
	@./crates/target/release/kessel play "$(GAME)"

ROOT ?= .
mcp: build
	@./crates/target/release/kessel mcp --root "$(ROOT)"

clean:
	@cd crates && cargo clean
	@echo "Cleaned."
