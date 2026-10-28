//! Bounded OpenAI-compatible response assembly for the built-in agent.
//!
//! NoKey Router forwards tool calls from multiple upstream providers. Some
//! return complete, unindexed calls (and occasionally object arguments), while
//! conventional streaming providers return indexed string fragments.

use std::collections::HashSet;

use serde_json::Value;

pub(crate) type ToolCall = (String, String, String);

#[derive(Debug)]
pub(crate) struct ResponseError {
    pub message: String,
    pub status: u16,
}

impl ResponseError {
    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: 502,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ResponseUpdate {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub used: Option<u64>,
}

#[derive(Default)]
pub(crate) struct ModelResponse {
    content: String,
    // None is an unused indexed slot; Some also records an otherwise-empty
    // fragment, so a malformed call cannot disappear at the end of a stream.
    calls: Vec<Option<ToolCall>>,
    finished: bool,
}

impl ModelResponse {
    /// Consume an SSE event or an entire non-streaming chat completion.
    pub fn push(&mut self, value: &Value) -> Result<ResponseUpdate, ResponseError> {
        if let Some(error) = value.get("error") {
            return Err(ResponseError::bad_gateway(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Model error"),
            ));
        }
        let mut update = ResponseUpdate {
            used: value
                .get("usage")
                .filter(|usage| !usage.is_null())
                .map(|usage| {
                    usage
                        .get("total_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                }),
            ..Default::default()
        };
        let Some(choice) = value.pointer("/choices/0") else {
            return Ok(update);
        };
        // A valid finish event need not include a delta at all.
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            if reason == "length" || reason == "content_filter" {
                return Err(ResponseError {
                    message: format!(
                        "Model stopped with {reason}; partial tool calls were not executed. Adjust the output budget or task."
                    ),
                    status: 413,
                });
            }
            self.finished = true;
        }
        let delta = choice
            .get("delta")
            .filter(|v| v.is_object())
            .or_else(|| choice.get("message").filter(|v| v.is_object()));
        let Some(delta) = delta else {
            return Ok(update);
        };
        update.reasoning = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(content) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.content.push_str(content);
            update.content = Some(content.to_string());
        }
        merge_tool_calls(&mut self.calls, delta).map_err(ResponseError::bad_gateway)?;
        Ok(update)
    }

    pub fn done(&mut self) {
        self.finished = true;
    }

    pub fn finish(self) -> Result<(String, Vec<ToolCall>), ResponseError> {
        if !self.finished {
            return Err(ResponseError::bad_gateway(
                "Model stream ended before a completion marker; no partial tool calls were executed",
            ));
        }
        let calls: Vec<_> = self.calls.into_iter().flatten().collect();
        if calls.iter().any(|call| call.1.is_empty()) {
            return Err(ResponseError::bad_gateway(
                "Model returned a tool call without a function name; no tool calls were executed",
            ));
        }
        if calls.is_empty() && self.content.trim().is_empty() {
            return Err(ResponseError::bad_gateway(
                "Model returned neither an answer nor tool calls",
            ));
        }
        Ok((self.content, calls))
    }
}

fn merge_tool_calls(calls: &mut Vec<Option<ToolCall>>, delta: &Value) -> Result<(), String> {
    let Some(tool_calls) = delta.get("tool_calls").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    let tool_calls = tool_calls
        .as_array()
        .ok_or("Model tool calls must be an array")?;
    let mut assigned = HashSet::new();
    for (position, call) in tool_calls.iter().enumerate() {
        if !call.is_object() {
            return Err("Model tool call must be an object".into());
        }
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let index = match call.get("index") {
            Some(v) => {
                let index = v
                    .as_u64()
                    .ok_or("Tool call index must be a nonnegative integer")?;
                if index >= 16 {
                    return Err("Model returned too many tool calls (limit 16)".into());
                }
                index as usize
            }
            None => {
                // Unindexed complete calls can arrive in separate events. Match
                // repeated IDs, otherwise append a new ID instead of merging it
                // into the first function. ID-less fragments use their position.
                id.and_then(|id| {
                    calls.iter().enumerate().find_map(|(index, call)| {
                        (!assigned.contains(&index)
                            && call.as_ref().is_some_and(|call| call.0 == id))
                        .then_some(index)
                    })
                })
                .unwrap_or_else(|| {
                    if assigned.contains(&position) {
                        return calls.len();
                    }
                    match (id, calls.get(position).and_then(Option::as_ref)) {
                        (Some(_), Some(existing)) if !existing.0.is_empty() => calls.len(),
                        _ => position,
                    }
                })
            }
        };
        if index >= 16 {
            return Err("Model returned too many tool calls (limit 16)".into());
        }
        if !assigned.insert(index) {
            return Err("Model returned duplicate tool call indexes in one event".into());
        }
        while calls.len() <= index {
            calls.push(None);
        }
        let target = calls[index].get_or_insert_with(Default::default);
        if let Some(id) = id {
            if id.len() > 128 {
                return Err("Tool call ID is too long".into());
            }
            if !target.0.is_empty() && target.0 != id {
                return Err("Model changed the ID of a tool call while streaming".into());
            }
            target.0 = id.into();
        }
        if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
            target.1.push_str(name);
        }
        if let Some(args) = call.pointer("/function/arguments") {
            match args {
                Value::String(fragment) => target.2.push_str(fragment),
                Value::Object(_) if target.2.is_empty() => {
                    target.2 = args.to_string();
                }
                Value::Null => {}
                _ => return Err("Model returned invalid tool call arguments".into()),
            }
        }
        if target.1.len() > 128 || target.2.len() > 64 * 1024 {
            return Err("Model tool call exceeds its size limit".into());
        }
    }
    Ok(())
}

/// Supply unique IDs for upstreams that omit or reuse them. Keep the set for
/// the session rather than deriving it from history, which can be compacted.
pub(crate) fn assign_tool_call_ids(
    calls: &mut [ToolCall],
    used: &mut HashSet<String>,
    counter: &mut u64,
) {
    for call in calls {
        if call.0.is_empty() || !used.insert(call.0.clone()) {
            loop {
                *counter = counter.wrapping_add(1);
                let id = format!("termed_call_{counter}");
                if used.insert(id.clone()) {
                    call.0 = id;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn router_complete_calls_and_indexed_fragments_are_kept_separate() {
        let mut calls = Vec::new();
        merge_tool_calls(
            &mut calls,
            &json!({"tool_calls":[
                {"id":"a","function":{"name":"read_file","arguments":{"path":"a"}}},
                {"id":"b","function":{"name":"search","arguments":"{\"pattern\":\"b\"}"}}
            ]}),
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].as_ref().unwrap().1, "read_file");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].as_ref().unwrap().2).unwrap(),
            json!({"path":"a"})
        );
        assert_eq!(calls[1].as_ref().unwrap().1, "search");

        let mut fragments = Vec::new();
        merge_tool_calls(&mut fragments, &json!({"tool_calls":[{"index":0,"id":"c","function":{"name":"read_","arguments":"{\"pa"}}]})).unwrap();
        merge_tool_calls(&mut fragments, &json!({"tool_calls":[{"index":0,"function":{"name":"file","arguments":"th\":\"a\"}"}}]})).unwrap();
        assert_eq!(fragments[0].as_ref().unwrap().1, "read_file");
        assert_eq!(
            serde_json::from_str::<Value>(&fragments[0].as_ref().unwrap().2).unwrap(),
            json!({"path":"a"})
        );
        assert!(
            merge_tool_calls(
                &mut fragments,
                &json!({"tool_calls":[{"index":1_000_000_000}]})
            )
            .is_err()
        );
        assert!(merge_tool_calls(&mut fragments, &json!({"tool_calls":[{"index":-1}]})).is_err());
        assert!(
            merge_tool_calls(
                &mut fragments,
                &json!({"tool_calls":[{"index":0,"id":"different"}]})
            )
            .is_err()
        );
        assert!(
            merge_tool_calls(
                &mut fragments,
                &json!({"tool_calls":[{"index":0,"function":{"arguments":[]}}]})
            )
            .is_err()
        );
    }

    #[test]
    fn unindexed_calls_in_separate_events_do_not_merge() {
        let mut calls = Vec::new();
        merge_tool_calls(&mut calls, &json!({"tool_calls":[{"id":"first","function":{"name":"read_file","arguments":"{}"}}]})).unwrap();
        merge_tool_calls(
            &mut calls,
            &json!({"tool_calls":[{"id":"second","function":{"name":"search","arguments":"{}"}}]}),
        )
        .unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|c| c.as_ref().unwrap().1.as_str())
                .collect::<Vec<_>>(),
            vec!["read_file", "search"]
        );

