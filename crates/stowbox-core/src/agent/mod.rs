//! A coding agent is a CLI (Claude Code, Codex) run *inside* a box through the
//! provider's exec stream. One process per turn: the prompt goes in on stdin,
//! the harness streams JSONL back, and the next turn resumes the same session
//! id. That keeps the transport trivial and works the same for both harnesses.
//! Each turn runs detached in the box and is followed from files, so it
//! outlives the exec stream that started it (see `detached`).

pub mod claude;
pub mod codex;
pub mod detached;

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
    /// A turn is running in the box but its output stream is not attached:
    /// it dropped, or Stowbox just started, or the box is not running.
    Reconnecting { message: String },
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
    /// The detached turn being followed, until its exit is applied.
    #[serde(default)]
    pub turn: Option<detached::TurnCursor>,
    /// The agent's own worktree and branch, when it was created with one;
    /// `cwd` is then inside it.
    #[serde(default)]
    pub worktree: Option<crate::worktree::Worktree>,
    /// Messages written while a turn ran, sent one per turn once it ends.
    #[serde(default)]
    pub queue: Vec<QueuedMessage>,
    /// Set by Stop, or when a queued message could not be sent: the queue
    /// waits for the person instead of sending on its own.
    #[serde(default)]
    pub queue_paused: bool,
}

/// A message waiting for the agent's current turn to end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<crate::upload::Attachment>,
    pub queued_at: i64,
}

/// Most messages an agent's queue holds.
pub const QUEUE_MAX: usize = 20;

fn is_busy(s: &AgentStatus) -> bool {
    matches!(s, AgentStatus::Starting | AgentStatus::Thinking | AgentStatus::Tool { .. } | AgentStatus::Reconnecting { .. })
}

impl AgentMeta {
    /// The queue will send its next message on its own when the current
    /// turn ends (or now, if none runs).
    pub fn queue_continues(&self) -> bool {
        !self.queue.is_empty() && !self.queue_paused && self.status != AgentStatus::Stopped
    }

    /// The next queued message is due now: the queue continues and no turn
    /// is running or still on record. Reconnecting counts as running, so a
    /// dropped stream never sends the next message early.
    pub fn queue_ready(&self) -> bool {
        self.queue_continues() && self.turn.is_none() && !is_busy(&self.status)
    }

    /// Adds a message at the end of the queue.
    pub fn enqueue(&mut self, msg: QueuedMessage) -> crate::Result<()> {
        if msg.text.trim().is_empty() && msg.attachments.is_empty() {
            return Err(crate::Error::Provider("write something for the agent first".into()));
        }
        if self.queue.len() >= QUEUE_MAX {
            return Err(crate::Error::Provider(format!("the queue holds {QUEUE_MAX} messages; wait for some to be sent or remove one")));
        }
        self.queue.push(msg);
        Ok(())
    }

    /// Replaces a queued message's text. An emptied message without files
    /// is removed.
    pub fn edit_queued(&mut self, id: &str, text: &str) -> crate::Result<()> {
        let i = self.queue.iter().position(|m| m.id == id).ok_or_else(|| crate::Error::NotFound("that message was already sent".into()))?;
        if text.trim().is_empty() && self.queue[i].attachments.is_empty() {
            self.queue.remove(i);
        } else {
            self.queue[i].text = text.trim_end().to_string();
        }
        Ok(())
    }

    pub fn remove_queued(&mut self, id: &str) {
        self.queue.retain(|m| m.id != id);
        if self.queue.is_empty() {
            self.queue_paused = false;
        }
    }

    /// Takes the next message off the front of the queue.
    pub fn take_queued(&mut self) -> Option<QueuedMessage> {
        if self.queue.is_empty() {
            None
        } else {
            Some(self.queue.remove(0))
        }
    }

    /// Puts a message that could not be sent back in front and pauses the
    /// queue, so it does not retry on its own.
    pub fn requeue_front(&mut self, msg: QueuedMessage) {
        self.queue.insert(0, msg);
        self.queue_paused = true;
    }
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
    UserMessage {
        text: String,
        /// Files sent with the message, already in the box.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::upload::Attachment>,
    },
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

