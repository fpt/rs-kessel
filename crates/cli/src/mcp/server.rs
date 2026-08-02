//! Method dispatch: MCP request in, MCP response out.
//!
//! Kept free of I/O so the whole protocol surface is testable without spawning a
//! process — [`Server::handle`] is a pure function of the request plus the VM's
//! state.

use serde_json::{json, Value};

use kessel_vm::tools::VmToolSet;
use kessel_vm::ToolResult;

use super::wire::{
    negotiate_version, CallParams, CallResult, Content, Request, Response, ToolInfo,
    INVALID_REQUEST, METHOD_NOT_FOUND,
};

pub const SERVER_NAME: &str = "kessel-vm";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Server {
    tools: VmToolSet,
}

impl Server {
    /// Build a server whose console is rooted at `root`. The root is not
    /// optional in practice: the point of this server is that the host agent
    /// edits `game.lua` with its own file tools and the VM compiles *that* file.
    pub fn new(root: std::path::PathBuf) -> Self {
        Self {
            tools: VmToolSet::new(Some(root)),
        }
    }

    /// The console these tools drive, for an attached play window to share.
    pub fn console(&self) -> &kessel_vm::Shared {
        self.tools.console()
    }

    /// Handle one request. `None` means "no reply" — the correct response to a
    /// notification, which JSON-RPC forbids answering.
    pub fn handle(&self, req: Request) -> Option<Response> {
        // Notifications never get a reply, whatever the method.
        if req.is_notification() {
            return None;
        }
        let id = req.id.clone().unwrap_or(Value::Null);

        match req.method.as_str() {
            "initialize" => Some(Response::success(
                id,
                json!({
                    "protocolVersion": negotiate_version(req.params.as_ref()),
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                }),
            )),

            "tools/list" => {
                let tools: Vec<ToolInfo> = self
                    .tools
                    .iter()
                    .map(|t| ToolInfo {
                        name: t.name().to_string(),
                        description: t.description().to_string(),
                        input_schema: t.parameters_schema(),
                    })
                    .collect();
                Some(Response::success(id, json!({ "tools": tools })))
            }

            "tools/call" => {
                let params = match req.params {
                    Some(p) => p,
                    None => {
                        return Some(Response::error(id, INVALID_REQUEST, "missing params"));
                    }
                };
                let call: CallParams = match serde_json::from_value(params) {
                    Ok(c) => c,
                    Err(e) => {
                        return Some(Response::error(
                            id,
                            INVALID_REQUEST,
                            format!("bad tools/call params: {e}"),
                        ));
                    }
                };
                Some(Response::success(id, self.call_tool(call)))
            }

            // Liveness check; an empty result is the whole contract.
            "ping" => Some(Response::success(id, json!({}))),

            other => Some(Response::error(
                id,
                METHOD_NOT_FOUND,
                format!("unknown method '{other}'"),
            )),
        }
    }

    /// Run a tool and shape it into MCP content. A tool that fails comes back as
    /// `isError: true` with the message as text, not a JSON-RPC error — the
    /// model is supposed to read the failure and fix its program.
    fn call_tool(&self, call: CallParams) -> Value {
        let result = match self.tools.call(&call.name, call.arguments) {
            Ok(r) => r,
            Err(e) => {
                return json!(CallResult {
                    content: vec![Content::Text {
                        text: e.to_string()
                    }],
                    is_error: Some(true),
                })
            }
        };
        json!(to_content(result))
    }
}

