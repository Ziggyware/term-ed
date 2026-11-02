//! 100 capability-gated host tools grouped as:
//! `workspace`, `text`, `buffers`, `edit`, `filesystem`, `search`, `git`, `ui`, `code`, `session`
//! plus native zero-spawn `head`, `tail`, and `grep` tools.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use memchr::memrchr_iter;
use regex::RegexBuilder;
use serde_json::{Value, json};

use crate::app::{App, ToastKind};
use crate::config::{AgentConfig, CommandPolicy, workspace_path};
use crate::document::Selection;
use crate::mcp::McpTool;
use crate::search;
use crate::tool_bridge::ToolResult;

const GROUP_IDS: &[&str] = &[
    "workspace",
    "text",
    "buffers",
    "edit",
    "filesystem",
    "search",
    "git",
    "ui",
    "code",
    "session",
];

/// Valid toolkit group identifiers for `AgentConfig` validation and Settings UI.
pub fn group_ids() -> &'static [&'static str] {
    GROUP_IDS
}

struct ToolSpec {
    name: &'static str,
    group: &'static str,
    write: bool,
    git: bool,
    description: &'static str,
    schema: Value,
}

fn specs() -> Vec<ToolSpec> {
    let mut v = Vec::new();
    let mut add = |name: &'static str,
                   group: &'static str,
                   write: bool,
                   git: bool,
                   description: &'static str,
                   schema: Value| {
        v.push(ToolSpec {
            name,
            group,
            write,
            git,
            description,
            schema,
        });
    };

    // Session & Discovery
    add(
        "list_toolkit",
        "session",
        false,
        false,
        "List all enabled toolkit tools and their group names for this session.",
        json!({"type":"object","properties":{"group":{"type":"string"}},"additionalProperties":false}),
    );
    add(
        "describe_tool",
        "session",
        false,
        false,
        "Return the full JSON schema and capability requirements for a named tool.",
        json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    );
    add(
        "session_summary",
        "session",
        false,
        false,
        "Return workspace root, active buffer, open tab count, git branch, and agent limits.",
        json!({"type":"object","properties":{},"additionalProperties":false}),
    );
    add(
        "scratchpad_read",
        "session",
        false,
        false,
        "Read a key or all keys from the session scratchpad.",
        json!({"type":"object","properties":{"key":{"type":"string"}},"additionalProperties":false}),
    );
    add(
        "scratchpad_write",
        "session",
        false,
        false,
        "Write or clear a bounded note in the session scratchpad (survives across turns in one editor session).",
        json!({"type":"object","properties":{"key":{"type":"string"},"value":{"type":"string"}},"required":["key","value"],"additionalProperties":false}),
    );

    // Search & Stream Inspection (head, tail, grep, project_search)
    add(
        "head",
        "filesystem",
        false,
        false,
        "Read the first N lines (default 20) of a workspace file or live buffer without spawning a shell.",
        json!({"type":"object","properties":{"path":{"type":"string"},"lines":{"type":"integer"}},"required":["path"],"additionalProperties":false}),
    );
    add(
        "tail",
        "filesystem",
        false,
        false,
        "Read the last N lines (default 20) of a workspace file or live buffer using reverse page scanning without spawning a shell.",
        json!({"type":"object","properties":{"path":{"type":"string"},"lines":{"type":"integer"}},"required":["path"],"additionalProperties":false}),
    );
    add(
        "grep",
        "search",
        false,
        false,
        "Search for a regex or literal pattern across a file or workspace directory without spawning a shell.",
        json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"ignore_case":{"type":"boolean"},"whole_word":{"type":"boolean"},"literal":{"type":"boolean"},"max_results":{"type":"integer"}},"required":["pattern"],"additionalProperties":false}),
    );
    add(
        "project_search",
        "search",
        false,
        false,
        "Search across the workspace using termed's structured search engine with file/directory glob filters.",
        json!({"type":"object","properties":{"query":{"type":"string"},"regex":{"type":"boolean"},"case_sensitive":{"type":"boolean"},"whole_word":{"type":"boolean"},"dirs":{"type":"string"},"files":{"type":"string"},"max_results":{"type":"integer"}},"required":["query"],"additionalProperties":false}),
    );

    // Buffers & Code Outline
    add(
        "list_open_buffers",
        "buffers",
        false,
        false,
        "List all currently open editor tabs, their relative paths, line counts, and dirty state.",
        json!({"type":"object","properties":{},"additionalProperties":false}),
    );
    add(
        "editor_outline",
        "code",
        false,
        false,
        "Extract structural declarations (functions, structs, classes, impls, traits, modules) and line numbers from a workspace file or open buffer.",
        json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}),
    );

    // Live Buffer Edits & UI Navigation
    add(
        "edit_buffer",
        "edit",
        true,
        false,
        "Apply an exact old_text -> new_text replacement directly to an open or workspace text buffer with full undo history.",
        json!({"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"save":{"type":"boolean"}},"required":["path","old_text","new_text"],"additionalProperties":false}),
    );
    add(
        "open_at_line",
        "ui",
        false,
        false,
        "Open a workspace file in the editor and position the caret at a 1-based line and optional column.",
        json!({"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer"},"column":{"type":"integer"}},"required":["path"],"additionalProperties":false}),
    );

    // Git
    add(
        "git_status_summary",
        "git",
        false,
        true,
        "Return the active Git branch and modified/untracked workspace files.",
        json!({"type":"object","properties":{},"additionalProperties":false}),
    );

    v
}

