use serde_json::json;
use crate::mcp::types::McpTool;

/// Returns the standard set of MCP tools provided by termed.
pub fn standard_mcp_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "list_files".into(),
            description: "List files in the workspace respecting .gitignore. Returns relative paths.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Subdirectory to list (default '.')" }
                }
            }),
        },
        McpTool {
            name: "read_file".into(),
            description: "Read a file from the workspace, optionally between line numbers.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative file path" },
                    "offset": { "type": "integer", "description": "1-based starting line" },
                    "limit": { "type": "integer", "description": "Maximum lines to read" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "search".into(),
            description: "Search workspace file contents with a regular expression. Returns file:line: text.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression to search" },
                    "path": { "type": "string", "description": "Directory to search; default '.'" }
                },
                "required": ["pattern"]
            }),
        },
        McpTool {
            name: "edit_file".into(),
            description: "Replace old_text with new_text in a workspace file (AEP atomic patch).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative file path" },
                    "old_text": { "type": "string", "description": "Text to replace" },
                    "new_text": { "type": "string", "description": "Replacement text" }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        },
        McpTool {
            name: "write_file".into(),
            description: "Create a new file or completely overwrite an existing file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative file path" },
                    "content": { "type": "string", "description": "Complete file content" }
                },
                "required": ["path", "content"]
            }),
        },
        McpTool {
            name: "run_command".into(),
            description: "Execute a shell command evaluated against the Jev security risk policy.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command string to execute in workspace root" }
                },
                "required": ["command"]
            }),
        },
    ]
}
