//! Which changes deserve a desktop notification.
//!
//! The desktop shell feeds every agent state it reads from a box, and every
//! setup run update, through a [`Notifier`]. It answers with a [`Notice`]
//! only for a real change: an agent that was working and now waits for the
//! person (its turn ended, or it asks for a permission), or a setup run that
//! ended. The first state seen for an agent is the baseline and never
//! notifies, so opening the app or a terminal does not announce old news.
//!
//! Whether the notice is shown (window focus, the setting, the OS
//! permission) is the UI's decision.

use crate::agent::session::{AgentLive, AgentState};
use crate::agent::AgentMeta;
use crate::provision::{ProvisionRun, RunStatus};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Longest body text; the rest is cut with an ellipsis.
const BODY_MAX: usize = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeKind {
    /// An agent finished working and waits for the person.
    AgentDone,
    SetupDone,
    SetupFailed,
}

/// What the UI shows, and what a click on it opens.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
    /// Set for agent notices; a click opens the agent.
    pub agent_id: Option<String>,
    /// The box; a click on a setup notice opens its Setup tab.
    pub machine_id: String,
}

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= BODY_MAX {
        return s.to_string();
    }
    let mut out: String = s.chars().take(BODY_MAX - 1).collect();
    out.push('…');
    out
}

fn agent_name(meta: &AgentMeta) -> String {
    let t = meta.title.trim();
    if t.is_empty() { format!("{} agent", meta.harness.label()) } else { t.to_string() }
}

#[derive(Default)]
pub struct Notifier {
    /// The last state written per agent, with when it was written.
    agents: HashMap<String, (Option<AgentState>, i64)>,
    /// Setup runs that already produced their notice.
    setups: HashSet<String>,
}

impl Notifier {
    /// Records what the box shows for the agent and returns a notice when
    /// it went from working to waiting since the last look.
    pub fn agent(&mut self, meta: &AgentMeta, live: &AgentLive) -> Option<Notice> {
        let now = (live.state, live.since);
        let prev = self.agents.insert(meta.id.clone(), now)?;
        if prev == now || prev.0 != Some(AgentState::Working) || live.state != Some(AgentState::Waiting) {
            return None;
        }
        Some(Notice {
            kind: NoticeKind::AgentDone,
            title: format!("{} is waiting for you", agent_name(meta)),
            body: format!("In {}", meta.machine_name),
            agent_id: Some(meta.id.clone()),
            machine_id: meta.machine_id.clone(),
        })
    }

    /// Forgets a removed agent.
    pub fn forget_agent(&mut self, id: &str) {
        self.agents.remove(id);
    }

