//! The kessel app-server: exposes the agent as a whole-turn backend over
//! JSON-RPC, speaking a subset of the codex app-server protocol.
//!
//! Kessel does not own this protocol — it implements enough of it that a client
//! already driving `codex app-server` (klein's `internal/codex` runner) can
//! drive kessel by swapping the binary. Methods served:
//!
//! | method          | direction | purpose                                   |
//! |-----------------|-----------|-------------------------------------------|
//! | `initialize`    | in        | capability negotiation                    |
//! | `account/read`  | in        | readiness probe (kessel needs no login)   |
//! | `thread/start`  | in        | create a thread (an `Agent` + registry)   |
//! | `turn/start`    | in        | run one turn, block until it completes    |
//! | `item/tool/call`| out       | invoke a client-provided dynamic tool     |
//! | `item/*/requestApproval` | out | ask the client to permit a mutation  |
//! | `item/completed`, `turn/completed`, `turn/failed` | out | progress |

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::appserver::rpc::{Connection, HandlerResult, RequestHandler, RpcFault};
use crate::appserver::tools::{AutoApproveSink, DynamicToolSpec, RemoteApprovalSink, RemoteTool};
use crate::llm::{create_provider, ChatMessage, LlmProvider};
use crate::react::{self, ReactEvent, ReactObserver};
use crate::situation::SituationMessages;
use crate::skill::SkillRegistry;
use crate::tool::{create_default_registry_with_session, ApprovalSink, ToolRegistry, ToolSession};
use crate::{AgentError, McpServerConfig};

/// Settings the process is launched with; a thread inherits these unless
/// `thread/start` overrides them.
#[derive(Clone, Debug, Default)]
pub struct ServerConfig {
    pub model_path: Option<String>,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    pub reasoning_effort: Option<String>,
    pub max_iterations: Option<u32>,
}

/// One conversation. Owns its provider, tools, and message history — the
/// client's `threadId` is the handle.
struct Thread {
    provider: Box<dyn LlmProvider>,
    registry: ToolRegistry,
    messages: Mutex<Vec<ChatMessage>>,
    max_iterations: Option<u32>,
    /// The turn currently running, read by this thread's `RemoteTool`s so their
    /// callbacks carry the right `turnId`.
    current_turn: Arc<Mutex<String>>,
}

/// Relays ReAct progress to the client as `item/completed` notifications, so a
/// long turn shows its work rather than going silent for minutes.
struct NotifyingObserver<'a> {
    conn: &'a Arc<Connection>,
    thread_id: &'a str,
    turn_id: &'a str,
}

impl ReactObserver for NotifyingObserver<'_> {
    fn on_event(&self, event: ReactEvent<'_>) {
        let item = match event {
            ReactEvent::ToolCall { name, arguments } => json!({
                "type": "commandExecution",
                "command": name,
                "arguments": arguments,
            }),
            ReactEvent::ToolResult { name, text } => json!({
                "type": "toolResult",
                "command": name,
                "text": truncate_for_notification(text),
            }),
        };
        self.conn.notify(
            "item/completed",
            json!({ "threadId": self.thread_id, "turnId": self.turn_id, "item": item }),
        );
    }
}

/// Tool output can be enormous (a whole file). The client only renders progress
/// from these, so cap what crosses the wire; the model still sees the full text.
const NOTIFICATION_TEXT_LIMIT: usize = 2000;

fn truncate_for_notification(text: &str) -> String {
    if text.len() <= NOTIFICATION_TEXT_LIMIT {
        return text.to_string();
    }
    // Cut on a char boundary at or below the limit.
    let mut end = NOTIFICATION_TEXT_LIMIT;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes total)", &text[..end], text.len())
}

/// Builds the LLM provider for a thread, given the server settings and the
/// model the thread asked for. Injectable so tests can drive a turn without a
/// real model behind it.
pub type ProviderFactory =
    Box<dyn Fn(&ServerConfig, &str) -> Result<Box<dyn LlmProvider>, AgentError> + Send + Sync>;

