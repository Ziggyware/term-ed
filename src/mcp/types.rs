use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard MCP tool representation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Request to call an MCP tool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Standard content block in an MCP tool response.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "resource")]
    Resource { uri: String, mime_type: Option<String>, text: Option<String> },
}

/// Result returned from an MCP tool execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

impl McpToolResult {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            content: vec![McpContent::Text { text: text.into() }],
            is_error: false,
        }
    }

    pub fn error(err: impl Into<String>) -> Self {
        Self {
            content: vec![McpContent::Text { text: err.into() }],
            is_error: true,
        }
    }
}