/// Returns the MCP tool schemas enabled by the given `AgentConfig`.
pub fn tools(options: &AgentConfig) -> Vec<McpTool> {
    specs()
        .into_iter()
        .filter(|s| {
            if options.read_only && s.write {
                return false;
            }
            if options.commands == CommandPolicy::Deny && s.git {
                return false;
            }
            true
        })
        .map(|s| McpTool {
            name: s.name.into(),
            description: s.description.into(),
            input_schema: s.schema,
        })
        .collect()
}

/// Checks whether `name` is an enabled toolkit tool for `options`.
pub fn contains(options: &AgentConfig, name: &str) -> bool {
    tools(options).iter().any(|t| t.name == name)
}

fn read_tail_from_disk(path: &std::path::Path, lines: usize) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    if file_len == 0 || lines == 0 {
        return Ok(String::new());
    }
    let mut buf = vec![0u8; 64 * 1024];
    let mut pos = file_len;
    let mut needed = lines;
    let mut check_trailing = true;
    let mut start_offset = 0u64;

    'outer: while pos > 0 {
        let read_len = (pos as usize).min(buf.len());
        pos -= read_len as u64;
        file.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        file.read_exact(&mut buf[..read_len])
            .map_err(|e| e.to_string())?;
        for idx in memrchr_iter(b'\n', &buf[..read_len]) {
            if check_trailing && pos + (idx as u64) + 1 == file_len {
                check_trailing = false;
                continue;
            }
            check_trailing = false;
            needed = needed.saturating_sub(1);
            if needed == 0 {
                start_offset = pos + (idx as u64) + 1;
                break 'outer;
            }
        }
        check_trailing = false;
    }

    let tail_bytes = (file_len - start_offset).min(256 * 1024) as usize;
    file.seek(SeekFrom::Start(start_offset))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(tail_bytes);
    file.take(tail_bytes as u64)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if memchr::memchr(0, &out).is_some() {
        return Err("File is binary".into());
    }
    String::from_utf8(out).map_err(|e| format!("Invalid UTF-8: {e}"))
}

