//! ACP client: drive a whole-turn agent backend over line-delimited JSON-RPC.
//!
//! kessel spawns a backend that speaks the codex-app-server subset — `gallium
//! app-server` (the default) or `codex app-server` — and drives it a turn at a
//! time, while serving its own local tools (the VM's `vm_*`, screen `capture`)
//! back to that backend as the protocol's `dynamicTools`.
//!
//! Transport is shared with the server: [`crate::appserver::rpc`] is symmetric
//! (it answers inbound requests, delivers inbound responses to our outbound
//! requests, and dispatches inbound requests on their own threads), so the same
//! `Connection` + `serve` drive either direction. Here kessel *sends*
//! `initialize` / `thread/start` / `turn/start` and *handles* the backend's
//! `item/tool/call` and approval requests.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::appserver::rpc::{serve, Connection, HandlerResult, RequestHandler, RpcFault};
use crate::AgentError;

/// A tool the client serves back to the backend agent (e.g. `vm_run`, a screen
/// capture). The backend's model calls it; the request arrives here as an
/// `item/tool/call` and is executed against local, resident state.
pub trait ClientTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for the tool's arguments.
    fn input_schema(&self) -> Value;
    /// `Ok(text)` on success; `Err(detail)` reports a tool failure back to the
    /// model (a normal ReAct outcome, not a transport error).
    fn call(&self, args: Value) -> Result<String, String>;
}

/// Serve an existing [`ToolHandler`] (the VM's `vm_*`, `capture`, …) back to the
/// backend as a [`ClientTool`], unchanged. The tool runs in-process against its
/// resident state; only `ToolResult.text` crosses the wire (images are dropped —
/// the app-server tool-call response carries text only for now).
pub struct HandlerClientTool(pub Box<dyn crate::tool::ToolHandler>);

impl ClientTool for HandlerClientTool {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn input_schema(&self) -> Value {
        self.0.parameters_schema()
    }
    fn call(&self, args: Value) -> Result<String, String> {
        self.0.call(args).map(|r| r.text).map_err(|e| e.to_string())
    }
}

/// How the client answers a mutation-approval request the backend raises for a
/// `write`/`edit`/`bash` it wants to run in *its* process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalReply {
    Accept,
    AcceptForSession,
    Decline,
}

impl ApprovalReply {
    fn wire(self) -> &'static str {
        match self {
            ApprovalReply::Accept => "accept",
            ApprovalReply::AcceptForSession => "accept_for_session",
            ApprovalReply::Decline => "decline",
        }
    }
}

/// Decides mutation approvals raised by the backend.
pub trait Approver: Send + Sync {
    /// `action` is e.g. `"run command"` or `"file change"`; `target` is the
    /// command or a human-readable reason.
    fn approve(&self, action: &str, target: &str) -> ApprovalReply;
}

/// Declines every mutation. A safe default: a backend turn that only reads or
/// calls client tools is unaffected; a `write`/`bash` is refused rather than
/// silently granted. Frontends that want writes install their own [`Approver`].
pub struct DeclineApprover;

impl Approver for DeclineApprover {
    fn approve(&self, _action: &str, _target: &str) -> ApprovalReply {
        ApprovalReply::Decline
    }
}

/// Inbound state the reader thread and the calling thread share.
struct Shared {
    tools: HashMap<String, Arc<dyn ClientTool>>,
    approver: Arc<dyn Approver>,
    /// Final `agentMessage` text captured from `item/completed`, keyed by turnId.
    replies: Mutex<HashMap<String, String>>,
}

/// Services the backend's inbound traffic: tool calls, approval requests, and the
/// `item/completed` notification that carries the turn's final text.
struct ClientHandler {
    shared: Arc<Shared>,
}

