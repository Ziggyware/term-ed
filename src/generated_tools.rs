//! Bounded, pure functions assembled by agents at runtime. This interpreter has
//! no filesystem, network, shell, eval, recursion or calls to other tools.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

const MAX_VALUE: usize = 64 * 1024;
const MAX_ITEMS: usize = 1024;
const MAX_FUNCTIONS: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Trim,
    Lowercase,
    Uppercase,
    Replace {
        from: String,
        to: String,
    },
    Split {
        separator: String,
    },
    Join {
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

fn bounded(value: Value) -> Result<Value, String> {
    if value.to_string().len() > MAX_VALUE {
        Err("Function value exceeds 64 KiB".into())
    } else {
        Ok(value)
    }
}
fn text(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .ok_or_else(|| "Operation requires a string".into())
}
fn array(value: &Value) -> Result<&Vec<Value>, String> {
    value
        .as_array()
        .filter(|v| v.len() <= MAX_ITEMS)
        .ok_or_else(|| "Operation requires an array of at most 1024 items".into())
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
                Operation::Join { separator } if separator.len() > 128 => {
                    return Err("join separator exceeds 128 bytes".into());
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
            .ok_or("Input JSON pointer was not found")?;
        for op in &self.steps {
            value = bounded(match op {
                Operation::Trim => json!(text(&value)?.trim()),
                Operation::Lowercase => json!(text(&value)?.to_lowercase()),
                Operation::Uppercase => json!(text(&value)?.to_uppercase()),
                Operation::Replace { from, to } => {
                    let s = text(&value)?;
                    let count = s.matches(from).count();
                    let size = s
                        .len()
                        .saturating_sub(count * from.len())
                        .saturating_add(count.saturating_mul(to.len()));
                    if size > MAX_VALUE {
                        return Err("Replacement would exceed 64 KiB".into());
                    }
                    json!(s.replace(from, to))
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
                Operation::Join { separator } => {
                    let items = array(&value)?;
                    let items: Vec<_> = items.iter().map(|v| text(v)).collect::<Result<_, _>>()?;
                    let size = items.iter().map(|v| v.len()).sum::<usize>().saturating_add(
                        separator
                            .len()
                            .saturating_mul(items.len().saturating_sub(1)),
                    );
                    if size > MAX_VALUE {
                        return Err("Joined value exceeds 64 KiB".into());
                    }
                    json!(items.join(separator))
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
        if value.to_string().len() > 16 * 1024 {
            return Err("Function definition exceeds 16 KiB".into());
        }
        let definition: Definition =
            serde_json::from_value(value).map_err(|e| format!("Invalid pure function: {e}"))?;
        definition.validate()?;
        if self.definitions.contains_key(&definition.name) {
            return Err("Function already exists; choose a new name".into());
        }
        if self.definitions.len() >= MAX_FUNCTIONS {
            return Err("Session limit of 16 generated functions reached".into());
        }
        let result = json!({"name":definition.name,"dynamic_function":definition.schema(),"scope":"this task / user turn only","execution":"pure bounded data transformations; no local code execution"});
        self.definitions.insert(definition.name.clone(), definition);
        Ok(result)
    }
    pub fn contains(&self, name: &str) -> bool {
        self.definitions.contains_key(name)
    }
    pub fn schemas(&self) -> Vec<Value> {
        self.definitions.values().map(Definition::schema).collect()
    }
    pub fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.definitions
            .get(name)
            .ok_or("Function was not defined in this task")?
            .execute(args)
    }
}

pub fn definition_properties() -> Value {
    json!({"name":{"type":"string","description":"Unique fn_ name, 4–48 ASCII characters"},"description":{"type":"string"},
        "input_schema":{"type":"object","description":"JSON Schema subset: typed objects/properties/required, arrays/items, scalar types/enum and length limits; no $ref or combinators. Unknown object properties are rejected by default."},
        "input":{"type":"string","description":"JSON Pointer into arguments, e.g. /text; empty = entire object"},
        "steps":{"type":"array","minItems":1,"maxItems":12,"items":{"type":"object"},"description":"Operations: trim, lowercase, uppercase, replace(from,to), split(separator), join(separator), lines, unique, sort, length, json_parse, json_stringify, json_pointer(pointer), regex_extract(pattern,group=0). Each object has op. No code, loops, I/O or tool calls."}})
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
}
