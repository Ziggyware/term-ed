//! Model Context Protocol (MCP) support for termed.
//!
//! Provides standardized schemas and handlers for file operations, AST search,
//! shell execution, and external tool integration.

pub mod tools;
pub mod types;

pub use tools::standard_mcp_tools;
pub use types::{McpContent, McpTool, McpToolCall, McpToolResult};