impl RequestHandler for ClientHandler {
    fn handle_request(&self, _conn: &Arc<Connection>, method: &str, params: Value) -> HandlerResult {
        match method {
            "item/tool/call" => {
                let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let (success, text) = match self.shared.tools.get(tool) {
                    Some(t) => match t.call(args) {
                        Ok(text) => (true, text),
                        Err(detail) => (false, detail),
                    },
                    None => (false, format!("unknown client tool '{tool}'")),
                };
                Ok(json!({
                    "success": success,
                    "contentItems": [{ "type": "inputText", "text": text }],
                }))
            }
            "item/commandExecution/requestApproval" => {
                let target = params.get("command").and_then(Value::as_str).unwrap_or("");
                let reply = self.shared.approver.approve("run command", target);
                Ok(json!({ "decision": reply.wire() }))
            }
            "item/fileChange/requestApproval" => {
                let target = params.get("reason").and_then(Value::as_str).unwrap_or("");
                let reply = self.shared.approver.approve("file change", target);
                Ok(json!({ "decision": reply.wire() }))
            }
            _ => Err(RpcFault::method_not_found(method)),
        }
    }

    fn handle_notification(&self, _conn: &Arc<Connection>, method: &str, params: Value) {
        if method != "item/completed" {
            return;
        }
        let item = params.get("item");
        let is_message = item
            .and_then(|i| i.get("type"))
            .and_then(Value::as_str)
            == Some("agentMessage");
        if !is_message {
            return;
        }
        if let (Some(turn), Some(text)) = (
            params.get("turnId").and_then(Value::as_str),
            item.and_then(|i| i.get("text")).and_then(Value::as_str),
        ) {
            self.shared.replies.lock().insert(turn.to_string(), text.to_string());
        }
    }
}

/// A driven connection to a backend agent process.
pub struct AcpClient {
    conn: Arc<Connection>,
    shared: Arc<Shared>,
    thread_id: Mutex<Option<String>>,
    /// The backend subprocess, if we spawned one. Killed on drop. `None` for an
    /// in-process (test) transport.
    child: Mutex<Option<Child>>,
    reader: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl AcpClient {
    /// Spawn `program args… app-server` and drive it. `envs` are set on the child
    /// (the backend reads its model/API config from the environment). `tools` are
    /// served back to the backend as `dynamicTools`; `approver` answers its
    /// mutation requests.
    pub fn spawn(
        program: &str,
        args: &[String],
        envs: &[(String, String)],
        tools: Vec<Arc<dyn ClientTool>>,
        approver: Arc<dyn Approver>,
    ) -> Result<Arc<Self>, AgentError> {
        let mut child = Command::new(program)
            .args(args)
            .arg("app-server")
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| AgentError::ConfigError(format!("spawn backend '{program}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::InternalError("backend stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::InternalError("backend stdout missing".into()))?;

        let client = Self::new_over(BufReader::new(stdout), Box::new(stdin), tools, approver);
        *client.child.lock() = Some(child);
        Ok(client)
    }

    /// Build a client over an arbitrary transport (a subprocess's pipes, or an
    /// in-memory duplex for tests). Spawns the reader thread.
    pub fn new_over<R: BufRead + Send + 'static>(
        reader: R,
        writer: Box<dyn Write + Send>,
        tools: Vec<Arc<dyn ClientTool>>,
        approver: Arc<dyn Approver>,
    ) -> Arc<Self> {
        let tool_map = tools.into_iter().map(|t| (t.name().to_string(), t)).collect();
        let shared = Arc::new(Shared {
            tools: tool_map,
            approver,
            replies: Mutex::new(HashMap::new()),
        });
        let conn = Connection::new(writer);
        let handler = Arc::new(ClientHandler { shared: Arc::clone(&shared) });

        let reader_conn = Arc::clone(&conn);
        let reader_handle = std::thread::spawn(move || serve(reader, reader_conn, handler));

        Arc::new(Self {
            conn,
            shared,
            thread_id: Mutex::new(None),
            child: Mutex::new(None),
            reader: Mutex::new(Some(reader_handle)),
        })
    }

    /// Negotiate capabilities. Returns the backend's `userAgent`.
    pub fn initialize(&self, client_name: &str) -> Result<String, AgentError> {
        let resp = self.conn.request(
            "initialize",
            json!({
                "clientInfo": { "name": client_name },
                "capabilities": { "experimentalApi": true },
            }),
        )?;
        Ok(resp
            .get("userAgent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string())
    }

    /// Open a thread, registering our [`ClientTool`]s as the backend's
    /// `dynamicTools`, and return its id. Does **not** become the client's main
    /// thread — use [`start_thread`](Self::start_thread) for that. Handy for a
    /// throwaway thread (an ambient observation, a goal evaluation) that must not
    /// pollute the conversation history.
    pub fn open_thread(
        &self,
        cwd: Option<&str>,
        model: Option<&str>,
        developer_instructions: Option<&str>,
        approval_policy: Option<&str>,
        config: Option<Value>,
    ) -> Result<String, AgentError> {
        let dynamic_tools: Vec<Value> = self
            .shared
            .tools
            .values()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name(),
                    "description": t.description(),
                    "inputSchema": t.input_schema(),
                })
            })
            .collect();