/// Map a [`ToolResult`] onto MCP content blocks: the text first, then any frames.
fn to_content(result: ToolResult) -> CallResult {
    let mut content = vec![Content::Text { text: result.text }];
    for img in result.images {
        content.push(Content::Image {
            data: img.base64,
            mime_type: img.media_type,
        });
    }
    CallResult {
        content,
        is_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new(std::env::temp_dir())
    }

    fn req(id: u64, method: &str, params: Value) -> Request {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))
        .unwrap()
    }

    fn result_of(resp: Response) -> Value {
        resp.result.expect("a result, not an error")
    }

    #[test]
    fn initialize_advertises_tools() {
        let r = result_of(
            server()
                .handle(req(
                    1,
                    "initialize",
                    json!({"protocolVersion": "2025-06-18"}),
                ))
                .unwrap(),
        );
        assert_eq!(r["protocolVersion"], "2025-06-18");
        assert_eq!(r["capabilities"]["tools"]["listChanged"], false);
        assert_eq!(r["serverInfo"]["name"], SERVER_NAME);
    }

    #[test]
    fn notifications_get_no_reply() {
        let n: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(server().handle(n).is_none());
    }

    #[test]
    fn tools_list_exposes_the_vm_surface() {
        let r = result_of(server().handle(req(1, "tools/list", json!({}))).unwrap());
        let names: Vec<&str> = r["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for expected in [
            "vm_write_source",
            "vm_assemble",
            "vm_load_rom",
            "vm_run_frame",
            "vm_run_frames",
            "vm_get_framebuffer",
            "vm_snapshot",
        ] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
        // Every tool must carry a usable schema, or hosts reject the listing.
        for t in r["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object", "bad schema: {t}");
            assert!(!t["description"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn unknown_method_is_a_jsonrpc_error() {
        let resp = server()
            .handle(req(7, "resources/list", json!({})))
            .unwrap();
        assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
    }

    /// A failing *tool* is a successful call with `isError` — not a JSON-RPC
    /// error — so the model sees the message and can react.
    #[test]
    fn unknown_tool_reports_is_error_in_the_result() {
        let r = result_of(
            server()
                .handle(req(
                    2,
                    "tools/call",
                    json!({"name": "vm_nope", "arguments": {}}),
                ))
                .unwrap(),
        );
        assert_eq!(r["isError"], true);
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("vm_nope"));
    }

    #[test]
    fn framebuffer_comes_back_as_an_image_block() {
        let s = server();
        let r = result_of(
            s.handle(req(
                3,
                "tools/call",
                json!({"name": "vm_get_framebuffer", "arguments": {}}),
            ))
            .unwrap(),
        );
        assert_eq!(r["content"][0]["type"], "text");
        assert_eq!(r["content"][1]["type"], "image");
        assert_eq!(r["content"][1]["mimeType"], "image/png");
        assert!(!r["content"][1]["data"].as_str().unwrap().is_empty());
    }

    /// The full authoring loop over the wire, which is the thing that actually
    /// has to work for an arbitrary MCP host.
    #[test]
    fn write_assemble_load_run_over_mcp() {
        let dir = std::env::temp_dir().join(format!("kessel-mcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Server::new(dir.clone());

        let src = "sprite hero {\n\
                   ..7777..\n\
                   .777777.\n\
                   77777777\n\
                   77.77.77\n\
                   77777777\n\
                   .777777.\n\
                   ..7777..\n\
                   .77..77.\n\
                   }\n\
                   local x = 60\n\
                   function update()\n\
                     if btn(LEFT) then x = x - 1 end\n\
                   end\n\
                   function draw()\n\
                     cls(0)\n\
                     spr(hero, x, 60, 0)\n\
                     entity(x, 60, 1)\n\
                   end\n";

        let call = |name: &str, args: Value| -> Value {
            result_of(
                s.handle(req(
                    9,
                    "tools/call",
                    json!({"name": name, "arguments": args}),
                ))
                .unwrap(),
            )
        };

        let w = call(
            "vm_write_source",
            json!({"path": "game.lua", "source": src}),
        );
        assert!(w["isError"].is_null(), "write failed: {w}");
        // Disk-backed: the file really lands where the host agent's own file
        // tools would find it.
        assert!(dir.join("game.lua").exists());

        let a = call("vm_assemble", json!({"path": "game.lua"}));
        let atext = a["content"][0]["text"].as_str().unwrap();
        assert!(atext.contains("ok"), "assemble said: {atext}");

        call("vm_load_rom", json!({"path": "game.lua"}));

        let r = call(
            "vm_run_frames",
            json!({"script": [
                {"buttons": ["LEFT"], "frames": 4}
            ]}),
        );
        let text = r["content"][0]["text"].as_str().unwrap();
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["frames_run"], 4);
        assert_eq!(v["final"]["entities"][0]["x"], 60 - 4, "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
