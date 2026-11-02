//! Extended AI toolkit: 100 capability-gated host tools for the built-in agent
//! and for MCP clients (external ACP agents).
//!
//! One declaration in [`catalog`] drives schema advertisement, capability
//! gating and dispatch, so an agent can never call a tool that its settings do
//! not advertise. Calls fail closed in `App::host_tool` before dispatch and
//! each implementation re-checks paths, live buffers and read-only previews.
//!
//! Safety rules:
//! * paths resolve through `config::workspace_path`, which rejects `..`/symlink
//!   escapes and credential files;
//! * editing a file that is open in the editor updates the live buffer
//!   (undoable, unsaved) instead of writing disk content over unsaved work;
//! * directory scans, matched items and returned text are bounded;
//! * read-only agents never receive mutating tools, and `deny` command policies
//!   never receive git tools;
//! * git tools run read-only git commands with fixed argv (no shell).
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::app::{App, Focus, PaletteMode, ToastKind};
use crate::commands::COMMANDS;
use crate::config::{AgentConfig, CommandPolicy, workspace_path};
use crate::mcp::McpTool;
use crate::tool_bridge::ToolResult;

/// Files considered by a workspace scan.
const MAX_SCAN_FILES: usize = 2_000;
/// Bytes read from any one file by a scanning tool.
const MAX_SCAN_BYTES: u64 = 512 * 1024;
/// Text returned by a single tool call.
const MAX_OUTPUT_BYTES: usize = 96 * 1024;
/// Directory entries returned by `list_directory`.
const MAX_ENTRIES: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Workspace,
    Text,
    Buffers,
    Edit,
    Filesystem,
    Search,
    Git,
    Ui,
    Code,
    Session,
}

impl Group {
    pub fn id(self) -> &'static str {
        match self {
            Group::Workspace => "workspace",
            Group::Text => "text",
            Group::Buffers => "buffers",
            Group::Edit => "edit",
            Group::Filesystem => "filesystem",
            Group::Search => "search",
            Group::Git => "git",
            Group::Ui => "ui",
            Group::Code => "code",
            Group::Session => "session",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Group::Workspace => "Workspace navigation and file metadata",
            Group::Text => "Text reading, analysis and diffing",
            Group::Buffers => "Live editor buffer inspection",
            Group::Edit => "Undoable edits to live buffers or files",
            Group::Filesystem => "File creation, moves, saves and deletion",
            Group::Search => "Workspace search and replace",
            Group::Git => "Read-only git introspection",
            Group::Ui => "Editor UI navigation and presentation",
            Group::Code => "Language, outline and dependency analysis",
            Group::Session => "Toolkit discovery, settings and scratch notes",
        }
    }

    pub const ALL: [Group; 10] = [
        Group::Workspace,
        Group::Text,
        Group::Buffers,
        Group::Edit,
        Group::Filesystem,
        Group::Search,
        Group::Git,
        Group::Ui,
        Group::Code,
        Group::Session,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Pure reads; always safe.
    Read,
    /// Mutates buffers or files; never advertised to a read-only agent.
    Write,
    /// Read-only git subprocesses; requires commands other than `deny`.
    Git,
    /// Moves the user's view or cursor without changing document text.
    Ui,
}

pub struct Spec {
    pub name: &'static str,
    pub group: Group,
    pub kind: Kind,
    pub description: &'static str,
    pub schema: Value,
}

fn spec(
    name: &'static str,
    group: Group,
    kind: Kind,
    description: &'static str,
    schema: Value,
) -> Spec {
    Spec {
        name,
        group,
        kind,
        description,
        schema,
    }
}

fn objp(props: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":props,"required":required})
}
fn sp(description: &str) -> Value {
    json!({"type":"string","description":description})
}
fn ip(description: &str) -> Value {
    json!({"type":"integer","description":description})
}
fn bp(description: &str) -> Value {
    json!({"type":"boolean","description":description})
}
fn ap(description: &str, items: Value) -> Value {
    json!({"type":"array","description":description,"items":items})
}

/// Every group id, in menu order.
pub fn group_ids() -> Vec<&'static str> {
    Group::ALL.iter().map(|g| g.id()).collect()
}

