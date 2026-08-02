//! The tool surface the `vm_*` tools implement.
//!
//! Deliberately tiny and self-owned: the VM crate must not depend on any host's
//! tool framework. `kessel mcp` adapts these to MCP's tool/content types, tests
//! call them directly, and a future host can adapt them to whatever it speaks —
//! none of which should require changing this crate.

use serde_json::Value;

/// An image a tool returned. `base64` is the raw encoded payload with no data-URI
/// prefix, so hosts can wrap it however their protocol wants.
#[derive(Debug, Clone)]
pub struct ImageContent {
    pub base64: String,
    /// MIME type, e.g. `"image/png"`.
    pub media_type: String,
}

/// What a `vm_*` tool produced: text for the model to read, plus any frames.
#[derive(Debug, Default)]
pub struct ToolResult {
    pub text: String,
    pub images: Vec<ImageContent>,
}

impl ToolResult {
    pub fn text(s: String) -> Self {
        Self {
            text: s,
            images: vec![],
        }
    }

    pub fn with_images(text: String, images: Vec<ImageContent>) -> Self {
        Self { text, images }
    }
}

impl From<String> for ToolResult {
    fn from(s: String) -> Self {
        Self::text(s)
    }
}

/// A tool call that could not be attempted — a malformed argument, or a console
/// operation that failed outright.
///
/// Note the distinction the `vm_*` tools maintain: a *program* that fails to
/// compile, faults, or halts is a successful tool call whose text reports the
/// failure (the model is meant to read it and debug), not a `VmToolError`.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct VmToolError(pub String);

impl From<String> for VmToolError {
    fn from(s: String) -> Self {
        VmToolError(s)
    }
}

impl From<&str> for VmToolError {
    fn from(s: &str) -> Self {
        VmToolError(s.to_string())
    }
}

/// One tool driving the console. Implementors share a single [`crate::VmConsole`]
/// behind an `Arc<Mutex<…>>`, so the whole set must be built together — see
/// [`crate::tools::vm_tool_handlers_rooted`].
pub trait VmTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for the tool's arguments.
    fn parameters_schema(&self) -> Value;
    fn call(&self, args: Value) -> Result<ToolResult, VmToolError>;
}
