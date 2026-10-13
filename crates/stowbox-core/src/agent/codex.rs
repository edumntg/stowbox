//! OpenAI Codex CLI: `codex exec --json`. Events are `thread.started`,
//! `turn.started`, `item.started|updated|completed` and `turn.completed|failed`.
//! Later turns run `codex exec resume <thread_id>`.

use super::{AgentEvent, Block, LineParser, TurnSpec};
use crate::boxd::shell_quote;
use serde_json::Value;

pub fn command(spec: &TurnSpec) -> String {
    let mut c = String::from("codex exec");
    if let Some(s) = spec.resume_session.as_deref().filter(|s| !s.trim().is_empty()) {
        c.push_str(&format!(" resume {}", shell_quote(s)));
    }
    c.push_str(" --json");
    if let Some(m) = spec.model.as_deref().filter(|m| !m.trim().is_empty()) {
        c.push_str(&format!(" --model {}", shell_quote(m)));
    }
    if spec.skip_permissions {
        c.push_str(" --dangerously-bypass-approvals-and-sandbox");
    } else {
        c.push_str(" --sandbox workspace-write");
    }
    // Prompt comes from stdin.
    c.push_str(" -");
    c
}

#[derive(Default)]
pub struct Parser {
    started_commands: std::collections::HashMap<String, String>,
}

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
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "thread.started" => vec![AgentEvent::Init {
                session_id: v.get("thread_id").and_then(Value::as_str).unwrap_or_default().to_string(),
                model: None,
                cwd: None,
            }],
            "item.started" | "item.updated" | "item.completed" => {
                let Some(item) = v.get("item") else { return vec![] };
                let id = item.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                match (kind, item_type) {
                    ("item.started", "command_execution") => {
                        let cmd = item.get("command").and_then(Value::as_str).unwrap_or("").to_string();
                        self.started_commands.insert(id.clone(), cmd.clone());
                        vec![AgentEvent::Assistant {
                            blocks: vec![Block::ToolUse {
                                id,
                                name: "command".into(),
                                input: serde_json::json!({ "command": cmd }),
                            }],
                        }]
                    }
                    ("item.completed", "command_execution") => {
                        let mut out = Vec::new();
                        if !self.started_commands.contains_key(&id) {
                            let cmd = item.get("command").and_then(Value::as_str).unwrap_or("").to_string();
                            out.push(AgentEvent::Assistant {
                                blocks: vec![Block::ToolUse {
                                    id: id.clone(),
                                    name: "command".into(),
                                    input: serde_json::json!({ "command": cmd }),
                                }],
                            });
                        }
                        self.started_commands.remove(&id);
                        let exit = item.get("exit_code").and_then(Value::as_i64);
                        out.push(AgentEvent::ToolResult {
                            tool_use_id: id,
                            content: item.get("aggregated_output").and_then(Value::as_str).unwrap_or("").to_string(),
                            is_error: exit.map(|c| c != 0).unwrap_or(false),
                        });
                        out
                    }
                    ("item.completed", "agent_message") => item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|t| vec![AgentEvent::Assistant { blocks: vec![Block::Text { text: t.to_string() }] }])
                        .unwrap_or_default(),
                    ("item.completed", "reasoning") => item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|t| vec![AgentEvent::Assistant { blocks: vec![Block::Thinking { text: t.to_string() }] }])
                        .unwrap_or_default(),
                    ("item.completed", "error") => vec![AgentEvent::Stderr {
                        text: item.get("message").and_then(Value::as_str).unwrap_or("error").to_string(),
                    }],
                    _ => vec![],
                }
            }
            "turn.completed" => vec![AgentEvent::Result {
                ok: true,
                subtype: "success".into(),
                message: None,
                cost_usd: None,
                duration_ms: None,
                turns: None,
                session_id: None,
                input_tokens: v.pointer("/usage/input_tokens").and_then(Value::as_u64),
                output_tokens: v.pointer("/usage/output_tokens").and_then(Value::as_u64),
            }],
            "turn.failed" | "error" => vec![AgentEvent::Result {
                ok: false,
                subtype: kind.replace('.', "_"),
                message: v
                    .pointer("/error/message")
                    .or_else(|| v.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                cost_usd: None,
                duration_ms: None,
                turns: None,
                session_id: None,
                input_tokens: None,
                output_tokens: None,
            }],
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_and_command_flow() {
        let mut p = Parser::default();
        assert_eq!(
            p.parse_line(r#"{"type":"thread.started","thread_id":"th1"}"#),
            vec![AgentEvent::Init { session_id: "th1".into(), model: None, cwd: None }]
        );
        let started = p.parse_line(r#"{"type":"item.started","item":{"id":"i0","type":"command_execution","command":"ls","status":"in_progress"}}"#);
        assert!(matches!(&started[0], AgentEvent::Assistant { blocks } if matches!(&blocks[0], Block::ToolUse { name, .. } if name == "command")));
        let done = p.parse_line(r#"{"type":"item.completed","item":{"id":"i0","type":"command_execution","command":"ls","aggregated_output":"a\n","exit_code":0,"status":"completed"}}"#);
        assert_eq!(done, vec![AgentEvent::ToolResult { tool_use_id: "i0".into(), content: "a\n".into(), is_error: false }]);
        let msg = p.parse_line(r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"All good"}}"#);
        assert_eq!(msg, vec![AgentEvent::Assistant { blocks: vec![Block::Text { text: "All good".into() }] }]);
        let end = p.parse_line(r#"{"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3}}"#);
        assert!(matches!(&end[0], AgentEvent::Result { ok: true, input_tokens: Some(7), .. }));
    }

    #[test]
    fn resume_command() {
        let spec = TurnSpec { resume_session: Some("th1".into()), skip_permissions: false, ..Default::default() };
        assert_eq!(command(&spec), "codex exec resume th1 --json --sandbox workspace-write -");
    }
}