fn default_provider_factory(config: &ServerConfig, model: &str) -> Result<Box<dyn LlmProvider>, AgentError> {
    create_provider(
        config.model_path.clone(),
        config.base_url.clone(),
        model.to_string(),
        config.api_key.clone(),
        config.temperature,
        config.max_tokens,
        config.reasoning_effort.clone(),
    )
    .map_err(|e| AgentError::ConfigError(e.to_string()))
}

pub struct AppServer {
    config: ServerConfig,
    make_provider: ProviderFactory,
    threads: Mutex<HashMap<String, Arc<Thread>>>,
    next_thread: AtomicU64,
    next_turn: AtomicU64,
}

impl AppServer {
    pub fn new(config: ServerConfig) -> Self {
        Self::with_provider_factory(config, Box::new(default_provider_factory))
    }

    pub fn with_provider_factory(config: ServerConfig, make_provider: ProviderFactory) -> Self {
        Self {
            config,
            make_provider,
            threads: Mutex::new(HashMap::new()),
            next_thread: AtomicU64::new(1),
            next_turn: AtomicU64::new(1),
        }
    }

    fn handle_initialize(&self, params: &Value) -> HandlerResult {
        let client = params
            .get("clientInfo")
            .and_then(|c| c.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let experimental = params
            .get("capabilities")
            .and_then(|c| c.get("experimentalApi"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // `dynamicTools` on thread/start is gated behind this capability in the
        // protocol. Kessel accepts threads either way, but a client that has not
        // negotiated it will never get its own tools registered.
        if !experimental {
            tracing::warn!(
                "client '{}' did not negotiate experimentalApi; its dynamicTools will be ignored",
                client
            );
        }
        tracing::info!("initialize from client '{}'", client);

        Ok(json!({
            "userAgent": format!("kessel/{}", env!("CARGO_PKG_VERSION")),
        }))
    }

    /// klein probes this before its first turn to catch an unauthenticated
    /// backend at startup. Kessel authenticates via its own config (an API key
    /// or a local GGUF), which `thread/start` validates by building the provider.
    fn handle_account_read(&self) -> HandlerResult {
        Ok(json!({ "requiresOpenaiAuth": false, "account": null }))
    }

    fn handle_thread_start(&self, conn: &Arc<Connection>, params: Value) -> HandlerResult {
        let params: ThreadStartParams = serde_json::from_value(params)
            .map_err(|e| RpcFault::invalid_params(format!("thread/start: {e}")))?;

        let thread_id = format!("thread_{}", self.next_thread.fetch_add(1, Ordering::SeqCst));

        let working_dir = params
            .cwd
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let model = params.model.clone().unwrap_or_else(|| self.config.model.clone());
        let provider = (self.make_provider)(&self.config, &model)?;

        // Mutations are approved by the client, not by a terminal prompt — except
        // under `approvalPolicy: "never"`, where the client has said it does not
        // want to be asked. An absent policy is treated as "ask": failing toward
        // a question is safer than silently granting write access.
        let approver: Arc<dyn ApprovalSink> = match params.approval_policy.as_deref() {
            Some("never") => Arc::new(AutoApproveSink),
            _ => Arc::new(RemoteApprovalSink::new(Arc::clone(conn), thread_id.clone())),
        };
        let session = Arc::new(ToolSession::with_approver(approver));

        let skills = Arc::new(SkillRegistry::new());
        let situation = Arc::new(SituationMessages::default());
        let mut registry = create_default_registry_with_session(
            working_dir,
            skills,
            situation,
            session,
        );

        // External MCP servers the client asked us to reach.
        for (name, cfg) in params.mcp_servers() {
            let args: Vec<&str> = cfg.args.iter().map(String::as_str).collect();
            match crate::mcp_client::McpClient::connect(&cfg.command, &args) {
                Ok(client) => {
                    for handler in client.tool_handlers() {
                        registry.register(handler);
                    }
                }
                Err(e) => tracing::warn!("failed to connect MCP server '{}': {}", name, e),
            }
        }

        // The client's own tools, dispatched back over this connection. They read
        // the live turn id out of the shared cell that `run_turn` sets.
        let current_turn = Arc::new(Mutex::new(String::new()));
        let dynamic_tools = params.dynamic_tools.clone();
        for spec in &dynamic_tools {
            registry.register(Box::new(RemoteTool::new(
                Arc::clone(conn),
                spec.clone(),
                thread_id.clone(),
                Arc::clone(&current_turn),
            )));
        }

        let mut messages = Vec::new();
        if let Some(instructions) = params.developer_instructions.filter(|s| !s.is_empty()) {
            messages.push(ChatMessage::system(instructions));
        }

        let thread = Arc::new(Thread {
            provider,
            registry,
            messages: Mutex::new(messages),
            max_iterations: self.config.max_iterations,
            current_turn,
        });
        self.threads.lock().insert(thread_id.clone(), thread);

        tracing::info!(
            "thread {} started ({} dynamic tools)",
            thread_id,
            dynamic_tools.len()
        );
        Ok(json!({ "threadId": thread_id }))
    }

    fn handle_turn_start(&self, conn: &Arc<Connection>, params: Value) -> HandlerResult {
        let params: TurnStartParams = serde_json::from_value(params)
            .map_err(|e| RpcFault::invalid_params(format!("turn/start: {e}")))?;

        let thread = self
            .threads
            .lock()
            .get(&params.thread_id)
            .cloned()
            .ok_or_else(|| RpcFault::invalid_params(format!("unknown thread '{}'", params.thread_id)))?;

        let turn_id = format!("turn_{}", self.next_turn.fetch_add(1, Ordering::SeqCst));
        let prompt = params.prompt();

        match self.run_turn(conn, &thread, &params.thread_id, &turn_id, prompt) {
            Ok(text) => {
                conn.notify(
                    "item/completed",
                    json!({
                        "threadId": params.thread_id,
                        "turnId": turn_id,
                        "item": { "type": "agentMessage", "text": text },
                    }),
                );
                conn.notify(
                    "turn/completed",
                    json!({ "threadId": params.thread_id, "turn": { "id": turn_id } }),
                );
                Ok(json!({ "turnId": turn_id }))
            }
            Err(e) => {
                conn.notify(
                    "turn/failed",
                    json!({
                        "threadId": params.thread_id,
                        "turnId": turn_id,
                        "error": { "message": e.to_string() },
                    }),
                );
                Err(RpcFault::from(e))
            }
        }
    }

    /// Run the ReAct loop for one turn against the thread's accumulated history.
    fn run_turn(
        &self,
        conn: &Arc<Connection>,
        thread: &Thread,
        thread_id: &str,
        turn_id: &str,
        prompt: String,
    ) -> Result<String, AgentError> {
        // Publish the turn id before any tool can fire a callback for it.
        *thread.current_turn.lock() = turn_id.to_string();

        let mut messages = thread.messages.lock();
        messages.push(ChatMessage::user(prompt));

        let observer = NotifyingObserver { conn, thread_id, turn_id };
        let (text, _reasoning, _usage) = react::run_observed(
            thread.provider.as_ref(),
            &mut messages,
            &thread.registry,
            thread.max_iterations,
            Some(&observer),
        )?;

        messages.push(ChatMessage::assistant(text.clone()));
        Ok(text)
    }
}

impl RequestHandler for AppServer {
    fn handle_request(&self, conn: &Arc<Connection>, method: &str, params: Value) -> HandlerResult {
        match method {
            "initialize" => self.handle_initialize(&params),
            "account/read" => self.handle_account_read(),
            "thread/start" => self.handle_thread_start(conn, params),
            "turn/start" => self.handle_turn_start(conn, params),
            _ => Err(RpcFault::method_not_found(method)),
        }
    }

    fn handle_notification(&self, _conn: &Arc<Connection>, method: &str, _params: Value) {
        match method {
            "initialized" => tracing::debug!("client finished initialization"),
            other => tracing::debug!("ignoring notification '{}'", other),
        }
    }
}

// ============================================================================
// Wire params
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadStartParams {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    developer_instructions: Option<String>,
    /// `never` auto-approves mutations; anything else asks the client. Kessel has
    /// no sandbox of its own, so codex's `sandbox` field is ignored.
    #[serde(default)]
    approval_policy: Option<String>,
    #[serde(default)]
    dynamic_tools: Vec<DynamicToolSpec>,
    /// `config.mcp_servers` — codex nests MCP config under a free-form table.
    #[serde(default)]
    config: Option<Value>,
}

impl ThreadStartParams {
    /// Pull `config.mcp_servers` out of codex's free-form config table. Only
    /// stdio servers (command/args) are usable — kessel's MCP client spawns
    /// subprocesses and has no URL transport.
    fn mcp_servers(&self) -> Vec<(String, McpServerConfig)> {
        let Some(servers) = self
            .config
            .as_ref()
            .and_then(|c| c.get("mcp_servers"))
            .and_then(Value::as_object)
        else {
            return Vec::new();
        };

        servers
            .iter()
            .filter_map(|(name, entry)| {
                let command = entry.get("command").and_then(Value::as_str)?;
                let args = entry
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                Some((
                    name.clone(),
                    McpServerConfig { command: command.to_string(), args },
                ))
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnStartParams {
    thread_id: String,
    #[serde(default)]
    input: Vec<Value>,
}

impl TurnStartParams {
    /// Concatenate the text items of the turn input. Non-text items (images) are
    /// not yet carried through.
    fn prompt(&self) -> String {
        self.input
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_prompt_joins_text_items() {
        let params: TurnStartParams = serde_json::from_value(json!({
            "threadId": "t1",
            "input": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": "world" },
            ],
        }))
        .unwrap();
        assert_eq!(params.thread_id, "t1");
        assert_eq!(params.prompt(), "hello\nworld");
    }

    #[test]
    fn turn_prompt_skips_non_text_items() {
        let params: TurnStartParams = serde_json::from_value(json!({
            "threadId": "t1",
            "input": [{ "type": "image", "imageUrl": "data:..." }, { "type": "text", "text": "hi" }],
        }))
        .unwrap();
        assert_eq!(params.prompt(), "hi");
    }

    #[test]
    fn thread_start_parses_dynamic_tools_and_instructions() {
        let params: ThreadStartParams = serde_json::from_value(json!({
            "cwd": "/tmp",
            "developerInstructions": "be brief",
            "dynamicTools": [
                { "type": "function", "name": "memory", "description": "d", "inputSchema": {"type": "object"} },
            ],
        }))
        .unwrap();
        assert_eq!(params.cwd.as_deref(), Some("/tmp"));
        assert_eq!(params.developer_instructions.as_deref(), Some("be brief"));
        assert_eq!(params.dynamic_tools.len(), 1);
        assert_eq!(params.dynamic_tools[0].name, "memory");
    }

    #[test]
    fn thread_start_tolerates_a_bare_params_object() {
        let params: ThreadStartParams = serde_json::from_value(json!({})).unwrap();
        assert!(params.dynamic_tools.is_empty());
        assert!(params.mcp_servers().is_empty());
    }

    #[test]
    fn extracts_stdio_mcp_servers_and_skips_url_servers() {
        let params: ThreadStartParams = serde_json::from_value(json!({
            "config": {
                "mcp_servers": {
                    "local": { "command": "srv", "args": ["--a"] },
                    "remote": { "url": "https://example.com" },
                },
            },
        }))
        .unwrap();

        let servers = params.mcp_servers();
        assert_eq!(servers.len(), 1, "url server should be skipped");
        assert_eq!(servers[0].0, "local");
        assert_eq!(servers[0].1.command, "srv");
        assert_eq!(servers[0].1.args, vec!["--a"]);
    }

    #[test]
    fn notification_text_is_truncated_on_a_char_boundary() {
        let text = "é".repeat(NOTIFICATION_TEXT_LIMIT); // 2 bytes each
        let out = truncate_for_notification(&text);
        assert!(out.contains("bytes total"));
        // Must not have panicked or produced invalid UTF-8.
        assert!(out.starts_with('é'));
    }

    #[test]
    fn short_notification_text_passes_through_unchanged() {
        assert_eq!(truncate_for_notification("hi"), "hi");
    }
}
