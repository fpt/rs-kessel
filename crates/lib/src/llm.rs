//! LLM data types shared across the crate (conversation messages, tool
//! definitions, token usage). The in-process provider layer was removed when
//! kessel became an ACP client; inference now lives in the backend agent.

use serde::{Deserialize, Serialize};

// ============================================================================
// Core types
// ============================================================================

/// Token usage information from an LLM API call
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

impl TokenUsage {
    /// Accumulate usage from another call
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
    }
}

/// Chat message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Image content for multimodal messages
#[derive(Debug, Clone)]
pub struct ImageContent {
    pub base64: String,
    pub media_type: String, // "image/png", "image/jpeg"
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Images attached to this message (for vision models)
    #[serde(skip)]
    pub images: Vec<ImageContent>,
    /// Tool calls made by assistant (set by ReAct loop)
    #[serde(skip)]
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    /// Tool call ID this message is responding to (for role=Tool)
    #[serde(skip)]
    pub tool_call_id: Option<String>,
    /// Tool name this message is responding to (for role=Tool)
    #[serde(skip)]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    pub fn user(content: String) -> Self {
        Self {
            role: ChatRole::User,
            content,
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn system(content: String) -> Self {
        Self {
            role: ChatRole::System,
            content,
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn assistant_tool_calls(calls: Vec<ToolCallInfo>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: String::new(),
            images: vec![],
            tool_calls: Some(calls),
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn tool_result(call_id: String, name: String, content: String) -> Self {
        Self {
            role: ChatRole::Tool,
            content,
            images: vec![],
            tool_calls: None,
            tool_call_id: Some(call_id),
            tool_name: Some(name),
        }
    }

    pub fn tool_result_with_images(
        call_id: String,
        name: String,
        content: String,
        images: Vec<ImageContent>,
    ) -> Self {
        Self {
            role: ChatRole::Tool,
            content,
            images,
            tool_calls: None,
            tool_call_id: Some(call_id),
            tool_name: Some(name),
        }
    }
}

/// Tool definition for LLM
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call recorded on an assistant [`ChatMessage`].
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}