    /// Returns a notice the first time a run is seen finished or failed.
    pub fn setup(&mut self, run: &ProvisionRun) -> Option<Notice> {
        let (kind, title, body) = match run.status {
            RunStatus::Done => {
                let n = run.steps.len();
                (NoticeKind::SetupDone, format!("Setup finished in {}", run.machine_name), format!("{}: {} command{} ran", run.preset_name, n, if n == 1 { "" } else { "s" }))
            }
            RunStatus::Failed => {
                let why = run.error.as_deref().unwrap_or("it did not finish");
                (NoticeKind::SetupFailed, format!("Setup failed in {}", run.machine_name), clip(&format!("{}: {}", run.preset_name, why)))
            }
            RunStatus::Waiting | RunStatus::Running | RunStatus::Cancelled => return None,
        };
        if !self.setups.insert(run.id.clone()) {
            return None;
        }
        Some(Notice { kind, title, body, agent_id: None, machine_id: run.machine_id.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Harness;
    use crate::provision::{StepRun, StepState};

    fn meta(id: &str, title: &str) -> AgentMeta {
        AgentMeta {
            id: id.into(),
            title: title.into(),
            harness: Harness::ClaudeCode,
            machine_id: "m1".into(),
            machine_name: "api-dev".into(),
            cwd: None,
            model: None,
            session_id: None,
            skip_permissions: true,
            created_at: 0,
            updated_at: 0,
            worktree: None,
        }
    }

    fn live(state: Option<AgentState>, since: i64) -> AgentLive {
        AgentLive { id: "a".into(), state, since, running: true }
    }

    #[test]
    fn working_then_waiting_notifies_once() {
        let mut n = Notifier::default();
        let a = meta("a", "Fix the login bug");
        assert!(n.agent(&a, &live(Some(AgentState::Working), 1)).is_none(), "the first look is the baseline");
        let notice = n.agent(&a, &live(Some(AgentState::Waiting), 2)).unwrap();
        assert_eq!(notice.title, "Fix the login bug is waiting for you");
        assert_eq!(notice.body, "In api-dev");
        assert!(n.agent(&a, &live(Some(AgentState::Waiting), 2)).is_none());
        assert!(n.agent(&a, &live(Some(AgentState::Waiting), 3)).is_none(), "waiting again is not news");
    }

    #[test]
    fn a_first_look_at_a_waiting_agent_does_not_notify() {
        let mut n = Notifier::default();
        assert!(n.agent(&meta("a", ""), &live(Some(AgentState::Waiting), 5)).is_none());
        n.forget_agent("a");
        assert!(n.agent(&meta("a", ""), &live(Some(AgentState::Working), 6)).is_none());
        let notice = n.agent(&meta("a", ""), &live(Some(AgentState::Waiting), 7)).unwrap();
        assert_eq!(notice.title, "Claude Code agent is waiting for you");
    }

    fn run(status: RunStatus, error: Option<&str>) -> ProvisionRun {
        ProvisionRun {
            id: "r1".into(),
            machine_id: "m1".into(),
            machine_name: "api-dev".into(),
            preset_id: "p1".into(),
            preset_name: "main commands".into(),
            status,
            error: error.map(Into::into),
            steps: vec![
                StepRun { command: "git clone x".into(), state: StepState::Ok, exit_code: Some(0) },
                StepRun { command: "make".into(), state: StepState::Ok, exit_code: Some(0) },
            ],
            secret_names: vec![],
            variables: vec![],
            secrets_written: false,
            output: String::new(),
            started_at: 0,
            finished_at: None,
        }
    }

    #[test]
    fn setup_runs_notify_once_when_they_end() {
        let mut n = Notifier::default();
        assert!(n.setup(&run(RunStatus::Waiting, None)).is_none());
        assert!(n.setup(&run(RunStatus::Running, None)).is_none());
        let done = n.setup(&run(RunStatus::Done, None)).unwrap();
        assert_eq!(done.kind, NoticeKind::SetupDone);
        assert_eq!(done.title, "Setup finished in api-dev");
        assert_eq!(done.body, "main commands: 2 commands ran");
        assert_eq!(done.agent_id, None);
        assert!(n.setup(&run(RunStatus::Done, None)).is_none());

        let mut failed = run(RunStatus::Failed, Some("step 2 exited with code 2"));
        failed.id = "r2".into();
        let notice = n.setup(&failed).unwrap();
        assert_eq!(notice.kind, NoticeKind::SetupFailed);
        assert_eq!(notice.body, "main commands: step 2 exited with code 2");

        let mut cancelled = run(RunStatus::Cancelled, None);
        cancelled.id = "r3".into();
        assert!(n.setup(&cancelled).is_none());
    }

    #[test]
    fn notice_serializes_for_the_ui() {
        let mut n = Notifier::default();
        n.agent(&meta("a", ""), &live(Some(AgentState::Working), 1));
        let v = serde_json::to_value(n.agent(&meta("a", ""), &live(Some(AgentState::Waiting), 2)).unwrap()).unwrap();
        assert_eq!(v["kind"], "agent-done");
        assert_eq!(v["agentId"], "a");
        assert_eq!(v["machineId"], "m1");
    }
}
