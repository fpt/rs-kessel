//! MCP / JSON-RPC 2.0 wire types for a **tools-only stdio server**.
//!
//! Hand-rolled rather than taken from an SDK. `rmcp`'s value is its `#[tool]`
//! macros over typed Rust functions; the VM's tools are dynamically dispatched
//! with hand-authored JSON schemas, so those macros buy nothing here — and the
//! tools-only surface is small enough that owning it beats pulling in tokio.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP revision this server implements. The tools-only surface is unchanged
/// across recent revisions, so [`negotiate_version`] echoes whatever the client
/// asks for rather than forcing a downgrade.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const JSONRPC_VERSION: &str = "2.0";

// Standard JSON-RPC 2.0 error codes.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;

/// A JSON-RPC 2.0 request or notification (`id` absent ⇒ notification).
#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

#[derive(Debug, Serialize)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
}

impl Response {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result: None,
            error: Some(ErrorObject {
                code,
                message: message.into(),
            }),
        }
    }
}

/// A tool as advertised by `tools/list`.
#[derive(Debug, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Parameters of a `tools/call` request.
#[derive(Debug, Deserialize)]
pub struct CallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// One content block in a tool result. The `image` variant is what lets an agent
/// actually *see* a frame — the VM's whole observe step depends on it.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        /// Base64, no data-URI prefix.
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// Result of a `tools/call`.
///
/// Note `is_error` is *not* a JSON-RPC error: a tool that ran and failed reports
/// it here so the model can read the message and react, which is exactly what
/// the debug loop needs. JSON-RPC errors are reserved for protocol faults.
#[derive(Debug, Serialize)]
pub struct CallResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// Pick the protocol version to report back. The client sends the revision it
/// wants in `initialize`; echoing it keeps us compatible with both older and
/// newer hosts, since the tools-only wire format they rely on is identical.
pub fn negotiate_version(params: Option<&Value>) -> String {
    params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notification_has_no_id() {
        let r: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(r.is_notification());
    }

    #[test]
    fn image_content_uses_mcp_field_names() {
        let c = Content::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["type"], "image");
        assert_eq!(j["mimeType"], "image/png");
        assert_eq!(j["data"], "AAAA");
    }

    #[test]
    fn clean_result_omits_is_error() {
        let r = CallResult {
            content: vec![Content::Text { text: "ok".into() }],
            is_error: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("isError"), "{s}");
    }

    #[test]
    fn version_echoes_the_client_then_falls_back() {
        let p = json!({"protocolVersion": "2024-11-05"});
        assert_eq!(negotiate_version(Some(&p)), "2024-11-05");
        assert_eq!(negotiate_version(None), PROTOCOL_VERSION);
    }
}
