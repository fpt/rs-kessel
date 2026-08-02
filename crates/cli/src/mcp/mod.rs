//! `kessel mcp` — the fantasy console as an MCP stdio server.
//!
//! Speaks line-delimited JSON-RPC 2.0 on stdin/stdout so any MCP-capable agent
//! (Claude Code, codex, gallium, …) can drive the console's full
//! write → assemble → load → run → observe → debug loop.
//!
//! **stdout is the protocol channel** — every diagnostic goes to stderr.

mod server;
mod wire;

use std::io::{BufRead, Write};
use std::path::PathBuf;

pub use server::Server;
use wire::{Request, Response, PARSE_ERROR};

/// Serve the console rooted at `root` on stdin/stdout until the input closes.
pub fn run(root: PathBuf) {
    eprintln!(
        "{} {} serving VM tools, root={}",
        crate::NAME,
        crate::VERSION,
        root.display()
    );
    let server = Server::new(root);
    serve(&server, std::io::stdin().lock(), std::io::stdout().lock());
}

/// The read → dispatch → write loop. Requests are handled one at a time, which
/// matches the console: it is a single machine with one timeline, and running
/// two frames concurrently would be meaningless.
fn serve(server: &Server, input: impl BufRead, mut output: impl Write) {
    for line in input.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("kessel mcp: stdin closed: {e}");
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => server.handle(req),
            Err(e) => {
                // A malformed frame has no id to answer against; JSON-RPC says
                // reply with a null id rather than staying silent.
                eprintln!("kessel mcp: parse error: {e}");
                Some(Response::error(
                    serde_json::Value::Null,
                    PARSE_ERROR,
                    format!("parse error: {e}"),
                ))
            }
        };

        if let Some(resp) = response {
            let json = match serde_json::to_string(&resp) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("kessel mcp: could not serialize response: {e}");
                    continue;
                }
            };
            if writeln!(output, "{json}").is_err() || output.flush().is_err() {
                eprintln!("kessel mcp: stdout closed");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Drive the real loop over in-memory pipes: the framing, the
    /// notification-gets-no-reply rule, and flushing all have to line up.
    #[test]
    fn serves_a_session_over_the_stdio_framing() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            "\n", // blank lines are skipped, not errors
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
        );
        let mut out = Vec::new();
        let server = Server::new(std::env::temp_dir());
        serve(&server, input.as_bytes(), &mut out);

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // Two requests, one notification → exactly two responses.
        assert_eq!(lines.len(), 2, "got: {text}");

        let init: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

        let list: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(list["id"], 2);
        assert!(!list["result"]["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn malformed_json_gets_a_parse_error_with_a_null_id() {
        let mut out = Vec::new();
        let server = Server::new(std::env::temp_dir());
        serve(&server, "not json at all\n".as_bytes(), &mut out);

        let v: Value = serde_json::from_str(String::from_utf8(out).unwrap().trim()).unwrap();
        assert!(v["id"].is_null());
        assert_eq!(v["error"]["code"], PARSE_ERROR);
    }
}