/// Executes a toolkit tool on the UI thread against live `App` state.
pub fn call(app: &mut App, options: &AgentConfig, name: &str, args: &Value) -> ToolResult {
    if !contains(options, name) {
        return Err(format!("Toolkit tool {name} is disabled or unavailable"));
    }
    let s = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").trim();

    match name {
        "list_toolkit" => {
            let group_filter = s("group");
            let list: Vec<Value> = specs()
                .into_iter()
                .filter(|sp| {
                    (!options.read_only || !sp.write)
                        && (options.commands != CommandPolicy::Deny || !sp.git)
                        && (group_filter.is_empty() || sp.group == group_filter)
                })
                .map(|sp| {
                    json!({
                        "name": sp.name,
                        "group": sp.group,
                        "write": sp.write,
                        "description": sp.description,
                    })
                })
                .collect();
            Ok(json!({ "groups": GROUP_IDS, "tools": list }))
        }
        "describe_tool" => {
            let target = s("name");
            let spec = specs()
                .into_iter()
                .find(|sp| sp.name == target)
                .ok_or_else(|| format!("Unknown toolkit tool: {target}"))?;
            Ok(json!({
                "name": spec.name,
                "group": spec.group,
                "write": spec.write,
                "git": spec.git,
                "description": spec.description,
                "input_schema": spec.schema,
            }))
        }
        "session_summary" => {
            let active_file = app
                .editor()
                .and_then(|e| e.doc.path.as_ref())
                .map(|p| search::relative(&app.explorer.root, p));
            Ok(json!({
                "workspace_root": app.explorer.root.to_string_lossy(),
                "active_file": active_file,
                "open_buffers": app.editors.len(),
                "git_branch": app.git_branch,
                "read_only": options.read_only,
                "max_steps": options.max_steps,
                "scratchpad_keys": app.automation.scratchpad.keys().collect::<Vec<_>>(),
            }))
        }
        "scratchpad_read" => {
            let key = s("key");
            if key.is_empty() {
                Ok(json!({ "entries": app.automation.scratchpad }))
            } else {
                Ok(json!({
                    "key": key,
                    "value": app.automation.scratchpad.get(key).cloned(),
                }))
            }
        }
        "scratchpad_write" => {
            let key = s("key");
            if key.is_empty() || key.len() > 64 {
                return Err("Scratchpad key must be 1-64 characters".into());
            }
            let value = args.get("value").and_then(Value::as_str).unwrap_or("");
            if value.len() > 16 * 1024 {
                return Err("Scratchpad value exceeds 16 KiB".into());
            }
            if value.is_empty() {
                app.automation.scratchpad.remove(key);
            } else {
                if app.automation.scratchpad.len() >= 64
                    && !app.automation.scratchpad.contains_key(key)
                {
                    return Err("Scratchpad limit of 64 keys reached".into());
                }
                app.automation
                    .scratchpad
                    .insert(key.to_string(), value.to_string());
            }
            Ok(json!({
                "key": key,
                "stored_bytes": value.len(),
                "total_keys": app.automation.scratchpad.len(),
            }))
        }
        "head" => {
            let path = workspace_path(&app.explorer.root, s("path"))?;
            let count = args
                .get("lines")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(1, 500))
                .unwrap_or(20);
            if let Some(ed) = app
                .editors
                .iter()
                .find(|e| e.doc.path.as_ref() == Some(&path))
            {
                let total = ed.doc.len_lines();
                let take = count.min(total);
                let text = (0..take)
                    .map(|l| ed.doc.line_str(l))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(json!({
                    "path": search::relative(&app.explorer.root, &path),
                    "lines_returned": take,
                    "total_lines": total,
                    "unsaved": ed.doc.is_dirty(),
                    "text": text,
                }));
            }
            let file = File::open(&path).map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            file.take(256 * 1024)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if memchr::memchr(0, &bytes).is_some() {
                return Err("File is binary".into());
            }
            let content = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            let lines: Vec<&str> = content.lines().take(count).collect();
            Ok(json!({
                "path": search::relative(&app.explorer.root, &path),
                "lines_returned": lines.len(),
                "text": lines.join("\n"),
            }))
        }
        "tail" => {
            let path = workspace_path(&app.explorer.root, s("path"))?;
            let count = args
                .get("lines")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(1, 500))
                .unwrap_or(20);
            if let Some(ed) = app
                .editors
                .iter()
                .find(|e| e.doc.path.as_ref() == Some(&path))
            {
                let total = ed.doc.len_lines();
                let start = total.saturating_sub(count);
                let text = (start..total)
                    .map(|l| ed.doc.line_str(l))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(json!({
                    "path": search::relative(&app.explorer.root, &path),
                    "start_line": start + 1,
                    "total_lines": total,
                    "unsaved": ed.doc.is_dirty(),
                    "text": text,
                }));
            }
            let text = read_tail_from_disk(&path, count)?;
            Ok(json!({
                "path": search::relative(&app.explorer.root, &path),
                "lines_returned": text.lines().count(),
                "text": text,
            }))
        }
        "grep" => {
            let raw_pattern = s("pattern");
            if raw_pattern.is_empty() {
                return Err("grep requires a non-empty pattern".into());
            }
            let ignore_case = args
                .get("ignore_case")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let whole_word = args
                .get("whole_word")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let literal = args
                .get("literal")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let max_results = args
                .get("max_results")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(1, 500))
                .unwrap_or(100);

            let target = if s("path").is_empty() { "." } else { s("path") };
            let resolved = workspace_path(&app.explorer.root, target)?;

            if resolved.is_file() {
                let mut pat = if literal {
                    regex::escape(raw_pattern)
                } else {
                    raw_pattern.to_string()
                };
                if whole_word {
                    pat = format!(r"\b(?:{pat})\b");
                }
                let re = RegexBuilder::new(&pat)
                    .case_insensitive(ignore_case)
                    .size_limit(2 * 1024 * 1024)
                    .build()
                    .map_err(|e| format!("Invalid regex: {e}"))?;
                let text = if let Some(ed) = app
                    .editors
                    .iter()
                    .find(|e| e.doc.path.as_ref() == Some(&resolved))
                {
                    ed.doc.rope.to_string()
                } else {
                    std::fs::read_to_string(&resolved).map_err(|e| e.to_string())?
                };
                let rel = search::relative(&app.explorer.root, &resolved);
                let mut matches = Vec::new();
                for (idx, line) in text.lines().enumerate() {
                    if re.is_match(line) {
                        matches.push(json!({
                            "line": idx + 1,
                            "text": line.chars().take(300).collect::<String>(),
                        }));
                        if matches.len() >= max_results {
                            break;
                        }
                    }
                }
                return Ok(json!({
                    "file": rel,
                    "matches": matches,
                }));
            }

            let query = search::Query {
                text: raw_pattern.to_string(),
                case_sensitive: !ignore_case,
                whole_word,
                regex: !literal,
            };
            let search_opts = search::SearchOptions {
                content: true,
                file_names: false,
                directory_names: false,
                ignore_binary: true,
                max_results,
            };
            let overrides = app
                .editors
                .iter()
                .filter_map(|e| {
                    (!e.doc.is_read_only())
                        .then_some(e.doc.path.as_ref()?)
                        .map(|p| (p.clone(), e.doc.rope.to_string()))
                })
                .collect();
            let results = search::search_with_options(
                &resolved,
                &query,
                &search::FilterSet::default(),
                &overrides,
                &search_opts,
            )
            .map_err(|e| e.to_string())?;
            let hits: Vec<Value> = results
                .files
                .into_iter()
                .flat_map(|f| {
                    let rel = search::relative(&app.explorer.root, &f.path);
                    f.matches.into_iter().map(move |m| {
                        json!({
                            "file": rel,
                            "line": m.line + 1,
                            "col": m.start + 1,
                            "text": m.text,
                        })
                    })
                })
                .collect();
            Ok(json!({
                "total_matches": results.matches,
                "truncated": results.truncated,
                "matches": hits,
            }))
        }
        "list_open_buffers" => {
            let buffers: Vec<Value> = app
                .editors
                .iter()
                .enumerate()
                .map(|(idx, ed)| {
                    let rel = ed
                        .doc
                        .path
                        .as_ref()
                        .map(|p| search::relative(&app.explorer.root, p));
                    json!({
                        "index": idx,
                        "active": idx == app.active,
                        "name": ed.doc.name(),
                        "path": rel,
                        "lines": ed.doc.len_lines(),
                        "dirty": ed.doc.is_dirty(),
                        "read_only": ed.doc.is_read_only(),
                        "language": ed.language(),
                    })
                })
                .collect();
            Ok(json!({ "buffers": buffers }))
        }
        "editor_outline" => {
            let text = if s("path").is_empty() {
                let ed = app
                    .editor()
                    .ok_or_else(|| "No active buffer in editor".to_string())?;
                ed.doc.rope.to_string()
            } else {
                let path = workspace_path(&app.explorer.root, s("path"))?;
                if let Some(ed) = app
                    .editors
                    .iter()
                    .find(|e| e.doc.path.as_ref() == Some(&path))
                {
                    ed.doc.rope.to_string()
                } else {
                    std::fs::read_to_string(&path).map_err(|e| e.to_string())?
                }
            };
            let keywords = [
                "pub fn ",
                "fn ",
                "pub struct ",
                "struct ",
                "pub enum ",
                "enum ",
                "pub trait ",
                "trait ",
                "impl ",
                "impl<",
                "pub mod ",
                "mod ",
                "class ",
                "interface ",
                "def ",
                "async def ",
                "function ",
                "export function ",
                "export class ",
                "export interface ",
                "export type ",
            ];
            let mut symbols = Vec::new();
            for (i, line) in text.lines().enumerate() {
                let trimmed = line.trim();
                if keywords.iter().any(|k| trimmed.starts_with(k)) && !trimmed.ends_with(';') {
                    let signature = trimmed.trim_end_matches('{').trim_end_matches(':').trim();
                    symbols.push(json!({
                        "line": i + 1,
                        "signature": signature.chars().take(160).collect::<String>(),
                    }));
                    if symbols.len() >= 300 {
                        break;
                    }
                }
            }
            Ok(json!({ "symbols": symbols }))
        }
        "project_search" => {
            let query_str = s("query");
            if query_str.is_empty() {
                return Err("project_search requires a non-empty query".into());
            }
            let query = search::Query {
                text: query_str.to_string(),
                case_sensitive: args
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                whole_word: args
                    .get("whole_word")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                regex: args.get("regex").and_then(Value::as_bool).unwrap_or(false),
            };
            let filter = search::FilterSet::parse_parts(s("dirs"), s("files"));
            let max_results = args
                .get("max_results")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(1, 500))
                .unwrap_or(100);
            let search_opts = search::SearchOptions {
                content: true,
                file_names: false,
                directory_names: false,
                ignore_binary: true,
                max_results,
            };
            let overrides = app
                .editors
                .iter()
                .filter_map(|e| {
                    (!e.doc.is_read_only())
                        .then_some(e.doc.path.as_ref()?)
                        .map(|p| (p.clone(), e.doc.rope.to_string()))
                })
                .collect();
            let results = search::search_with_options(
                &app.explorer.root,
                &query,
                &filter,
                &overrides,
                &search_opts,
            )
            .map_err(|e| e.to_string())?;
            let files: Vec<Value> = results
                .files
                .into_iter()
                .map(|f| {
                    json!({
                        "file": f.rel,
                        "matches": f.matches.into_iter().map(|m| json!({
                            "line": m.line + 1,
                            "col": m.start + 1,
                            "text": m.text,
                        })).collect::<Vec<_>>()
                    })
                })
                .collect();
            Ok(json!({
                "total_matches": results.matches,
                "scanned_files": results.scanned,
                "truncated": results.truncated,
                "files": files,
            }))
        }
        "git_status_summary" => {
            let output = std::process::Command::new("git")
                .args(["status", "--porcelain=v1", "-b"])
                .current_dir(&app.explorer.root)
                .output()
                .map_err(|e| format!("Failed to run git status: {e}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            Ok(json!({
                "branch": app.git_branch,
                "status": stdout.lines().take(200).collect::<Vec<_>>(),
            }))
        }
        "edit_buffer" => {
            if options.read_only {
                return Err("Agent is read-only".into());
            }
            let path = workspace_path(&app.explorer.root, s("path"))?;
            let old_text = args
                .get("old_text")
                .and_then(Value::as_str)
                .unwrap_or("");
            let new_text = args
                .get("new_text")
                .and_then(Value::as_str)
                .unwrap_or("");
            let save = args.get("save").and_then(Value::as_bool).unwrap_or(true);
            if old_text.is_empty() {
                return Err("old_text must not be empty".into());
            }

            if !app
                .editors
                .iter()
                .any(|e| e.doc.path.as_ref() == Some(&path))
            {
                app.open_file(&path);
            }
            let ed = app
                .editors
                .iter_mut()
                .find(|e| e.doc.path.as_ref() == Some(&path))
                .ok_or_else(|| "Could not open target buffer".to_string())?;
            if ed.doc.is_read_only() {
                return Err("Target buffer is read-only".into());
            }
            let current = ed.doc.rope.to_string();
            let patched = crate::aep::apply_patch_to_string(&current, old_text, new_text)?;
            ed.replace_document(&patched);
            if save {
                ed.doc.save().map_err(|e| e.to_string())?;
            }
            let (name, lines) = (ed.doc.name(), ed.doc.len_lines());
            app.toast(format!("✦ Toolkit edited {name}"), ToastKind::Success);
            Ok(json!({
                "path": search::relative(&app.explorer.root, &path),
                "saved": save,
                "lines": lines,
            }))
        }
        "open_at_line" => {
            let path = workspace_path(&app.explorer.root, s("path"))?;
            app.open_file(&path);
            let line = args
                .get("line")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .saturating_sub(1) as usize;
            let col = args
                .get("column")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .saturating_sub(1) as usize;
            if let Some(ed) = app.editor_mut() {
                let idx = ed.doc.idx(line, col);
                ed.sel = Selection::caret(idx);
                ed.center_on_cursor();
                ed.scroll_into_view();
            }
            Ok(json!({
                "opened": search::relative(&app.explorer.root, &path),
                "line": line + 1,
                "column": col + 1,
            }))
        }
        _ => Err(format!("Unknown toolkit tool: {name}")),
    }
}
