//! Claude Code headless mode: `claude -p --output-format stream-json --verbose`.
//! Every line is one JSON object; `--include-partial-messages` adds
//! `stream_event` lines with text deltas while the model is typing.

use super::{AgentEvent, Block, LineParser, TurnSpec};
use crate::boxd::shell_quote;
use serde_json::Value;

pub fn command(spec: &TurnSpec) -> String {
    let mut c = String::from("claude -p --output-format stream-json --verbose --include-partial-messages");
    if let Some(m) = spec.model.as_deref().filter(|m| !m.trim().is_empty()) {
        c.push_str(&format!(" --model {}", shell_quote(m)));
    }
    if let Some(s) = spec.resume_session.as_deref().filter(|s| !s.trim().is_empty()) {
        c.push_str(&format!(" --resume {}", shell_quote(s)));
    }
    if let Some(n) = spec.max_turns {
        c.push_str(&format!(" --max-turns {n}"));
    }
    if let Some(b) = spec.max_budget_usd.filter(|b| *b > 0.0) {
        c.push_str(&format!(" --max-budget-usd {b}"));
    }
    if spec.skip_permissions {
        c.push_str(" --dangerously-skip-permissions");
    } else {
        // One process per turn means nobody can answer a permission prompt,
        // so prompts are denied instead of hanging the run (Claude Code 2.1.259+).
        c.push_str(" --permission-prompts none");
    }
    c
}

/// The interactive TUI on the same session, without a prompt.
pub fn interactive_command(spec: &TurnSpec) -> String {
    let mut c = String::from("exec claude");
    if let Some(s) = spec.resume_session.as_deref().filter(|s| !s.trim().is_empty()) {
        c.push_str(&format!(" --resume {}", shell_quote(s)));
    }
    if let Some(m) = spec.model.as_deref().filter(|m| !m.trim().is_empty()) {
        c.push_str(&format!(" --model {}", shell_quote(m)));
    }
    if spec.skip_permissions {
        c.push_str(" --dangerously-skip-permissions");
    }
    c
}

#[derive(Default)]
pub struct Parser;

