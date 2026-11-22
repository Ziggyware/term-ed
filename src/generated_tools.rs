//! Bounded, pure functions assembled by agents at runtime. This interpreter has
//! no filesystem, network, shell, eval, recursion or calls to other tools.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

const MAX_VALUE: usize = 64 * 1024;
const MAX_ITEMS: usize = 1024;
const MAX_FUNCTIONS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// JSON Pointer into the function arguments; empty selects the whole object.
    #[serde(default)]
    pub input: String,
    pub steps: Vec<Operation>,
}

fn default_join_separator() -> String {
    " ".into()
}

fn default_split_separator() -> String {
    " ".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Trim,
    Lowercase,
    Uppercase,
    Replace {
        from: String,
        #[serde(default)]
        to: String,
    },
    Split {
        #[serde(default = "default_split_separator")]
        separator: String,
    },
    Join {
        #[serde(default = "default_join_separator")]
        separator: String,
        #[serde(default)]
        items: Vec<Value>,
    },
    Template {
        template: String,
    },
    Prepend {
        value: String,
        #[serde(default)]
        separator: String,
    },
    Append {
        value: String,
        #[serde(default)]
        separator: String,
    },
    Lines,
    Unique,
    Sort,
    Length,
    JsonParse,
    JsonStringify,
    JsonPointer {
        pointer: String,
    },
    RegexExtract {
        pattern: String,
        #[serde(default)]
        group: usize,
    },
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn interpolate_template(template: &str, args: &Value, current: &Value) -> String {
    let resolve_key = |key: &str| -> Option<String> {
        let trimmed = key.trim().trim_start_matches('/');
        if matches!(trimmed, "value" | "input" | "0" | ".") {
            if let Some(s) = scalar_string(current) {
                return Some(s);
            }
            if let Some(arr) = current.as_array() {
                let parts: Vec<String> = arr.iter().filter_map(scalar_string).collect();
                return Some(parts.join(" "));
            }
        }
        let target = args
            .get(trimmed)
            .or_else(|| args.pointer(&format!("/{trimmed}")))
            .or_else(|| current.get(trimmed));
        match target {
            Some(v) => {
                if let Some(s) = scalar_string(v) {
                    Some(s)
                } else if let Some(arr) = v.as_array() {
                    let parts: Vec<String> = arr.iter().filter_map(scalar_string).collect();
                    Some(parts.join(" "))
                } else {
                    Some(v.to_string())
                }
            }
            None => None,
        }
    };

    let mut out = String::with_capacity(template.len());
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Handle `{{key}}`
        if chars[i] == '{' && chars.get(i + 1) == Some(&'{') {
            if let Some(end) = chars[i + 2..]
                .windows(2)
                .position(|w| w == ['}', '}'])
                .map(|p| i + 2 + p)
            {
                let key: String = chars[i + 2..end].iter().collect();
                if let Some(val) = resolve_key(&key) {
                    out.push_str(&val);
                    i = end + 2;
                    continue;
                }
            }
        }
        // Handle `${key}`
        if chars[i] == '$' && chars.get(i + 1) == Some(&'{') {
            if let Some(end) = chars[i + 2..].iter().position(|&c| c == '}').map(|p| i + 2 + p) {
                let key: String = chars[i + 2..end].iter().collect();
                if let Some(val) = resolve_key(&key) {
                    out.push_str(&val);
                    i = end + 1;
                    continue;
                }
            }
        }
        // Handle `{key}`
        if chars[i] == '{' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '}').map(|p| i + 1 + p) {
                let key: String = chars[i + 1..end].iter().collect();
                if !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'))
                    && let Some(val) = resolve_key(&key)
                {
                    out.push_str(&val);
                    i = end + 1;
                    continue;
                }
            }
        }
        // Handle `$key` when `key` is a known property on `args`
        if chars[i] == '$'
            && chars
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
        {
            let mut end = i + 1;
            while end < chars.len()
                && (chars[end].is_ascii_alphanumeric() || chars[end] == '_')
            {
                end += 1;
            }
            let key: String = chars[i + 1..end].iter().collect();
            if let Some(val) = resolve_key(&key) {
                out.push_str(&val);
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn clean_schema_value(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    for benign in ["title", "default", "examples", "$schema"] {
        obj.remove(benign);
    }
    if !obj.contains_key("type") {
        if obj.contains_key("properties") || obj.contains_key("required") {
            obj.insert("type".into(), Value::String("object".into()));
        } else if obj.contains_key("items") {
            obj.insert("type".into(), Value::String("array".into()));
        } else {
            obj.insert("type".into(), Value::String("string".into()));
        }
    }
    if obj.get("type").and_then(Value::as_str) == Some("array") && !obj.contains_key("items") {
        obj.insert("items".into(), json!({"type": "string"}));
    }
    if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
        for prop in props.values_mut() {
            clean_schema_value(prop);
        }
    }
    if let Some(items) = obj.get_mut("items") {
        clean_schema_value(items);
    }
}

fn normalize_step_value(step: Value) -> Value {
    if let Value::String(op_str) = step {
        let trimmed = op_str.trim().to_ascii_lowercase();
        return json!({ "op": trimmed });
    }
    let Value::Object(mut map) = step else {
        return step;
    };
    if !map.contains_key("op") {
        for alias in ["type", "action", "operation", "kind"] {
            if let Some(v) = map.remove(alias) {
                map.insert("op".into(), v);
                break;
            }
        }
    }
    if !map.contains_key("op") {
        let inferred = if map.contains_key("template") || map.contains_key("format") {
            Some("template")
        } else if map.contains_key("separator")
            || map.contains_key("sep")
            || map.contains_key("delimiter")
            || map.contains_key("items")
            || map.contains_key("args")
            || map.contains_key("tokens")
        {
            Some("join")
        } else if map.contains_key("from")
            || map.contains_key("old")
            || map.contains_key("replacement")
        {
            Some("replace")
        } else if map.contains_key("pattern") || map.contains_key("regex") {
            Some("regex_extract")
        } else if map.contains_key("pointer") || map.contains_key("field") || map.contains_key("path") {
            Some("json_pointer")
        } else if map.contains_key("prefix") {
            Some("prepend")
        } else if map.contains_key("suffix") {
            Some("append")
        } else if map.contains_key("value") || map.contains_key("token") {
            Some("template")
        } else {
            None
        };
        if let Some(op) = inferred {
            map.insert("op".into(), Value::String(op.into()));
        }
    }

    let Some(op_raw) = map.get("op").and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase()) else {
        return Value::Object(map);
    };
    let canonical_op = match op_raw.as_str() {
        "format" | "interpolate" | "render" | "sprintf" | "template" => "template",
        "concat" | "implode" | "join" => "join",
        "explode" | "split" => "split",
        "prefix" | "prepend" => "prepend",
        "suffix" | "push" | "add" | "append" => "append",
        "strip" | "trim" => "trim",
        "lower" | "to_lower" | "lowercase" => "lowercase",
        "upper" | "to_upper" | "uppercase" => "uppercase",
        "substitute" | "sub" | "replace" => "replace",
        "pointer" | "get" | "select" | "extract_field" | "json_pointer" => "json_pointer",
        "regex" | "match" | "regex_extract" => "regex_extract",
        "dedup" | "distinct" | "unique" => "unique",
        "count" | "len" | "length" => "length",
        "lines" => "lines",
        "sort" => "sort",
        "json_parse" | "parse_json" => "json_parse",
        "json_stringify" | "to_json" | "stringify" => "json_stringify",
        _ => return Value::Object(map),
    };

    let mut clean = serde_json::Map::new();
    clean.insert("op".into(), Value::String(canonical_op.into()));

    match canonical_op {
        "join" => {
            let default_sep = if op_raw == "concat" { "" } else { " " };
            let sep = ["separator", "sep", "delimiter", "by", "with"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
                .unwrap_or_else(|| default_sep.into());
            clean.insert("separator".into(), Value::String(sep));
            let items = ["items", "args", "values", "tokens", "parts", "elements"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_array).cloned())
                .or_else(|| {
                    ["value", "token", "arg", "item"]
                        .into_iter()
                        .find_map(|k| map.get(k).and_then(scalar_string))
                        .map(|s| vec![Value::String(s)])
                })
                .unwrap_or_default();
            if !items.is_empty() {
                clean.insert("items".into(), Value::Array(items));
            }
        }
        "split" => {
            let sep = ["separator", "sep", "delimiter", "by", "token", "value"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| " ".into());
            clean.insert("separator".into(), Value::String(sep));
        }
        "template" => {
            let tpl = ["template", "format", "value", "text", "token", "pattern"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
                .or_else(|| {
                    ["items", "args", "tokens", "values"]
                        .into_iter()
                        .find_map(|k| map.get(k).and_then(Value::as_array))
                        .map(|arr| {
                            arr.iter()
                                .filter_map(scalar_string)
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                })
                .unwrap_or_else(|| "{value}".into());
            clean.insert("template".into(), Value::String(tpl));
        }
        "prepend" | "append" => {
            let val = ["value", "prefix", "suffix", "token", "text", "item"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(scalar_string))
                .or_else(|| {
                    ["items", "args", "values", "tokens"]
                        .into_iter()
                        .find_map(|k| map.get(k).and_then(Value::as_array))
                        .map(|arr| {
                            arr.iter()
                                .filter_map(scalar_string)
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                })
                .unwrap_or_default();
            let sep = ["separator", "sep", "delimiter"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
                .unwrap_or_default();
            clean.insert("value".into(), Value::String(val));
            if !sep.is_empty() {
                clean.insert("separator".into(), Value::String(sep));
            }
        }
        "replace" => {
            if let Some(from) = ["from", "old", "search", "pattern", "target"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
            {
                clean.insert("from".into(), Value::String(from));
            }
            let to = ["to", "new", "replacement", "value", "with", "token"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
                .unwrap_or_default();
            clean.insert("to".into(), Value::String(to));
        }
        "json_pointer" => {
            if let Some(mut ptr) = ["pointer", "path", "field", "key", "value", "token"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
            {
                let trimmed = ptr.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('/') {
                    let stripped = trimmed.trim_start_matches("$.").trim_start_matches('$');
                    ptr = format!("/{}", stripped.replace('.', "/"));
                } else {
                    ptr = trimmed.to_string();
                }
                clean.insert("pointer".into(), Value::String(ptr));
            }
        }
        "regex_extract" => {
            if let Some(pat) = ["pattern", "regex", "expr", "value"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str).map(String::from))
            {
                clean.insert("pattern".into(), Value::String(pat));
            }
            if let Some(group) = map.get("group").and_then(Value::as_u64) {
                clean.insert("group".into(), Value::Number(group.into()));
            }
        }
        _ => {}
    }

    Value::Object(clean)
}

pub fn normalize_definition_value(mut value: Value) -> Value {
    let Some(obj) = value.as_object_mut() else {
        return value;
    };
    if let Some(raw_name) = obj.get("name").and_then(Value::as_str) {
        let sanitized: String = raw_name
            .trim()
            .chars()
            .map(|c| if c == '-' || c.is_ascii_whitespace() { '_' } else { c })
            .collect();
        if !sanitized.is_empty()
            && sanitized
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            let normalized = if sanitized.starts_with("fn_") {
                sanitized
            } else {
                format!("fn_{sanitized}")
            };
            obj.insert("name".into(), Value::String(normalized));
        }
    }
    if obj
        .get("description")
        .and_then(Value::as_str)
        .is_none_or(|s| s.trim().is_empty())
    {
        let fallback = obj
            .get("name")
            .and_then(Value::as_str)
            .map(|n| format!("Generated function {n}"))
            .unwrap_or_else(|| "Generated function".into());
        obj.insert("description".into(), Value::String(fallback));
    }
    if !obj.contains_key("input_schema") {
        for alias in ["parameters", "schema", "args_schema"] {
            if let Some(s) = obj.remove(alias) {
                obj.insert("input_schema".into(), s);
                break;
            }
        }
    }
    if !obj.contains_key("input_schema") {
        obj.insert(
            "input_schema".into(),
            json!({"type": "object", "properties": {}}),
        );
    }
    if let Some(schema) = obj.get_mut("input_schema") {
        clean_schema_value(schema);
    }
    if let Some(input) = obj.get("input").and_then(Value::as_str) {
        let trimmed = input.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('/') {
            let stripped = trimmed.trim_start_matches("$.").trim_start_matches('$');
            let pointer = format!("/{}", stripped.replace('.', "/"));
            obj.insert("input".into(), Value::String(pointer));
        }
    }
    if !obj.contains_key("steps") {
        for alias in ["operations", "pipeline"] {
            if let Some(s) = obj.remove(alias) {
                obj.insert("steps".into(), s);
                break;
            }
        }
    }
    if obj
        .get("steps")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        if let Some(tpl) = ["template", "format", "command"]
            .into_iter()
            .find_map(|k| obj.get(k).and_then(Value::as_str).map(String::from))
        {
            obj.insert(
                "steps".into(),
                json!([{ "op": "template", "template": tpl }]),
            );
        }
    }
    if let Some(steps) = obj.get_mut("steps").and_then(Value::as_array_mut) {
        for step in steps.iter_mut() {
            let current = std::mem::replace(step, Value::Null);
            *step = normalize_step_value(current);
        }
    }
    let mut cleaned = serde_json::Map::new();
    for key in ["name", "description", "input_schema", "input", "steps"] {
        if let Some(v) = obj.remove(key) {
            cleaned.insert(key.into(), v);
        }
    }
    Value::Object(cleaned)
}

fn bounded(value: Value) -> Result<Value, String> {
    if value.to_string().len() > MAX_VALUE {
        Err("Function value exceeds 64 KiB".into())
    } else {
        Ok(value)
    }
}
fn text(value: &Value) -> Result<&str, String> {
    if let Some(s) = value.as_str() {
        return Ok(s);
    }
    if let Some(obj) = value.as_object() {
        if obj.len() == 1
            && let Some(s) = obj.values().next().and_then(Value::as_str)
        {
            return Ok(s);
        }
        for key in ["text", "input", "value", "content", "source", "command", "payload"] {
            if let Some(s) = obj.get(key).and_then(Value::as_str) {
                return Ok(s);
            }
        }
    }
    Err("Operation requires a string".into())
}
fn array(value: &Value) -> Result<&Vec<Value>, String> {
    if let Some(arr) = value.as_array().filter(|v| v.len() <= MAX_ITEMS) {
        return Ok(arr);
    }
    if let Some(obj) = value.as_object() {
        if obj.len() == 1
            && let Some(arr) = obj
                .values()
                .next()
                .and_then(Value::as_array)
                .filter(|v| v.len() <= MAX_ITEMS)
        {
            return Ok(arr);
        }
        for key in ["args", "items", "values", "tokens", "lines", "list"] {
            if let Some(arr) = obj
                .get(key)
                .and_then(Value::as_array)
                .filter(|v| v.len() <= MAX_ITEMS)
            {
                return Ok(arr);
            }
        }
    }
    Err("Operation requires an array of at most 1024 items".into())
}
fn pointer_ok(pointer: &str) -> bool {
    pointer.len() <= 512 && (pointer.is_empty() || pointer.starts_with('/'))
}
fn regex(pattern: &str) -> Result<regex::Regex, String> {
    if pattern.len() > 1024 {
        return Err("Regex pattern exceeds 1024 bytes".into());
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(256 * 1024)
        .dfa_size_limit(256 * 1024)
        .build()
        .map_err(|e| format!("Invalid function regex: {e}"))
}

/// Deliberately small JSON Schema subset. Reject unsupported keywords instead
/// of advertising constraints the interpreter would not actually enforce.
fn check_schema(schema: &Value, depth: usize) -> Result<(), String> {
    if depth > 6 {
        return Err("Function schema is too deeply nested".into());
    }
    let object = schema.as_object().ok_or("Schema must be an object")?;
    if object.keys().any(|k| {
        !matches!(
            k.as_str(),
            "type"
                | "description"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "enum"
                | "minLength"
                | "maxLength"
                | "minItems"
                | "maxItems"
        )
    }) {
        return Err("Unsupported schema keyword (no $ref, patterns or combinators)".into());
    }
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Every schema needs a type")?;
    if !matches!(
        kind,
        "object" | "array" | "string" | "integer" | "number" | "boolean" | "null"
    ) {
        return Err("Unsupported schema type".into());
    }
    if let Some(v) = schema.get("description") {
        if v.as_str().is_none_or(|s| s.len() > 512) {
            return Err("Schema description exceeds 512 bytes".into());
        }
    }
    if let Some(v) = schema.get("additionalProperties") {
        if kind != "object" || !v.is_boolean() {
            return Err("additionalProperties must be boolean on an object".into());
        }
    }
    if let Some(props) = schema.get("properties") {
        if kind != "object" {
            return Err("properties requires an object schema".into());
        }
        let props = props
            .as_object()
            .filter(|p| p.len() <= 32)
            .ok_or("At most 32 object properties are supported")?;
        for (name, property) in props {
            if name.len() > 128 {
                return Err("Property name is too long".into());
            }
            check_schema(property, depth + 1)?;
        }
    }
    if let Some(required) = schema.get("required") {
        if kind != "object" {
            return Err("required requires an object schema".into());
        }
        let fields = required
            .as_array()
            .filter(|a| a.len() <= 32)
            .ok_or("Invalid required fields")?;
        for field in fields {
            let name = field.as_str().ok_or("Required fields must be strings")?;
            if schema.get("properties").and_then(|p| p.get(name)).is_none() {
                return Err("Required property must have a schema".into());
            }
        }
    }
    if let Some(items) = schema.get("items") {
        if kind != "array" {
            return Err("items requires an array schema".into());
        }
        check_schema(items, depth + 1)?;
    }
    if kind == "array" && schema.get("items").is_none() {
        return Err("Array schemas need items".into());
    }
    for (key, expected, cap) in [
        ("minLength", "string", MAX_VALUE),
        ("maxLength", "string", MAX_VALUE),
        ("minItems", "array", MAX_ITEMS),
        ("maxItems", "array", MAX_ITEMS),
    ] {
        if let Some(v) = schema.get(key) {
            if kind != expected || v.as_u64().is_none_or(|n| n > cap as u64) {
                return Err(format!("Invalid {key} constraint"));
            }
        }
    }
    if let Some(values) = schema.get("enum") {
        if values.as_array().is_none_or(|a| {
            a.is_empty() || a.len() > 32 || a.iter().any(|v| v.is_array() || v.is_object())
        }) {
            return Err("enum must contain 1–32 scalar values".into());
        }
    }
    Ok(())
}

fn validate_input(value: &Value, schema: &Value) -> Result<(), String> {
    let valid = match schema["type"].as_str().unwrap_or("") {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if !valid {
        return Err("Function arguments do not match the declared schema type".into());
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.contains(value) {
            return Err("Function argument is outside its enum".into());
        }
    }
    if let Some(object) = value.as_object() {
        for field in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !object.contains_key(field.as_str().unwrap_or("")) {
                return Err(format!("Missing required argument {field}"));
            }
        }
        for (key, value) in object {
            if let Some(property) = schema.get("properties").and_then(|p| p.get(key)) {
                validate_input(value, property)?;
            } else if schema.get("additionalProperties").and_then(Value::as_bool) != Some(true) {
                return Err(format!("Unknown function argument {key}"));
            }
        }
    }
    if let Some(items) = value.as_array() {
        if items.len() > MAX_ITEMS {
            return Err("Function array exceeds 1024 items".into());
        }
        for item in items {
            validate_input(item, &schema["items"])?;
        }
    }
    for (min, max, length) in [
        (
            "minLength",
            "maxLength",
            value.as_str().map(|s| s.chars().count()),
        ),
        ("minItems", "maxItems", value.as_array().map(Vec::len)),
    ] {
        if let Some(length) = length {
            if schema
                .get(min)
                .and_then(Value::as_u64)
                .is_some_and(|n| length < n as usize)
                || schema
                    .get(max)
                    .and_then(Value::as_u64)
                    .is_some_and(|n| length > n as usize)
            {
                return Err("Function argument violates a length constraint".into());
            }
        }
    }
    Ok(())
}

impl Definition {
    fn validate(&self) -> Result<(), String> {
        if !self.name.starts_with("fn_")
            || !(4..=48).contains(&self.name.len())
            || !self
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err("Function names must start with fn_ and contain 4–48 ASCII letters, digits or underscores".into());
        }
        if self.description.trim().is_empty()
            || self.description.len() > 512
            || self.steps.is_empty()
            || self.steps.len() > 12
            || !pointer_ok(&self.input)
        {
            return Err("Supply a description, JSON input pointer and 1–12 operations".into());
        }
        if serde_json::to_vec(self)
            .map_err(|_| "Invalid definition")?
            .len()
            > 16 * 1024
        {
            return Err("Function definition exceeds 16 KiB".into());
        }
        check_schema(&self.input_schema, 0)?;
        if self.input_schema["type"] != "object" {
            return Err("Top-level function input must be an object".into());
        }
        for op in &self.steps {
            match op {
                Operation::Replace { from, to }
                    if from.is_empty() || from.len() > 512 || to.len() > 512 =>
                {
                    return Err(
                        "replace requires nonempty from and strings of at most 512 bytes".into(),
                    );
                }
                Operation::Split { separator } if separator.is_empty() || separator.len() > 128 => {
                    return Err("split separator must contain 1–128 bytes".into());
                }
                Operation::Join { separator, items }
                    if separator.len() > 128 || items.len() > MAX_ITEMS =>
                {
                    return Err("join separator exceeds 128 bytes or items exceed 1024".into());
                }
                Operation::Template { template } if template.len() > 4096 => {
                    return Err("template exceeds 4096 bytes".into());
                }
                Operation::Prepend { value, separator }
                | Operation::Append { value, separator }
                    if value.len() > 4096 || separator.len() > 128 =>
                {
                    return Err("prepend/append value exceeds 4096 bytes".into());
                }
                Operation::JsonPointer { pointer } if !pointer_ok(pointer) => {
                    return Err("Invalid JSON pointer".into());
                }
                Operation::RegexExtract { pattern, group } => {
                    let r = regex(pattern)?;
                    if *group >= r.captures_len() {
                        return Err("Regex group does not exist".into());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn schema(&self) -> Value {
        json!({"type":"function","function":{"name":self.name,"description":self.description,"parameters":self.input_schema}})
    }
    fn execute(&self, args: &Value) -> Result<Value, String> {
        bounded(args.clone())?;
        validate_input(args, &self.input_schema)?;
        let mut value = args
            .pointer(&self.input)
            .cloned()
            .or_else(|| {
                if !self.input.is_empty() {
                    args.get(self.input.trim_start_matches('/')).cloned()
                } else {
                    None
                }
            })
            .ok_or("Input JSON pointer was not found")?;
        for op in &self.steps {
            value = bounded(match op {
                Operation::Trim => json!(text(&value)?.trim()),
                Operation::Lowercase => json!(text(&value)?.to_lowercase()),
                Operation::Uppercase => json!(text(&value)?.to_uppercase()),
                Operation::Replace { from, to } => {
                    let s = text(&value)?.to_string();
                    let from_resolved = interpolate_template(from, args, &value);
                    let to_resolved = interpolate_template(to, args, &value);
                    if from_resolved.is_empty() {
                        return Err("replace requires nonempty from".into());
                    }
                    let count = s.matches(&from_resolved).count();
                    let size = s
                        .len()
                        .saturating_sub(count * from_resolved.len())
                        .saturating_add(count.saturating_mul(to_resolved.len()));
                    if size > MAX_VALUE {
                        return Err("Replacement would exceed 64 KiB".into());
                    }
                    json!(s.replace(&from_resolved, &to_resolved))
                }
                Operation::Split { separator } => {
                    let parts: Vec<_> =
                        text(&value)?.split(separator).take(MAX_ITEMS + 1).collect();
                    if parts.len() > MAX_ITEMS {
                        return Err("Split exceeds 1024 items".into());
                    }
                    json!(parts)
                }
                Operation::Lines => {
                    let parts: Vec<_> = text(&value)?.lines().take(MAX_ITEMS + 1).collect();
                    if parts.len() > MAX_ITEMS {
                        return Err("Lines exceed 1024 items".into());
                    }
                    json!(parts)
                }
                Operation::Join { separator, items } => {
                    let resolved: Vec<String> = if !items.is_empty() {
                        let mut out = Vec::with_capacity(items.len());
                        for item in items {
                            if let Some(s) = item.as_str() {
                                if s.starts_with('/')
                                    && let Some(ptr_val) =
                                        args.pointer(s).or_else(|| value.pointer(s))
                                {
                                    if let Some(arr) = ptr_val.as_array() {
                                        for elem in arr {
                                            if let Some(sc) = scalar_string(elem) {
                                                out.push(sc);
                                            }
                                        }
                                        continue;
                                    } else if let Some(sc) = scalar_string(ptr_val) {
                                        out.push(sc);
                                        continue;
                                    }
                                }
                                out.push(interpolate_template(s, args, &value));
                            } else if let Some(sc) = scalar_string(item) {
                                out.push(sc);
                            }
                        }
                        out
                    } else if let Ok(arr) = array(&value) {
                        arr.iter()
                            .map(|v| {
                                scalar_string(v)
                                    .ok_or_else(|| "Operation requires a string".to_string())
                            })
                            .collect::<Result<Vec<_>, _>>()?
                    } else if let Some(s) = value.as_str() {
                        vec![s.to_string()]
                    } else if let Some(obj) = value.as_object() {
                        let mut vals = Vec::new();
                        for v in obj.values() {
                            if let Some(arr) = v.as_array() {
                                for elem in arr {
                                    if let Some(sc) = scalar_string(elem) {
                                        vals.push(sc);
                                    }
                                }
                            } else if let Some(sc) = scalar_string(v) {
                                vals.push(sc);
                            }
                        }
                        vals
                    } else {
                        return Err("Operation requires an array of at most 1024 items".into());
                    };
                    let size = resolved.iter().map(|v| v.len()).sum::<usize>().saturating_add(
                        separator
                            .len()
                            .saturating_mul(resolved.len().saturating_sub(1)),
                    );
                    if size > MAX_VALUE {
                        return Err("Joined value exceeds 64 KiB".into());
                    }
                    json!(resolved.join(separator))
                }
                Operation::Template { template } => {
                    let rendered = interpolate_template(template, args, &value);
                    if rendered.len() > MAX_VALUE {
                        return Err("Template output exceeds 64 KiB".into());
                    }
                    json!(rendered)
                }
                Operation::Prepend {
                    value: prefix,
                    separator,
                } => {
                    let rendered = interpolate_template(prefix, args, &value);
                    if let Some(arr) = value.as_array() {
                        let mut next = Vec::with_capacity(arr.len() + 1);
                        next.push(Value::String(rendered));
                        next.extend(arr.iter().cloned());
                        json!(next)
                    } else {
                        let cur = text(&value).unwrap_or("");
                        let joined = if cur.is_empty() {
                            rendered
                        } else {
                            format!("{rendered}{separator}{cur}")
                        };
                        json!(joined)
                    }
                }
                Operation::Append {
                    value: suffix,
                    separator,
                } => {
                    let rendered = interpolate_template(suffix, args, &value);
                    if let Some(arr) = value.as_array() {
                        let mut next = arr.clone();
                        next.push(Value::String(rendered));
                        json!(next)
                    } else {
                        let cur = text(&value).unwrap_or("");
                        let joined = if cur.is_empty() {
                            rendered
                        } else {
                            format!("{cur}{separator}{rendered}")
                        };
                        json!(joined)
                    }
                }
                Operation::Unique => {
                    let mut seen = HashSet::new();
                    json!(
                        array(&value)?
                            .iter()
                            .filter(|v| seen.insert(v.to_string()))
                            .cloned()
                            .collect::<Vec<_>>()
                    )
                }
                Operation::Sort => {
                    let mut items: Vec<_> = array(&value)?
                        .iter()
                        .map(|v| text(v).map(str::to_string))
                        .collect::<Result<_, _>>()?;
                    items.sort();
                    json!(items)
                }
                Operation::Length => json!(match &value {
                    Value::String(s) => s.chars().count(),
                    Value::Array(a) => a.len(),
                    Value::Object(o) => o.len(),
                    _ => return Err("length requires text, array or object".into()),
                }),
                Operation::JsonParse => {
                    serde_json::from_str(text(&value)?).map_err(|_| "Invalid JSON input")?
                }
                Operation::JsonStringify => json!(value.to_string()),
                Operation::JsonPointer { pointer } => value
                    .pointer(pointer)
                    .cloned()
                    .or_else(|| args.pointer(pointer).cloned())
                    .ok_or("JSON pointer was not found")?,
                Operation::RegexExtract { pattern, group } => {
                    let r = regex(pattern)?;
                    let found: Vec<_> = r
                        .captures_iter(text(&value)?)
                        .take(257)
                        .filter_map(|c| c.get(*group).map(|m| m.as_str().to_string()))
                        .collect();
                    if found.len() > 256 {
                        return Err("Regex extraction exceeds 256 matches".into());
                    }
                    json!(found)
                }
            })?;
        }
        Ok(value)
    }
}

#[derive(Clone, Default)]
pub struct Registry {
    definitions: BTreeMap<String, Definition>,
}
impl Registry {
    pub fn define(&mut self, value: Value) -> Result<Value, String> {
        self.define_inner(value, false)
    }
    pub fn define_or_replace(&mut self, value: Value) -> Result<Value, String> {
        self.define_inner(value, true)
    }
    fn define_inner(&mut self, value: Value, allow_identical_replace: bool) -> Result<Value, String> {
        if value.to_string().len() > 16 * 1024 {
            return Err("Function definition exceeds 16 KiB".into());
        }
        let normalized = normalize_definition_value(value);
        let definition: Definition =
            serde_json::from_value(normalized).map_err(|e| format!("Invalid pure function: {e}"))?;
        definition.validate()?;
        if let Some(existing) = self.definitions.get(&definition.name) {
            if !allow_identical_replace && existing == &definition {
                return Err("Function already exists; choose a new name".into());
            }
        } else if self.definitions.len() >= MAX_FUNCTIONS {
            return Err("Session limit of 16 generated functions reached".into());
        }
        let result = json!({"name":definition.name,"dynamic_function":definition.schema(),"scope":"this task / user turn only","execution":"pure bounded data transformations; no local code execution"});
        self.definitions.insert(definition.name.clone(), definition);
        Ok(result)
    }
    pub fn contains(&self, name: &str) -> bool {
        self.definitions.contains_key(name)
            || (!name.starts_with("fn_") && self.definitions.contains_key(&format!("fn_{name}")))
    }
    pub fn schemas(&self) -> Vec<Value> {
        self.definitions.values().map(Definition::schema).collect()
    }
    pub fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        let def = self
            .definitions
            .get(name)
            .or_else(|| {
                (!name.starts_with("fn_"))
                    .then(|| self.definitions.get(&format!("fn_{name}")))
                    .flatten()
            })
            .ok_or("Function was not defined in this task")?;
        def.execute(args)
    }
}

pub fn definition_properties() -> Value {
    json!({"name":{"type":"string","description":"Function name (auto-prefixed with fn_ if omitted, 4–48 ASCII letters, digits or underscores)"},"description":{"type":"string"},
        "input_schema":{"type":"object","description":"JSON Schema subset: typed objects/properties/required, arrays/items, scalar types/enum and length limits; no $ref or combinators. Unknown object properties are rejected by default."},
        "input":{"type":"string","description":"JSON Pointer into arguments, e.g. /text or /args; empty = entire object"},
        "template":{"type":"string","description":"Optional shorthand template string with {field} placeholders (used when steps is omitted)"},
        "steps":{"type":"array","minItems":1,"maxItems":12,"items":{"type":"object","properties":{"op":{"type":"string","description":"trim, lowercase, uppercase, template, replace, split, join, prepend, append, lines, unique, sort, length, json_parse, json_stringify, json_pointer, regex_extract"},"template":{"type":"string","description":"For template op: format string with {arg} placeholders"},"separator":{"type":"string","description":"For join/split/prepend/append ops (defaults to space)"},"items":{"type":"array","items":{"type":"string"},"description":"For join op: optional token/template list to join"},"value":{"type":"string","description":"For prepend/append/replace ops"},"from":{"type":"string"},"to":{"type":"string"},"pointer":{"type":"string"},"pattern":{"type":"string"},"group":{"type":"integer"}}},"description":"Operations: template(template), join(separator=' ', items=[...]), split(separator=' '), replace(from,to), prepend(value,separator), append(value,separator), trim, lowercase, uppercase, lines, unique, sort, length, json_parse, json_stringify, json_pointer(pointer), regex_extract(pattern,group=0)."}})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition() -> Value {
        json!({"name":"fn_names","description":"Normalize unique names","input_schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},"input":"/text","steps":[{"op":"trim"},{"op":"lowercase"},{"op":"lines"},{"op":"unique"},{"op":"sort"},{"op":"join","separator":", "}]})
    }
    #[test]
    fn runtime_function_has_schema_and_executes_validated_pipeline() {
        let mut registry = Registry::default();
        let registered = registry.define(definition()).unwrap();
        assert_eq!(
            registered["dynamic_function"]["function"]["name"],
            "fn_names"
        );
        assert_eq!(
            registry
                .call("fn_names", &json!({"text":" Bob\nalice\nbob "}))
                .unwrap(),
            "alice, bob"
        );
        assert!(registry.call("fn_names", &json!({"text":7})).is_err());
        assert!(
            registry
                .call("fn_names", &json!({"text":"ok","extra":"no"}))
                .is_err()
        );
        assert!(registry.define(definition()).is_err());
        assert!(registry.call("fn_other", &json!({})).is_err());
    }
    #[test]
    fn generated_code_unsupported_schemas_and_resource_exhaustion_are_refused() {
        let mut r = Registry::default();
        let mut d = definition();
        d["steps"] = json!([{"op":"shell","command":"echo unsafe"}]);
        assert!(r.define(d).is_err());
        let mut d = definition();
        d["input_schema"]["$ref"] = json!("file:///etc/passwd");
        assert!(r.define(d).is_err());
        let mut d = definition();
        d["steps"] = json!([{"op":"regex_extract","pattern":"(a)\\1"}]);
        assert!(r.define(d).is_err());
        let mut d = definition();
        d["steps"] = json!([{"op":"replace","from":"a","to":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}]);
        r.define(d).unwrap();
        assert!(
            r.call("fn_names", &json!({"text":"a".repeat(3000)}))
                .is_err()
        );
        assert!(
            r.call("fn_names", &json!({"text":"x".repeat(MAX_VALUE+1)}))
                .is_err()
        );
    }
    #[test]
    fn json_and_regex_functions_are_pure_and_bounded() {
        let mut d = definition();
        d["steps"] = json!([{"op":"json_parse"},{"op":"json_pointer","pointer":"/source"},{"op":"regex_extract","pattern":"([a-z]+)=","group":1}]);
        let mut r = Registry::default();
        r.define(d).unwrap();
        assert_eq!(
            r.call(
                "fn_names",
                &json!({"text":"{\"source\":\"alpha=1 beta=2\"}"})
            )
            .unwrap(),
            json!(["alpha", "beta"])
        );
    }
    #[test]
    fn llm_shaped_definitions_and_templates_succeed() {
        let mut r = Registry::default();
        // Missing fn_ prefix, omitted input pointer, and join with items/args aliases.
        let join_def = json!({
            "name": "ffmpeg-join-args",
            "description": "Join CLI arguments",
            "input_schema": {
                "properties": {
                    "args": { "type": "array" }
                },
                "required": ["args"]
            },
            "steps": [
                { "op": "join", "separator": " " }
            ]
        });
        let reg = r.define(join_def).unwrap();
        assert_eq!(reg["name"], "fn_ffmpeg_join_args");
        assert_eq!(
            r.call(
                "ffmpeg_join_args",
                &json!({"args": ["ffmpeg", "-y", "-i", "in.mp3", "out.mp3"]})
            )
            .unwrap(),
            json!("ffmpeg -y -i in.mp3 out.mp3")
        );

        // Template operation with {field} interpolation and join with items array.
        let trim_def = json!({
            "name": "ffmpeg_trim",
            "description": "Build ffmpeg trim command",
            "input_schema": {
                "type": "object",
                "properties": {
                    "input": { "type": "string" },
                    "output": { "type": "string" },
                    "start": { "type": "number" },
                    "duration": { "type": "number" }
                },
                "required": ["input", "output", "start", "duration"]
            },
            "steps": [
                {
                    "op": "join",
                    "items": ["ffmpeg", "-y", "-ss", "{start}", "-t", "{duration}", "-i", "\"{input}\"", "-c", "copy", "\"{output}\""]
                }
            ]
        });
        r.define_or_replace(trim_def).unwrap();
        assert_eq!(
            r.call(
                "fn_ffmpeg_trim",
                &json!({"input": "song.mp3", "output": "hook.mp3", "start": 30, "duration": 5})
            )
            .unwrap(),
            json!("ffmpeg -y -ss 30 -t 5 -i \"song.mp3\" -c copy \"hook.mp3\"")
        );
    }
}