/// The complete toolkit, independent of the caller's capabilities.
pub fn catalog() -> Vec<Spec> {
    use Group::*;
    use Kind::*;
    let file = || sp("Workspace-relative path; the active buffer when omitted");
    let mut out = Vec::with_capacity(100);

    // ------------------------------------------------------------ workspace
    out.push(spec(
        "workspace_info",
        Workspace,
        Read,
        "Summarize the workspace: root, file count and size, git branch, open and dirty buffers, theme and the toolkit groups this agent can use.",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "list_directory",
        Workspace,
        Read,
        "List one directory with entry type, size and modification time. Does not recurse.",
        objp(
            json!({
                "path": sp("Directory relative to the workspace root; default '.'"),
                "include_hidden": bp("Include dotfiles; default false"),
                "limit": ip("Maximum entries, 1-500; default 200")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "tree_view",
        Workspace,
        Read,
        "Render a bounded text tree of a directory for orientation before deeper exploration.",
        objp(
            json!({
                "path": sp("Directory relative to the workspace root; default '.'"),
                "depth": ip("Maximum depth, 1-6; default 2"),
                "limit": ip("Maximum entries, 1-500; default 200"),
                "include_hidden": bp("Include dotfiles; default false")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "file_info",
        Workspace,
        Read,
        "Report metadata and text facts for a file: size, mtime, lines, characters, language, line endings, indentation, whether it is open or dirty.",
        objp(json!({"path": sp("Workspace-relative file path")}), &["path"]),
    ));
    out.push(spec(
        "find_files",
        Workspace,
        Read,
        "Find workspace files by glob (supports **, *, ?, [abc]) or by case-insensitive name substring.",
        objp(
            json!({
                "pattern": sp("Glob such as '**/*.rs' or a plain name fragment such as 'agent'"),
                "limit": ip("Maximum files, 1-1000; default 100")
            }),
            &["pattern"],
        ),
    ));
    out.push(spec(
        "path_exists",
        Workspace,
        Read,
        "Check up to 64 workspace paths at once, reporting existence, type and size.",
        objp(
            json!({"paths": ap("Workspace-relative paths; at most 64", sp("Path"))}),
            &["paths"],
        ),
    ));
    out.push(spec(
        "workspace_stats",
        Workspace,
        Read,
        "Count files and bytes per extension and list the largest files, to size up a project quickly.",
        objp(
            json!({
                "limit": ip("Largest files to return, 1-50; default 10"),
                "include_hidden": bp("Include dotfiles; default false")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "recent_files",
        Workspace,
        Read,
        "List the most recently modified files, optionally under one directory.",
        objp(
            json!({
                "path": sp("Directory relative to the workspace root; default '.'"),
                "limit": ip("Maximum files, 1-100; default 15")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "file_checksum",
        Workspace,
        Read,
        "Return an FNV-1a 64-bit checksum, byte size and mtime for a file, useful for confirming that an edit landed.",
        objp(json!({"path": sp("Workspace-relative file path")}), &["path"]),
    ));
    out.push(spec(
        "directory_sizes",
        Workspace,
        Read,
        "Report the total size of each direct child of a directory, largest first.",
        objp(
            json!({
                "path": sp("Directory relative to the workspace root; default '.'"),
                "limit": ip("Maximum children, 1-100; default 20")
            }),
            &[],
        ),
    ));

    // ----------------------------------------------------------------- text
    out.push(spec(
        "read_lines",
        Text,
        Read,
        "Read a file or live buffer with 1-based line numbers and an explicit range (default 200 lines).",
        objp(
            json!({
                "path": file(),
                "start_line": ip("First line, 1-based; default 1"),
                "end_line": ip("Last line, inclusive; default start_line + 199")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "grep_context",
        Text,
        Read,
        "Regex search with before/after context lines around each match. Bounded by file count and byte budget.",
        objp(
            json!({
                "pattern": sp("Rust regular expression"),
                "path": sp("Directory to search; default '.'"),
                "include": sp("Optional glob filter such as '**/*.rs'"),
                "before": ip("Context lines before each match, 0-10; default 2"),
                "after": ip("Context lines after each match, 0-10; default 2"),
                "limit": ip("Maximum matches, 1-200; default 40")
            }),
            &["pattern"],
        ),
    ));
    out.push(spec(
        "todo_scan",
        Text,
        Read,
        "Collect TODO, FIXME, HACK and XXX markers with file, line and the remaining message.",
        objp(
            json!({
                "path": sp("Directory to search; default '.'"),
                "tags": ap("Tags to find, default TODO/FIXME/HACK/XXX", sp("Tag")),
                "limit": ip("Maximum markers, 1-500; default 100")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "word_stats",
        Text,
        Read,
        "Frequency of words in a file or live buffer, plus character and line totals.",
        objp(
            json!({
                "path": file(),
                "limit": ip("Top words to return, 1-100; default 20")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "line_stats",
        Text,
        Read,
        "Line counts: total, blank, comment-like, code-like, longest and average length.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "whitespace_report",
        Text,
        Read,
        "Report trailing whitespace, tabs, mixed indentation, CRLF usage and a missing final newline.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "text_diff",
        Text,
        Read,
        "Unified diff between two texts or two workspace files, with added/removed line counts.",
        objp(
            json!({
                "old_text": sp("Original text; omit to read old_path"),
                "new_text": sp("Replacement text; omit to read new_path"),
                "old_path": sp("Workspace-relative original file"),
                "new_path": sp("Workspace-relative replacement file"),
                "context": ip("Context lines, 0-20; default 3")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "buffer_disk_diff",
        Text,
        Read,
        "Diff the live (possibly unsaved) buffer against the saved file content, including change counts.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "json_query",
        Text,
        Read,
        "Validate JSON from a file or live buffer and return the value at an RFC 6901 JSON Pointer.",
        objp(
            json!({
                "path": file(),
                "pointer": sp("JSON Pointer such as '/dependencies/serde'; empty returns the root")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "regex_test",
        Text,
        Read,
        "Test a regular expression against supplied text, returning match text, offsets and capture groups.",
        objp(
            json!({
                "pattern": sp("Rust regular expression"),
                "text": sp("Text to test; at most 200 KiB"),
                "case_insensitive": bp("Case-insensitive matching; default false"),
                "multiline": bp("^ and $ match line boundaries; default false"),
                "dot_all": bp("'.' matches newlines; default false"),
                "limit": ip("Maximum matches, 1-100; default 20")
            }),
            &["pattern", "text"],
        ),
    ));

    // -------------------------------------------------------------- buffers
    out.push(spec(
        "list_buffers",
        Buffers,
        Read,
        "List open editor tabs with paths, dirty state, language, cursor position and line counts.",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "buffer_info",
        Buffers,
        Read,
        "Detailed state of one live buffer: path, dirty/read-only flags, language, endings, indentation, counts, cursor and selection.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "get_selection",
        Buffers,
        Read,
        "Return the current selection text and its 1-based start/end positions.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "cursor_info",
        Buffers,
        Read,
        "Return the caret line/column, line length, indentation and the word under the caret.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "buffer_text",
        Buffers,
        Read,
        "Read live buffer text, optionally a 1-based line range, without touching disk.",
        objp(
            json!({
                "path": file(),
                "start_line": ip("First line, 1-based"),
                "end_line": ip("Last line, inclusive")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "selection_count",
        Buffers,
        Read,
        "Count occurrences of a string, or of the current selection when no text is supplied, in a buffer or file.",
        objp(
            json!({
                "text": sp("Text to count; default the current selection"),
                "path": file(),
                "whole_word": bp("Match whole words only; default false"),
                "case_sensitive": bp("Case-sensitive matching; default true")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "bracket_balance",
        Buffers,
        Read,
        "Check (), [] and {} balance, skipping quoted strings and comments, and report the first mismatch.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "indent_report",
        Buffers,
        Read,
        "Histogram of leading indentation: tab lines, space widths, mixed lines and the dominant unit.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "buffer_changes",
        Buffers,
        Read,
        "Summarize gutter change markers: added, modified and deleted-above line ranges versus the saved text.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "line_at",
        Buffers,
        Read,
        "Return a numbered snippet around a line (the caret line by default) for quick local context.",
        objp(
            json!({
                "path": file(),
                "line": ip("Center line, 1-based; default the caret line"),
                "radius": ip("Lines before and after, 0-50; default 3")
            }),
            &[],
        ),
    ));

    // ----------------------------------------------------------------- edit
    out.push(spec(
        "replace_in_buffer",
        Edit,
        Write,
        "Replace text in a live buffer or file. Literal by default; set regex for $1 replacements. Returns the replacement count.",
        objp(
            json!({
                "path": file(),
                "find": sp("Text or pattern to find"),
                "replace": sp("Replacement text; $1..$9 and $name work with regex"),
                "all": bp("Replace every match; default true"),
                "regex": bp("Treat find as a Rust regular expression; default false"),
                "case_sensitive": bp("Case-sensitive matching; default true")
            }),
            &["find", "replace"],
        ),
    ));
    out.push(spec(
        "replace_line_range",
        Edit,
        Write,
        "Replace an inclusive 1-based line range with new text, preserving line structure.",
        objp(
            json!({
                "path": file(),
                "start_line": ip("First line to replace, 1-based"),
                "end_line": ip("Last line to replace, inclusive"),
                "text": sp("Replacement text; a trailing newline is added when needed")
            }),
            &["start_line", "end_line", "text"],
        ),
    ));
    out.push(spec(
        "insert_lines",
        Edit,
        Write,
        "Insert text before or after a given 1-based line, keeping one line per input line.",
        objp(
            json!({
                "path": file(),
                "line": ip("Reference line, 1-based; 0 means the start of the buffer"),
                "text": sp("Text to insert"),
                "position": sp("'before' (default) or 'after'")
            }),
            &["line", "text"],
        ),
    ));
    out.push(spec(
        "delete_lines",
        Edit,
        Write,
        "Delete an inclusive 1-based line range from a live buffer or file. Undoable when the buffer is open.",
        objp(
            json!({
                "path": file(),
                "start_line": ip("First line, 1-based"),
                "end_line": ip("Last line, inclusive")
            }),
            &["start_line", "end_line"],
        ),
    ));
    out.push(spec(
        "append_text",
        Edit,
        Write,
        "Append text at the end of a buffer or file, adding a separating newline when needed.",
        objp(
            json!({"path": file(),"text": sp("Text to append")}),
            &["text"],
        ),
    ));
    out.push(spec(
        "prepend_text",
        Edit,
        Write,
        "Insert text at the start of a buffer or file, adding a trailing newline when needed.",
        objp(
            json!({"path": file(),"text": sp("Text to prepend")}),
            &["text"],
        ),
    ));
    out.push(spec(
        "trim_trailing_whitespace",
        Edit,
        Write,
        "Remove trailing spaces and tabs from every line and report how many lines changed.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "sort_lines",
        Edit,
        Write,
        "Sort a 1-based line range (or the whole document) ascending or descending, optionally case-insensitively and de-duplicated.",
        objp(
            json!({
                "path": file(),
                "start_line": ip("First line, 1-based; default 1"),
                "end_line": ip("Last line, inclusive; default the last line"),
                "descending": bp("Sort Z-A; default false"),
                "case_insensitive": bp("Ignore case; default false"),
                "unique": bp("Drop duplicate lines, keeping the first; default false")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "dedupe_lines",
        Edit,
        Write,
        "Remove duplicate lines from a 1-based line range, keeping the first occurrence.",
        objp(
            json!({
                "path": file(),
                "start_line": ip("First line, 1-based; default 1"),
                "end_line": ip("Last line, inclusive; default the last line")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "transform_case",
        Edit,
        Write,
        "Change case of a buffer or line range: lower, upper, title, snake, kebab, camel or pascal.",
        objp(
            json!({
                "path": file(),
                "case": sp("lower, upper, title, snake, kebab, camel or pascal"),
                "start_line": ip("First line, 1-based; default 1"),
                "end_line": ip("Last line, inclusive; default the last line")
            }),
            &["case"],
        ),
    ));

    // ----------------------------------------------------------- filesystem
    out.push(spec(
        "create_file",
        Filesystem,
        Write,
        "Create a new text file, or replace it when overwrite is true. An open buffer is updated instead of disk.",
        objp(
            json!({
                "path": sp("Workspace-relative file path"),
                "content": sp("Initial content; default empty"),
                "overwrite": bp("Replace an existing file; default false")
            }),
            &["path"],
        ),
    ));
    out.push(spec(
        "create_directory",
        Filesystem,
        Write,
        "Create a directory and any missing parents inside the workspace.",
        objp(json!({"path": sp("Workspace-relative directory path")}), &["path"]),
    ));
    out.push(spec(
        "copy_path",
        Filesystem,
        Write,
        "Copy one workspace file to another path; refuses directories and existing targets unless overwrite is true.",
        objp(
            json!({
                "from": sp("Source file"),
                "to": sp("Destination file"),
                "overwrite": bp("Replace an existing destination; default false")
            }),
            &["from", "to"],
        ),
    ));
    out.push(spec(
        "move_path",
        Filesystem,
        Write,
        "Rename or move a workspace file, updating an open buffer's path. Refuses existing targets unless overwrite is true.",
        objp(
            json!({
                "from": sp("Source file"),
                "to": sp("Destination file"),
                "overwrite": bp("Replace an existing destination; default false")
            }),
            &["from", "to"],
        ),
    ));
    out.push(spec(
        "delete_path",
        Filesystem,
        Write,
        "Permanently delete a workspace file (or a directory with recursive=true). Requires confirm='DELETE' and never touches .git or open buffers.",
        objp(
            json!({
                "path": sp("Workspace-relative path"),
                "recursive": bp("Allow deleting a directory tree; default false"),
                "confirm": sp("Must be exactly DELETE")
            }),
            &["path", "confirm"],
        ),
    ));
    out.push(spec(
        "write_lines",
        Filesystem,
        Write,
        "Write an array of lines to a file with a chosen line ending and a final newline.",
        objp(
            json!({
                "path": sp("Workspace-relative file path"),
                "lines": ap("Lines to write", sp("Line")),
                "newline": sp("'lf' (default) or 'crlf'"),
                "overwrite": bp("Replace an existing file; default true"),
                "final_newline": bp("End the file with a newline; default true")
            }),
            &["path", "lines"],
        ),
    ));
    out.push(spec(
        "patch_file",
        Filesystem,
        Write,
        "Preview, then atomically apply up to 32 exact, unique text replacements to a file. Nothing is written if any edit fails.",
        objp(
            json!({
                "path": file(),
                "edits": ap("Replacements applied in order", json!({
                    "type":"object",
                    "properties":{"old_text":sp("Text that must match exactly once"),"new_text":sp("Replacement")},
                    "required":["old_text","new_text"]
                })),
                "dry_run": bp("Only validate and show the diff; default true"),
                "confirm": sp("Must be exactly APPLY when dry_run is false")
            }),
            &["edits"],
        ),
    ));
    out.push(spec(
        "save_buffer",
        Filesystem,
        Write,
        "Save one live buffer (the active buffer by default) to its file, including ZIP members.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "save_all_buffers",
        Filesystem,
        Write,
        "Save every dirty buffer that has a path and report saved files and failures.",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "revert_buffer",
        Filesystem,
        Write,
        "Discard unsaved edits in an open buffer and reload it from disk. Requires confirm='DISCARD'.",
        objp(
            json!({"path": file(),"confirm": sp("Must be exactly DISCARD")}),
            &["confirm"],
        ),
    ));

    // --------------------------------------------------------------- search
    out.push(spec(
        "search_workspace",
        Search,
        Read,
        "Regex search across workspace files and live dirty buffers, with case, include/exclude globs and a result limit.",
        objp(
            json!({
                "pattern": sp("Rust regular expression"),
                "path": sp("Directory to search; default '.'"),
                "include": sp("Optional glob filter such as 'src/**/*.rs'"),
                "exclude": sp("Optional glob exclusion such as '**/vendor/**'"),
                "case_sensitive": bp("Case-sensitive matching; default false"),
                "limit": ip("Maximum matches, 1-500; default 100")
            }),
            &["pattern"],
        ),
    ));
    out.push(spec(
        "match_count",
        Search,
        Read,
        "Count regex matches in one buffer/file, or per file across a directory.",
        objp(
            json!({
                "pattern": sp("Rust regular expression"),
                "path": sp("Directory to search, or a file to count in; default '.'"),
                "case_sensitive": bp("Case-sensitive matching; default false"),
                "limit": ip("Maximum matched files, 1-200; default 40")
            }),
            &["pattern"],
        ),
    ));
    out.push(spec(
        "find_references",
        Search,
        Read,
        "Find whole-word references to an identifier across the workspace, with per-file counts and sample locations.",
        objp(
            json!({
                "name": sp("Identifier to find"),
                "path": sp("Directory to search; default '.'"),
                "limit": ip("Maximum locations, 1-300; default 60")
            }),
            &["name"],
        ),
    ));
    out.push(spec(
        "grep_patterns",
        Search,
        Read,
        "Search for up to 16 words or patterns at once and report which patterns matched on each line.",
        objp(
            json!({
                "patterns": ap("Patterns to find (literal by default)", sp("Pattern")),
                "path": sp("Directory to search; default '.'"),
                "regex": bp("Treat patterns as regular expressions; default false"),
                "case_sensitive": bp("Case-sensitive matching; default false"),
                "limit": ip("Maximum matching lines, 1-300; default 100")
            }),
            &["patterns"],
        ),
    ));
    out.push(spec(
        "list_large_files",
        Search,
        Read,
        "List the largest workspace files above a size threshold.",
        objp(
            json!({
                "path": sp("Directory to inspect; default '.'"),
                "threshold_kb": ip("Minimum size in KiB; default 512"),
                "limit": ip("Maximum files, 1-100; default 30")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "list_empty_files",
        Search,
        Read,
        "List zero-byte or whitespace-only text files, candidates for removal or accidental stubs.",
        objp(
            json!({
                "path": sp("Directory to inspect; default '.'"),
                "limit": ip("Maximum files, 1-200; default 100")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "duplicate_basenames",
        Search,
        Read,
        "Group files that share a basename in different directories (useful for splitting or consolidating modules).",
        objp(
            json!({
                "path": sp("Directory to inspect; default '.'"),
                "limit": ip("Maximum groups, 1-100; default 30")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "replace_in_files",
        Search,
        Write,
        "Workspace-wide literal or regex replace. Dry-run by default; applying requires confirm='APPLY'. Live buffers are edited in place.",
        objp(
            json!({
                "pattern": sp("Text or pattern to find"),
                "replacement": sp("Replacement text"),
                "path": sp("Directory to search; default '.'"),
                "include": sp("Optional glob filter such as '**/*.md'"),
                "exclude": sp("Optional glob exclusion"),
                "regex": bp("Treat pattern as a regular expression; default false"),
                "case_sensitive": bp("Case-sensitive matching; default true"),
                "dry_run": bp("Only report changes; default true"),
                "max_files": ip("Maximum files to change, 1-500; default 200"),
                "confirm": sp("Required as APPLY when dry_run is false")
            }),
            &["pattern", "replacement"],
        ),
    ));
    out.push(spec(
        "files_containing_all",
        Search,
        Read,
        "Find files that contain all (or any) of up to eight literal terms.",
        objp(
            json!({
                "terms": ap("Terms to look for", sp("Term")),
                "path": sp("Directory to search; default '.'"),
                "mode": sp("'all' (default) or 'any'"),
                "limit": ip("Maximum files, 1-200; default 50")
            }),
            &["terms"],
        ),
    ));
    out.push(spec(
        "symbol_search",
        Search,
        Read,
        "Heuristically find function, class, struct and constant definitions by name across the workspace.",
        objp(
            json!({
                "name": sp("Name fragment to match; empty lists all definitions"),
                "path": sp("Directory to search; default '.'"),
                "limit": ip("Maximum symbols, 1-300; default 100")
            }),
            &[],
        ),
    ));

    // ------------------------------------------------------------------ git
    out.push(spec(
        "git_status",
        Git,
        Read,
        "Short git status of the workspace, including the branch and staged/unstaged paths.",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "git_diff",
        Git,
        Read,
        "Read-only git diff for the working tree or index, optionally limited to a path or stat-only.",
        objp(
            json!({
                "path": sp("Optional workspace-relative path"),
                "staged": bp("Diff the index against HEAD; default false"),
                "stat": bp("Only show the diffstat; default false")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "git_log",
        Git,
        Read,
        "Recent commit subjects with hashes, authors and dates, optionally for one path.",
        objp(
            json!({
                "limit": ip("Maximum commits, 1-100; default 20"),
                "path": sp("Optional workspace-relative path")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "git_show",
        Git,
        Read,
        "Show one commit, or one file at a revision with rev:path.",
        objp(
            json!({
                "rev": sp("Commit, tag or branch; default HEAD"),
                "path": sp("Optional file to show at that revision")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "git_branches",
        Git,
        Read,
        "List local and remote branches with their latest commit and tracking state.",
        objp(json!({"all": bp("Include remote branches; default true")}), &[]),
    ));
    out.push(spec(
        "git_blame",
        Git,
        Read,
        "Blame a line range of a file: commit, author, date and the source line.",
        objp(
            json!({
                "path": sp("Workspace-relative file"),
                "start_line": ip("First line, 1-based; default 1"),
                "end_line": ip("Last line, inclusive; default start_line + 39")
            }),
            &["path"],
        ),
    ));
    out.push(spec(
        "git_file_history",
        Git,
        Read,
        "Commit history for one file, following renames.",
        objp(
            json!({
                "path": sp("Workspace-relative file"),
                "limit": ip("Maximum commits, 1-100; default 20")
            }),
            &["path"],
        ),
    ));
    out.push(spec(
        "git_remotes",
        Git,
        Read,
        "List configured git remotes and their fetch/push URLs.",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "git_stash_list",
        Git,
        Read,
        "List stash entries with their index and description.",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "git_tags",
        Git,
        Read,
        "List tags, newest first where git supports it.",
        objp(json!({}), &[]),
    ));

    // ------------------------------------------------------------------- ui
    out.push(spec(
        "notify_user",
        Ui,
        Read,
        "Show a short toast to the user, for confirmations or when work needs their attention.",
        objp(
            json!({
                "message": sp("Message, at most 500 bytes"),
                "level": sp("info (default), success, warning or error")
            }),
            &["message"],
        ),
    ));
    out.push(spec(
        "open_file",
        Ui,
        Read,
        "Open a workspace file, image or ZIP member in the editor and optionally place the caret.",
        objp(
            json!({
                "path": sp("Workspace-relative path"),
                "line": ip("Optional 1-based line to focus"),
                "column": ip("Optional 1-based column to focus")
            }),
            &["path"],
        ),
    ));
    out.push(spec(
        "activate_buffer",
        Ui,
        Read,
        "Switch the active editor tab to a buffer by path or name.",
        objp(json!({"path": sp("Buffer path or unique name")}), &["path"]),
    ));
    out.push(spec(
        "close_buffer",
        Ui,
        Write,
        "Close a clean editor tab. Dirty buffers are refused unless discard=true and confirm='DISCARD'.",
        objp(
            json!({
                "path": sp("Buffer path or unique name; default the active buffer"),
                "discard": bp("Discard unsaved edits; default false"),
                "confirm": sp("Required as DISCARD when discard is true")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "go_to_line",
        Ui,
        Read,
        "Move the caret, scroll the view and reveal the line in the active or named buffer.",
        objp(
            json!({
                "line": ip("1-based target line"),
                "column": ip("1-based target column; default 1"),
                "path": file()
            }),
            &["line"],
        ),
    ));
    out.push(spec(
        "select_range",
        Ui,
        Read,
        "Set the editor selection to a 1-based start/end position, optionally in a named buffer.",
        objp(
            json!({
                "start_line": ip("1-based start line"),
                "start_column": ip("1-based start column; default 1"),
                "end_line": ip("1-based end line"),
                "end_column": ip("1-based end column; default end of line"),
                "path": file()
            }),
            &["start_line", "end_line"],
        ),
    ));
    out.push(spec(
        "list_themes",
        Ui,
        Read,
        "List every built-in color theme and mark the active one.",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "set_theme",
        Ui,
        Read,
        "Apply a built-in color theme by exact name.",
        objp(json!({"name": sp("Exact theme name")}), &["name"]),
    ));
    out.push(spec(
        "toggle_view",
        Ui,
        Read,
        "Show, hide or toggle a view setting: sidebar, agent_panel, word_wrap, toolbar, minimap, whitespace, indent_guides or line_numbers.",
        objp(
            json!({
                "setting": sp("Setting name"),
                "value": bp("Explicit value; omit to toggle or cycle automatic")
            }),
            &["setting"],
        ),
    ));
    out.push(spec(
        "get_ui_state",
        Ui,
        Read,
        "Snapshot the interface state: focus, palette/dialog visibility, panels, layout mode, active tab and view settings.",
        objp(json!({}), &[]),
    ));

    // ----------------------------------------------------------------- code
    out.push(spec(
        "detect_language",
        Code,
        Read,
        "Detect the language and syntax name for a path or the active buffer.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "outline_symbols",
        Code,
        Read,
        "Outline one file: Markdown headings or heuristic code definitions with line numbers.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "dependency_summary",
        Code,
        Read,
        "Summarize dependencies declared by Cargo.toml, package.json, pyproject.toml, requirements.txt or go.mod.",
        objp(json!({"path": sp("Directory to inspect; default the workspace root")}), &[]),
    ));
    out.push(spec(
        "license_check",
        Code,
        Read,
        "Scan headers for SPDX identifiers and common license texts, and list files without one.",
        objp(
            json!({
                "path": sp("Directory to scan; default '.'"),
                "limit": ip("Maximum files, 1-1000; default 300")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "markdown_stats",
        Code,
        Read,
        "Markdown structure report: headings by level, links, images, code fences, lists, words and reading time.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "import_summary",
        Code,
        Read,
        "Collect import/use/include lines for a file or directory and count them per file.",
        objp(json!({"path": sp("File or directory; default '.'")}), &[]),
    ));
    out.push(spec(
        "comment_ratio",
        Code,
        Read,
        "Estimate code, comment and blank lines for a file and report the comment ratio.",
        objp(json!({"path": file()}), &[]),
    ));
    out.push(spec(
        "function_sizes",
        Code,
        Read,
        "Estimate definition lengths from definition-line spacing and list the longest blocks.",
        objp(
            json!({
                "path": file(),
                "limit": ip("Longest blocks to return, 1-50; default 15")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "test_map",
        Code,
        Read,
        "Find likely test files for a source path, or list test-like files in the workspace.",
        objp(
            json!({"path": sp("Source file, or a directory to list tests")}),
            &[],
        ),
    ));
    out.push(spec(
        "json_validate",
        Code,
        Read,
        "Validate one JSON file, or every JSON file in the workspace, and report parse errors with line and column.",
        objp(
            json!({
                "path": sp("Optional JSON file or directory; default the whole workspace"),
                "limit": ip("Maximum files, 1-1000; default 200")
            }),
            &[],
        ),
    ));

    // -------------------------------------------------------------- session
    out.push(spec(
        "list_toolkit",
        Session,
        Read,
        "List toolkit groups and tools with their enabled state, optionally for one group.",
        objp(
            json!({
                "group": sp("Optional group id: workspace, text, buffers, edit, filesystem, search, git, ui, code, session"),
                "verbose": bp("Include each tool's description; default false")
            }),
            &[],
        ),
    ));
    out.push(spec(
        "describe_tool",
        Session,
        Read,
        "Return the full description and JSON schema of one tool declared by this editor.",
        objp(json!({"name": sp("Exact tool name")}), &["name"]),
    ));
    out.push(spec(
        "get_agent_settings",
        Session,
        Read,
        "Report this agent's effective capabilities and budgets (no credential values).",
        objp(json!({}), &[]),
    ));
    out.push(spec(
        "list_editor_commands",
        Session,
        Read,
        "List editor commands with their titles and key bindings, for use with run_editor_command.",
        objp(
            json!({"filter": sp("Optional case-insensitive title filter")}),
            &[],
        ),
    ));
    out.push(spec(
        "run_editor_command",
        Session,
        Write,
        "Run an editor command by title or command id. Exits, new chats and agent stops are blocked.",
        objp(json!({"command": sp("Exact title or command id")}), &["command"]),
    ));
    out.push(spec(
        "open_palette",
        Session,
        Read,
        "Open the command palette in a mode: files, commands, themes, languages, goto_line, save_as, agent_models, agent_picker, plugins, settings, agent_settings, sub_agents or skills.",
        objp(json!({"mode": sp("Palette mode")}), &["mode"]),
    ));
    out.push(spec(
        "focus_panel",
        Session,
        Read,
        "Focus the editor, explorer, agent panel, find bar or search panel.",
        objp(json!({"panel": sp("editor, explorer, agent, find or search")}), &["panel"]),
    ));
    out.push(spec(
        "scratchpad_write",
        Session,
        Read,
        "Store or delete a bounded note in the session scratchpad shared across turns (64 keys, 4 KiB each).",
        objp(
            json!({
                "key": sp("Key, at most 64 bytes"),
                "value": sp("Value, at most 4096 bytes"),
                "delete": bp("Delete the key instead; default false")
            }),
            &["key"],
        ),
    ));
    out.push(spec(
        "scratchpad_read",
        Session,
        Read,
        "Read one scratchpad value, or list every key with a short preview and size.",
        objp(json!({"key": sp("Optional key; omit to list all")}), &[]),
    ));
    out.push(spec(
        "session_summary",
        Session,
        Read,
        "One compact snapshot: workspace, buffers, UI state, tasks, generated functions and scratchpad keys.",
        objp(json!({}), &[]),
    ));

    out
}

fn group_enabled(options: &AgentConfig, group: Group) -> bool {
    options.toolkit_groups.is_empty()
        || options
            .toolkit_groups
            .iter()
            .any(|g| g.as_str() == group.id())
}

fn spec_enabled(options: &AgentConfig, spec: &Spec) -> bool {
    options.toolkit
        && group_enabled(options, spec.group)
        && match spec.kind {
            Kind::Read | Kind::Ui => true,
            Kind::Write => !options.read_only,
            Kind::Git => options.commands != CommandPolicy::Deny,
        }
}

/// Advertised tools for one agent configuration, in catalog order.
pub fn tools(options: &AgentConfig) -> Vec<McpTool> {
    catalog()
        .into_iter()
        .filter(|spec| spec_enabled(options, spec))
        .map(|spec| McpTool {
            name: spec.name.into(),
            description: spec.description.into(),
            input_schema: spec.schema,
        })
        .collect()
}

/// Groups this configuration can use, in menu order.
pub fn enabled_group_ids(options: &AgentConfig) -> Vec<&'static str> {
    Group::ALL
        .iter()
        .filter(|group| options.toolkit && group_enabled(options, **group))
        .map(|group| group.id())
        .collect()
}

/// Whether `name` is an enabled toolkit tool for this configuration.
pub fn contains(options: &AgentConfig, name: &str) -> bool {
    catalog()
        .iter()
        .any(|spec| spec.name == name && spec_enabled(options, spec))
}

/// Whether a toolkit tool writes to buffers or files.
pub fn is_write(name: &str) -> bool {
    catalog()
        .iter()
        .any(|spec| spec.name == name && spec.kind == Kind::Write)
}

#[derive(Clone, Copy)]
struct Args<'a>(&'a Value);

impl<'a> Args<'a> {
    fn at(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key)
    }
    fn text(&self, key: &str) -> &'a str {
        self.at(key).and_then(Value::as_str).unwrap_or("")
    }
    fn string(&self, key: &str) -> Result<String, String> {
        match self.at(key) {
            Some(Value::String(text)) => Ok(text.clone()),
            Some(_) => Err(format!("{key} must be a string")),
            None => Err(format!("{key} is required")),
        }
    }
    fn number(&self, key: &str) -> Option<usize> {
        match self.at(key) {
            Some(Value::Number(number)) => number.as_u64().map(|n| n as usize),
            Some(Value::String(text)) => text.trim().parse::<usize>().ok(),
            _ => None,
        }
    }
    fn number_or(&self, key: &str, default: usize) -> usize {
        self.number(key).unwrap_or(default)
    }
    fn flag(&self, key: &str) -> bool {
        self.at(key).and_then(Value::as_bool).unwrap_or(false)
    }
    fn list(&self, key: &str) -> &'a [Value] {
        match self.at(key).and_then(Value::as_array) {
            Some(items) => items.as_slice(),
            None => &[],
        }
    }
    fn string_list(&self, key: &str) -> Result<Vec<String>, String> {
        let items = self
            .at(key)
            .ok_or_else(|| format!("{key} is required"))?
            .as_array()
            .ok_or_else(|| format!("{key} must be an array of strings"))?;
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let text = item
                .as_str()
                .ok_or_else(|| format!("{key} must contain only strings"))?;
            out.push(text.to_string());
        }
        Ok(out)
    }
}

fn display(app: &App, path: &Path) -> String {
    crate::search::relative(&app.explorer.root, path)
}

fn resolve(app: &App, a: &Args, key: &str) -> Result<PathBuf, String> {
    let text = a.text(key);
    if text.is_empty() {
        return Err(format!("{key} is required"));
    }
    workspace_path(&app.explorer.root, text)
}

/// The buffer a tool acts on: `path` when supplied, otherwise the active file.
fn buffer_path(app: &App, a: &Args) -> Result<PathBuf, String> {
    let text = a.text("path");
    if !text.is_empty() {
        return match workspace_path(&app.explorer.root, text) {
            Ok(path) => Ok(path),
            Err(error) => match buffer_index(app, a) {
                Ok(index) => app
                    .editors
                    .get(index)
                    .and_then(|e| e.doc.path.clone())
                    .ok_or(error),
                Err(_) => Err(error),
            },
        };
    }
    app.editors
        .get(app.active)
        .and_then(|e| e.doc.path.clone())
        .ok_or_else(|| "The active buffer has no file path; pass path".to_string())
}

fn buffer_index(app: &App, a: &Args) -> Result<usize, String> {
    let text = a.text("path");
    if text.is_empty() {
        if app.editors.is_empty() {
            return Err("No editor buffer is open".into());
        }
        return Ok(app.active.min(app.editors.len().saturating_sub(1)));
    }
    if let Ok(path) = workspace_path(&app.explorer.root, text)
        && let Some(index) = app
            .editors
            .iter()
            .position(|e| e.doc.path.as_deref() == Some(path.as_path()))
    {
        return Ok(index);
    }
    let needle = text.rsplit(['/', '\\']).next().unwrap_or(text);
    let matches: Vec<usize> = app
        .editors
        .iter()
        .enumerate()
        .filter(|(_, e)| e.doc.name() == needle)
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [only] => Ok(*only),
        [] => Err(format!("No open buffer matches {text}")),
        _ => Err(format!(
            "{needle} matches {} open buffers; pass a full path",
            matches.len()
        )),
    }
}

fn live_text(app: &App, path: &Path) -> Option<String> {
    app.editors
        .iter()
        .find(|e| e.doc.path.as_deref() == Some(path) && !e.doc.is_read_only())
        .map(|e| e.doc.rope.to_string())
}

/// Keep only the first `max` bytes on a character boundary.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (truncated at {max} bytes)", &text[..end])
}

fn read_file_text(path: &Path, limit: usize) -> Result<(String, bool), String> {
    if let Some(member) = crate::archive::member_path(path) {
        let (bytes, _) = crate::archive::read_member(&member).map_err(|e| e.to_string())?;
        let text = String::from_utf8(bytes)
            .map_err(|_| "Archive member is not UTF-8 text".to_string())?;
        let truncated = text.len() > limit;
        return Ok((cut(&text, limit), truncated));
    }
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if meta.len() > 4 * 1024 * 1024 {
        return Err(format!(
            "{} is larger than 4 MiB; read a line range with the read_file tool instead",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(4 * 1024 * 1024).read_to_end(&mut bytes))
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if bytes.contains(&0) {
        return Err(format!("{} is binary, not UTF-8 text", path.display()));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{} is not valid UTF-8 text", path.display()))?;
    let truncated = text.len() > limit;
    Ok((cut(&text, limit), truncated))
}

/// Read a workspace text (live buffer first, then archive member, then disk).
fn read_text(app: &App, path: &Path, limit: usize) -> Result<(String, bool), String> {
    if let Some(text) = live_text(app, path) {
        let truncated = text.len() > limit;
        return Ok((cut(&text, limit), truncated));
    }
    read_file_text(path, limit)
}

/// Edit an open buffer in place (undoable and unsaved) or write a closed file.
fn write_text(app: &mut App, path: &Path, text: &str) -> Result<String, String> {
    if let Some(index) = app
        .editors
        .iter()
        .position(|e| e.doc.path.as_deref() == Some(path))
    {
        let editor = app
            .editors
            .get_mut(index)
            .ok_or_else(|| "Buffer closed while editing".to_string())?;
        if editor.doc.is_read_only() {
            return Err("That buffer is an image or archive preview, not text".into());
        }
        editor.replace_document(text);
        editor.scroll_into_view();
        return Ok("Updated the live editor buffer (unsaved; Ctrl+Z undoes it)".into());
    }
    if crate::archive::member_path(path).is_some() {
        return Err(
            "Open this ZIP member in the editor first; archives are only rewritten through a buffer"
                .into(),
        );
    }
    crate::document::ensure_text_target(path).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    app.invalidate_project_files();
    Ok(format!("Wrote {} ({} bytes)", display(app, path), text.len()))
}

fn normalize(text: &str) -> (String, bool) {
    if text.contains("\r\n") {
        (text.replace("\r\n", "\n"), true)
    } else {
        (text.to_string(), false)
    }
}

fn restore(text: &str, crlf: bool) -> String {
    if crlf { text.replace('\n', "\r\n") } else { text.to_string() }
}

/// Byte offset of the start of every line; the final entry may equal the length.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// Lines of a 1-based inclusive range, without the final newline.
fn segment_items(text: &str, start_line: usize, end_line: usize) -> Result<Vec<String>, String> {
    let starts = line_starts(text);
    let lines = starts.len();
    if start_line == 0 || start_line > lines {
        return Err(format!(
            "start_line {start_line} is outside 1-{lines}"
        ));
    }
    let end_line = end_line.clamp(start_line, lines);
    let start = starts[start_line - 1];
    let end = starts.get(end_line).copied().unwrap_or(text.len());
    let segment = &text[start..end];
    let body = segment.strip_suffix('\n').unwrap_or(segment);
    if body.is_empty() {
        return Ok(vec![String::new()]);
    }
    Ok(body.split('\n').map(String::from).collect())
}

/// Replace a 1-based inclusive line range with `items`, preserving a trailing newline.
fn replace_segment(
    text: &str,
    start_line: usize,
    end_line: usize,
    items: Vec<String>,
) -> Result<String, String> {
    let starts = line_starts(text);
    let lines = starts.len();
    if start_line == 0 || start_line > lines {
        return Err(format!("start_line {start_line} is outside 1-{lines}"));
    }
    let end_line = end_line.clamp(start_line, lines);
    let start = starts[start_line - 1];
    let end = starts.get(end_line).copied().unwrap_or(text.len());
    let trailing = text[start..end].ends_with('\n');
    let mut body = items.join("\n");
    if trailing {
        body.push('\n');
    }
    let mut out = String::with_capacity(text.len() + body.len());
    out.push_str(&text[..start]);
    out.push_str(&body);
    out.push_str(&text[end..]);
    Ok(out)
}

fn compile(
    pattern: &str,
    case_sensitive: bool,
    multiline: bool,
    dot_all: bool,
) -> Result<regex::Regex, String> {
    if pattern.is_empty() {
        return Err("pattern must not be empty".into());
    }
    if pattern.len() > 1024 {
        return Err("pattern exceeds 1024 bytes".into());
    }
    regex::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .multi_line(multiline)
        .dot_matches_new_line(dot_all)
        .size_limit(256 * 1024)
        .dfa_size_limit(256 * 1024)
        .build()
        .map_err(|e| format!("invalid pattern: {e}"))
}

fn modified_secs(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs())
}

fn kind_of(meta: &std::fs::Metadata) -> &'static str {
    if meta.is_dir() {
        "dir"
    } else if meta.is_file() {
        "file"
    } else {
        "other"
    }
}

/// Visit workspace text files, newest content first (live buffers win over disk),
/// for at most `max_files` files. Stops early when `visit` returns false.
fn scan_files<F>(
    app: &App,
    base: &Path,
    include: &str,
    exclude: &str,
    max_files: usize,
    mut visit: F,
) -> usize
where
    F: FnMut(&str, &str) -> bool,
{
    let mut scanned = 0usize;
    let mut seen: HashSet<PathBuf> = HashSet::new();
    if base.is_file() {
        if crate::credentials::is_private_path(base) {
            return 0;
        }
        let Ok(meta) = std::fs::metadata(base) else {
            return 0;
        };
        if !meta.is_file() || meta.len() > MAX_SCAN_BYTES {
            return 0;
        }
        let name = base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let relative = display(app, base);
        let included = include.is_empty()
            || crate::search::glob_match(include, &relative)
            || crate::search::glob_match(include, &name);
        let excluded = !exclude.is_empty()
            && (crate::search::glob_match(exclude, &relative)
                || crate::search::glob_match(exclude, &name));
        if !included || excluded {
            return 0;
        }
        if let Some(text) = live_text(app, base).or_else(|| std::fs::read_to_string(base).ok()) {
            let _ = visit(&relative, &text);
            return 1;
        }
        return 0;
    }
    for relative in crate::explorer::project_files(base, max_files) {
        let path = base.join(&relative);
        if crate::credentials::is_private_path(&path) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_SCAN_BYTES {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !include.is_empty()
            && !crate::search::glob_match(include, &relative)
            && !crate::search::glob_match(include, &name)
        {
            continue;
        }
        if !exclude.is_empty()
            && (crate::search::glob_match(exclude, &relative)
                || crate::search::glob_match(exclude, &name))
        {
            continue;
        }
        let Some(text) = live_text(app, &path).or_else(|| std::fs::read_to_string(&path).ok())
        else {
            continue;
        };
        seen.insert(path.clone());
        scanned += 1;
        if !visit(&display(app, &path), &text) {
            return scanned;
        }
    }
    // Open buffers outside the walk (new or gitignored files) are still live.
    for editor in &app.editors {
        if editor.doc.is_read_only() {
            continue;
        }
        let Some(path) = editor.doc.path.clone() else {
            continue;
        };
        if !path.starts_with(base) || seen.contains(&path) {
            continue;
        }
        let relative = display(app, &path);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !include.is_empty()
            && !crate::search::glob_match(include, &relative)
            && !crate::search::glob_match(include, &name)
        {
            continue;
        }
        if !exclude.is_empty() && crate::search::glob_match(exclude, &relative) {
            continue;
        }
        scanned += 1;
        if !visit(&relative, &editor.doc.rope.to_string()) {
            break;
        }
    }
    scanned
}

fn git_run(app: &App, args: &[String]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&app.explorer.root)
        .arg("--no-optional-locks")
        .arg("-c")
        .arg("core.pager=cat")
        .arg("-c")
        .arg("color.ui=false")
        .args(args)
        .output()
        .map_err(|e| format!("Cannot run git: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            cut(stderr.trim(), 2000)
        ));
    }
    Ok(cut(stdout.trim_end(), 60_000))
}

fn safe_rev(rev: &str) -> Result<String, String> {
    if rev.is_empty() {
        return Ok("HEAD".into());
    }
    let valid = rev.len() <= 200
        && !rev.starts_with('-')
        && rev.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '-' | '.' | '/' | '~' | '^' | '@' | '{' | '}' | ':')
        });
    if !valid {
        return Err("rev must be a revision name (letters, digits, . / _ - ^ ~ @ { } :) and must not start with '-'".into());
    }
    Ok(rev.to_string())
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Read => "read",
        Kind::Write => "write",
        Kind::Git => "git",
        Kind::Ui => "ui",
    }
}

fn policy_label(policy: CommandPolicy) -> &'static str {
    match policy {
        CommandPolicy::Ask => "ask",
        CommandPolicy::Deny => "deny",
        CommandPolicy::RiskAssessed => "risk_assessed",
        CommandPolicy::Allow => "allow",
    }
}

fn guard(value: Value) -> Value {
    let text = value.to_string();
    if text.len() <= MAX_OUTPUT_BYTES {
        return value;
    }
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({
        "truncated": true,
        "bytes": text.len(),
        "note": "Tool output exceeded 96 KiB; narrow the request to see the rest.",
        "preview": &text[..end]
    })
}

/// Dispatch one toolkit call. `App::host_tool` has already verified that the
/// tool is enabled for this agent, so this only implements behavior.
pub(crate) fn call(app: &mut App, options: &AgentConfig, name: &str, raw: &Value) -> ToolResult {
    let args = Args(raw);
    let result = dispatch(app, options, name, args)?;
    Ok(guard(result))
}

fn dispatch(app: &mut App, options: &AgentConfig, name: &str, a: Args) -> ToolResult {
    match name {
        // workspace
        "workspace_info" => workspace_info(app, options),
        "list_directory" => list_directory(app, a),
        "tree_view" => tree_view(app, a),
        "file_info" => file_info(app, a),
        "find_files" => find_files(app, a),
        "path_exists" => path_exists(app, a),
        "workspace_stats" => workspace_stats(app, a),
        "recent_files" => recent_files(app, a),
        "file_checksum" => file_checksum(app, a),
        "directory_sizes" => directory_sizes(app, a),
        // text
        "read_lines" => read_lines(app, a),
        "grep_context" => grep_context(app, a),
        "todo_scan" => todo_scan(app, a),
        "word_stats" => word_stats(app, a),
        "line_stats" => line_stats(app, a),
        "whitespace_report" => whitespace_report(app, a),
        "text_diff" => text_diff(app, a),
        "buffer_disk_diff" => buffer_disk_diff(app, a),
        "json_query" => json_query(app, a),
        "regex_test" => regex_test(a),
        // buffers
        "list_buffers" => list_buffers(app),
        "buffer_info" => buffer_info(app, a),
        "get_selection" => get_selection(app, a),
        "cursor_info" => cursor_info(app, a),
        "buffer_text" => buffer_text(app, a),
        "selection_count" => selection_count(app, a),
        "bracket_balance" => bracket_balance(app, a),
        "indent_report" => indent_report(app, a),
        "buffer_changes" => buffer_changes(app, a),
        "line_at" => line_at(app, a),
        // edit
        "replace_in_buffer" => replace_in_buffer(app, a),
        "replace_line_range" => replace_line_range(app, a),
        "insert_lines" => insert_lines(app, a),
        "delete_lines" => delete_lines(app, a),
        "append_text" => append_text(app, a),
        "prepend_text" => prepend_text(app, a),
        "trim_trailing_whitespace" => trim_trailing_whitespace(app, a),
        "sort_lines" => sort_lines(app, a),
        "dedupe_lines" => dedupe_lines(app, a),
        "transform_case" => transform_case(app, a),
        // filesystem
        "create_file" => create_file(app, a),
        "create_directory" => create_directory(app, a),
        "copy_path" => copy_path(app, a),
        "move_path" => move_path(app, a),
        "delete_path" => delete_path(app, a),
        "write_lines" => write_lines(app, a),
        "patch_file" => patch_file(app, a),
        "save_buffer" => save_buffer(app, a),
        "save_all_buffers" => save_all_buffers(app),
        "revert_buffer" => revert_buffer(app, a),
        // search
        "search_workspace" => search_workspace(app, a),
        "match_count" => match_count(app, a),
        "find_references" => find_references(app, a),
        "grep_patterns" => grep_patterns(app, a),
        "list_large_files" => list_large_files(app, a),
        "list_empty_files" => list_empty_files(app, a),
        "duplicate_basenames" => duplicate_basenames(app, a),
        "replace_in_files" => replace_in_files(app, a),
        "files_containing_all" => files_containing_all(app, a),
        "symbol_search" => symbol_search(app, a),
        // git
        "git_status" => git_status(app),
        "git_diff" => git_diff(app, a),
        "git_log" => git_log(app, a),
        "git_show" => git_show(app, a),
        "git_branches" => git_branches(app, a),
        "git_blame" => git_blame(app, a),
        "git_file_history" => git_file_history(app, a),
        "git_remotes" => git_remotes(app),
        "git_stash_list" => git_stash_list(app),
        "git_tags" => git_tags(app),
        // ui
        "notify_user" => notify_user(app, a),
        "open_file" => open_file(app, a),
        "activate_buffer" => activate_buffer(app, a),
        "close_buffer" => close_buffer(app, a),
        "go_to_line" => go_to_line(app, a),
        "select_range" => select_range(app, a),
        "list_themes" => list_themes(app),
        "set_theme" => set_theme(app, a),
        "toggle_view" => toggle_view(app, a),
        "get_ui_state" => get_ui_state(app),
        // code
        "detect_language" => detect_language(app, a),
        "outline_symbols" => outline_symbols(app, a),
        "dependency_summary" => dependency_summary(app, a),
        "license_check" => license_check(app, a),
        "markdown_stats" => markdown_stats(app, a),
        "import_summary" => import_summary(app, a),
        "comment_ratio" => comment_ratio(app, a),
        "function_sizes" => function_sizes(app, a),
        "test_map" => test_map(app, a),
        "json_validate" => json_validate(app, a),
        // session
        "list_toolkit" => list_toolkit(options, a),
        "describe_tool" => describe_tool(options, a),
        "get_agent_settings" => get_agent_settings(options),
        "list_editor_commands" => list_editor_commands(a),
        "run_editor_command" => run_editor_command(app, options, a),
        "open_palette" => open_palette(app, a),
        "focus_panel" => focus_panel(app, a),
        "scratchpad_write" => scratchpad_write(app, a),
        "scratchpad_read" => scratchpad_read(app, a),
        "session_summary" => session_summary(app, options),
        other => Err(format!(
            "Unknown toolkit tool: {other}; call list_toolkit to see enabled tools"
        )),
    }
}

// ==================================================================== workspace

fn workspace_info(app: &mut App, options: &AgentConfig) -> ToolResult {
    let root = app.explorer.root.clone();
    let files = crate::explorer::project_files(&root, 20_000);
    let mut bytes = 0u64;
    for file in &files {
        if let Ok(meta) = std::fs::metadata(root.join(file)) {
            bytes = bytes.saturating_add(meta.len());
        }
    }
    let groups: Vec<&str> = Group::ALL
        .iter()
        .filter(|group| options.toolkit && group_enabled(options, **group))
        .map(|group| group.id())
        .collect();
    Ok(json!({
        "root": root.to_string_lossy(),
        "name": app.explorer.root_name(),
        "files": files.len(),
        "files_capped": files.len() >= 20_000,
        "bytes": bytes,
        "git_branch": app.git_branch.clone(),
        "open_buffers": app.editors.len(),
        "dirty_buffers": app.editors.iter().filter(|e| e.doc.is_dirty()).count(),
        "theme": app.theme.name.clone(),
        "toolkit": {
            "enabled": options.toolkit,
            "groups": groups,
            "tools": if options.toolkit { tools(options).len() } else { 0 }
        }
    }))
}

fn list_directory(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let hidden = a.flag("include_hidden");
    let limit = a.number_or("limit", 200).clamp(1, MAX_ENTRIES);
    if !base.is_dir() {
        return Err(format!("{} is not a directory", display(app, &base)));
    }
    let read = std::fs::read_dir(&base)
        .map_err(|e| format!("cannot read {}: {e}", display(app, &base)))?;
    let mut entries: Vec<(bool, String, String, u64, Option<u64>)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if crate::credentials::is_private_path(&path) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().ok();
        let kind = match &file_type {
            Some(t) if t.is_dir() => "dir",
            Some(t) if t.is_symlink() => "symlink",
            Some(_) => "file",
            None => "unknown",
        }
        .to_string();
        let is_dir = kind == "dir";
        let meta = entry.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = meta.as_ref().and_then(modified_secs);
        entries.push((is_dir, name, kind, size, modified));
    }
    entries.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    let items: Vec<Value> = entries
        .iter()
        .map(|(_, name, kind, size, modified)| {
            json!({"name": name, "type": kind, "bytes": size, "modified": modified})
        })
        .collect();
    Ok(json!({
        "path": display(app, &base),
        "entries": items,
        "truncated": truncated
    }))
}

fn tree_view(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    if !base.is_dir() {
        return Err(format!("{} is not a directory", display(app, &base)));
    }
    let depth = a.number_or("depth", 2).clamp(1, 6);
    let limit = a.number_or("limit", 200).clamp(1, 500);
    let hidden = a.flag("include_hidden");

    fn walk(
        dir: &Path,
        prefix: &str,
        level: usize,
        depth: usize,
        limit: usize,
        hidden: bool,
        out: &mut Vec<String>,
        truncated: &mut bool,
    ) {
        if level > depth || *truncated {
            return;
        }
        let mut items: Vec<(bool, String, PathBuf)> = Vec::new();
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !hidden && name.starts_with('.') {
                    continue;
                }
                let path = entry.path();
                if crate::credentials::is_private_path(&path) {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                items.push((is_dir, name, path));
            }
        }
        items.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
        for (is_dir, name, path) in items {
            if out.len() >= limit {
                *truncated = true;
                return;
            }
            out.push(format!("{prefix}{name}{}", if is_dir { "/" } else { "" }));
            if is_dir {
                walk(
                    &path,
                    &format!("{prefix}  "),
                    level + 1,
                    depth,
                    limit,
                    hidden,
                    out,
                    truncated,
                );
            }
        }
    }

    let mut out = Vec::new();
    let mut truncated = false;
    walk(
        &base,
        "",
        1,
        depth,
        limit,
        hidden,
        &mut out,
        &mut truncated,
    );
    Ok(json!({
        "path": display(app, &base),
        "depth": depth,
        "entries": out.len(),
        "truncated": truncated,
        "tree": out.join("\n")
    }))
}

fn file_info(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| format!("cannot stat {}: {e}", display(app, &path)))?;
    let relative = display(app, &path);
    let mut out = json!({
        "path": relative,
        "bytes": meta.len(),
        "modified": modified_secs(&meta),
        "type": kind_of(&meta),
        "permissions_readonly": meta.permissions().readonly(),
        "open_in_editor": app.editors.iter().any(|e| e.doc.path.as_deref() == Some(path.as_path())),
        "dirty": app.editors.iter().any(|e| e.doc.path.as_deref() == Some(path.as_path()) && e.doc.is_dirty())
    });
    if !meta.is_file() {
        return Ok(out);
    }
    let live = live_text(app, &path);
    let text = match &live {
        Some(text) => Some(text.clone()),
        None => read_file_text(&path, 2 * 1024 * 1024)
            .ok()
            .map(|(text, _)| text),
    };
    if let Some(obj) = out.as_object_mut() {
        obj.insert("live_buffer".into(), json!(live.is_some()));
        match text {
            None => {
                obj.insert("binary_or_too_large".into(), json!(true));
            }
            Some(text) => {
                let first = text.lines().next().unwrap_or("");
                let language = crate::highlight::detect_syntax(Some(&path), first).name.clone();
                let indent = if text.contains("\n\t") || text.starts_with('\t') {
                    "tabs".to_string()
                } else {
                    let mut spaces = 0usize;
                    for line in text.lines().take(2000) {
                        let width = line.chars().take_while(|c| *c == ' ').count();
                        if width > 0 {
                            spaces = spaces.max(width);
                        }
                    }
                    format!("spaces ({})", spaces.min(8).max(2))
                };
                obj.insert("lines".into(), json!(text.lines().count()));
                obj.insert("characters".into(), json!(text.chars().count()));
                obj.insert("language".into(), json!(language));
                obj.insert(
                    "line_endings".into(),
                    json!(if text.contains("\r\n") { "CRLF" } else { "LF" }),
                );
                obj.insert("final_newline".into(), json!(text.ends_with('\n')));
                obj.insert("indentation".into(), json!(indent));
                obj.insert(
                    "extension".into(),
                    json!(path
                        .extension()
                        .map(|e| e.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default()),
                );
            }
        }
    }
    Ok(out)
}

fn find_files(app: &mut App, a: Args) -> ToolResult {
    let pattern = a.string("pattern")?;
    let limit = a.number_or("limit", 100).clamp(1, 1000);
    let is_glob =
        pattern.contains('*') || pattern.contains('?') || pattern.contains('[');
    let needle = pattern.to_lowercase();
    let files = crate::explorer::project_files(&app.explorer.root, 20_000);
    let mut hits = Vec::new();
    for file in files {
        let name = file.rsplit('/').next().unwrap_or(file.as_str());
        let matched = if is_glob {
            crate::search::glob_match(&pattern, &file)
                || crate::search::glob_match(&pattern, name)
        } else {
            file.to_lowercase().contains(&needle)
        };
        if matched {
            let size = std::fs::metadata(app.explorer.root.join(&file))
                .map(|m| m.len())
                .unwrap_or(0);
            hits.push(json!({"path": file, "bytes": size}));
            if hits.len() >= limit {
                break;
            }
        }
    }
    Ok(json!({"pattern": pattern, "glob": is_glob, "files": hits}))
}

fn path_exists(app: &mut App, a: Args) -> ToolResult {
    let paths = a.string_list("paths")?;
    if paths.len() > 64 {
        return Err("paths is limited to 64 entries".into());
    }
    let mut out = Vec::with_capacity(paths.len());
    for input in paths {
        match workspace_path(&app.explorer.root, &input) {
            Ok(path) => match std::fs::metadata(&path) {
                Ok(meta) => out.push(json!({
                    "path": input,
                    "exists": true,
                    "type": kind_of(&meta),
                    "bytes": meta.len(),
                    "modified": modified_secs(&meta)
                })),
                Err(_) => out.push(json!({"path": input, "exists": false})),
            },
            Err(error) => out.push(json!({"path": input, "exists": false, "error": error})),
        }
    }
    Ok(json!(out))
}

fn workspace_stats(app: &mut App, a: Args) -> ToolResult {
    let limit = a.number_or("limit", 10).clamp(1, 50);
    let files = crate::explorer::project_files(&app.explorer.root, 20_000);
    let mut by_extension: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut largest: Vec<(u64, String)> = Vec::new();
    let mut total_bytes = 0u64;
    for file in &files {
        let Ok(meta) = std::fs::metadata(app.explorer.root.join(file)) else {
            continue;
        };
        total_bytes = total_bytes.saturating_add(meta.len());
        let extension = Path::new(file)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| "(none)".to_string());
        let entry = by_extension.entry(extension).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(meta.len());
        largest.push((meta.len(), file.clone()));
    }
    largest.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    largest.truncate(limit);
    let extensions: Vec<Value> = by_extension
        .into_iter()
        .map(|(ext, (count, bytes))| json!({"extension": ext, "files": count, "bytes": bytes}))
        .collect();
    let large: Vec<Value> = largest
        .into_iter()
        .map(|(bytes, path)| json!({"path": path, "bytes": bytes}))
        .collect();
    Ok(json!({
        "files": files.len(),
        "files_capped": files.len() >= 20_000,
        "bytes": total_bytes,
        "extensions": extensions,
        "largest": large
    }))
}

fn recent_files(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 15).clamp(1, 100);
    let mut files: Vec<(u64, String, u64)> = Vec::new();
    for relative in crate::explorer::project_files(&base, 20_000) {
        let path = base.join(&relative);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if let Some(secs) = modified_secs(&meta) {
            files.push((secs, display(app, &path), meta.len()));
        }
    }
    files.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    files.truncate(limit);
    let items: Vec<Value> = files
        .into_iter()
        .map(|(secs, path, bytes)| {
            json!({"path": path, "modified": secs, "bytes": bytes})
        })
        .collect();
    Ok(json!({"files": items}))
}

fn fnv1a(path: &Path) -> Result<(u64, u64), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut total = 0u64;
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        for byte in &buffer[..read] {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    Ok((hash, total))
}

fn file_checksum(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| format!("cannot stat {}: {e}", display(app, &path)))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", display(app, &path)));
    }
    let (hash, bytes) = fnv1a(&path)?;
    let live = app
        .editors
        .iter()
        .any(|e| e.doc.path.as_deref() == Some(path.as_path()) && e.doc.is_dirty());
    Ok(json!({
        "path": display(app, &path),
        "algorithm": "fnv1a64",
        "hex": format!("{hash:016x}"),
        "bytes": bytes,
        "modified": modified_secs(&meta),
        "dirty_buffer": live,
        "note": if live { "Checksum covers the file on disk; the buffer has unsaved edits" } else { "Checksum covers the file on disk" }
    }))
}

fn directory_sizes(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    if !base.is_dir() {
        return Err(format!("{} is not a directory", display(app, &base)));
    }
    let limit = a.number_or("limit", 20).clamp(1, 100);
    fn size_of(path: &Path, budget: &mut usize) -> u64 {
        let mut total = 0u64;
        if *budget == 0 {
            return 0;
        }
        let Ok(read) = std::fs::read_dir(path) else {
            return 0;
        };
        for entry in read.flatten() {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                total = total.saturating_add(size_of(&entry.path(), budget));
            } else if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
        total
    }
    let read = std::fs::read_dir(&base)
        .map_err(|e| format!("cannot read {}: {e}", display(app, &base)))?;
    let mut items: Vec<(u64, String)> = Vec::new();
    for entry in read.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut budget = 2_000usize;
        let bytes = if file_type.is_dir() {
            size_of(&entry.path(), &mut budget)
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        items.push((bytes, name));
    }
    items.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    let truncated = items.len() > limit;
    items.truncate(limit);
    let entries: Vec<Value> = items
        .into_iter()
        .map(|(bytes, name)| json!({"name": name, "bytes": bytes}))
        .collect();
    Ok(json!({
        "path": display(app, &base),
        "entries": entries,
        "truncated": truncated,
        "note": "Directory sizes are bounded to 2,000 entries each"
    }))
}

// ========================================================================= text

fn read_lines(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, truncated) = read_text(app, &path, 2 * 1024 * 1024)?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = a.number("start_line").unwrap_or(1).max(1);
    let end = a
        .number("end_line")
        .unwrap_or_else(|| start.saturating_add(199))
        .max(start)
        .min(total.max(start));
    let body = match lines.get(start.saturating_sub(1)..end) {
        Some(slice) => slice
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{}\t{line}", start + index))
            .collect::<Vec<_>>()
            .join("\n"),
        None => String::new(),
    };
    Ok(json!({
        "path": display(app, &path),
        "total_lines": total,
        "start_line": start,
        "end_line": end,
        "text": body,
        "truncated": truncated
    }))
}

fn grep_context(app: &mut App, a: Args) -> ToolResult {
    let pattern = a.string("pattern")?;
    let re = compile(&pattern, true, false, false)?;
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let include = a.text("include").to_string();
    let before = a.number_or("before", 2).min(10);
    let after = a.number_or("after", 2).min(10);
    let limit = a.number_or("limit", 40).clamp(1, 200);
    let mut hits: Vec<Value> = Vec::new();
    let scanned = scan_files(app, &base, &include, "", MAX_SCAN_FILES, |relative, text| {
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !re.is_match(line) {
                continue;
            }
            let from = index.saturating_sub(before);
            let to = (index + after + 1).min(lines.len());
            let context_before: Vec<&str> = lines.get(from..index).unwrap_or(&[]).to_vec();
            let context_after: Vec<&str> = lines.get(index + 1..to).unwrap_or(&[]).to_vec();
            hits.push(json!({
                "file": relative,
                "line": index + 1,
                "text": cut(line, 300),
                "before": context_before,
                "after": context_after
            }));
            if hits.len() >= limit {
                return false;
            }
        }
        true
    });
    Ok(json!({
        "pattern": pattern,
        "scanned": scanned,
        "matches": hits,
        "truncated": hits.len() >= limit
    }))
}

fn todo_scan(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let mut tags = a.string_list("tags")?;
    if tags.is_empty() {
        tags = vec![
            "TODO".into(),
            "FIXME".into(),
            "HACK".into(),
            "XXX".into(),
        ];
    }
    for tag in &tags {
        if tag.is_empty() || tag.len() > 24 {
            return Err("Each tag must be 1-24 bytes".into());
        }
    }
    let limit = a.number_or("limit", 100).clamp(1, 500);
    let mut hits: Vec<Value> = Vec::new();
    let scanned = scan_files(app, &base, "", "", MAX_SCAN_FILES, |relative, text| {
        for (index, line) in text.lines().enumerate() {
            let upper = line.to_uppercase();
            for tag in &tags {
                let Some(position) = upper.find(tag.as_str()) else {
                    continue;
                };
                let rest = line
                    .get(position + tag.len()..)
                    .unwrap_or("")
                    .trim_start_matches(|c: char| c == ':' || c == '-' || c.is_whitespace())
                    .trim();
                hits.push(json!({
                    "tag": tag,
                    "file": relative,
                    "line": index + 1,
                    "message": cut(rest, 240)
                }));
                if hits.len() >= limit {
                    return false;
                }
            }
        }
        true
    });
    Ok(json!({"tags": tags, "scanned": scanned, "markers": hits}))
}

fn word_stats(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 4 * 1024 * 1024)?;
    let limit = a.number_or("limit", 20).clamp(1, 100);
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut word = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() || character == '_' || character == '\'' {
            for lower in character.to_lowercase() {
                word.push(lower);
            }
        } else if !word.is_empty() {
            *counts.entry(std::mem::take(&mut word)).or_insert(0) += 1;
        }
    }
    if !word.is_empty() {
        *counts.entry(word).or_insert(0) += 1;
    }
    let total_words: usize = counts.values().sum();
    let unique = counts.len();
    let mut top: Vec<(String, usize)> = counts.into_iter().collect();
    top.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    top.truncate(limit);
    let words: Vec<Value> = top
        .into_iter()
        .map(|(word, count)| json!({"word": word, "count": count}))
        .collect();
    Ok(json!({
        "path": display(app, &path),
        "words": total_words,
        "unique_words": unique,
        "characters": text.chars().count(),
        "lines": text.lines().count(),
        "top": words
    }))
}

fn is_comment_line(trimmed: &str) -> bool {
    trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with("--")
        || trimmed.starts_with(';')
        || trimmed.starts_with("<!--")
        || trimmed.starts_with("rem ")
}

fn line_stats(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 4 * 1024 * 1024)?;
    let mut total = 0usize;
    let mut blank = 0usize;
    let mut comments = 0usize;
    let mut longest = (0usize, 0usize);
    let mut over_120 = 0usize;
    let mut length_sum = 0usize;
    for (index, line) in text.lines().enumerate() {
        total += 1;
        let length = line.chars().count();
        length_sum += length;
        if line.trim().is_empty() {
            blank += 1;
        } else if is_comment_line(line.trim_start()) {
            comments += 1;
        }
        if length > longest.1 {
            longest = (index + 1, length);
        }
        if length > 120 {
            over_120 += 1;
        }
    }
    let average = if total == 0 { 0 } else { length_sum / total };
    Ok(json!({
        "path": display(app, &path),
        "lines": total,
        "blank": blank,
        "comment_like": comments,
        "code_like": total.saturating_sub(blank + comments),
        "longest_line": {"line": longest.0, "characters": longest.1},
        "lines_over_120": over_120,
        "average_characters": average,
        "final_newline": text.ends_with('\n')
    }))
}

fn whitespace_report(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let mut trailing = Vec::new();
    let mut trailing_lines = 0usize;
    let mut tab_lines = 0usize;
    let mut space_indented = 0usize;
    let mut mixed = 0usize;
    let mut blank_run = 0usize;
    let mut longest_blank_run = 0usize;
    for (index, line) in text.lines().enumerate() {
        if line.len() != line.trim_end().len() {
            trailing_lines += 1;
            if trailing.len() < 20 {
                trailing.push(index + 1);
            }
        }
        let leading: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        if leading.contains('\t') {
            tab_lines += 1;
        }
        if leading.contains(' ') && leading.contains('\t') {
            mixed += 1;
        } else if leading.contains(' ') {
            space_indented += 1;
        }
        if line.trim().is_empty() {
            blank_run += 1;
            longest_blank_run = longest_blank_run.max(blank_run);
        } else {
            blank_run = 0;
        }
    }
    Ok(json!({
        "path": display(app, &path),
        "trailing_whitespace_lines": trailing_lines,
        "trailing_samples": trailing,
        "tab_indented_lines": tab_lines,
        "space_indented_lines": space_indented,
        "mixed_indentation_lines": mixed,
        "crlf": text.contains("\r\n"),
        "final_newline": text.ends_with('\n'),
        "longest_blank_run": longest_blank_run
    }))
}

fn text_diff(app: &mut App, a: Args) -> ToolResult {
    let old = if let Some(input) = a
        .at("old_path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let path = workspace_path(&app.explorer.root, input)?;
        read_text(app, &path, 2 * 1024 * 1024)?.0
    } else {
        a.text("old_text").to_string()
    };
    let new = if let Some(input) = a
        .at("new_path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let path = workspace_path(&app.explorer.root, input)?;
        read_text(app, &path, 2 * 1024 * 1024)?.0
    } else {
        a.text("new_text").to_string()
    };
    let context = a.number_or("context", 3).min(20);
    let diff = similar::TextDiff::from_lines(&old, &new);
    let unified = diff
        .unified_diff()
        .context_radius(context)
        .header("old", "new")
        .to_string();
    let added = unified
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let removed = unified
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    Ok(json!({
        "added": added,
        "removed": removed,
        "identical": old == new,
        "diff": cut(&unified, 60_000)
    }))
}

fn buffer_disk_diff(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let editor = app
        .editors
        .get(index)
        .ok_or_else(|| "Buffer closed".to_string())?;
    if editor.doc.is_read_only() {
        return Err("That buffer is an image or archive preview, not text".into());
    }
    let current = editor.doc.rope.to_string();
    let saved = editor.doc.saved_text.clone();
    let diff = similar::TextDiff::from_lines(&saved, &current);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header("saved", "buffer")
        .to_string();
    if saved == current {
        return Ok(json!({
            "path": editor.doc.path.as_ref().map(|p| display(app, p)),
            "dirty": false,
            "diff": "",
            "note": "The live buffer matches its saved content"
        }));
    }
    let name = editor.doc.name();
    let path = editor.doc.path.as_ref().map(|p| display(app, p));
    Ok(json!({
        "path": path,
        "name": name,
        "dirty": true,
        "diff": cut(&unified, 60_000)
    }))
}

fn json_query(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let pointer = a.text("pointer").to_string();
    match serde_json::from_str::<Value>(&text) {
        Err(error) => Ok(json!({
            "path": display(app, &path),
            "valid": false,
            "error": error.to_string()
        })),
        Ok(value) => {
            let found = if pointer.is_empty() {
                Some(&value)
            } else {
                value.pointer(&pointer)
            };
            let kind = match &value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            let keys: Vec<String> = value
                .as_object()
                .map(|object| object.keys().take(100).cloned().collect())
                .unwrap_or_default();
            let (found_json, found_text) = match found {
                Some(value) => (
                    true,
                    cut(&serde_json::to_string_pretty(value).unwrap_or_default(), 8_000),
                ),
                None => (false, String::new()),
            };
            Ok(json!({
                "path": display(app, &path),
                "valid": true,
                "root_type": kind,
                "top_level_keys": keys,
                "pointer": pointer,
                "found": found_json,
                "value": found_text
            }))
        }
    }
}

fn regex_test(a: Args) -> ToolResult {
    let pattern = a.string("pattern")?;
    let text = a.string("text")?;
    if text.len() > 200 * 1024 {
        return Err("text is limited to 200 KiB".into());
    }
    let re = compile(
        &pattern,
        !a.flag("case_insensitive"),
        a.flag("multiline"),
        a.flag("dot_all"),
    )?;
    let limit = a.number_or("limit", 20).clamp(1, 100);
    let mut matches = Vec::new();
    for (index, captures) in re.captures_iter(&text).enumerate() {
        if index >= limit {
            break;
        }
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let groups: Vec<Value> = captures
            .iter()
            .skip(1)
            .enumerate()
            .map(|(group, value)| {
                json!({
                    "index": group + 1,
                    "text": value.map(|m| m.as_str()).unwrap_or("")
                })
            })
            .collect();
        matches.push(json!({
            "text": whole.as_str(),
            "start": whole.start(),
            "end": whole.end(),
            "groups": groups
        }));
    }
    Ok(json!({
        "pattern": pattern,
        "matches": matches,
        "count": matches.len(),
        "truncated": matches.len() >= limit
    }))
}

// ====================================================================== buffers

fn list_buffers(app: &mut App) -> ToolResult {
    let active = app.active;
    let mut buffers = Vec::with_capacity(app.editors.len());
    for (index, editor) in app.editors.iter().enumerate() {
        let (line, column) = editor.cursor();
        buffers.push(json!({
            "index": index,
            "active": index == active,
            "name": editor.doc.name(),
            "path": editor.doc.path.as_ref().map(|p| display(app, p)),
            "dirty": editor.doc.is_dirty(),
            "read_only": editor.doc.is_read_only(),
            "language": editor.language(),
            "lines": editor.doc.len_lines(),
            "cursor": {"line": line + 1, "column": column + 1}
        }));
    }
    Ok(json!({"buffers": buffers, "active": active}))
}

fn buffer_info(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let editor = app
        .editors
        .get(index)
        .ok_or_else(|| "Buffer closed".to_string())?;
    let (line, column) = editor.cursor();
    let selection = editor.sel;
    let (start_line, start_column) = editor.doc.pos(selection.start());
    let (end_line, end_column) = editor.doc.pos(selection.end());
    Ok(json!({
        "index": index,
        "name": editor.doc.name(),
        "path": editor.doc.path.as_ref().map(|p| display(app, p)),
        "dirty": editor.doc.is_dirty(),
        "read_only": editor.doc.is_read_only(),
        "language": editor.language(),
        "lines": editor.doc.len_lines(),
        "characters": editor.doc.rope.len_chars(),
        "saved_bytes": editor.doc.saved_text.len(),
        "line_endings": editor.doc.line_ending.label(),
        "indentation": editor.doc.indent.label(),
        "tab_width": editor.doc.tab_width,
        "cursor": {"line": line + 1, "column": column + 1},
        "selection": {
            "empty": selection.is_empty(),
            "start": {"line": start_line + 1, "column": start_column + 1},
            "end": {"line": end_line + 1, "column": end_column + 1},
            "characters": selection.end().saturating_sub(selection.start())
        }
    }))
}

fn selection_payload(app: &App, index: usize) -> Result<Value, String> {
    let editor = app
        .editors
        .get(index)
        .ok_or_else(|| "Buffer closed".to_string())?;
    let selection = editor.sel;
    let (start_line, start_column) = editor.doc.pos(selection.start());
    let (end_line, end_column) = editor.doc.pos(selection.end());
    let text = editor.selected_text();
    Ok(json!({
        "path": editor.doc.path.as_ref().map(|p| display(app, p)),
        "empty": selection.is_empty(),
        "text": cut(&text, 20_000),
        "characters": selection.end().saturating_sub(selection.start()),
        "start": {"line": start_line + 1, "column": start_column + 1},
        "end": {"line": end_line + 1, "column": end_column + 1}
    }))
}

fn get_selection(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    selection_payload(app, index)
}

fn cursor_info(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let editor = app
        .editors
        .get(index)
        .ok_or_else(|| "Buffer closed".to_string())?;
    let (line, column) = editor.cursor();
    let line_text = editor.doc.line_str(line);
    let indent = line_text.chars().take_while(|c| c.is_whitespace()).count();
    let word = editor
        .word_at(editor.sel.head)
        .map(|(start, end)| editor.doc.rope.slice(start..end).to_string())
        .unwrap_or_default();
    let preferred = editor.preferred_col;
    let in_selection = !editor.sel.is_empty();
    Ok(json!({
        "path": editor.doc.path.as_ref().map(|p| display(app, p)),
        "line": line + 1,
        "column": column + 1,
        "line_characters": line_text.chars().count(),
        "line_text": cut(&line_text, 2_000),
        "indent_characters": indent,
        "word": word,
        "preferred_column": preferred,
        "has_selection": in_selection,
        "line_ending": editor.doc.line_ending.label()
    }))
}

fn buffer_text(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, truncated) = read_text(app, &path, 4 * 1024 * 1024)?;
    let start = a.number("start_line");
    let end = a.number("end_line");
    if start.is_none() && end.is_none() {
        return Ok(json!({
            "path": display(app, &path),
            "lines": text.lines().count(),
            "text": cut(&text, MAX_OUTPUT_BYTES / 2),
            "truncated": truncated || text.len() > MAX_OUTPUT_BYTES / 2
        }));
    }
    let lines: Vec<&str> = text.lines().collect();
    let from = start.unwrap_or(1).max(1);
    let to = end.unwrap_or(lines.len()).max(from);
    let body = match lines.get(from.saturating_sub(1)..to) {
        Some(slice) => slice.join("\n"),
        None => String::new(),
    };
    Ok(json!({
        "path": display(app, &path),
        "total_lines": lines.len(),
        "start_line": from,
        "end_line": to,
        "text": body,
        "truncated": truncated
    }))
}

fn selection_count(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let mut needle = a.text("text").to_string();
    if needle.is_empty() {
        needle = app
            .editors
            .get(index)
            .map(|e| e.selected_text())
            .unwrap_or_default();
        if needle.is_empty() {
            return Err("Select text or pass text to count".into());
        }
    }
    let path = buffer_path(app, &a)?;
    let (haystack, _) = read_text(app, &path, 4 * 1024 * 1024)?;
    let whole_word = a.flag("whole_word");
    let case_sensitive = a
        .at("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let pattern = if whole_word {
        format!(r"\b{}\b", regex::escape(&needle))
    } else {
        regex::escape(&needle)
    };
    let re = compile(&pattern, case_sensitive, false, false)?;
    let mut positions = Vec::new();
    let mut count = 0usize;
    for found in re.find_iter(&haystack) {
        count += 1;
        if positions.len() < 10 {
            let line = haystack[..found.start()].matches('\n').count() + 1;
            let column = haystack[..found.start()]
                .rsplit('\n')
                .next()
                .map(|s| s.chars().count())
                .unwrap_or(0)
                + 1;
            positions.push(json!({"line": line, "column": column}));
        }
    }
    Ok(json!({
        "path": display(app, &path),
        "text": cut(&needle, 200),
        "count": count,
        "positions": positions,
        "whole_word": whole_word,
        "case_sensitive": case_sensitive
    }))
}

fn bracket_balance(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let mut stack: Vec<(char, usize, usize)> = Vec::new();
    let mut mismatch: Option<Value> = None;
    let mut pairs = [0usize; 3];
    let mut line = 1usize;
    let mut column = 0usize;
    let mut chars = text.chars().peekable();
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if character == '\n' {
            line += 1;
            column = 0;
            in_line_comment = false;
            if quote == Some('\'') {
                quote = None;
            }
            continue;
        }
        column += 1;
        if in_line_comment {
            continue;
        }
        if in_block_comment {
            if character == '*' && chars.peek() == Some(&'/') {
                chars.next();
                column += 1;
                in_block_comment = false;
            }
            continue;
        }
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                column += 1;
                in_line_comment = true;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                column += 1;
                in_block_comment = true;
            }
            '"' | '\'' | '`' => quote = Some(character),
            '(' | '[' | '{' => stack.push((character, line, column)),
            ')' | ']' | '}' => {
                let expected = match character {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                match stack.pop() {
                    Some((open, _, _)) if open == expected => {
                        let slot = match expected {
                            '(' => 0,
                            '[' => 1,
                            _ => 2,
                        };
                        pairs[slot] += 1;
                    }
                    Some((open, open_line, open_column)) => {
                        if mismatch.is_none() {
                            mismatch = Some(json!({
                                "expected": format!("closing for '{open}' opened at line {open_line}, column {open_column}"),
                                "found": character.to_string(),
                                "line": line,
                                "column": column
                            }));
                        }
                    }
                    None => {
                        if mismatch.is_none() {
                            mismatch = Some(json!({
                                "expected": "no closing bracket without an opening one",
                                "found": character.to_string(),
                                "line": line,
                                "column": column
                            }));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let unclosed: Vec<Value> = stack
        .iter()
        .rev()
        .take(20)
        .map(|(open, line, column)| json!({"bracket": open.to_string(), "line": line, "column": column}))
        .collect();
    Ok(json!({
        "path": display(app, &path),
        "balanced": stack.is_empty() && mismatch.is_none(),
        "first_mismatch": mismatch,
        "unclosed": unclosed,
        "pairs": {"()": pairs[0], "[]": pairs[1], "{}": pairs[2]},
        "note": "Heuristic scan; quoted strings and // /* */ comments are skipped"
    }))
}

fn indent_report(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let mut tabs = 0usize;
    let mut spaces = [0usize; 5];
    let mut mixed = 0usize;
    let mut blank = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        let leading: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        if leading.is_empty() {
            continue;
        }
        if leading.contains('\t') {
            tabs += 1;
            if leading.contains(' ') {
                mixed += 1;
            }
        } else {
            let width = leading.chars().count();
            let bucket = match width {
                1 => 1,
                2 => 2,
                3 | 4 => 3,
                _ => 4,
            };
            spaces[bucket] += 1;
        }
    }
    let space_total = spaces[1] + spaces[2] + spaces[3] + spaces[4];
    let dominant = if tabs == 0 && space_total == 0 {
        "none"
    } else if tabs >= space_total {
        "tabs"
    } else {
        "spaces"
    };
    Ok(json!({
        "path": display(app, &path),
        "tab_indented": tabs,
        "space_indented": {"1": spaces[1], "2": spaces[2], "3-4": spaces[3], "5+": spaces[4]},
        "mixed": mixed,
        "blank_lines": blank,
        "dominant": dominant,
        "detected": app
            .editors
            .iter()
            .find(|e| e.doc.path.as_deref() == Some(path.as_path()))
            .map(|e| e.doc.indent.label())
    }))
}

fn runs(lines: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for line in lines {
        match out.last_mut() {
            Some(last) if *line == last.1 + 1 => last.1 = *line,
            _ => out.push((*line, *line)),
        }
    }
    out
}

fn buffer_changes(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let editor = app
        .editors
        .get_mut(index)
        .ok_or_else(|| "Buffer closed".to_string())?;
    if editor.doc.is_read_only() {
        return Err("That buffer is an image or archive preview, not text".into());
    }
    let marks = editor.line_changes().to_vec();
    let name = editor.doc.name();
    let path = editor.doc.path.clone();
    let dirty = editor.doc.is_dirty();
    let mut added_lines = Vec::new();
    let mut modified_lines = Vec::new();
    let mut deleted_above = Vec::new();
    for (index, mark) in marks.iter().enumerate() {
        match mark {
            crate::editor::LineChange::Added => added_lines.push(index + 1),
            crate::editor::LineChange::Modified => modified_lines.push(index + 1),
            crate::editor::LineChange::DeletedAbove => deleted_above.push(index + 1),
            crate::editor::LineChange::None => {}
        }
    }
    let ranges = |lines: &[usize]| -> Vec<Value> {
        runs(lines)
            .into_iter()
            .take(100)
            .map(|(start, end)| json!({"start_line": start, "end_line": end}))
            .collect()
    };
    Ok(json!({
        "path": path.as_ref().map(|p| display(app, p)),
        "name": name,
        "dirty": dirty,
        "added": {"lines": added_lines.len(), "ranges": ranges(&added_lines)},
        "modified": {"lines": modified_lines.len(), "ranges": ranges(&modified_lines)},
        "deleted_above": {"lines": deleted_above.len(), "ranges": ranges(&deleted_above)}
    }))
}

fn line_at(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let (cursor_line, cursor_column) = app
        .editors
        .get(index)
        .map(|e| e.cursor())
        .unwrap_or((0, 0));
    let line = a.number("line").unwrap_or(cursor_line + 1).max(1);
    let radius = a.number_or("radius", 3).min(50);
    let path = buffer_path(app, &a)?;
    let (text, truncated) = read_text(app, &path, 2 * 1024 * 1024)?;
    let lines: Vec<&str> = text.lines().collect();
    let first = line.saturating_sub(radius).max(1);
    let last = (line + radius).min(lines.len());
    let body = match lines.get(first.saturating_sub(1)..last) {
        Some(slice) => slice
            .iter()
            .enumerate()
            .map(|(offset, content)| {
                format!("{}\t{content}", first + offset)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => String::new(),
    };
    Ok(json!({
        "path": display(app, &path),
        "line": line,
        "cursor": {"line": cursor_line + 1, "column": cursor_column + 1},
        "first_line": first,
        "last_line": last,
        "total_lines": lines.len(),
        "text": body,
        "truncated": truncated
    }))
}

// ========================================================================= edit

/// Read the target, let `edit` transform it, then write once (buffer or disk).
fn edit_document<F>(app: &mut App, a: &Args, edit: F) -> ToolResult
where
    F: FnOnce(&str) -> Result<(String, Value), String>,
{
    let path = buffer_path(app, a)?;
    let (text, truncated) = read_text(app, &path, 4 * 1024 * 1024)?;
    if truncated {
        return Err("File is too large for an in-place edit".into());
    }
    let (updated, details) = edit(&text)?;
    if updated == text {
        return Ok(json!({
            "path": display(app, &path),
            "changed": false,
            "details": details
        }));
    }
    let result = write_text(app, &path, &updated)?;
    Ok(json!({
        "path": display(app, &path),
        "changed": true,
        "result": result,
        "details": details
    }))
}

/// Replace a 1-based inclusive line span with arbitrary text.
fn replace_span(
    text: &str,
    start_line: usize,
    end_line: usize,
    body: &str,
) -> Result<String, String> {
    let starts = line_starts(text);
    let lines = starts.len();
    if start_line == 0 || start_line > lines {
        return Err(format!("start_line {start_line} is outside 1-{lines}"));
    }
    let end_line = end_line.clamp(start_line, lines);
    let start = starts[start_line - 1];
    let end = starts.get(end_line).copied().unwrap_or(text.len());
    let ends_with_newline = text[..end].ends_with('\n');
    let mut body = body.to_string();
    if !body.is_empty()
        && !body.ends_with('\n')
        && (end < text.len() || ends_with_newline)
    {
        body.push('\n');
    }
    let mut out = String::with_capacity(text.len() + body.len());
    out.push_str(&text[..start]);
    out.push_str(&body);
    out.push_str(&text[end..]);
    Ok(out)
}

fn replace_in_buffer(app: &mut App, a: Args) -> ToolResult {
    let find = a.string("find")?;
    let replacement = a.string("replace")?;
    let all = a.at("all").and_then(Value::as_bool).unwrap_or(true);
    let case_sensitive = a
        .at("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let pattern = if a.flag("regex") {
        find.clone()
    } else {
        regex::escape(&find)
    };
    edit_document(app, &a, |text| {
        let re = compile(&pattern, case_sensitive, false, false)?;
        let matches = re.find_iter(text).count();
        let applied = if all {
            matches
        } else if matches > 0 {
            1
        } else {
            0
        };
        let updated = if all {
            re.replace_all(text, replacement.as_str()).to_string()
        } else {
            re.replace(text, replacement.as_str()).to_string()
        };
        Ok((
            updated,
            json!({"matches": matches, "replacements": applied, "regex": a.flag("regex")}),
        ))
    })
}

fn replace_line_range(app: &mut App, a: Args) -> ToolResult {
    let start = a
        .number("start_line")
        .ok_or_else(|| "start_line is required".to_string())?;
    let end = a
        .number("end_line")
        .ok_or_else(|| "end_line is required".to_string())?;
    let body = a.string("text")?;
    if start == 0 || end < start {
        return Err("Use 1-based lines with end_line >= start_line".into());
    }
    edit_document(app, &a, |text| {
        let (full, crlf) = normalize(text);
        let body = normalize(&body).0;
        let updated = replace_span(&full, start, end, &body)?;
        Ok((
            restore(&updated, crlf),
            json!({"start_line": start, "end_line": end, "bytes": body.len()}),
        ))
    })
}

fn insert_lines(app: &mut App, a: Args) -> ToolResult {
    let line = a
        .number("line")
        .ok_or_else(|| "line is required".to_string())?;
    let body = a.string("text")?;
    let after = a.text("position").eq_ignore_ascii_case("after");
    edit_document(app, &a, |text| {
        let (full, crlf) = normalize(text);
        let starts = line_starts(&full);
        let lines = starts.len();
        let at = if line == 0 {
            0
        } else if after {
            starts.get(line).copied().unwrap_or(full.len())
        } else if line <= lines {
            starts[line - 1]
        } else {
            full.len()
        };
        let mut body = normalize(&body).0;
        if at == full.len() && at > 0 && !full.ends_with('\n') && !body.is_empty() {
            body.insert(0, '\n');
        }
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        let mut out = String::with_capacity(full.len() + body.len());
        out.push_str(&full[..at]);
        out.push_str(&body);
        out.push_str(&full[at..]);
        Ok((
            restore(&out, crlf),
            json!({"line": line, "position": if after { "after" } else { "before" }}),
        ))
    })
}

fn delete_lines(app: &mut App, a: Args) -> ToolResult {
    let start = a
        .number("start_line")
        .ok_or_else(|| "start_line is required".to_string())?;
    let end = a
        .number("end_line")
        .ok_or_else(|| "end_line is required".to_string())?;
    if start == 0 || end < start {
        return Err("Use 1-based lines with end_line >= start_line".into());
    }
    edit_document(app, &a, |text| {
        let (full, crlf) = normalize(text);
        let updated = replace_span(&full, start, end, "")?;
        Ok((
            restore(&updated, crlf),
            json!({"start_line": start, "end_line": end}),
        ))
    })
}

fn append_text(app: &mut App, a: Args) -> ToolResult {
    let addition = a.string("text")?;
    edit_document(app, &a, |text| {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') && !addition.is_empty() {
            out.push('\n');
        }
        out.push_str(&addition);
        Ok((out, json!({"appended_bytes": addition.len()})))
    })
}

fn prepend_text(app: &mut App, a: Args) -> ToolResult {
    let addition = a.string("text")?;
    edit_document(app, &a, |text| {
        let mut out = addition.clone();
        if !out.is_empty() && !text.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(text);
        Ok((out, json!({"prepended_bytes": addition.len()})))
    })
}

fn trim_trailing_whitespace(app: &mut App, a: Args) -> ToolResult {
    edit_document(app, &a, |text| {
        let (full, crlf) = normalize(text);
        let mut changed = 0usize;
        let lines: Vec<String> = full
            .split('\n')
            .map(|line| {
                let trimmed = line.trim_end_matches(|c: char| c == ' ' || c == '\t');
                if trimmed.len() != line.len() {
                    changed += 1;
                }
                trimmed.to_string()
            })
            .collect();
        let out = restore(&lines.join("\n"), crlf);
        Ok((out, json!({"lines_changed": changed})))
    })
}

fn export_range(
    text: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> (String, bool, usize, usize) {
    let (full, crlf) = normalize(text);
    let lines = line_starts(&full).len();
    let start = start_line.unwrap_or(1).max(1).min(lines);
    let end = end_line.unwrap_or(lines).clamp(start, lines);
    (full, crlf, start, end)
}

fn sort_lines(app: &mut App, a: Args) -> ToolResult {
    let descending = a.flag("descending");
    let case_insensitive = a.flag("case_insensitive");
    let unique = a.flag("unique");
    edit_document(app, &a, |text| {
        let (full, crlf, start, end) = export_range(text, a.number("start_line"), a.number("end_line"));
        let mut items = segment_items(&full, start, end)?;
        if case_insensitive {
            items.sort_by_key(|item| item.to_lowercase());
        } else {
            items.sort();
        }
        if descending {
            items.reverse();
        }
        let mut removed = 0usize;
        if unique {
            let before = items.len();
            let mut seen: HashSet<String> = HashSet::new();
            items.retain(|item| seen.insert(item.clone()));
            removed = before.saturating_sub(items.len());
        }
        let out = replace_segment(&full, start, end, items)?;
        Ok((
            restore(&out, crlf),
            json!({
                "start_line": start,
                "end_line": end,
                "lines": end - start + 1,
                "duplicates_removed": removed
            }),
        ))
    })
}

fn dedupe_lines(app: &mut App, a: Args) -> ToolResult {
    edit_document(app, &a, |text| {
        let (full, crlf, start, end) = export_range(text, a.number("start_line"), a.number("end_line"));
        let items = segment_items(&full, start, end)?;
        let before = items.len();
        let mut seen: HashSet<String> = HashSet::new();
        let mut kept: Vec<String> = Vec::new();
        for item in items {
            if seen.insert(item.clone()) {
                kept.push(item);
            }
        }
        let removed = before.saturating_sub(kept.len());
        let out = replace_segment(&full, start, end, kept)?;
        Ok((
            restore(&out, crlf),
            json!({
                "start_line": start,
                "end_line": end,
                "lines_removed": removed
            }),
        ))
    })
}

fn case_words(line: &str) -> Vec<String> {
    line.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(String::from)
        .collect()
}

fn apply_case_line(line: &str, mode: &str) -> Result<String, String> {
    let out = match mode {
        "lower" | "lowercase" => line.to_lowercase(),
        "upper" | "uppercase" => line.to_uppercase(),
        "title" | "titlecase" => {
            let mut out = String::with_capacity(line.len());
            let mut start_of_word = true;
            for character in line.chars() {
                if character.is_alphanumeric() {
                    if start_of_word {
                        out.extend(character.to_uppercase());
                    } else {
                        out.extend(character.to_lowercase());
                    }
                    start_of_word = false;
                } else {
                    out.push(character);
                    start_of_word = true;
                }
            }
            out
        }
        "snake" | "snake_case" => case_words(line)
            .iter()
            .map(|word| word.to_lowercase())
            .collect::<Vec<_>>()
            .join("_"),
        "kebab" | "kebab-case" => case_words(line)
            .iter()
            .map(|word| word.to_lowercase())
            .collect::<Vec<_>>()
            .join("-"),
        "camel" | "camelcase" => {
            let mut out = String::new();
            for (index, word) in case_words(line).iter().enumerate() {
                if index == 0 {
                    out.push_str(&word.to_lowercase());
                } else {
                    out.push_str(&capitalize(word));
                }
            }
            out
        }
        "pascal" | "pascalcase" => case_words(line)
            .iter()
            .map(|word| capitalize(word))
            .collect::<Vec<_>>()
            .join(""),
        other => {
            return Err(format!(
                "Unknown case {other}; use lower, upper, title, snake, kebab, camel or pascal"
            ));
        }
    };
    Ok(out)
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out: String = first.to_uppercase().collect();
            out.push_str(&chars.as_str().to_lowercase());
            out
        }
    }
}

fn transform_case(app: &mut App, a: Args) -> ToolResult {
    let mode = a.string("case")?.to_lowercase();
    edit_document(app, &a, |text| {
        let (full, crlf, start, end) = export_range(text, a.number("start_line"), a.number("end_line"));
        let items = segment_items(&full, start, end)?;
        let mut converted = Vec::with_capacity(items.len());
        for item in items {
            converted.push(apply_case_line(&item, &mode)?);
        }
        let out = replace_segment(&full, start, end, converted)?;
        Ok((
            restore(&out, crlf),
            json!({
                "case": mode,
                "start_line": start,
                "end_line": end,
                "note": "snake/kebab/camel/pascal treat each line as words and drop punctuation"
            }),
        ))
    })
}

// =================================================================== filesystem

fn create_file(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    if path.is_dir() {
        return Err(format!("{} is a directory", display(app, &path)));
    }
    let existed = path.exists();
    if existed && !a.flag("overwrite") {
        return Err(format!(
            "{} already exists; pass overwrite=true to replace it",
            display(app, &path)
        ));
    }
    let content = a.text("content").to_string();
    let result = write_text(app, &path, &content)?;
    Ok(json!({
        "path": display(app, &path),
        "bytes": content.len(),
        "overwrote": existed,
        "result": result
    }))
}

fn create_directory(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    let existed = path.is_dir();
    std::fs::create_dir_all(&path)
        .map_err(|e| format!("cannot create {}: {e}", display(app, &path)))?;
    app.invalidate_project_files();
    app.explorer.refresh();
    Ok(json!({
        "path": display(app, &path),
        "created": !existed,
        "existed": existed
    }))
}

fn copy_path(app: &mut App, a: Args) -> ToolResult {
    let from = resolve(app, &a, "from")?;
    let to = resolve(app, &a, "to")?;
    if !from.is_file() {
        return Err("copy_path supports regular files only".into());
    }
    if to.is_dir() {
        return Err(format!("{} is a directory", display(app, &to)));
    }
    if to.exists() && !a.flag("overwrite") {
        return Err(format!(
            "{} already exists; pass overwrite=true to replace it",
            display(app, &to)
        ));
    }
    let meta = std::fs::metadata(&from).map_err(|e| e.to_string())?;
    if meta.len() > 32 * 1024 * 1024 {
        return Err("copy_path is limited to 32 MiB files".into());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(&from, &to)
        .map_err(|e| format!("cannot copy to {}: {e}", display(app, &to)))?;
    let open = app.editors.iter().any(|e| e.doc.path.as_deref() == Some(to.as_path()) && e.doc.is_dirty());
    app.invalidate_project_files();
    Ok(json!({
        "from": display(app, &from),
        "to": display(app, &to),
        "bytes": meta.len(),
        "note": if open { "The destination has an open dirty buffer; reload it to see the copied content" } else { "" }
    }))
}

fn move_path(app: &mut App, a: Args) -> ToolResult {
    let from = resolve(app, &a, "from")?;
    let to = resolve(app, &a, "to")?;
    if from == to {
        return Err("from and to are the same path".into());
    }
    if !from.is_file() {
        return Err("move_path supports regular files only".into());
    }
    if to.is_dir() {
        return Err(format!("{} is a directory", display(app, &to)));
    }
    if to.exists() && !a.flag("overwrite") {
        return Err(format!(
            "{} already exists; pass overwrite=true to replace it",
            display(app, &to)
        ));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let renamed = std::fs::rename(&from, &to);
    let moved = match renamed {
        Ok(()) => true,
        Err(_) => {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("cannot move to {}: {e}", display(app, &to)))?;
            std::fs::remove_file(&from)
                .map_err(|e| format!("copied but cannot remove {}: {e}", display(app, &from)))?;
            false
        }
    };
    let mut updated = 0usize;
    for editor in &mut app.editors {
        if editor.doc.path.as_deref() == Some(from.as_path()) {
            editor.doc.path = Some(to.clone());
            updated += 1;
        }
    }
    app.invalidate_project_files();
    app.explorer.refresh();
    Ok(json!({
        "from": display(app, &from),
        "to": display(app, &to),
        "renamed": moved,
        "open_buffers_updated": updated
    }))
}

fn delete_path(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    if a.text("confirm") != "DELETE" {
        return Err("confirm must be exactly DELETE to remove a path".into());
    }
    if path == app.explorer.root {
        return Err("Refusing to delete the workspace root".into());
    }
    if path
        .components()
        .any(|component| component.as_os_str().to_string_lossy().as_ref() == ".git")
    {
        return Err("Refusing to delete inside .git".into());
    }
    if app
        .editors
        .iter()
        .any(|e| e.doc.path.as_deref() == Some(path.as_path()))
    {
        return Err("Close the editor buffer before deleting its file".into());
    }
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|e| format!("cannot stat {}: {e}", display(app, &path)))?;
    if meta.file_type().is_symlink() {
        return Err("Refusing to delete symlinks".into());
    }
    let was_dir = meta.is_dir();
    if was_dir {
        if !a.flag("recursive") {
            return Err("path is a directory; pass recursive=true to delete it".into());
        }
        std::fs::remove_dir_all(&path)
            .map_err(|e| format!("cannot delete {}: {e}", display(app, &path)))?;
    } else {
        std::fs::remove_file(&path)
            .map_err(|e| format!("cannot delete {}: {e}", display(app, &path)))?;
    }
    app.invalidate_project_files();
    app.explorer.refresh();
    Ok(json!({
        "path": display(app, &path),
        "deleted": true,
        "was_directory": was_dir
    }))
}

fn write_lines(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    let lines = a.string_list("lines")?;
    let newline = a.text("newline").to_lowercase();
    let separator = if newline == "crlf" { "\r\n" } else { "\n" };
    let final_newline = a
        .at("final_newline")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if path.exists() && !a.flag("overwrite") {
        return Err(format!(
            "{} already exists; pass overwrite=true to replace it",
            display(app, &path)
        ));
    }
    let mut text = lines.join(separator);
    if final_newline && !lines.is_empty() {
        text.push_str(separator);
    }
    let result = write_text(app, &path, &text)?;
    Ok(json!({
        "path": display(app, &path),
        "lines": lines.len(),
        "bytes": text.len(),
        "newline": if separator == "\r\n" { "crlf" } else { "lf" },
        "result": result
    }))
}

fn patch_file(app: &mut App, a: Args) -> ToolResult {
    let edits = a.list("edits");
    if edits.is_empty() {
        return Err("edits must contain at least one {old_text, new_text} object".into());
    }
    if edits.len() > 32 {
        return Err("patch_file accepts at most 32 edits".into());
    }
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let old = edit
            .get("old_text")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("edit {} needs old_text", index + 1))?;
        let new = edit
            .get("new_text")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("edit {} needs new_text", index + 1))?;
        if old.is_empty() {
            return Err(format!("edit {} has an empty old_text", index + 1));
        }
        pairs.push((old.to_string(), new.to_string()));
    }
    let dry_run = a.at("dry_run").and_then(Value::as_bool).unwrap_or(true);
    if !dry_run && a.text("confirm") != "APPLY" {
        return Err("confirm must be exactly APPLY when dry_run is false".into());
    }
    if dry_run {
        let path = buffer_path(app, &a)?;
        let (before, _) = read_text(app, &path, 4 * 1024 * 1024)?;
        let mut after = before.clone();
        let mut preview: Vec<Value> = Vec::with_capacity(pairs.len());
        for (index, (old, new)) in pairs.iter().enumerate() {
            let matches = after.matches(old.as_str()).count();
            if matches != 1 {
                return Err(format!(
                    "edit {} matched {matches} times; each old_text must match exactly once, so nothing would be written",
                    index + 1
                ));
            }
            after = after.replacen(old.as_str(), new, 1);
            preview.push(json!({
                "edit": index + 1,
                "old": cut(old, 200),
                "new": cut(new, 200)
            }));
        }
        let diff = similar::TextDiff::from_lines(&before, &after)
            .unified_diff()
            .context_radius(2)
            .header("before", "after")
            .to_string();
        return Ok(json!({
            "dry_run": true,
            "path": display(app, &path),
            "edits": preview,
            "diff": cut(&diff, 20_000),
            "note": "Nothing was written; repeat with dry_run=false and confirm=APPLY to apply"
        }));
    }
    edit_document(app, &a, |text| {
        let mut current = text.to_string();
        for (index, (old, new)) in pairs.iter().enumerate() {
            let matches = current.matches(old.as_str()).count();
            if matches != 1 {
                return Err(format!(
                    "edit {} matched {matches} times; each old_text must match exactly once, so nothing was written",
                    index + 1
                ));
            }
            current = current.replacen(old.as_str(), new, 1);
        }
        Ok((current, json!({"edits_applied": pairs.len()})))
    })
}

fn save_buffer(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let result = {
        let editor = app
            .editors
            .get_mut(index)
            .ok_or_else(|| "Buffer closed".to_string())?;
        if editor.doc.is_read_only() {
            return Err("That buffer is an image or archive preview and cannot be saved".into());
        }
        if editor.doc.path.is_none() {
            return Err("This buffer has no path; use Save As in the editor first".into());
        }
        editor
            .doc
            .save()
            .map_err(|error| format!("save failed: {error:#}"))
    };
    result?;
    let path = app
        .editors
        .get(index)
        .and_then(|e| e.doc.path.clone());
    app.invalidate_project_files();
    Ok(json!({
        "saved": path.as_ref().map(|p| display(app, p)),
        "buffers": app.editors.len()
    }))
}

fn save_all_buffers(app: &mut App) -> ToolResult {
    let mut saved = Vec::new();
    let mut errors = Vec::new();
    let root = app.explorer.root.clone();
    for index in 0..app.editors.len() {
        let Some(editor) = app.editors.get_mut(index) else {
            continue;
        };
        if !editor.doc.is_dirty() || editor.doc.is_read_only() || editor.doc.path.is_none() {
            continue;
        }
        let name = editor.doc.name();
        let path = editor.doc.path.clone();
        match editor.doc.save() {
            Ok(()) => saved.push(json!({
                "name": name,
                "path": path.as_ref().map(|p| crate::search::relative(&root, p))
            })),
            Err(error) => errors.push(format!("{name}: {error:#}")),
        }
    }
    app.invalidate_project_files();
    Ok(json!({"saved": saved, "errors": errors}))
}

fn revert_buffer(app: &mut App, a: Args) -> ToolResult {
    if a.text("confirm") != "DISCARD" {
        return Err("confirm must be exactly DISCARD to reload an edited buffer".into());
    }
    let index = buffer_index(app, &a)?;
    let path = buffer_path(app, &a)?;
    let (disk, _) = read_file_text(&path, 4 * 1024 * 1024)?;
    let editor = app
        .editors
        .get_mut(index)
        .ok_or_else(|| "Buffer closed".to_string())?;
    if editor.doc.is_read_only() {
        return Err("That buffer is an image or archive preview".into());
    }
    editor.replace_document(&disk);
    editor.doc.saved_text = disk.clone();
    editor.doc.mark_saved();
    Ok(json!({
        "path": display(app, &path),
        "bytes": disk.len(),
        "reloaded": true
    }))
}

// ======================================================================= search

fn search_workspace(app: &mut App, a: Args) -> ToolResult {
    let pattern = a.string("pattern")?;
    let case_sensitive = a.flag("case_sensitive");
    let re = compile(&pattern, case_sensitive, true, false)?;
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let include = a.text("include").to_string();
    let exclude = a.text("exclude").to_string();
    let limit = a.number_or("limit", 100).clamp(1, 500);
    let mut hits: Vec<Value> = Vec::new();
    let scanned = scan_files(
        app,
        &base,
        &include,
        &exclude,
        MAX_SCAN_FILES,
        |relative, text| {
            for (index, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(json!({
                        "file": relative,
                        "line": index + 1,
                        "text": cut(line, 300)
                    }));
                    if hits.len() >= limit {
                        return false;
                    }
                }
            }
            true
        },
    );
    Ok(json!({
        "pattern": pattern,
        "scanned": scanned,
        "matches": hits,
        "truncated": hits.len() >= limit,
        "note": "Searches workspace files and unsaved live buffers; skipped files exceed 512 KiB"
    }))
}

fn match_count(app: &mut App, a: Args) -> ToolResult {
    let pattern = a.string("pattern")?;
    let re = compile(&pattern, a.flag("case_sensitive"), false, false)?;
    let target = a.text("path").to_string();
    let limit = a.number_or("limit", 40).clamp(1, 200);
    let base = if target.is_empty() {
        app.explorer.root.clone()
    } else {
        workspace_path(&app.explorer.root, &target)?
    };
    if base.is_file() {
        let (text, _) = read_text(app, &base, 4 * 1024 * 1024)?;
        let matches = re.find_iter(&text).count();
        return Ok(json!({
            "path": display(app, &base),
            "matches": matches,
            "file": true
        }));
    }
    let mut per_file: Vec<(usize, String)> = Vec::new();
    let mut total = 0usize;
    let scanned = scan_files(app, &base, "", "", MAX_SCAN_FILES, |relative, text| {
        let matches = re.find_iter(text).count();
        if matches > 0 {
            total += matches;
            per_file.push((matches, relative.to_string()));
        }
        per_file.len() < limit
    });
    per_file.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    per_file.truncate(limit);
    let files: Vec<Value> = per_file
        .into_iter()
        .map(|(matches, file)| json!({"file": file, "matches": matches}))
        .collect();
    Ok(json!({
        "pattern": pattern,
        "scanned": scanned,
        "total_matches": total,
        "files": files
    }))
}

fn find_references(app: &mut App, a: Args) -> ToolResult {
    let name = a.string("name")?;
    if name.is_empty()
        || name.len() > 200
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    {
        return Err("name must be a single identifier (letters, digits, _ or $)".into());
    }
    let re = compile(&format!(r"\b{}\b", regex::escape(&name)), true, false, false)?;
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 60).clamp(1, 300);
    let mut per_file: Vec<(usize, String)> = Vec::new();
    let mut locations: Vec<Value> = Vec::new();
    let mut total = 0usize;
    let scanned = scan_files(app, &base, "", "", MAX_SCAN_FILES, |relative, text| {
        let mut count = 0usize;
        for found in re.find_iter(text) {
            count += 1;
            total += 1;
            if locations.len() < limit {
                let line = text[..found.start()].matches('\n').count() + 1;
                locations.push(json!({"file": relative, "line": line}));
            }
        }
        if count > 0 {
            per_file.push((count, relative.to_string()));
        }
        true
    });
    per_file.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    per_file.truncate(limit);
    let files: Vec<Value> = per_file
        .into_iter()
        .map(|(count, file)| json!({"file": file, "references": count}))
        .collect();
    Ok(json!({
        "name": name,
        "scanned": scanned,
        "total_references": total,
        "files": files,
        "locations": locations
    }))
}

fn grep_patterns(app: &mut App, a: Args) -> ToolResult {
    let patterns = a.string_list("patterns")?;
    if patterns.is_empty() || patterns.len() > 16 {
        return Err("patterns must contain 1-16 entries".into());
    }
    let case_sensitive = a.flag("case_sensitive");
    let as_regex = a.flag("regex");
    let mut compiled: Vec<(String, regex::Regex)> = Vec::with_capacity(patterns.len());
    for pattern in &patterns {
        let source = if as_regex {
            pattern.clone()
        } else {
            regex::escape(pattern)
        };
        compiled.push((pattern.clone(), compile(&source, case_sensitive, false, false)?));
    }
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 100).clamp(1, 300);
    let mut hits: Vec<Value> = Vec::new();
    let scanned = scan_files(app, &base, "", "", MAX_SCAN_FILES, |relative, text| {
        for (index, line) in text.lines().enumerate() {
            let mut matched: Vec<&str> = Vec::new();
            for (name, re) in &compiled {
                if re.is_match(line) {
                    matched.push(name.as_str());
                }
            }
            if !matched.is_empty() {
                hits.push(json!({
                    "file": relative,
                    "line": index + 1,
                    "patterns": matched,
                    "text": cut(line, 300)
                }));
                if hits.len() >= limit {
                    return false;
                }
            }
        }
        true
    });
    Ok(json!({
        "patterns": patterns,
        "scanned": scanned,
        "matches": hits,
        "truncated": hits.len() >= limit
    }))
}

fn list_large_files(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let threshold = a.number_or("threshold_kb", 512).max(1) as u64 * 1024;
    let limit = a.number_or("limit", 30).clamp(1, 100);
    let mut files: Vec<(u64, String)> = Vec::new();
    for relative in crate::explorer::project_files(&base, 20_000) {
        let path = base.join(&relative);
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
            && meta.len() >= threshold
        {
            files.push((meta.len(), display(app, &path)));
        }
    }
    files.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    let truncated = files.len() > limit;
    files.truncate(limit);
    let items: Vec<Value> = files
        .into_iter()
        .map(|(bytes, path)| json!({"path": path, "bytes": bytes}))
        .collect();
    Ok(json!({"threshold_bytes": threshold, "files": items, "truncated": truncated}))
}

fn list_empty_files(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 100).clamp(1, 200);
    let mut items: Vec<Value> = Vec::new();
    for relative in crate::explorer::project_files(&base, 20_000) {
        let path = base.join(&relative);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > 64 * 1024 {
            continue;
        }
        if meta.len() == 0 {
            items.push(json!({"path": display(app, &path), "kind": "empty"}));
        } else if let Ok(text) = std::fs::read_to_string(&path)
            && text.trim().is_empty()
        {
            items.push(json!({"path": display(app, &path), "kind": "whitespace"}));
        }
        if items.len() >= limit {
            break;
        }
    }
    Ok(json!({"files": items, "truncated": items.len() >= limit}))
}

fn duplicate_basenames(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 30).clamp(1, 100);
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for relative in crate::explorer::project_files(&base, 20_000) {
        let name = relative.rsplit('/').next().unwrap_or(relative.as_str());
        groups
            .entry(name.to_string())
            .or_default()
            .push(relative.clone());
    }
    let mut duplicates: Vec<(usize, String, Vec<String>)> = groups
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(name, paths)| (paths.len(), name, paths))
        .collect();
    duplicates.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    let truncated = duplicates.len() > limit;
    duplicates.truncate(limit);
    let items: Vec<Value> = duplicates
        .into_iter()
        .map(|(count, name, paths)| json!({"name": name, "count": count, "paths": paths}))
        .collect();
    Ok(json!({"duplicates": items, "truncated": truncated}))
}

fn replace_in_files(app: &mut App, a: Args) -> ToolResult {
    let pattern = a.string("pattern")?;
    let replacement = a.string("replacement")?;
    let dry_run = a.at("dry_run").and_then(Value::as_bool).unwrap_or(true);
    if !dry_run && a.text("confirm") != "APPLY" {
        return Err("confirm must be exactly APPLY when dry_run is false".into());
    }
    let source = if a.flag("regex") {
        pattern.clone()
    } else {
        regex::escape(&pattern)
    };
    let re = compile(&source, a.flag("case_sensitive"), false, false)?;
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let include = a.text("include").to_string();
    let exclude = a.text("exclude").to_string();
    let max_files = a.number_or("max_files", 200).clamp(1, 500);
    let mut candidates: Vec<String> = Vec::new();
    scan_files(app, &base, &include, &exclude, MAX_SCAN_FILES, |relative, _| {
        candidates.push(relative.to_string());
        candidates.len() < max_files
    });
    let mut changed: Vec<Value> = Vec::new();
    let mut total = 0usize;
    for relative in candidates {
        let path = if Path::new(&relative).is_absolute() {
            PathBuf::from(relative.as_str())
        } else {
            app.explorer.root.join(&relative)
        };
        let Ok((text, _)) = read_text(app, &path, 4 * 1024 * 1024) else {
            continue;
        };
        let count = re.find_iter(&text).count();
        if count == 0 {
            continue;
        }
        total += count;
        if dry_run {
            changed.push(json!({"file": display(app, &path), "replacements": count}));
            continue;
        }
        let updated = re.replace_all(&text, replacement.as_str()).to_string();
        match write_text(app, &path, &updated) {
            Ok(_) => changed.push(json!({
                "file": display(app, &path),
                "replacements": count
            })),
            Err(error) => changed.push(json!({
                "file": display(app, &path),
                "error": error
            })),
        }
    }
    Ok(json!({
        "dry_run": dry_run,
        "files_changed": changed.len(),
        "replacements": total,
        "files": changed,
        "note": if dry_run { "Nothing was written; pass dry_run=false and confirm=APPLY to apply" } else { "Open buffers were edited in place and stay unsaved" }
    }))
}

fn files_containing_all(app: &mut App, a: Args) -> ToolResult {
    let terms = a.string_list("terms")?;
    if terms.is_empty() || terms.len() > 8 {
        return Err("terms must contain 1-8 entries".into());
    }
    let lower: Vec<String> = terms.iter().map(|term| term.to_lowercase()).collect();
    let any = a.text("mode").eq_ignore_ascii_case("any");
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 50).clamp(1, 200);
    let mut files: Vec<Value> = Vec::new();
    let scanned = scan_files(app, &base, "", "", MAX_SCAN_FILES, |relative, text| {
        let haystack = text.to_lowercase();
        let matched = if any {
            lower.iter().any(|term| haystack.contains(term.as_str()))
        } else {
            lower.iter().all(|term| haystack.contains(term.as_str()))
        };
        if matched {
            files.push(json!({"file": relative}));
        }
        files.len() < limit
    });
    Ok(json!({
        "terms": terms,
        "mode": if any { "any" } else { "all" },
        "scanned": scanned,
        "files": files
    }))
}

fn definition_regex() -> Result<regex::Regex, String> {
    compile(
        r"^\s*(?:pub(?:\([^)]*\))?\s+|export\s+|default\s+|async\s+|static\s+|public\s+|private\s+|protected\s+|internal\s+|final\s+|override\s+|abstract\s+)*(fn|def|func|function|class|struct|enum|trait|interface|type|const|let|var|impl|mod|package)\s+([A-Za-z_$][A-Za-z0-9_$]*)",
        true,
        true,
        false,
    )
}

fn symbol_search(app: &mut App, a: Args) -> ToolResult {
    let needle = a.text("name").to_lowercase();
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 100).clamp(1, 300);
    let re = definition_regex()?;
    let mut symbols: Vec<Value> = Vec::new();
    let scanned = scan_files(app, &base, "", "", MAX_SCAN_FILES, |relative, text| {
        for captures in re.captures_iter(text) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            let kind = captures.get(1).map(|m| m.as_str()).unwrap_or("");
            let name = captures.get(2).map(|m| m.as_str()).unwrap_or("");
            if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
                continue;
            }
            let line = text[..whole.start()].matches('\n').count() + 1;
            symbols.push(json!({
                "file": relative,
                "line": line,
                "kind": kind,
                "name": name,
                "signature": cut(whole.as_str().trim(), 200)
            }));
            if symbols.len() >= limit {
                return false;
            }
        }
        true
    });
    Ok(json!({
        "name": a.text("name"),
        "scanned": scanned,
        "symbols": symbols,
        "truncated": symbols.len() >= limit
    }))
}

// ========================================================================== git

fn git_status(app: &mut App) -> ToolResult {
    let args: Vec<String> = vec!["status".into(), "--short".into(), "--branch".into()];
    let output = git_run(app, &args)?;
    Ok(json!({
        "branch": app.git_branch.clone(),
        "output": output,
        "clean": output.lines().all(|line| line.starts_with("##"))
    }))
}

fn git_diff(app: &mut App, a: Args) -> ToolResult {
    let mut args: Vec<String> = vec!["diff".into()];
    if a.flag("staged") {
        args.push("--cached".into());
    }
    if a.flag("stat") {
        args.push("--stat".into());
    }
    let path_arg = if a.text("path").is_empty() {
        None
    } else {
        let path = resolve(app, &a, "path")?;
        Some(display(app, &path))
    };
    if let Some(path) = &path_arg {
        args.push("--".into());
        args.push(path.clone());
    }
    let output = git_run(app, &args)?;
    Ok(json!({
        "staged": a.flag("staged"),
        "path": path_arg,
        "output": if output.is_empty() { "(no differences)".to_string() } else { output }
    }))
}

fn git_log(app: &mut App, a: Args) -> ToolResult {
    let limit = a.number_or("limit", 20).clamp(1, 100);
    let mut args: Vec<String> = vec![
        "log".into(),
        format!("-n{limit}"),
        "--pretty=format:%h %ad %an %s".into(),
        "--date=short".into(),
    ];
    let path_arg = if a.text("path").is_empty() {
        None
    } else {
        let path = resolve(app, &a, "path")?;
        Some(display(app, &path))
    };
    if let Some(path) = &path_arg {
        args.push("--".into());
        args.push(path.clone());
    }
    let output = git_run(app, &args)?;
    Ok(json!({"limit": limit, "path": path_arg, "commits": output}))
}

fn git_show(app: &mut App, a: Args) -> ToolResult {
    let rev = safe_rev(a.text("rev"))?;
    let spec = if a.text("path").is_empty() {
        rev.clone()
    } else {
        let path = resolve(app, &a, "path")?;
        format!("{rev}:{}", display(app, &path))
    };
    let output = git_run(app, &["show".into(), spec.clone()])?;
    Ok(json!({"rev": rev, "spec": spec, "output": output}))
}

fn git_branches(app: &mut App, a: Args) -> ToolResult {
    let mut args: Vec<String> = vec!["branch".into(), "--list".into(), "-vv".into()];
    if a.at("all").and_then(Value::as_bool).unwrap_or(true) {
        args.push("-a".into());
    }
    let output = git_run(app, &args)?;
    Ok(json!({"output": output, "current": app.git_branch.clone()}))
}

fn git_blame(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    let start = a.number("start_line").unwrap_or(1).max(1);
    let end = a.number("end_line").unwrap_or(start + 39).max(start);
    let output = git_run(
        app,
        &[
            "blame".into(),
            "-L".into(),
            format!("{start},{end}"),
            "--".into(),
            display(app, &path),
        ],
    )?;
    Ok(json!({
        "path": display(app, &path),
        "start_line": start,
        "end_line": end,
        "output": output
    }))
}

fn git_file_history(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    let limit = a.number_or("limit", 20).clamp(1, 100);
    let output = git_run(
        app,
        &[
            "log".into(),
            "--follow".into(),
            format!("-n{limit}"),
            "--pretty=format:%h %ad %an %s".into(),
            "--date=short".into(),
            "--".into(),
            display(app, &path),
        ],
    )?;
    Ok(json!({
        "path": display(app, &path),
        "limit": limit,
        "commits": output
    }))
}

fn git_remotes(app: &mut App) -> ToolResult {
    let args: Vec<String> = vec!["remote".into(), "-v".into()];
    let output = git_run(app, &args)?;
    Ok(json!({
        "output": if output.is_empty() { "(no remotes)".to_string() } else { output }
    }))
}

fn git_stash_list(app: &mut App) -> ToolResult {
    let args: Vec<String> = vec!["stash".into(), "list".into()];
    let output = git_run(app, &args)?;
    Ok(json!({
        "output": if output.is_empty() { "(no stash entries)".to_string() } else { output }
    }))
}

fn git_tags(app: &mut App) -> ToolResult {
    let args: Vec<String> = vec![
        "tag".into(),
        "--list".into(),
        "--sort=-creatordate".into(),
    ];
    let output = git_run(app, &args)?;
    Ok(json!({
        "tags": if output.is_empty() { "(no tags)".to_string() } else { output }
    }))
}

// =========================================================================== ui

fn notify_user(app: &mut App, a: Args) -> ToolResult {
    let message = a.string("message")?;
    let message = message.trim();
    if message.is_empty() || message.len() > 500 {
        return Err("message must contain 1-500 bytes".into());
    }
    let level = a.text("level").to_lowercase();
    let kind = match level.as_str() {
        "success" => ToastKind::Success,
        "error" => ToastKind::Error,
        _ => ToastKind::Info,
    };
    app.toast(message.to_string(), kind);
    let level = if level.is_empty() { "info".to_string() } else { level };
    Ok(json!({"shown": true, "message": message, "level": level}))
}

fn open_file(app: &mut App, a: Args) -> ToolResult {
    let path = resolve(app, &a, "path")?;
    let is_member = crate::archive::member_path(&path).is_some();
    if !path.exists() && !is_member {
        return Err(format!("{} does not exist", display(app, &path)));
    }
    if path.is_dir() {
        return Err(format!("{} is a directory", display(app, &path)));
    }
    let before = app.editors.len();
    app.open_file(&path);
    let Some(index) = app
        .editors
        .iter()
        .position(|e| e.doc.path.as_deref() == Some(path.as_path()))
    else {
        return Err(format!(
            "The editor could not open {}; check its notification for the reason",
            display(app, &path)
        ));
    };
    app.active = index;
    if let Some(line) = a.number("line") {
        let column = a.number("column").unwrap_or(1);
        if let Some(editor) = app.editors.get_mut(index) {
            editor.move_to(line.saturating_sub(1), column.saturating_sub(1), false);
            editor.scroll_into_view();
        }
    }
    Ok(json!({
        "opened": display(app, &path),
        "active": index,
        "new_tab": app.editors.len() > before,
        "buffers": app.editors.len(),
        "read_only": app.editors.get(index).map(|e| e.doc.is_read_only()).unwrap_or(false)
    }))
}

fn activate_buffer(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    app.active = index;
    app.focus = Focus::Editor;
    let name = app
        .editors
        .get(index)
        .map(|e| e.doc.name())
        .unwrap_or_default();
    if let Some(path) = app.editors.get(index).and_then(|e| e.doc.path.clone()) {
        app.explorer.reveal(&path);
    }
    Ok(json!({"active": index, "name": name}))
}

fn close_buffer(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let Some(editor) = app.editors.get(index) else {
        return Err("Buffer closed".into());
    };
    let name = editor.doc.name();
    if editor.doc.is_dirty() {
        if !a.flag("discard") {
            return Err(format!(
                "{name} has unsaved changes; save it, or pass discard=true and confirm=DISCARD"
            ));
        }
        if a.text("confirm") != "DISCARD" {
            return Err("confirm must be exactly DISCARD to close an edited buffer".into());
        }
    }
    app.editors.remove(index);
    if app.active >= app.editors.len() {
        app.active = app.editors.len().saturating_sub(1);
    } else if index < app.active {
        app.active -= 1;
    }
    if app.editors.is_empty() {
        app.active = 0;
        app.focus = Focus::Explorer;
    }
    Ok(json!({
        "closed": name,
        "buffers": app.editors.len(),
        "active": app.active
    }))
}

fn go_to_line(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let line = a
        .number("line")
        .ok_or_else(|| "line is required".to_string())?
        .max(1);
    let column = a.number("column").unwrap_or(1).max(1);
    let Some(editor) = app.editors.get_mut(index) else {
        return Err("Buffer closed".into());
    };
    let total = editor.doc.len_lines();
    let line = line.min(total.max(1));
    editor.move_to(line - 1, column - 1, false);
    editor.center_on_cursor();
    let name = editor.doc.name();
    app.active = index;
    app.focus = Focus::Editor;
    Ok(json!({"name": name, "line": line, "column": column, "lines": total}))
}

fn select_range(app: &mut App, a: Args) -> ToolResult {
    let index = buffer_index(app, &a)?;
    let start_line = a
        .number("start_line")
        .ok_or_else(|| "start_line is required".to_string())?
        .max(1);
    let end_line = a
        .number("end_line")
        .ok_or_else(|| "end_line is required".to_string())?
        .max(start_line);
    let start_column = a.number("start_column").unwrap_or(1).max(1);
    let Some(editor) = app.editors.get_mut(index) else {
        return Err("Buffer closed".into());
    };
    let lines = editor.doc.len_lines();
    let start_line = start_line.min(lines.max(1));
    let end_line = end_line.min(lines.max(1));
    let start = editor.doc.idx(start_line - 1, start_column - 1);
    let end = match a.number("end_column") {
        Some(column) => editor.doc.idx(end_line - 1, column.saturating_sub(1)),
        None => {
            let line_start = editor.doc.line_start(end_line - 1);
            line_start + editor.doc.line_len(end_line - 1)
        }
    };
    let anchor = start.min(end);
    let head = start.max(end);
    editor.sel = crate::document::Selection::new(anchor, head);
    editor.sync();
    editor.scroll_into_view();
    let text = editor.selected_text();
    Ok(json!({
        "start": {"line": start_line, "column": start_column},
        "end_line": end_line,
        "characters": head.saturating_sub(anchor),
        "text": cut(&text, 2_000)
    }))
}

fn list_themes(app: &mut App) -> ToolResult {
    let active = app.theme.name.clone();
    let names: Vec<Value> = crate::theme::theme_names()
        .into_iter()
        .map(|name| json!({"name": name, "active": name == active}))
        .collect();
    Ok(json!({"active": active, "themes": names}))
}

fn set_theme(app: &mut App, a: Args) -> ToolResult {
    let name = a.string("name")?;
    if crate::theme::load(&name).is_none() {
        return Err(format!(
            "Unknown theme {name}; call list_themes for the exact names"
        ));
    }
    app.set_theme(&name);
    Ok(json!({"theme": app.theme.name.clone()}))
}

fn toggle_view(app: &mut App, a: Args) -> ToolResult {
    let setting = a.string("setting")?.to_lowercase().replace('-', "_");
    let explicit = a.at("value").and_then(Value::as_bool);
    let state: String;
    match setting.as_str() {
        "sidebar" | "explorer" => {
            app.show_sidebar = explicit.unwrap_or(!app.show_sidebar);
            state = if app.show_sidebar { "on" } else { "off" }.into();
        }
        "agent_panel" | "agent" => {
            app.show_agent = explicit.unwrap_or(!app.show_agent);
            state = if app.show_agent { "on" } else { "off" }.into();
        }
        "minimap" => {
            app.settings.minimap = explicit.unwrap_or(!app.settings.minimap);
            state = if app.settings.minimap { "on" } else { "off" }.into();
        }
        "whitespace" => {
            app.settings.whitespace = explicit.unwrap_or(!app.settings.whitespace);
            state = if app.settings.whitespace { "on" } else { "off" }.into();
        }
        "indent_guides" => {
            app.settings.indent_guides = explicit.unwrap_or(!app.settings.indent_guides);
            state = if app.settings.indent_guides { "on" } else { "off" }.into();
        }
        "line_numbers" => {
            app.settings.line_numbers = explicit.unwrap_or(!app.settings.line_numbers);
            state = if app.settings.line_numbers { "on" } else { "off" }.into();
        }
        "word_wrap" => {
            app.settings.word_wrap = match explicit {
                Some(value) => Some(value),
                None => match app.settings.word_wrap {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                },
            };
            state = match app.settings.word_wrap {
                None => "automatic".into(),
                Some(true) => "on".into(),
                Some(false) => "off".into(),
            };
        }
        "toolbar" => {
            app.settings.toolbar = match explicit {
                Some(value) => Some(value),
                None => match app.settings.toolbar {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                },
            };
            state = match app.settings.toolbar {
                None => "automatic".into(),
                Some(true) => "on".into(),
                Some(false) => "off".into(),
            };
        }
        other => {
            return Err(format!(
                "Unknown view setting {other}; use sidebar, agent_panel, word_wrap, toolbar, minimap, whitespace, indent_guides or line_numbers"
            ));
        }
    }
    Ok(json!({"setting": setting, "state": state}))
}

fn auto_label(value: Option<bool>) -> &'static str {
    match value {
        None => "automatic",
        Some(true) => "on",
        Some(false) => "off",
    }
}

fn get_ui_state(app: &mut App) -> ToolResult {
    let focus = match app.focus {
        Focus::Editor => "editor",
        Focus::Explorer => "explorer",
        Focus::Find => "find",
        Focus::Search => "search",
        Focus::Agent => "agent",
    };
    let active = app.editor().map(|editor| {
        let (line, column) = editor.cursor();
        json!({
            "name": editor.doc.name(),
            "path": editor.doc.path.as_ref().map(|p| display(app, p)),
            "dirty": editor.doc.is_dirty(),
            "language": editor.language(),
            "cursor": {"line": line + 1, "column": column + 1}
        })
    });
    Ok(json!({
        "focus": focus,
        "palette_open": app.palette.is_some(),
        "dialog_open": app.dialog.is_some(),
        "show_sidebar": app.show_sidebar,
        "show_agent": app.show_agent,
        "compact": app.compact,
        "theme": app.theme.name.clone(),
        "find_visible": app.find.visible,
        "search_visible": app.search.visible,
        "word_wrap": auto_label(app.settings.word_wrap),
        "toolbar": auto_label(app.settings.toolbar),
        "minimap": app.settings.minimap,
        "whitespace": app.settings.whitespace,
        "indent_guides": app.settings.indent_guides,
        "line_numbers": app.settings.line_numbers,
        "buffers": app.editors.len(),
        "active_buffer": active,
        "running_agent": app.agent.is_working(),
        "automation_busy": app.automation.busy()
    }))
}

// ========================================================================= code

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn detect_language(app: &mut App, a: Args) -> ToolResult {
    if !app.editors.is_empty() || a.text("path").is_empty() {
        let index = buffer_index(app, &a)?;
        let editor = app
            .editors
            .get(index)
            .ok_or_else(|| "Buffer closed".to_string())?;
        let path = editor.doc.path.clone();
        return Ok(json!({
            "name": editor.doc.name(),
            "path": path.as_ref().map(|p| display(app, p)),
            "language": editor.language(),
            "extension": path.as_ref().map(|p| extension_of(p)),
            "source": "editor syntax",
            "read_only_preview": editor.doc.is_read_only()
        }));
    }
    let path = resolve(app, &a, "path")?;
    let first = read_file_text(&path, 4 * 1024)
        .map(|(text, _)| text)
        .unwrap_or_default();
    let syntax = crate::highlight::detect_syntax(Some(&path), first.lines().next().unwrap_or(""));
    Ok(json!({
        "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "path": display(app, &path),
        "language": syntax.name.clone(),
        "extension": extension_of(&path),
        "source": "file name and shebang",
        "read_only_preview": false
    }))
}

fn outline_symbols(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let extension = extension_of(&path);
    let is_markdown = matches!(extension.as_str(), "md" | "markdown" | "mdx");
    let mut symbols: Vec<Value> = Vec::new();
    if is_markdown {
        let headings = compile(r"^(#{1,6})\s+(.+)$", true, true, false)?;
        for captures in headings.captures_iter(&text) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            let level = captures.get(1).map(|m| m.as_str().len()).unwrap_or(0);
            let title = captures.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            let line = text[..whole.start()].matches('\n').count() + 1;
            symbols.push(json!({
                "line": line,
                "kind": format!("h{level}"),
                "name": cut(title, 200)
            }));
        }
    } else {
        let re = definition_regex()?;
        for captures in re.captures_iter(&text) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            let line = text[..whole.start()].matches('\n').count() + 1;
            symbols.push(json!({
                "line": line,
                "kind": captures.get(1).map(|m| m.as_str()).unwrap_or(""),
                "name": captures.get(2).map(|m| m.as_str()).unwrap_or(""),
                "signature": cut(whole.as_str().trim(), 200)
            }));
            if symbols.len() >= 500 {
                break;
            }
        }
    }
    Ok(json!({
        "path": display(app, &path),
        "format": if is_markdown { "markdown" } else { "code" },
        "symbols": symbols,
        "truncated": symbols.len() >= 500
    }))
}

fn dependency_summary(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let mut manifests: Vec<Value> = Vec::new();
    for (name, kind) in [
        ("Cargo.toml", "cargo"),
        ("package.json", "npm"),
        ("pyproject.toml", "python"),
        ("requirements.txt", "python"),
        ("go.mod", "go"),
    ] {
        let path = base.join(name);
        if !path.is_file() {
            continue;
        }
        let Ok((text, _)) = read_file_text(&path, 512 * 1024) else {
            continue;
        };
        let dependencies: Vec<Value> = match kind {
            "cargo" => {
                let mut section = String::new();
                let mut out = Vec::new();
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('[') && trimmed.ends_with(']') {
                        section = trimmed.trim_matches(['[', ']']).to_string();
                        continue;
                    }
                    if section.contains("dependencies")
                        && let Some((key, value)) = trimmed.split_once('=')
                        && !key.trim().is_empty()
                    {
                        out.push(json!({
                            "name": key.trim(),
                            "version": cut(value.trim(), 80)
                        }));
                    }
                }
                out
            }
            "npm" => serde_json::from_str::<Value>(&text)
                .ok()
                .map(|value| {
                    let mut out = Vec::new();
                    for field in ["dependencies", "devDependencies"] {
                        if let Some(object) = value.get(field).and_then(Value::as_object) {
                            for (key, version) in object {
                                out.push(json!({
                                    "name": key,
                                    "version": version.as_str().unwrap_or(""),
                                    "scope": field
                                }));
                            }
                        }
                    }
                    out
                })
                .unwrap_or_default(),
            "python" => {
                let mut section = String::new();
                let mut out = Vec::new();
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('[') {
                        section = trimmed.trim_matches(['[', ']']).to_string();
                        continue;
                    }
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    if name == "requirements.txt" || section.contains("dependencies") {
                        let spec = trimmed.split(['=', '<', '>', '!', ';']).next().unwrap_or("");
                        let spec = spec.trim();
                        if !spec.is_empty() {
                            out.push(json!({"name": spec, "version": cut(trimmed, 80)}));
                        }
                    }
                }
                out
            }
            "go" => {
                let mut in_require = false;
                let mut out = Vec::new();
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("require (") {
                        in_require = true;
                        continue;
                    }
                    if in_require && trimmed == ")" {
                        in_require = false;
                        continue;
                    }
                    let candidate = if in_require {
                        Some(trimmed)
                    } else if trimmed.starts_with("require ") {
                        Some(trimmed.trim_start_matches("require ").trim())
                    } else {
                        None
                    };
                    if let Some(candidate) = candidate
                        && let Some((module, version)) = candidate.split_once(' ')
                    {
                        out.push(json!({"name": module.trim(), "version": version.trim()}));
                    }
                }
                out
            }
            _ => Vec::new(),
        };
        manifests.push(json!({
            "manifest": name,
            "kind": kind,
            "dependencies": dependencies
        }));
    }
    Ok(json!({
        "path": display(app, &base),
        "manifests": manifests
    }))
}

fn license_check(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let limit = a.number_or("limit", 300).clamp(1, 1000);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    scan_files(app, &base, "", "", limit, |relative, text| {
        scanned += 1;
        let head: String = text.chars().take(2048).collect();
        let lower = head.to_lowercase();
        let license = if let Some(index) = lower.find("spdx-license-identifier:") {
            let rest = &lower[index + "spdx-license-identifier:".len()..];
            let id = rest.lines().next().unwrap_or("").trim().to_string();
            Some(id)
        } else if lower.contains("apache license") {
            Some("Apache-2.0".to_string())
        } else if lower.contains("gnu general public license") {
            Some("GPL".to_string())
        } else if lower.contains("mit license") || lower.contains("permission is hereby granted, free of charge") {
            Some("MIT".to_string())
        } else if lower.contains("bsd 3-clause") || lower.contains("redistribution and use in source and binary forms") {
            Some("BSD".to_string())
        } else {
            None
        };
        match license {
            Some(id) => {
                let id = if id.is_empty() { "unspecified".to_string() } else { id };
                *counts.entry(id).or_insert(0) += 1;
            }
            None => {
                if missing.len() < 100 {
                    missing.push(relative.to_string());
                }
            }
        }
        scanned < limit
    });
    let licenses: Vec<Value> = counts
        .into_iter()
        .map(|(id, files)| json!({"license": id, "files": files}))
        .collect();
    Ok(json!({
        "scanned": scanned,
        "licenses": licenses,
        "files_without_header": missing,
        "note": "Header heuristic over the first 2 KiB of each text file"
    }))
}

fn markdown_stats(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let mut headings = [0usize; 7];
    let mut links = 0usize;
    let mut images = 0usize;
    let mut fences = 0usize;
    let mut list_items = 0usize;
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fences += 1;
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            if level <= 6 && trimmed.chars().nth(level) == Some(' ') {
                headings[level] += 1;
            }
        }
        images += trimmed.matches("![").count();
        links += trimmed.matches("](").count().saturating_sub(trimmed.matches("![").count());
        if trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || trimmed
                .split_once(". ")
                .is_some_and(|(prefix, _)| prefix.chars().all(|c| c.is_ascii_digit()))
        {
            list_items += 1;
        }
    }
    let words = text.split_whitespace().count();
    Ok(json!({
        "path": display(app, &path),
        "headings": {
            "h1": headings[1], "h2": headings[2], "h3": headings[3],
            "h4": headings[4], "h5": headings[5], "h6": headings[6],
            "total": headings.iter().sum::<usize>()
        },
        "links": links,
        "images": images,
        "code_fences": fences,
        "list_items": list_items,
        "words": words,
        "reading_minutes": ((words as f64) / 200.0).ceil() as usize
    }))
}

fn import_summary(app: &mut App, a: Args) -> ToolResult {
    let base = if a.text("path").is_empty() {
        app.explorer.root.clone()
    } else {
        resolve(app, &a, "path")?
    };
    let re = compile(
        r"^\s*(use |import |from |require\(|#include |using )",
        true,
        true,
        false,
    )?;
    let mut per_file: Vec<(usize, String, Vec<String>)> = Vec::new();
    let mut total = 0usize;
    let scanned = if base.is_file() {
        let (text, _) = read_text(app, &base, 2 * 1024 * 1024)?;
        let mut count = 0usize;
        let mut samples = Vec::new();
        for line in text.lines() {
            if re.is_match(line) {
                count += 1;
                if samples.len() < 10 {
                    samples.push(cut(line.trim(), 200));
                }
            }
        }
        total = count;
        if count > 0 {
            per_file.push((count, display(app, &base), samples));
        }
        1
    } else {
        scan_files(app, &base, "", "", MAX_SCAN_FILES, |relative, text| {
            let mut count = 0usize;
            let mut samples = Vec::new();
            for line in text.lines() {
                if re.is_match(line) {
                    count += 1;
                    if samples.len() < 5 {
                        samples.push(cut(line.trim(), 200));
                    }
                }
            }
            if count > 0 {
                total += count;
                per_file.push((count, relative.to_string(), samples));
            }
            true
        })
    };
    per_file.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    per_file.truncate(40);
    let files: Vec<Value> = per_file
        .into_iter()
        .map(|(count, file, samples)| {
            json!({"file": file, "imports": count, "samples": samples})
        })
        .collect();
    Ok(json!({
        "path": display(app, &base),
        "scanned": scanned,
        "total_imports": total,
        "files": files
    }))
}

fn comment_ratio(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let mut code = 0usize;
    let mut comments = 0usize;
    let mut blank = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank += 1;
        } else if is_comment_line(trimmed) {
            comments += 1;
        } else {
            code += 1;
        }
    }
    let meaningful = code + comments;
    let ratio = if meaningful == 0 {
        0.0
    } else {
        (comments as f64 * 100.0 / meaningful as f64 * 10.0).round() / 10.0
    };
    Ok(json!({
        "path": display(app, &path),
        "language": extension_of(&path),
        "code": code,
        "comment_like": comments,
        "blank": blank,
        "comment_percent": ratio
    }))
}

fn function_sizes(app: &mut App, a: Args) -> ToolResult {
    let path = buffer_path(app, &a)?;
    let (text, _) = read_text(app, &path, 2 * 1024 * 1024)?;
    let limit = a.number_or("limit", 15).clamp(1, 50);
    let re = definition_regex()?;
    let mut definitions: Vec<(usize, String, String)> = Vec::new();
    for captures in re.captures_iter(&text) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let line = text[..whole.start()].matches('\n').count() + 1;
        definitions.push((
            line,
            captures.get(1).map(|m| m.as_str()).unwrap_or("").to_string(),
            captures.get(2).map(|m| m.as_str()).unwrap_or("").to_string(),
        ));
    }
    let total_lines = text.lines().count();
    let mut blocks: Vec<(usize, usize, String, String)> = Vec::new();
    for (index, (line, kind, name)) in definitions.iter().enumerate() {
        let end = definitions
            .get(index + 1)
            .map(|(next, _, _)| next.saturating_sub(1))
            .unwrap_or(total_lines);
        let size = end.saturating_sub(*line) + 1;
        blocks.push((size, *line, kind.clone(), name.clone()));
    }
    blocks.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
    let truncated = blocks.len() > limit;
    blocks.truncate(limit);
    let items: Vec<Value> = blocks
        .into_iter()
        .map(|(lines, line, kind, name)| {
            json!({"name": name, "kind": kind, "line": line, "lines": lines})
        })
        .collect();
    Ok(json!({
        "path": display(app, &path),
        "definitions": definitions.len(),
        "longest": items,
        "truncated": truncated,
        "note": "Approximate: each block runs until the next definition line"
    }))
}

fn looks_like_test(relative: &str) -> bool {
    let name = relative.rsplit('/').next().unwrap_or(relative).to_lowercase();
    name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_test.rs")
        || relative.starts_with("tests/")
        || relative.contains("/tests/")
        || relative.contains("__tests__/")
        || name.ends_with("tests.rs")
}

fn test_map(app: &mut App, a: Args) -> ToolResult {
    let input = a.text("path");
    if input.is_empty() {
        let mut files: Vec<String> = crate::explorer::project_files(&app.explorer.root, 20_000)
            .into_iter()
            .filter(|relative| looks_like_test(relative))
            .take(200)
            .collect();
        files.sort();
        return Ok(json!({"test_files": files, "count": files.len()}));
    }
    let path = resolve(app, &a, "path")?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = extension_of(&path);
    let parent = path.parent().unwrap_or(&app.explorer.root).to_path_buf();
    let mut candidates: Vec<PathBuf> = vec![
        parent.join(format!("test_{stem}.{extension}")),
        parent.join(format!("{stem}_test.{extension}")),
        parent.join(format!("{stem}.test.{extension}")),
        parent.join(format!("{stem}.spec.{extension}")),
        app.explorer.root.join("tests").join(format!("{stem}.rs")),
        app.explorer.root.join("tests").join(format!("test_{stem}.py")),
        app.explorer.root.join("tests").join(format!("{stem}.py")),
        parent.join("__tests__").join(format!("{stem}.test.{extension}")),
    ];
    candidates.sort();
    candidates.dedup();
    let found: Vec<Value> = candidates
        .iter()
        .map(|candidate| {
            json!({
                "path": display(app, candidate),
                "exists": candidate.exists()
            })
        })
        .collect();
    let mut related: Vec<String> = crate::explorer::project_files(&app.explorer.root, 20_000)
        .into_iter()
        .filter(|relative| {
            looks_like_test(relative) && relative.to_lowercase().contains(&stem.to_lowercase())
        })
        .take(50)
        .collect();
    related.sort();
    Ok(json!({
        "source": display(app, &path),
        "stem": stem,
        "candidates": found,
        "related_test_files": related
    }))
}

fn json_validate(app: &mut App, a: Args) -> ToolResult {
    let input = a.text("path");
    if !input.is_empty() {
        let path = resolve(app, &a, "path")?;
        let (text, _) = read_text(app, &path, 4 * 1024 * 1024)?;
        return match serde_json::from_str::<Value>(&text) {
            Ok(value) => Ok(json!({
                "path": display(app, &path),
                "valid": true,
                "root_type": match value {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Array(_) => "array",
                    Value::Object(_) => "object",
                }
            })),
            Err(error) => Ok(json!({
                "path": display(app, &path),
                "valid": false,
                "error": error.to_string()
            })),
        };
    }
    let limit = a.number_or("limit", 200).clamp(1, 1000);
    let mut checked = 0usize;
    let mut invalid: Vec<Value> = Vec::new();
    let root = app.explorer.root.clone();
    scan_files(app, &root, "", "", limit, |relative, text| {
        if !relative.to_lowercase().ends_with(".json") {
            return true;
        }
        checked += 1;
        if let Err(error) = serde_json::from_str::<Value>(text) {
            invalid.push(json!({"file": relative, "error": error.to_string()}));
        }
        true
    });
    Ok(json!({
        "checked": checked,
        "invalid": invalid,
        "valid": invalid.is_empty()
    }))
}

// ====================================================================== session

fn list_toolkit(options: &AgentConfig, a: Args) -> ToolResult {
    let filter = a.text("group").to_string();
    if !filter.is_empty() && !group_ids().contains(&filter.as_str()) {
        return Err(format!(
            "Unknown group {filter}; use {}",
            group_ids().join(", ")
        ));
    }
    let verbose = a.flag("verbose");
    let all = catalog();
    let mut groups = Vec::new();
    for group in Group::ALL {
        if !filter.is_empty() && group.id() != filter.as_str() {
            continue;
        }
        let items: Vec<Value> = all
            .iter()
            .filter(|spec| spec.group == group)
            .map(|spec| {
                let mut item = json!({
                    "name": spec.name,
                    "kind": kind_label(spec.kind),
                    "enabled": spec_enabled(options, spec)
                });
                if verbose
                    && let Some(object) = item.as_object_mut()
                {
                    object.insert("description".into(), json!(spec.description));
                }
                item
            })
            .collect();
        groups.push(json!({
            "id": group.id(),
            "title": group.title(),
            "enabled": options.toolkit && group_enabled(options, group),
            "tools": items
        }));
    }
    Ok(json!({
        "enabled": options.toolkit,
        "groups": groups,
        "configured_groups": options.toolkit_groups,
        "note": "Enable groups with the agent setting 'Toolkit groups'; read-only agents never receive write tools"
    }))
}

fn describe_tool(options: &AgentConfig, a: Args) -> ToolResult {
    let name = a.string("name")?;
    let all = catalog();
    let Some(spec) = all.iter().find(|spec| spec.name == name) else {
        return Err(format!("Unknown tool {name}; call list_toolkit for names"));
    };
    Ok(json!({
        "name": spec.name,
        "group": spec.group.id(),
        "kind": kind_label(spec.kind),
        "enabled": spec_enabled(options, spec),
        "description": spec.description,
        "schema": spec.schema.clone()
    }))
}

fn get_agent_settings(options: &AgentConfig) -> ToolResult {
    let groups: Vec<&str> = Group::ALL
        .iter()
        .filter(|group| options.toolkit && group_enabled(options, **group))
        .map(|group| group.id())
        .collect();
    let model = if options.model.is_empty() {
        "(provider default)".to_string()
    } else {
        options.model.clone()
    };
    let endpoint = if options.endpoint.is_empty() {
        "(provider default)".to_string()
    } else {
        options.endpoint.clone()
    };
    Ok(json!({
        "backend": options.backend,
        "provider": options.provider,
        "model": model,
        "endpoint": endpoint,
        "api_key_env": options.api_key_env,
        "instructions_bytes": options.instructions.len(),
        "read_only": options.read_only,
        "commands": policy_label(options.commands),
        "plugins": {
            "enabled": options.plugins,
            "allowed": options.allowed_plugins,
            "denied": options.denied_plugins
        },
        "sub_agents": options.sub_agents,
        "generated_tools": options.generated_tools,
        "toolkit": {
            "enabled": options.toolkit,
            "groups": groups,
            "tools": if options.toolkit { tools(options).len() } else { 0 }
        },
        "budgets": {
            "max_steps": options.max_steps,
            "max_delegations": options.max_delegations,
            "max_agent_depth": options.max_agent_depth,
            "max_concurrent_tasks": options.max_concurrent_tasks,
            "task_timeout_secs": options.task_timeout_secs,
            "stall_timeout_secs": options.stall_timeout_secs,
            "max_output_tokens": options.max_output_tokens
        },
        "auto_context": options.auto_context,
        "skills": options.skills
    }))
}

fn list_editor_commands(a: Args) -> ToolResult {
    let filter = a.text("filter").to_lowercase();
    let mut items = Vec::new();
    for command in COMMANDS {
        if !filter.is_empty() && !command.title.to_lowercase().contains(&filter) {
            continue;
        }
        items.push(json!({
            "id": format!("{:?}", command.cmd),
            "title": command.title,
            "keys": command.keys
        }));
    }
    Ok(json!({"commands": items, "count": items.len()}))
}

fn normalize_id(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Editor commands that change buffer or file content, blocked for read-only agents.
fn command_edits_content(id: &str) -> bool {
    matches!(
        id,
        "NewFile"
            | "Save"
            | "SaveAs"
            | "SaveAll"
            | "Undo"
            | "Redo"
            | "Cut"
            | "Paste"
            | "Replace"
            | "ReplaceInFiles"
            | "ToggleComment"
            | "Indent"
            | "Outdent"
            | "MoveLineUp"
            | "MoveLineDown"
            | "DuplicateLineDown"
            | "DuplicateLineUp"
            | "DeleteLine"
            | "InsertLineBelow"
            | "InsertLineAbove"
            | "ConvertIndentToSpaces"
            | "ConvertIndentToTabs"
            | "ToggleLineEnding"
    )
}

fn run_editor_command(app: &mut App, options: &AgentConfig, a: Args) -> ToolResult {
    let input = a.string("command")?;
    let wanted = normalize_id(&input);
    if wanted.is_empty() {
        return Err("command must name an editor command".into());
    }
    let found = COMMANDS.iter().find(|command| {
        normalize_id(command.title) == wanted || normalize_id(&format!("{:?}", command.cmd)) == wanted
    });
    let Some(command) = found else {
        return Err(format!(
            "Unknown editor command {input}; call list_editor_commands for exact titles"
        ));
    };
    let id = format!("{:?}", command.cmd);
    if matches!(id.as_str(), "Quit" | "AgentNewChat" | "AgentStop") {
        return Err(format!("{id} is blocked for AI tools because it ends work or the session"));
    }
    if options.read_only && command_edits_content(&id) {
        return Err(format!(
            "{id} edits buffers or files, and this agent is read-only"
        ));
    }
    let title = command.title;
    let cmd = command.cmd;
    app.run(cmd);
    Ok(json!({"ran": id, "title": title}))
}

fn open_palette(app: &mut App, a: Args) -> ToolResult {
    let mode = a.string("mode")?.to_lowercase().replace(['-', ' '], "_");
    let (palette, needs_editor) = match mode.as_str() {
        "files" | "quick_open" => (PaletteMode::Files, false),
        "commands" | "command_palette" => (PaletteMode::Commands, false),
        "themes" | "theme" => (PaletteMode::Themes, false),
        "languages" | "language" => (PaletteMode::Languages, true),
        "goto_line" | "line" => (PaletteMode::GotoLine, true),
        "save_as" => (PaletteMode::SaveAs, true),
        "agent_models" | "models" => (PaletteMode::AgentModels, false),
        "agent_picker" | "agents" => (PaletteMode::AgentPicker, false),
        "plugins" => (PaletteMode::Plugins, false),
        "settings" | "preferences" => (PaletteMode::Settings, false),
        "agent_settings" | "agent" => (PaletteMode::AgentSettings, false),
        "sub_agents" | "subagents" => (PaletteMode::SubAgents, false),
        "skills" => (PaletteMode::Skills, false),
        other => {
            return Err(format!(
                "Unknown palette mode {other}; use files, commands, themes, languages, goto_line, save_as, agent_models, agent_picker, plugins, settings, agent_settings, sub_agents or skills"
            ));
        }
    };
    if needs_editor && app.editors.is_empty() {
        return Err("Open a text file before using this palette mode".into());
    }
    app.open_palette(palette);
    Ok(json!({"opened": mode}))
}

fn focus_panel(app: &mut App, a: Args) -> ToolResult {
    let panel = a.string("panel")?.to_lowercase();
    match panel.as_str() {
        "editor" => {
            if app.editors.is_empty() {
                return Err("No editor buffer is open".into());
            }
            app.focus = Focus::Editor;
        }
        "explorer" | "sidebar" => {
            app.show_sidebar = true;
            app.focus = Focus::Explorer;
        }
        "agent" | "ai" => {
            app.show_agent = true;
            app.focus = Focus::Agent;
        }
        "find" => {
            if app.editors.is_empty() {
                return Err("No editor buffer is open".into());
            }
            app.find.visible = true;
            app.focus = Focus::Find;
        }
        "search" => {
            app.search.visible = true;
            app.focus = Focus::Search;
        }
        other => {
            return Err(format!(
                "Unknown panel {other}; use editor, explorer, agent, find or search"
            ));
        }
    }
    Ok(json!({"focused": panel}))
}

fn scratchpad_write(app: &mut App, a: Args) -> ToolResult {
    let key = a.string("key")?;
    if key.is_empty() || key.len() > 64 {
        return Err("key must contain 1-64 bytes".into());
    }
    if a.flag("delete") {
        let removed = app.automation.scratchpad.remove(&key).is_some();
        return Ok(json!({"deleted": key, "existed": removed}));
    }
    let value = match a.at("value").and_then(Value::as_str) {
        Some(value) => value.to_string(),
        None => return Err("value is required unless delete is true".into()),
    };
    if value.len() > 4096 {
        return Err("value is limited to 4096 bytes".into());
    }
    let exists = app.automation.scratchpad.contains_key(&key);
    if !exists && app.automation.scratchpad.len() >= 64 {
        return Err("scratchpad is full (64 keys); delete a key first".into());
    }
    let current: usize = app
        .automation
        .scratchpad
        .iter()
        .filter(|(existing, _)| existing.as_str() != key.as_str())
        .map(|(existing, text)| existing.len() + text.len())
        .sum();
    if current + key.len() + value.len() > 32 * 1024 {
        return Err("scratchpad would exceed 32 KiB; delete or shorten notes".into());
    }
    app.automation.scratchpad.insert(key.clone(), value.clone());
    Ok(json!({
        "key": key,
        "bytes": value.len(),
        "keys": app.automation.scratchpad.len()
    }))
}

fn scratchpad_read(app: &mut App, a: Args) -> ToolResult {
    let key = a.text("key");
    if !key.is_empty() {
        return match app.automation.scratchpad.get(key) {
            Some(value) => Ok(json!({"key": key, "value": value})),
            None => Err(format!("Scratchpad key {key} is not set")),
        };
    }
    let notes: Vec<Value> = app
        .automation
        .scratchpad
        .iter()
        .map(|(key, value)| {
            json!({
                "key": key,
                "bytes": value.len(),
                "preview": cut(value, 200)
            })
        })
        .collect();
    Ok(json!({"keys": notes, "count": notes.len()}))
}

fn session_summary(app: &mut App, options: &AgentConfig) -> ToolResult {
    let root = app.explorer.root.clone();
    let files = crate::explorer::project_files(&root, 20_000);
    let groups: Vec<&str> = Group::ALL
        .iter()
        .filter(|group| options.toolkit && group_enabled(options, **group))
        .map(|group| group.id())
        .collect();
    let scratchpad: Vec<&String> = app.automation.scratchpad.keys().collect();
    let focus = match app.focus {
        Focus::Editor => "editor",
        Focus::Explorer => "explorer",
        Focus::Find => "find",
        Focus::Search => "search",
        Focus::Agent => "agent",
    };
    Ok(json!({
        "workspace": {
            "root": root.to_string_lossy(),
            "name": app.explorer.root_name(),
            "files": files.len(),
            "git_branch": app.git_branch.clone()
        },
        "buffers": {
            "count": app.editors.len(),
            "dirty": app.editors.iter().filter(|e| e.doc.is_dirty()).count(),
            "active": app.editor().map(|e| e.doc.name())
        },
        "ui": {
            "focus": focus,
            "sidebar": app.show_sidebar,
            "agent_panel": app.show_agent,
            "theme": app.theme.name.clone(),
            "palette_open": app.palette.is_some()
        },
        "tasks": app.automation.overwatch.snapshot(None),
        "scratchpad_keys": scratchpad,
        "scratchpad_keys_count": app.automation.scratchpad.len(),
        "toolkit": {
            "enabled": options.toolkit,
            "groups": groups,
            "tools": if options.toolkit { tools(options).len() } else { 0 }
        },
        "note": "Use list_functions for this task's generated functions and task_status for task detail"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{Reply, Source};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    fn app(tag: &str) -> App {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "termed-toolkit-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "beta\nalpha\n").unwrap();
        std::fs::write(root.join("b.txt"), "do not change\n").unwrap();
        App::new(
            root.clone(),
            vec![root.join("a.txt")],
            "Visual Studio Dark",
            false,
        )
    }

    fn tool(app: &mut App, name: &str, args: Value) -> ToolResult {
        let (tx, rx) = mpsc::channel();
        app.host_tool(Source::Main, Reply::Mcp(tx), name, args);
        let start = Instant::now();
        loop {
            app.pump_agent();
            app.pump_plugins();
            match rx.try_recv() {
                Ok(result) => return result,
                Err(mpsc::TryRecvError::Disconnected) => panic!("host reply was dropped"),
                Err(_) => {}
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "toolkit tool did not answer"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn names(options: &AgentConfig) -> Vec<String> {
        tools(options).into_iter().map(|tool| tool.name).collect()
    }

    #[test]
    fn catalog_is_unique_and_matches_group_counts() {
        let all = catalog();
        assert_eq!(all.len(), 100, "the catalog advertises exactly 100 tools");
        let mut seen: HashSet<&str> = HashSet::new();
        for spec in &all {
            assert!(seen.insert(spec.name), "duplicate tool name {}", spec.name);
            assert!(!spec.description.is_empty(), "{} needs a description", spec.name);
            assert!(
                spec.schema.is_object(),
                "{} needs an object schema",
                spec.name
            );
        }
        for group in Group::ALL {
            let count = all.iter().filter(|spec| spec.group == group).count();
            assert_eq!(count, 10, "group {} lists {count} tools", group.id());
        }
        assert_eq!(group_ids().len(), 10);
    }

    #[test]
    fn permissions_decide_which_tools_are_advertised() {
        let mut options = AgentConfig::default();
        let baseline = names(&options);
        assert_eq!(baseline.len(), 100);
        assert!(baseline.iter().any(|name| name == "delete_path"));
        assert!(baseline.iter().any(|name| name == "git_status"));

        options.toolkit = false;
        assert!(names(&options).is_empty());
        assert!(!contains(&options, "workspace_info"));

        let mut read_only = AgentConfig::default();
        read_only.read_only = true;
        let visible = names(&read_only);
        assert!(!visible.iter().any(|name| is_write(name)));
        assert!(visible.iter().any(|name| name == "read_lines"));
        assert!(visible.iter().any(|name| name == "git_log"));
        assert!(!contains(&read_only, "create_file"));
        assert!(contains(&read_only, "list_buffers"));

        let mut denied = AgentConfig::default();
        denied.commands = CommandPolicy::Deny;
        let visible = names(&denied);
        assert!(visible.iter().any(|name| name == "delete_path"));
        assert!(!visible.iter().any(|name| name == "git_status"));
        assert!(!contains(&denied, "git_blame"));

        let mut grouped = AgentConfig::default();
        grouped.toolkit_groups = vec!["workspace".into(), "session".into()];
        let visible = names(&grouped);
        assert_eq!(visible.len(), 20);
        assert!(visible.iter().any(|name| name == "workspace_info"));
        assert!(visible.iter().any(|name| name == "session_summary"));
        assert!(!contains(&grouped, "read_lines"));

        grouped.toolkit_groups = vec!["nonsense".into()];
        assert!(names(&grouped).is_empty());
        assert!(!contains(&grouped, "session_summary"));
    }

    #[test]
    fn read_only_agents_never_reach_mutating_toolkit_tools() {
        let mut app = app("readonly");
        app.agent.options.read_only = true;
        let result = tool(
            &mut app,
            "create_file",
            json!({"path": "b.txt", "content": "x", "overwrite": true}),
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(app.explorer.root.join("b.txt")).unwrap(),
            "do not change\n"
        );
        let result = tool(&mut app, "run_editor_command", json!({"command": "Edit: Undo"}));
        assert!(result.is_err());
        let result = tool(&mut app, "read_lines", json!({"path": "a.txt"}));
        assert!(result.is_ok(), "read tools stay available: {result:?}");
    }

    #[test]
    fn destructive_tools_require_explicit_confirmation() {
        let mut app = app("confirm");
        let err = tool(&mut app, "delete_path", json!({"path": "b.txt"})).unwrap_err();
        assert!(err.contains("DELETE"), "{err}");
        assert!(app.explorer.root.join("b.txt").exists());
        let ok = tool(
            &mut app,
            "delete_path",
            json!({"path": "b.txt", "confirm": "DELETE"}),
        )
        .unwrap();
        assert_eq!(ok["deleted"], json!(true));
        assert!(!app.explorer.root.join("b.txt").exists());

        let err = tool(
            &mut app,
            "revert_buffer",
            json!({"path": "a.txt", "confirm": "yes"}),
        )
        .unwrap_err();
        assert!(err.contains("DISCARD"), "{err}");

        let err = tool(
            &mut app,
            "patch_file",
            json!({
                "path": "a.txt",
                "edits": [{"old_text": "beta", "new_text": "gamma"}]
            }),
        )
        .unwrap();
        assert_eq!(err["dry_run"], json!(true));
        assert_eq!(
            std::fs::read_to_string(app.explorer.root.join("a.txt")).unwrap(),
            "beta\nalpha\n",
            "a dry run never writes"
        );
        let err = tool(
            &mut app,
            "patch_file",
            json!({
                "path": "a.txt",
                "edits": [{"old_text": "beta", "new_text": "gamma"}],
                "dry_run": false
            }),
        )
        .unwrap_err();
        assert!(err.contains("APPLY"), "{err}");
    }

    #[test]
    fn edits_hit_live_buffers_instead_of_disk() {
        let mut app = app("live");
        let result = tool(
            &mut app,
            "patch_file",
            json!({
                "path": "a.txt",
                "edits": [{"old_text": "beta", "new_text": "gamma"}],
                "dry_run": false,
                "confirm": "APPLY"
            }),
        )
        .unwrap();
        assert_eq!(result["changed"], json!(true));
        let editor = app.editor().unwrap();
        assert_eq!(editor.doc.rope.to_string(), "gamma\nalpha\n");
        assert!(
            editor.doc.is_dirty(),
            "the live buffer must stay unsaved and undoable"
        );
        assert_eq!(
            std::fs::read_to_string(app.explorer.root.join("a.txt")).unwrap(),
            "beta\nalpha\n",
            "disk is untouched"
        );
    }

    #[test]
    fn read_tools_report_real_workspace_facts() {
        let mut app = app("read");
        let info = tool(&mut app, "workspace_info", json!({})).unwrap();
        assert!(info["bytes"].as_u64().unwrap() >= 22);
        let listing = tool(&mut app, "list_directory", json!({})).unwrap();
        let entries = listing["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry["name"] == json!("a.txt")));
        let files = tool(&mut app, "find_files", json!({"pattern": "*.txt"})).unwrap();
        assert_eq!(files["files"].as_array().unwrap().len(), 2);
        let counts = tool(
            &mut app,
            "buffer_info",
            json!({"path": "a.txt"}),
        )
        .unwrap();
        assert!(counts["lines"].as_u64().unwrap() >= 2);
        assert_eq!(counts["dirty"], json!(false));
        let missing = tool(&mut app, "read_lines", json!({"path": "../escape.txt"}));
        assert!(missing.is_err(), "path escapes must fail closed");
    }

    #[test]
    fn scratchpad_is_bounded_and_session_scoped() {
        let mut app = app("notes");
        tool(
            &mut app,
            "scratchpad_write",
            json!({"key": "plan", "value": "step one"}),
        )
        .unwrap();
        let read = tool(&mut app, "scratchpad_read", json!({"key": "plan"})).unwrap();
        assert_eq!(read["value"], json!("step one"));
        let listed = tool(&mut app, "scratchpad_read", json!({})).unwrap();
        assert_eq!(listed["count"], json!(1));
        let deleted = tool(
            &mut app,
            "scratchpad_write",
            json!({"key": "plan", "delete": true}),
        )
        .unwrap();
        assert_eq!(deleted["existed"], json!(true));
        let oversized = tool(
            &mut app,
            "scratchpad_write",
            json!({"key": "big", "value": "x".repeat(5000)}),
        );
        assert!(oversized.is_err());
    }

    #[test]
    fn session_tools_describe_the_toolkit_itself() {
        let mut app = app("session");
        let listed = tool(&mut app, "list_toolkit", json!({})).unwrap();
        assert_eq!(listed["enabled"], json!(true));
        assert_eq!(listed["groups"].as_array().unwrap().len(), 10);
        let described = tool(
            &mut app,
            "describe_tool",
            json!({"name": "delete_path"}),
        )
        .unwrap();
        assert_eq!(described["group"], json!("filesystem"));
        assert_eq!(described["kind"], json!("write"));
        assert_eq!(described["enabled"], json!(true));
        let commands = tool(&mut app, "list_editor_commands", json!({"filter": "save"})).unwrap();
        assert!(commands["count"].as_u64().unwrap() >= 1);
        let summary = tool(&mut app, "session_summary", json!({})).unwrap();
        assert_eq!(summary["buffers"]["count"], json!(1));
        assert!(summary["toolkit"]["tools"].as_u64().unwrap() == 100);
    }
}
