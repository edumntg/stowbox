//! A coding agent is a CLI (Claude Code, Codex) run *inside* a box through the
//! provider's exec stream. One process per turn: the prompt goes in on stdin,
//! the harness streams JSONL back, and the next turn resumes the same session
//! id. That keeps the transport trivial and works the same for both harnesses.

pub mod claude;
pub mod codex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    ClaudeCode,
    Codex,
}

impl Harness {
    pub fn label(self) -> &'static str {
        match self {
            Harness::ClaudeCode => "Claude Code",
            Harness::Codex => "Codex",
        }
    }
}

/// What the UI asks for when it starts or continues an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSpec {
    pub harness: Option<Harness>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub resume_session: Option<String>,
    /// Boxes are disposable VMs, so the default is to let the agent act
    /// without per-tool approval. Off means permission-needing tools are
    /// denied in headless mode, not prompted.
    #[serde(default = "default_true")]
    pub skip_permissions: bool,
    pub max_turns: Option<u32>,
    /// Claude Code only: stops the turn once its own cost estimate passes this.
    pub max_budget_usd: Option<f64>,
}

fn default_true() -> bool {
    true
}

impl Default for TurnSpec {
    fn default() -> Self {
        Self {
            harness: None,
            cwd: None,
            model: None,
            resume_session: None,
            skip_permissions: true,
            max_turns: None,
            max_budget_usd: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AgentStatus {
    /// Created, no turn run yet.
    Idle,
    Starting,
    Thinking,
    Tool { name: String },
    Done,
    Failed { message: String },
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMeta {
    pub id: String,
    pub title: String,
    pub harness: Harness,
    pub machine_id: String,
    pub machine_name: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub status: AgentStatus,
    pub skip_permissions: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub total_cost_usd: f64,
    pub turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One content block of an assistant message, as the UI renders it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text { text: String },
    Thinking { text: String },
    ToolUse { id: String, name: String, input: Value },
}

/// Normalized transcript events. Each one is appended to the agent's JSONL
/// transcript and pushed to the UI as it happens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The user's prompt for this turn (recorded by the runner, not parsed).
    UserMessage { text: String },
    Init { session_id: String, model: Option<String>, cwd: Option<String> },
    /// Live text as the model types. Superseded by the next `Assistant`.
    TextDelta { text: String },
    Assistant { blocks: Vec<Block> },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Result {
        ok: bool,
        subtype: String,
        message: Option<String>,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        turns: Option<u32>,
        session_id: Option<String>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    /// Output that was not JSON (install banners, warnings, crashes).
    Stderr { text: String },
    /// The process ended without a `Result`.
    Exit { code: i32 },
}

pub trait LineParser: Send {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent>;
}

pub fn parser_for(h: Harness) -> Box<dyn LineParser> {
    match h {
        Harness::ClaudeCode => Box::new(claude::Parser::default()),
        Harness::Codex => Box::new(codex::Parser::default()),
    }
}

/// Shell line that runs one turn. The prompt is *not* in here: it is written
/// to stdin, so no prompt text ever touches shell quoting.
pub fn turn_command(h: Harness, spec: &TurnSpec) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cwd) = spec.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
        parts.push(format!("cd {} &&", crate::boxd::shell_quote(cwd)));
    }
    match h {
        Harness::ClaudeCode => parts.push(claude::command(spec)),
        Harness::Codex => parts.push(codex::command(spec)),
    }
    parts.join(" ")
}

/// Splits a byte stream into complete lines, keeping a partial tail.
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let s = String::from_utf8_lossy(&line[..line.len() - 1]).trim_end_matches('\r').to_string();
            lines.push(s);
        }
        lines
    }

    pub fn flush(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let s = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_buffer_keeps_partial_tail() {
        let mut lb = LineBuffer::default();
        assert_eq!(lb.push(b"abc\nde"), vec!["abc".to_string()]);
        assert_eq!(lb.push(b"f\r\n"), vec!["def".to_string()]);
        assert_eq!(lb.push(b"tail"), Vec::<String>::new());
        assert_eq!(lb.flush(), Some("tail".to_string()));
        assert_eq!(lb.flush(), None);
    }

    #[test]
    fn turn_command_includes_cwd_and_resume() {
        let spec = TurnSpec {
            cwd: Some("/home/user/my app".into()),
            resume_session: Some("sess-1".into()),
            model: Some("opus".into()),
            ..Default::default()
        };
        let c = turn_command(Harness::ClaudeCode, &spec);
        assert!(c.starts_with("cd '/home/user/my app' && claude -p"));
        assert!(c.contains("--resume sess-1"));
        assert!(c.contains("--model opus"));
        assert!(c.contains("--dangerously-skip-permissions"));
        assert!(c.contains("--output-format stream-json"));
    }
}
