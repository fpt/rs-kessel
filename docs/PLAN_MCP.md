# Plan: MCP Client Support

## Context

The kessel needs to connect to external MCP (Model Context Protocol) servers to extend its tool capabilities. The user has `godevmcp serve` — an MCP server providing dev tools (code search, file reading, Go/Rust/Python docs, etc.). Swift will call `add_mcp_server(name, command, args)` on the Rust agent via FFI. Rust spawns the MCP server as a child process, connects via stdio JSON-RPC, lists its tools, and makes them available in the ReAct loop.

## Design

### Sync→Async Bridge

The `rmcp` crate (v0.15.0) requires tokio. Current codebase is fully sync. Solution: a lazy-initialized singleton `tokio::runtime::Runtime` in the `mcp` module, used only for MCP operations via `block_on()`. This is safe since all callers are sync (UniFFI entry points from Swift).

### ToolRegistry Interior Mutability

`Agent` is behind `Arc` (UniFFI), so `tool_registry` needs interior mutability for dynamic registration. Change `tools: Vec<...>` → `tools: parking_lot::RwLock<Vec<...>>`. This makes `register()` take `&self` instead of `&mut self`.

### Per-Tool Proxy

Each MCP tool becomes an `McpToolProxy` implementing `ToolHandler`. It holds a shared reference to the rmcp client and the tool's metadata. Its `call()` bridges sync→async via `mcp_runtime().block_on(client.call_tool(...))`.

## Files to Create/Modify

### 1. `crates/Cargo.toml` — Add workspace deps

```toml
tokio = { version = "1", features = ["rt-multi-thread", "process", "sync", "io-util", "macros"] }
rmcp = { version = "0.15", features = ["client", "transport-child-process"] }
```

### 2. `crates/lib/Cargo.toml` — Add deps

```toml
tokio.workspace = true
rmcp.workspace = true
```

### 3. `crates/lib/src/tool.rs` — Interior mutability

Change `ToolRegistry`:
```rust
pub struct ToolRegistry {
    tools: parking_lot::RwLock<Vec<Box<dyn ToolHandler>>>,
}
```
- `register(&self, ...)` — takes `&self`, acquires write lock
- `get_definitions(&self)` — acquires read lock
- `call(&self, ...)` — acquires read lock
- `is_empty(&self)` — acquires read lock
- `create_default_registry()` — no longer needs `mut`

### 4. `crates/lib/src/mcp.rs` — NEW (core implementation)

**a) Lazy tokio runtime:**
```rust
fn mcp_runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new_multi_thread().worker_threads(2).enable_all().build().unwrap())
}
```

**b) `McpServerHandle`** — stores name + rmcp `RunningService` client + cached tool list

**c) `connect_mcp_server(name, command, args)`:**
1. `mcp_runtime().block_on(async { ... })`
2. Create `TokioChildProcess::new(Command::new(command).args(args))`
3. `().serve(transport).await` — MCP handshake
4. `client.list_all_tools().await` — discover tools
5. Return `McpServerHandle`

**d) `McpToolProxy`** implements `ToolHandler`:
- `name()` → tool name from MCP
- `description()` → tool description from MCP
- `parameters_schema()` → `serde_json::to_value(&tool.input_schema)` (MCP `Arc<JsonObject>` → `serde_json::Value`)
- `call(args)` → `mcp_runtime().block_on(client.call_tool(CallToolRequestParams { name, arguments, meta: None, task: None }))` → extract text from `CallToolResult.content` via `Content.as_text().map(|t| t.text)`

### 5. `crates/lib/src/lib.rs` — Agent integration

- Add `mod mcp;`
- Add `mcp_servers: parking_lot::Mutex<Vec<mcp::McpServerHandle>>` to `Agent`
- Add method:
```rust
pub fn add_mcp_server(&self, name: String, command: String, args: Vec<String>) -> Result<(), AgentError> {
    let handle = mcp::connect_mcp_server(&name, &command, &args)?;
    for tool in &handle.tools {
        self.tool_registry.register(Box::new(
            mcp::McpToolProxy::from_mcp_tool(&name, tool, handle.client.clone())
        ));
    }
    self.mcp_servers.lock().push(handle);
    Ok(())
}
```

### 6. `crates/lib/src/agent.udl` — Expose to Swift

```
[Throws=AgentError]
void add_mcp_server(string name, string command, sequence<string> args);
```

### 7. `crates/app/src/main.rs` — Update for ToolRegistry change

No functional change needed — `create_default_registry()` signature unchanged, only `register` goes from `&mut self` to `&self`.

### 8. UniFFI regeneration

```bash
bash scripts/gen_uniffi.sh
cp vendor/uniffi-swift/kessel_core.swift swift/Sources/AgentBridge/
```

### 9. `swift/Sources/KesselCli/main.swift` — Add MCP server

After agent init, before the mode selection:
```swift
// Connect MCP servers
do {
    try agent.addMcpServer(name: "godevmcp", command: "godevmcp", args: ["serve"])
    logger.info("MCP server 'godevmcp' connected")
} catch {
    logger.warning("Failed to connect MCP server: \(error)")
}
```

### 10. `swift/Sources/Util/Config.swift` — MCP config (optional)

Add `McpServerConfig` to YAML config:
```swift
struct McpServerConfig: Codable {
    let name: String
    let command: String
    let args: [String]?
}
// Add to Config: var mcpServers: [McpServerConfig]?
```

## Key rmcp API Types

```
Tool { name: Cow<str>, description: Option<Cow<str>>, input_schema: Arc<JsonObject> }
CallToolRequestParams { meta: Option<Meta>, name: Cow<str>, arguments: Option<JsonObject>, task: Option<TaskMeta> }
CallToolResult { content: Vec<Content>, structured_content: Option<JsonObject>, is_error: Option<bool> }
Content = Annotated<RawContent>  (Deref to RawContent)
RawContent::Text(RawTextContent { text: String })
RawContent.as_text() -> Option<&RawTextContent>
```

## Implementation Order

1. Add tokio + rmcp to workspace and lib Cargo.toml
2. Change ToolRegistry to use RwLock (fix all callers)
3. Create `mcp.rs` — McpServerHandle, McpToolProxy, connect_mcp_server, mcp_runtime
4. Add mcp_servers + add_mcp_server() to Agent in lib.rs
5. Update agent.udl
6. `cd crates && cargo test` — verify existing tests pass
7. `cd crates && cargo build --release`
8. Regenerate UniFFI + copy binding
9. Update main.swift + optionally Config.swift
10. `cd swift && swift build`

## Verification

1. `cd crates && cargo test` — all existing tests pass
2. `cd crates && cargo build --release`
3. `bash scripts/gen_uniffi.sh` + copy binding
4. `cd swift && swift build`
5. Run agent in text mode, ask it to use an MCP tool (e.g., "search for 'ToolHandler' in the codebase") — should invoke godevmcp's `search_local_files` tool via ReAct loop
6. Verify godevmcp process spawns and cleans up on exit