impl LineParser for Parser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return vec![AgentEvent::Stderr { text: line.to_string() }],
        };
        match v.get("type").and_then(Value::as_str) {
            Some("system") => match v.get("subtype").and_then(Value::as_str) {
                Some("init") => vec![AgentEvent::Init {
                    session_id: str_field(&v, "session_id").unwrap_or_default(),
                    model: str_field(&v, "model"),
                    cwd: str_field(&v, "cwd"),
                }],
                _ => vec![],
            },
            Some("stream_event") => {
                let delta = v.pointer("/event/delta");
                match delta.and_then(|d| d.get("type")).and_then(Value::as_str) {
                    Some("text_delta") => delta
                        .and_then(|d| d.get("text"))
                        .and_then(Value::as_str)
                        .map(|t| vec![AgentEvent::TextDelta { text: t.to_string() }])
                        .unwrap_or_default(),
                    _ => vec![],
                }
            }
            Some("assistant") => {
                let blocks = v
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(block).collect::<Vec<_>>())
                    .unwrap_or_default();
                if blocks.is_empty() {
                    vec![]
                } else {
                    vec![AgentEvent::Assistant { blocks }]
                }
            }
            Some("user") => v
                .pointer("/message/content")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                        .map(|b| AgentEvent::ToolResult {
                            tool_use_id: str_field(b, "tool_use_id").unwrap_or_default(),
                            content: content_text(b.get("content")),
                            is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Some("result") => {
                let subtype = str_field(&v, "subtype").unwrap_or_else(|| "unknown".into());
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(subtype != "success");
                vec![AgentEvent::Result {
                    ok: !is_error,
                    message: str_field(&v, "result").or_else(|| {
                        v.get("errors")
                            .and_then(Value::as_array)
                            .map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
                            .filter(|s| !s.is_empty())
                    }),
                    subtype,
                    cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                    duration_ms: v.get("duration_ms").and_then(Value::as_u64),
                    turns: v.get("num_turns").and_then(Value::as_u64).map(|n| n as u32),
                    session_id: str_field(&v, "session_id"),
                    input_tokens: v.pointer("/usage/input_tokens").and_then(Value::as_u64),
                    output_tokens: v.pointer("/usage/output_tokens").and_then(Value::as_u64),
                }]
            }
            _ => vec![],
        }
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn block(b: &Value) -> Option<Block> {
    match b.get("type").and_then(Value::as_str)? {
        "text" => Some(Block::Text { text: str_field(b, "text")? }),
        "thinking" => Some(Block::Thinking { text: str_field(b, "thinking").unwrap_or_default() }),
        "tool_use" => Some(Block::ToolUse {
            id: str_field(b, "id").unwrap_or_default(),
            name: str_field(b, "name").unwrap_or_default(),
            input: b.get("input").cloned().unwrap_or(Value::Null),
        }),
        _ => None,
    }
}

/// Tool results carry either a string or a list of content blocks.
fn content_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Vec<AgentEvent> {
        Parser.parse_line(line)
    }

    #[test]
    fn init_line() {
        let ev = parse(r#"{"type":"system","subtype":"init","cwd":"/work","session_id":"s1","tools":["Bash"],"model":"claude-opus","permissionMode":"bypassPermissions"}"#);
        assert_eq!(
            ev,
            vec![AgentEvent::Init { session_id: "s1".into(), model: Some("claude-opus".into()), cwd: Some("/work".into()) }]
        );
    }

    #[test]
    fn text_delta_and_full_message() {
        let ev = parse(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}},"session_id":"s1"}"#);
        assert_eq!(ev, vec![AgentEvent::TextDelta { text: "Hel".into() }]);
        let ev = parse(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]},"session_id":"s1"}"#);
        assert_eq!(
            ev,
            vec![AgentEvent::Assistant {
                blocks: vec![
                    Block::Text { text: "Hello".into() },
                    Block::ToolUse { id: "t1".into(), name: "Bash".into(), input: serde_json::json!({"command":"ls"}) }
                ]
            }]
        );
    }

    #[test]
    fn tool_result_string_and_blocks() {
        let ev = parse(r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a.txt","is_error":false}]},"session_id":"s1"}"#);
        assert_eq!(ev, vec![AgentEvent::ToolResult { tool_use_id: "t1".into(), content: "a.txt".into(), is_error: false }]);
        let ev = parse(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"boom"}],"is_error":true}]}}"#);
        assert_eq!(ev, vec![AgentEvent::ToolResult { tool_use_id: "t2".into(), content: "boom".into(), is_error: true }]);
    }

    #[test]
    fn result_success_and_error() {
        let ev = parse(r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1500,"num_turns":3,"result":"Done.","session_id":"s1","total_cost_usd":0.0123,"usage":{"input_tokens":10,"output_tokens":5}}"#);
        match &ev[0] {
            AgentEvent::Result { ok, subtype, message, cost_usd, turns, session_id, input_tokens, .. } => {
                assert!(ok);
                assert_eq!(subtype, "success");
                assert_eq!(message.as_deref(), Some("Done."));
                assert_eq!(*cost_usd, Some(0.0123));
                assert_eq!(*turns, Some(3));
                assert_eq!(session_id.as_deref(), Some("s1"));
                assert_eq!(*input_tokens, Some(10));
            }
            other => panic!("unexpected {other:?}"),
        }
        let ev = parse(r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":10,"session_id":"s1"}"#);
        match &ev[0] {
            AgentEvent::Result { ok, subtype, .. } => {
                assert!(!ok);
                assert_eq!(subtype, "error_max_turns");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn non_json_is_stderr() {
        assert_eq!(parse("Welcome to Claude Code"), vec![AgentEvent::Stderr { text: "Welcome to Claude Code".into() }]);
        assert_eq!(parse("   "), vec![]);
    }
}