        let mut params = json!({ "dynamicTools": dynamic_tools });
        if let Some(cwd) = cwd {
            params["cwd"] = json!(cwd);
        }
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        if let Some(instr) = developer_instructions {
            params["developerInstructions"] = json!(instr);
        }
        if let Some(policy) = approval_policy {
            params["approvalPolicy"] = json!(policy);
        }
        // codex nests MCP config under a free-form `config` table; the backend's
        // thread/start reads `config.mcp_servers` and connects them.
        if let Some(config) = config {
            params["config"] = config;
        }

        let resp = self.conn.request("thread/start", params)?;
        resp.get("threadId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| AgentError::InternalError("thread/start returned no threadId".into()))
    }

    /// Open a thread and adopt it as the client's main conversation thread (the
    /// one [`run_turn`](Self::run_turn) drives). Calling again replaces it — the
    /// way to `reset` a conversation.
    pub fn start_thread(
        &self,
        cwd: Option<&str>,
        model: Option<&str>,
        developer_instructions: Option<&str>,
        approval_policy: Option<&str>,
        config: Option<Value>,
    ) -> Result<String, AgentError> {
        let thread_id = self.open_thread(cwd, model, developer_instructions, approval_policy, config)?;
        *self.thread_id.lock() = Some(thread_id.clone());
        Ok(thread_id)
    }

