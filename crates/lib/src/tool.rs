use crate::llm::{ImageContent, ToolDefinition};
use crate::AgentError;

/// Maximum characters in a tool result before truncation (~2k tokens).
const MAX_OUTPUT_CHARS: usize = 8000;

/// Result of a tool call, containing text and optional images
#[derive(Debug)]
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

    /// Truncate text output if it exceeds `MAX_OUTPUT_CHARS`.
    fn truncate(&mut self) {
        if self.text.len() > MAX_OUTPUT_CHARS {
            let total = self.text.len();
            // Find a safe char boundary to truncate at
            let end = self.text.floor_char_boundary(MAX_OUTPUT_CHARS);
            self.text.truncate(end);
            self.text.push_str(&format!(
                "\n\n... (truncated: showing {}/{} chars. Use offset/limit or filter to narrow results.)",
                end, total
            ));
        }
    }
}

impl From<String> for ToolResult {
    fn from(s: String) -> Self {
        Self::text(s)
    }
}

/// Trait for tool implementations
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError>;

    /// Optional live state snippet appended to description (e.g. "3 messages, last at 12:34").
    /// The framework combines it as: `"{description} [{dynamic_state}]"`.
    fn dynamic_state(&self) -> Option<String> {
        None
    }
}

/// Build the full description for a tool: static description + optional dynamic state.
pub fn full_description(tool: &dyn ToolHandler) -> String {
    match tool.dynamic_state() {
        Some(state) => format!("{} [{}]", tool.description(), state),
        None => tool.description().to_string(),
    }
}

/// Trait for accessing a set of tools (implemented by `ToolRegistry`).
pub trait ToolAccess {
    fn get_definitions(&self) -> Vec<ToolDefinition>;
    fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, AgentError>;
    fn is_empty(&self) -> bool;
}

/// Registry of available tools
pub struct ToolRegistry {
    tools: Vec<Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn ToolHandler>) {
        tracing::info!("Registered tool: {}", tool.name());
        self.tools.push(tool);
    }
}

impl ToolAccess for ToolRegistry {
    fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: full_description(t.as_ref()),
                parameters: t.parameters_schema(),
            })
            .collect()
    }

    fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| AgentError::InternalError(format!("Unknown tool: {}", name)))?;

        tracing::info!("Calling tool: {} with args: {}", name, args);
        let mut result = tool.call(args)?;
        result.truncate();
        tracing::debug!("Tool {} returned {} chars", name, result.text.len());
        Ok(result)
    }

    fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial tool for exercising the registry surface without any built-ins.
    struct EchoTool {
        n: String,
        state: Option<String>,
    }

    impl ToolHandler for EchoTool {
        fn name(&self) -> &str {
            &self.n
        }
        fn description(&self) -> &str {
            "echoes its input"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }
        fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(ToolResult::text(text))
        }
        fn dynamic_state(&self) -> Option<String> {
            self.state.clone()
        }
    }

    #[test]
    fn registry_registers_lists_and_calls() {
        let mut reg = ToolRegistry::new();
        assert!(reg.is_empty());
        reg.register(Box::new(EchoTool {
            n: "echo".into(),
            state: None,
        }));

        let defs = reg.get_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
        assert!(!reg.is_empty());

        let out = reg
            .call("echo", serde_json::json!({ "text": "hi" }))
            .unwrap();
        assert_eq!(out.text, "hi");
    }

    #[test]
    fn registry_reports_unknown_tool() {
        let reg = ToolRegistry::new();
        assert!(reg.call("nope", serde_json::json!({})).is_err());
    }

    #[test]
    fn full_description_appends_dynamic_state() {
        let plain = EchoTool {
            n: "a".into(),
            state: None,
        };
        assert_eq!(full_description(&plain), "echoes its input");
        let stateful = EchoTool {
            n: "b".into(),
            state: Some("2 items".into()),
        };
        assert_eq!(full_description(&stateful), "echoes its input [2 items]");
    }

    #[test]
    fn tool_result_truncates_long_text() {
        let mut r = ToolResult::text("x".repeat(MAX_OUTPUT_CHARS + 500));
        r.truncate();
        assert!(r.text.len() < MAX_OUTPUT_CHARS + 500);
        assert!(r.text.contains("truncated"));
    }

    #[test]
    fn registry_call_truncates_result() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool {
            n: "echo".into(),
            state: None,
        }));
        let big = "y".repeat(MAX_OUTPUT_CHARS + 100);
        let out = reg
            .call("echo", serde_json::json!({ "text": big }))
            .unwrap();
        assert!(out.text.contains("truncated"));
    }
}
