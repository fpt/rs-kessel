# Makefile Targets Reference

## Overview

The project includes a comprehensive Makefile with targets for building, running, and testing the kessel with different LLM providers.

## Available Targets

### Building

#### `make build`
Build Rust and Swift components with the default llamacpp provider.

```bash
make build
```

**What it does:**
- Compiles Rust library with `--release` and `llamacpp` feature
- Builds Swift CLI with `-c release`

#### `make build-openai`
Build with OpenAI provider instead of llamacpp.

```bash
make build-openai
```

**What it does:**
- Rebuilds Rust with `--no-default-features --features openai`
- Regenerates UniFFI bindings for new library
- Rebuilds Swift CLI

**⚠️ Warning:** This replaces the llamacpp build. To switch back, run `make build`.

### Running (Local Mode)

#### `make run`
Run in auto-listen voice mode with local llama.cpp.

```bash
make run
```

**Config:** `configs/voice.yaml`
**Requires:** llama.cpp server running (`scripts/start_llm.sh`)

#### `make run-text`
Run in text mode with local llama.cpp.

```bash
make run-text
```

**Config:** `configs/text.yaml`
**Requires:** llama.cpp server running

#### `make run-verbose`
Run voice mode with verbose logging.

```bash
make run-verbose
```

#### `make run-text-verbose`
Run text mode with verbose logging.

```bash
make run-text-verbose
```

### Running (OpenAI Mode)

#### `make run-openai`
Run with OpenAI cloud API.

```bash
export OPENAI_API_KEY=sk-...
make run-openai
```

**Or inline:**
```bash
OPENAI_API_KEY=sk-... make run-openai
```

**Config:** `configs/openai.yaml`
**Requires:**
- `make build-openai` completed first
- `OPENAI_API_KEY` environment variable set

**Error handling:**
- Checks if `OPENAI_API_KEY` is set
- Shows helpful error message if missing
- Exits with status 1 if not set

### Development

#### `make test`
Run all tests (Rust and Swift).

```bash
make test
```

**What it runs:**
- `cargo test` in `crates/`
- `swift test` in `swift/`

#### `make gen-uniffi`
Generate UniFFI Swift bindings from Rust.

```bash
make gen-uniffi
```

**When to use:**
- After changing `agent.udl` interface definition
- After adding new public Rust functions
- After modifying Rust structs exposed to Swift

**Output:** `vendor/uniffi-swift/`
- `kessel_core.swift` - Swift bindings
- `kessel_coreFFI.h` - C header
- `kessel_core.modulemap` - Module map

#### `make install-deps`
Install development dependencies.

```bash
make install-deps
```

**What it does:**
- `cargo fetch` - Downloads Rust dependencies
- `swift package resolve` - Resolves Swift dependencies

#### `make clean`
Clean all build artifacts.

```bash
make clean
```

**What it removes:**
- `crates/target/` - Rust build outputs
- `swift/.build/` - Swift build outputs
- `vendor/uniffi-swift/` - Generated bindings

### Utility Targets

#### `make fmt`
Check code formatting (does not modify files).

```bash
make fmt
```

**What it checks:**
- Rust: `cargo fmt --check`
- Swift: `swift format --check`

#### `make fmt-fix`
Apply code formatting.

```bash
make fmt-fix
```

**What it does:**
- Rust: `cargo fmt --all`
- Swift: `swift format --in-place`

#### `make dev-rust`
Watch Rust code and auto-run checks/tests.

```bash
make dev-rust
```

**Requires:** `cargo-watch` installed
```bash
cargo install cargo-watch
```

#### `make dev-swift`
Quick Swift build for development.

```bash
make dev-swift
```

## Common Workflows

### First Time Setup

```bash
# Install dependencies
make install-deps

# Build everything
make build

# Run tests
make test
```

### Switching to OpenAI

```bash
# Build with OpenAI provider
make build-openai

# Set API key
export OPENAI_API_KEY=sk-...

# Run with OpenAI
make run-openai
```

### Switching Back to Local

```bash
# Rebuild with llamacpp
make build

# Start local server
bash scripts/start_llm.sh

# Run locally
make run
```

### After Modifying Rust Code

```bash
# If you changed the API (.udl file or public functions):
make gen-uniffi

# Always rebuild:
cd crates
cargo build --release

# If OpenAI mode:
make build-openai
```

### Development Cycle

```bash
# Terminal 1: Watch Rust code
make dev-rust

# Terminal 2: Run the agent
make run-text

# Make changes, tests run automatically in Terminal 1
```

## Target Dependencies

```
make run
    ↓
Requires: make build (done once)
    ↓
Requires: llama.cpp server running

make run-openai
    ↓
Requires: make build-openai (done once)
    ↓
Requires: OPENAI_API_KEY environment variable

make build-openai
    ↓
Builds: crates with openai feature
    ↓
Runs: make gen-uniffi
    ↓
Builds: swift
```

## Environment Variables

### `OPENAI_API_KEY`

**Used by:** `make run-openai`
**Required:** Yes (for OpenAI mode)
**Format:** `sk-proj-...` (OpenAI API key)

**Set it:**
```bash
# Export (persistent in session)
export OPENAI_API_KEY=sk-...

# Or inline (one-time)
OPENAI_API_KEY=sk-... make run-openai

# Or in shell profile (~/.zshrc)
echo 'export OPENAI_API_KEY=sk-...' >> ~/.zshrc
source ~/.zshrc
```

### `RUST_LOG`

**Used by:** All run targets
**Required:** No (defaults to "info")
**Format:** Log level (trace, debug, info, warn, error)

**Set it:**
```bash
RUST_LOG=debug make run
```

## Tips

### Quick OpenAI Test

```bash
# One-liner: build and run with OpenAI
make build-openai && OPENAI_API_KEY=sk-... make run-openai
```

### Parallel Development

```bash
# Keep two builds (switch between without rebuilding)

# Build llamacpp version
make build
cp crates/target/release/libkessel_core.dylib /tmp/libkessel_core_llama.dylib

# Build OpenAI version
make build-openai
cp crates/target/release/libkessel_core.dylib /tmp/libkessel_core_openai.dylib

# Switch versions
cp /tmp/libkessel_core_llama.dylib crates/target/release/libkessel_core.dylib
make run

cp /tmp/libkessel_core_openai.dylib crates/target/release/libkessel_core.dylib
OPENAI_API_KEY=sk-... make run-openai
```

### Silent Builds

```bash
make build > /dev/null 2>&1
```

### Show Commands

```bash
# See actual commands being run
make -n run
```

## Troubleshooting

### "library 'kessel_core' not found"

**Solution:** Run `make build` or `make build-openai`

### "OPENAI_API_KEY not set"

**Solution:**
```bash
export OPENAI_API_KEY=sk-...
make run-openai
```

### "llama.cpp server not responding"

**Solution:**
```bash
# Start the server first
bash scripts/start_llm.sh

# Then run
make run
```

### Build fails after switching providers

**Solution:**
```bash
make clean
make build-openai  # or make build
```

## See Also

- [Configuration Guide](./CONFIGURATION.md)
- [OpenAI Setup Guide](./OPENAI_SETUP.md)
- [Development Guide](../CLAUDE.md)