        // Reused IDs in one event still represent separate calls.
        let mut calls = Vec::new();
        merge_tool_calls(
            &mut calls,
            &json!({"tool_calls":[
                {"id":"same","function":{"name":"read_file","arguments":"{}"}},
                {"id":"same","function":{"name":"search","arguments":"{}"}}
            ]}),
        )
        .unwrap();
        let mut calls: Vec<_> = calls.into_iter().flatten().collect();
        let mut used = HashSet::new();
        let mut counter = 0;
        assign_tool_call_ids(&mut calls, &mut used, &mut counter);
        assert_eq!(calls[0].1, "read_file");
        assert_eq!(calls[1].1, "search");
        assert_ne!(calls[0].0, calls[1].0);

        let mut calls = Vec::new();
        assert!(
            merge_tool_calls(
                &mut calls,
                &json!({"tool_calls":[
                    {"index":0}, {"index":0}
                ]})
            )
            .is_err()
        );
    }

    #[test]
    fn generated_and_reused_tool_ids_stay_unique_across_rounds() {
        let mut used = HashSet::from(["termed_call_1".to_owned(), "reused".to_owned()]);
        let mut calls = vec![
            (String::new(), "read_file".into(), "{}".into()),
            ("reused".into(), "search".into(), "{}".into()),
            ("fresh".into(), "search".into(), "{}".into()),
        ];
        let mut counter = 0;
        assign_tool_call_ids(&mut calls, &mut used, &mut counter);
        assert_eq!(calls[0].0, "termed_call_2");
        assert_eq!(calls[1].0, "termed_call_3");
        assert_eq!(calls[2].0, "fresh");

        // IDs remain reserved even after the corresponding history is compacted.
        let mut later = vec![("fresh".into(), "search".into(), "{}".into())];
        assign_tool_call_ids(&mut later, &mut used, &mut counter);
        assert_eq!(later[0].0, "termed_call_4");
    }

    #[test]
    fn accepts_complete_json_and_finish_events_without_a_delta() {
        let mut response = ModelResponse::default();
        let update = response.push(&json!({"choices":[{"message":{"content":null,"tool_calls":[{"function":{"name":"read_file","arguments":{"path":"notes.txt"}}}]},"finish_reason":"tool_calls"}],"usage":{"total_tokens":12}})).unwrap();
        assert_eq!(update.used, Some(12));
        let (_, calls) = response.finish().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].2).unwrap(),
            json!({"path":"notes.txt"})
        );

        let mut response = ModelResponse::default();
        response
            .push(&json!({"choices":[{"delta":{"content":"hello"}}]}))
            .unwrap();
        response
            .push(&json!({"choices":[{"finish_reason":"stop"}]}))
            .unwrap();
        assert_eq!(response.finish().unwrap().0, "hello");
    }

    #[test]
    fn refuses_incomplete_or_truncated_calls_before_execution() {
        let mut response = ModelResponse::default();
        response.push(&json!({"choices":[{"delta":{"tool_calls":[{"id":"x","function":{"arguments":"{}"}}]}}]})).unwrap();
        assert!(response.finish().is_err());

        let mut response = ModelResponse::default();
        response.push(&json!({"choices":[{"delta":{"content":"looks fine","tool_calls":[{"id":"x","function":{"arguments":"{}"}}]}}]})).unwrap();
        response.done();
        assert!(
            response
                .finish()
                .unwrap_err()
                .message
                .contains("without a function name")
        );

        let mut response = ModelResponse::default();
        response
            .push(
                &json!({"choices":[{"delta":{"content":"looks fine","tool_calls":[{"index":1}]}}]}),
            )
            .unwrap();
        response.done();
        assert!(response.finish().is_err());

        let mut response = ModelResponse::default();
        let error = response
            .push(&json!({"choices":[{"finish_reason":"length"}]}))
            .unwrap_err();
        assert_eq!(error.status, 413);
    }
}
