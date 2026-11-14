//! A coding agent is the harness's own interactive interface (Claude Code,
//! Codex) running inside a box, shown in a terminal. One agent is one
//! session: it has its own id, directory and, when the box has tmux, its own
//! tmux session, so it keeps working while nobody watches (see `session`).

pub mod account;
pub mod session;

use serde::{Deserialize, Serialize};

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

/// An agent as Stowbox keeps it. Records from the chat era carry more
/// fields; they are ignored.
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
    /// Claude Code's session id. New agents get one up front and start
    /// with `--session-id`; every later open resumes it.
    pub session_id: Option<String>,
    pub skip_permissions: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// The agent's own worktree and branch, when it was created with one;
    /// `cwd` is then inside it.
    #[serde(default)]
    pub worktree: Option<crate::worktree::Worktree>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_era_records_still_load() {
        let a: AgentMeta = serde_json::from_value(serde_json::json!({
            "id": "a1", "title": "Fix it", "harness": "claude-code", "machineId": "vm", "machineName": "box",
            "cwd": "/w", "model": null, "sessionId": "s1", "skipPermissions": true, "createdAt": 1, "updatedAt": 2,
            "status": {"kind": "done"}, "totalCostUsd": 0.5, "turns": 3, "inputTokens": 1, "outputTokens": 2,
            "turn": null, "queue": [], "queuePaused": false
        }))
        .unwrap();
        assert_eq!(a.session_id.as_deref(), Some("s1"));
        assert!(a.worktree.is_none());
    }
}
