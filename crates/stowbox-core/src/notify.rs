//! Which status changes deserve a desktop notification.
//!
//! The desktop shell feeds every agent status it pushes to the UI, and every
//! setup run update, through a [`Notifier`]. It answers with a [`Notice`]
//! only for a real outcome: a turn that was running and is now finished or
//! failed, or a setup run that ended. Reconnecting is not an outcome, and a
//! person pressing Stop does not need to be told.
//!
//! One turn notifies once: an outcome only counts when the status seen just
//! before it was busy, so the result line and the exit that follows it do
//! not both notify. Agents read from disk at startup are the baseline, and
//! for a short quiet period after startup outcomes are recorded but not
//! announced: a reattached turn that finished while Stowbox was closed
//! replays its last lines then, and the person just opened the app.
//!
//! Whether the notice is shown (window focus, the setting, the OS
//! permission) is the UI's decision.

use crate::agent::{AgentMeta, AgentStatus};
use crate::provision::{ProvisionRun, RunStatus};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// How long after startup outcomes are recorded without a notice.
pub const STARTUP_QUIET_MS: i64 = 10_000;

/// Longest body text; the rest is cut with an ellipsis.
const BODY_MAX: usize = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeKind {
    AgentDone,
    AgentFailed,
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

fn is_busy(s: &AgentStatus) -> bool {
    matches!(s, AgentStatus::Starting | AgentStatus::Thinking | AgentStatus::Tool { .. } | AgentStatus::Reconnecting { .. })
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

pub struct Notifier {
    /// The last status seen per agent.
    agents: HashMap<String, AgentStatus>,
    /// Setup runs that already produced their notice.
    setups: HashSet<String>,
    quiet_until_ms: i64,
}

impl Notifier {
    /// `agents` is what was read from disk; nothing about them is announced.
    pub fn new(agents: &[AgentMeta], now_ms: i64) -> Self {
        Self {
            agents: agents.iter().map(|a| (a.id.clone(), a.status.clone())).collect(),
            setups: HashSet::new(),
            quiet_until_ms: now_ms + STARTUP_QUIET_MS,
        }
    }

    /// Records the agent's status and returns a notice when a running turn
    /// just reached its outcome.
    pub fn agent(&mut self, meta: &AgentMeta, now_ms: i64) -> Option<Notice> {
        let prev = self.agents.insert(meta.id.clone(), meta.status.clone());
        if !prev.as_ref().is_some_and(is_busy) || now_ms < self.quiet_until_ms {
            return None;
        }
        let (kind, title, body) = match &meta.status {
            AgentStatus::Done => (NoticeKind::AgentDone, format!("{} finished", agent_name(meta)), format!("Waiting for you in {}", meta.machine_name)),
            AgentStatus::Failed { message } => (NoticeKind::AgentFailed, format!("{} failed", agent_name(meta)), clip(&format!("{}: {}", meta.machine_name, message))),
            _ => return None,
        };
        Some(Notice { kind, title, body, agent_id: Some(meta.id.clone()), machine_id: meta.machine_id.clone() })
    }

    /// Forgets a removed agent.
    pub fn forget_agent(&mut self, id: &str) {
        self.agents.remove(id);
    }

    /// Returns a notice the first time a run is seen finished or failed.
    /// Runs do not survive a restart, so there is nothing to keep quiet.
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

    const T0: i64 = 1_000_000;
    const LATER: i64 = T0 + STARTUP_QUIET_MS + 1;

    fn meta(id: &str, status: AgentStatus) -> AgentMeta {
        AgentMeta {
            id: id.into(),
            title: "Fix the login bug".into(),
            harness: Harness::ClaudeCode,
            machine_id: "m1".into(),
            machine_name: "api-dev".into(),
            cwd: None,
            model: None,
            session_id: None,
            status,
            skip_permissions: true,
            created_at: 0,
            updated_at: 0,
            total_cost_usd: 0.0,
            turns: 0,
            input_tokens: 0,
            output_tokens: 0,
            turn: None,
            worktree: None,
        }
    }

    fn feed(n: &mut Notifier, id: &str, statuses: &[AgentStatus], now: i64) -> Vec<Notice> {
        statuses.iter().filter_map(|s| n.agent(&meta(id, s.clone()), now)).collect()
    }

    fn failed(m: &str) -> AgentStatus {
        AgentStatus::Failed { message: m.into() }
    }

    #[test]
    fn a_turn_notifies_once_when_it_finishes() {
        let mut n = Notifier::new(&[], T0);
        let out = feed(
            &mut n,
            "a",
            &[AgentStatus::Starting, AgentStatus::Thinking, AgentStatus::Tool { name: "Bash".into() }, AgentStatus::Thinking, AgentStatus::Done, AgentStatus::Done],
            LATER,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, NoticeKind::AgentDone);
        assert_eq!(out[0].title, "Fix the login bug finished");
        assert_eq!(out[0].body, "Waiting for you in api-dev");
        assert_eq!(out[0].agent_id.as_deref(), Some("a"));
        // The next turn notifies again.
        let out = feed(&mut n, "a", &[AgentStatus::Starting, AgentStatus::Thinking, failed("error max turns"), failed("error max turns")], LATER);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, NoticeKind::AgentFailed);
        assert_eq!(out[0].body, "api-dev: error max turns");
    }

    #[test]
    fn reconnecting_is_not_an_outcome_but_what_follows_is() {
        let mut n = Notifier::new(&[], T0);
        let out = feed(&mut n, "a", &[AgentStatus::Thinking, AgentStatus::Reconnecting { message: "the stream dropped".into() }], LATER);
        assert!(out.is_empty());
        let out = feed(&mut n, "a", &[AgentStatus::Done], LATER);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn stop_and_idle_do_not_notify() {
        let mut n = Notifier::new(&[], T0);
        assert!(feed(&mut n, "a", &[AgentStatus::Idle, AgentStatus::Starting, AgentStatus::Thinking, AgentStatus::Stopped, AgentStatus::Done], LATER).is_empty());
    }

    #[test]
    fn an_agent_first_seen_finished_does_not_notify() {
        let mut n = Notifier::new(&[], T0);
        assert!(feed(&mut n, "new", &[AgentStatus::Done], LATER).is_empty());
    }

    #[test]
    fn startup_baseline_and_quiet_period() {
        // Read from disk: one finished, one with a turn still running.
        let mut n = Notifier::new(&[meta("done", AgentStatus::Done), meta("busy", AgentStatus::Thinking), meta("late", AgentStatus::Thinking)], T0);
        // The finished one is pushed again: no notice.
        assert!(feed(&mut n, "done", &[AgentStatus::Done], LATER).is_empty());
        // The running one replays its end right after startup: recorded, not announced.
        assert!(feed(&mut n, "busy", &[AgentStatus::Reconnecting { message: "starting".into() }, AgentStatus::Done], T0 + 2_000).is_empty());
        assert!(feed(&mut n, "busy", &[AgentStatus::Done], LATER).is_empty());
        // One that really finishes later is announced.
        assert_eq!(feed(&mut n, "late", &[AgentStatus::Done], LATER).len(), 1);
    }

    #[test]
    fn forgotten_agents_start_over() {
        let mut n = Notifier::new(&[], T0);
        feed(&mut n, "a", &[AgentStatus::Thinking], LATER);
        n.forget_agent("a");
        assert!(feed(&mut n, "a", &[AgentStatus::Done], LATER).is_empty());
    }

    #[test]
    fn untitled_agents_and_long_messages() {
        let mut n = Notifier::new(&[], T0);
        let mut m = meta("a", AgentStatus::Thinking);
        m.title = "  ".into();
        n.agent(&m, LATER);
        m.status = failed(&"x".repeat(500));
        let notice = n.agent(&m, LATER).unwrap();
        assert_eq!(notice.title, "Claude Code agent failed");
        assert_eq!(notice.body.chars().count(), BODY_MAX);
        assert!(notice.body.ends_with('…'));
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
        // Setup runs ignore the startup quiet period: they never replay.
        let mut n = Notifier::new(&[], T0);
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
        let mut n = Notifier::new(&[], T0);
        n.agent(&meta("a", AgentStatus::Thinking), LATER);
        let v = serde_json::to_value(n.agent(&meta("a", AgentStatus::Done), LATER).unwrap()).unwrap();
        assert_eq!(v["kind"], "agent-done");
        assert_eq!(v["agentId"], "a");
        assert_eq!(v["machineId"], "m1");
    }
}