    /// Run one turn on an explicit thread and return the agent's final text.
    /// Blocks until the turn completes (the backend may call our tools
    /// reentrantly meanwhile).
    pub fn run_turn_on(&self, thread_id: &str, text: &str) -> Result<String, AgentError> {
        let resp = self.conn.request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": text }],
            }),
        )?;

        // The final text arrives as an `item/completed` agentMessage notification,
        // which the server writes before the `turn/start` response — so by the
        // time `request` returns, the reader has already captured it.
        let turn_id = resp.get("turnId").and_then(Value::as_str).unwrap_or("");
        Ok(self.shared.replies.lock().remove(turn_id).unwrap_or_default())
    }

    /// Run one turn on the client's main conversation thread.
    pub fn run_turn(&self, text: &str) -> Result<String, AgentError> {
        let thread_id = self
            .thread_id
            .lock()
            .clone()
            .ok_or_else(|| AgentError::InternalError("run_turn before start_thread".into()))?;
        self.run_turn_on(&thread_id, text)
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        // Kill the child first: that closes its stdout, so the reader's `serve`
        // loop sees EOF and returns — only then is it safe to join.
        let had_child = if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
            true
        } else {
            false
        };
        if let Some(handle) = self.reader.lock().take() {
            // With no child (an in-process transport, e.g. tests) nothing closes
            // the reader's input, and joining while we still hold the connection's
            // writer would deadlock — the writer keeps the peer, and thus the
            // reader's input, alive. Detach instead; teardown reaps the thread.
            if had_child {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crossbeam::channel::{unbounded, Receiver, Sender};

    // --- in-memory duplex byte plumbing (mirrors appserver::e2e_tests) --------

    struct ChannelReader {
        rx: Receiver<Vec<u8>>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Read for ChannelReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            while self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(bytes) => {
                        self.buf = bytes;
                        self.pos = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = std::cmp::min(out.len(), self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    struct ByteChannelWriter {
        tx: Sender<Vec<u8>>,
    }

    impl Write for ByteChannelWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            // A dropped receiver just means the peer went away; report it as EOF-ish.
            let _ = self.tx.send(data.to_vec());
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn reader_over(rx: Receiver<Vec<u8>>) -> BufReader<ChannelReader> {
        BufReader::new(ChannelReader { rx, buf: Vec::new(), pos: 0 })
    }

    // --- a hand-rolled backend that speaks the protocol directly --------------

    /// A minimal codex-app-server peer: it answers the client's
    /// `initialize`/`thread/start`/`turn/start` and, on a turn, calls back the
    /// client tool `ping` before emitting the final `agentMessage`. This replaces
    /// the old in-process `AppServer` fixture (removed with the agent core) so the
    /// client is exercised against the wire protocol, not our own server.
    struct StubBackend;

    impl RequestHandler for StubBackend {
        fn handle_request(
            &self,
            conn: &Arc<Connection>,
            method: &str,
            _params: Value,
        ) -> HandlerResult {
            match method {
                "initialize" => Ok(json!({ "userAgent": "kessel-test/0.1.0" })),
                "thread/start" => Ok(json!({ "threadId": "t1" })),
                "turn/start" => {
                    // Reentrantly call the client's `ping` tool mid-turn — the
                    // reader must stay live to deliver the response.
                    let _ = conn.request("item/tool/call", json!({ "tool": "ping", "arguments": {} }));
                    // The final text arrives as an item/completed notification,
                    // written before the turn/start response.
                    conn.notify(
                        "item/completed",
                        json!({
                            "turnId": "turn1",
                            "item": { "type": "agentMessage", "text": "all done" },
                        }),
                    );
                    Ok(json!({ "turnId": "turn1" }))
                }
                _ => Err(RpcFault::method_not_found(method)),
            }
        }
    }

    struct PingTool {
        called: Arc<AtomicBool>,
    }

    impl ClientTool for PingTool {
        fn name(&self) -> &str {
            "ping"
        }
        fn description(&self) -> &str {
            "returns pong"
        }
        fn input_schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        fn call(&self, _args: Value) -> Result<String, String> {
            self.called.store(true, Ordering::SeqCst);
            Ok("pong".to_string())
        }
    }

    /// A full turn where the backend calls a client tool, then answers — driven
    /// end-to-end over the shared transport, no model or network.
    #[test]
    fn drives_a_turn_that_calls_a_client_tool() {
        // client → server bytes, and server → client bytes.
        let (client_out, server_in) = unbounded::<Vec<u8>>();
        let (server_out, client_in) = unbounded::<Vec<u8>>();

        let server_conn = Connection::new(Box::new(ByteChannelWriter { tx: server_out }));
        std::thread::spawn(move || {
            serve(reader_over(server_in), server_conn, Arc::new(StubBackend));
        });

        let called = Arc::new(AtomicBool::new(false));
        let client = AcpClient::new_over(
            reader_over(client_in),
            Box::new(ByteChannelWriter { tx: client_out }),
            vec![Arc::new(PingTool { called: Arc::clone(&called) })],
            Arc::new(DeclineApprover),
        );

        // Watchdog: a transport deadlock would otherwise hang forever. Run the
        // driving calls on a thread and fail if they don't finish promptly.
        let (done_tx, done_rx) = unbounded::<(String, bool)>();
        std::thread::spawn(move || {
            let ua = client.initialize("kessel-test").expect("initialize");
            assert!(ua.contains("kessel"), "userAgent: {ua}");
            client.start_thread(None, None, None, Some("never"), None).expect("thread/start");
            let reply = client.run_turn("hi").expect("run_turn");
            let _ = done_tx.send((reply, called.load(Ordering::SeqCst)));
        });

        let (reply, tool_called) = done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("turn completed within 10s (deadlock otherwise)");
        assert_eq!(reply, "all done");
        assert!(tool_called, "backend never called the client tool");
    }

    /// The VM's real `vm_*` tools serve verbatim through the adapter — no rewrite.
    #[test]
    fn wraps_a_vm_tool_as_a_client_tool() {
        let reset = crate::vm::tools::vm_tool_handlers()
            .into_iter()
            .find(|h| h.name() == "vm_reset")
            .expect("vm_reset present");
        let tool = HandlerClientTool(reset);

        assert_eq!(tool.name(), "vm_reset");
        assert_eq!(tool.input_schema()["type"], "object");
        let out = tool.call(json!({})).expect("vm_reset runs");
        assert!(!out.is_empty(), "vm_reset should report something");
    }
}