    fn meta(status: AgentStatus) -> AgentMeta {
        let mut a: AgentMeta = serde_json::from_value(serde_json::json!({
            "id": "ag", "title": "", "harness": "claude-code", "machineId": "vm_1", "machineName": "b",
            "cwd": null, "model": null, "sessionId": null, "status": {"kind": "idle"}, "skipPermissions": true,
            "createdAt": 0, "updatedAt": 0, "totalCostUsd": 0.0, "turns": 0, "inputTokens": 0, "outputTokens": 0
        }))
        .unwrap();
        a.status = status;
        a
    }

    fn queued(id: &str, text: &str) -> QueuedMessage {
        QueuedMessage { id: id.into(), text: text.into(), attachments: vec![], queued_at: 0 }
    }

    #[test]
    fn older_records_load_with_an_empty_queue() {
        let a = meta(AgentStatus::Done);
        assert!(a.queue.is_empty());
        assert!(!a.queue_paused);
        assert!(!a.queue_continues());
        let ev: AgentEvent = serde_json::from_str(r#"{"type":"user_message","text":"hi"}"#).unwrap();
        assert_eq!(ev, AgentEvent::UserMessage { text: "hi".into(), attachments: vec![] });
        // No attachments: the line stays as it was before.
        assert_eq!(serde_json::to_string(&ev).unwrap(), r#"{"type":"user_message","text":"hi"}"#);
    }

    #[test]
    fn queue_is_due_only_after_the_turn_really_ends() {
        let mut a = meta(AgentStatus::Thinking);
        a.turn = Some(detached::TurnCursor::new("t1"));
        a.enqueue(queued("q1", "next")).unwrap();
        assert!(a.queue_continues());
        assert!(!a.queue_ready(), "a turn is running");
        a.status = AgentStatus::Reconnecting { message: "the stream dropped".into() };
        assert!(!a.queue_ready(), "reconnecting is not an end");
        // The result arrived but the exit is not applied yet.
        a.status = AgentStatus::Done;
        assert!(!a.queue_ready());
        a.turn = None;
        assert!(a.queue_ready());
        a.status = AgentStatus::Failed { message: "boom".into() };
        assert!(a.queue_ready(), "a failed turn ends too");
        a.status = AgentStatus::Stopped;
        assert!(!a.queue_ready() && !a.queue_continues(), "Stop keeps the queue for the person");
    }

    #[test]
    fn a_message_that_could_not_be_sent_pauses_the_queue() {
        let mut a = meta(AgentStatus::Done);
        a.enqueue(queued("q1", "one")).unwrap();
        a.enqueue(queued("q2", "two")).unwrap();
        let m = a.take_queued().unwrap();
        assert_eq!(m.id, "q1");
        a.status = AgentStatus::Failed { message: "box stopped".into() };
        a.requeue_front(m);
        assert_eq!(a.queue.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["q1", "q2"]);
        assert!(!a.queue_ready() && !a.queue_continues());
        a.remove_queued("q1");
        assert!(a.queue_paused);
        a.remove_queued("q2");
        assert!(!a.queue_paused, "an empty queue is not paused");
    }

    #[test]
    fn queue_edits() {
        let mut a = meta(AgentStatus::Thinking);
        assert!(a.enqueue(queued("q0", "  ")).is_err());
        a.enqueue(queued("q1", "one")).unwrap();
        a.edit_queued("q1", "one, but better\n").unwrap();
        assert_eq!(a.queue[0].text, "one, but better");
        a.edit_queued("q1", "").unwrap();
        assert!(a.queue.is_empty(), "emptied without files: removed");
        assert!(a.edit_queued("q1", "x").is_err());
        for i in 0..QUEUE_MAX {
            a.enqueue(queued(&format!("q{i}"), "x")).unwrap();
        }
        assert!(a.enqueue(queued("over", "x")).is_err());
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
