//! Cost guards: putting a box in standby once nothing has happened in it for
//! a while, and checking the day's spend against a limit the person set.
//!
//! Idleness is judged from what Stowbox knows about the box: its agents (one
//! last seen working keeps running in its tmux session with no terminal
//! attached, so it holds the box), its setup run, its open terminals and the
//! last time any of those did something. Client connections are not a
//! signal on their own. Everything here is pure; the shell feeds it and acts
//! on the verdict.

use crate::agent::session::{AgentLive, AgentState};
use crate::types::MachineStatus;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What Stowbox knows about one box at the moment of the check.
#[derive(Debug, Clone)]
pub struct BoxActivity {
    pub machine_id: String,
    pub status: MachineStatus,
    /// Agents that hold the box: see [`agent_holds_box`].
    pub agents_busy: u32,
    pub setup_active: bool,
    pub terminals_open: u32,
    /// The latest moment anything happened in the box as far as Stowbox saw,
    /// unix ms. Never earlier than when Stowbox started watching it run.
    pub last_activity_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdlePolicy {
    /// Minutes without activity before standby; 0 turns the guard off.
    pub idle_minutes: u32,
    /// Boxes the person excluded.
    #[serde(default)]
    pub opted_out: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeepReason {
    GuardOff,
    NotRunning,
    OptedOut,
    AgentWorking,
    SetupRunning,
    TerminalOpen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Verdict {
    /// Idle for the whole window: put it in standby.
    Suspend { idle_since_ms: i64 },
    /// Nothing holds it, but it has not been idle long enough yet.
    Wait { due_at_ms: i64 },
    /// Left alone, and why.
    Keep { reason: KeepReason },
}

/// An agent holds its box while it was last seen working in a session that
/// still runs.
pub fn agent_holds_box(a: &AgentLive) -> bool {
    a.running && a.state == Some(AgentState::Working)
}

/// The guard's decision for one box. Conservative by construction: any
/// sign of work keeps the box running, and the idle clock starts at the
/// latest activity.
pub fn decide(b: &BoxActivity, policy: &IdlePolicy, now_ms: i64) -> Verdict {
    let keep = |reason| Verdict::Keep { reason };
    if policy.idle_minutes == 0 {
        return keep(KeepReason::GuardOff);
    }
    if b.status != MachineStatus::Running {
        return keep(KeepReason::NotRunning);
    }
    if policy.opted_out.iter().any(|id| id == &b.machine_id) {
        return keep(KeepReason::OptedOut);
    }
    if b.agents_busy > 0 {
        return keep(KeepReason::AgentWorking);
    }
    if b.setup_active {
        return keep(KeepReason::SetupRunning);
    }
    if b.terminals_open > 0 {
        return keep(KeepReason::TerminalOpen);
    }
    let due = b.last_activity_ms + policy.idle_minutes as i64 * 60_000;
    if now_ms >= due {
        Verdict::Suspend { idle_since_ms: b.last_activity_ms }
    } else {
        Verdict::Wait { due_at_ms: due }
    }
}

/// When things last happened per box, as the shell saw them while it ran.
/// Nothing is known about the time before the app started, so a box's idle
/// clock starts no earlier than the first time it was seen running.
#[derive(Debug, Default)]
pub struct IdleWatch {
    running_since: HashMap<String, i64>,
    touched: HashMap<String, i64>,
}

impl IdleWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a box's state from a listing. A box seen leaving `running`
    /// starts a fresh clock the next time it runs.
    pub fn observe(&mut self, machine_id: &str, status: MachineStatus, now_ms: i64) {
        if status == MachineStatus::Running {
            self.running_since.entry(machine_id.to_string()).or_insert(now_ms);
        } else {
            self.running_since.remove(machine_id);
        }
    }

    /// Something happened in the box: a terminal, a git action, an Undo.
    pub fn touch(&mut self, machine_id: &str, now_ms: i64) {
        let t = self.touched.entry(machine_id.to_string()).or_insert(now_ms);
        *t = (*t).max(now_ms);
    }

    /// The latest of what this watch saw and `other` signals (agent updates,
    /// setup runs), never earlier than the box was first seen running.
    pub fn last_activity(&self, machine_id: &str, other: impl IntoIterator<Item = i64>, now_ms: i64) -> i64 {
        let since = self.running_since.get(machine_id).copied().unwrap_or(now_ms);
        let touched = self.touched.get(machine_id).copied().unwrap_or(0);
        other.into_iter().fold(since.max(touched), i64::max)
    }
}

/// Where the day's spend figure came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum SpendFigure {
    /// boxd's meter since local midnight. Lags up to ~25 minutes;
    /// `complete` false means the figure can still grow.
    Metered { total_micro: i64, complete: bool },
    /// The meter is not readable with this key: the fleet's current hourly
    /// estimate, projected over a whole day. `why` is the provider's error.
    Projected { hourly_micro: i64, why: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendCheck {
    #[serde(flatten)]
    pub figure: SpendFigure,
    /// The number compared with the limit: metered so far, or a day at the
    /// current rate.
    pub amount_micro: i64,
    pub limit_micro: i64,
    pub currency: String,
    pub over: bool,
}

pub fn check_spend(figure: SpendFigure, limit_micro: i64, currency: &str) -> SpendCheck {
    let amount_micro = match &figure {
        SpendFigure::Metered { total_micro, .. } => *total_micro,
        SpendFigure::Projected { hourly_micro, .. } => hourly_micro.saturating_mul(24),
    };
    SpendCheck { over: limit_micro > 0 && amount_micro >= limit_micro, amount_micro, limit_micro, currency: currency.to_string(), figure }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn idle_box(last: i64) -> BoxActivity {
        BoxActivity { machine_id: "vm_1".into(), status: MachineStatus::Running, agents_busy: 0, setup_active: false, terminals_open: 0, last_activity_ms: last }
    }

    fn policy(mins: u32) -> IdlePolicy {
        IdlePolicy { idle_minutes: mins, opted_out: vec![] }
    }

    #[test]
    fn idle_box_is_suspended_only_after_the_window() {
        let b = idle_box(1_000 * MIN);
        assert_eq!(decide(&b, &policy(30), 1_029 * MIN), Verdict::Wait { due_at_ms: 1_030 * MIN });
        assert_eq!(decide(&b, &policy(30), 1_030 * MIN), Verdict::Suspend { idle_since_ms: 1_000 * MIN });
    }

    #[test]
    fn any_sign_of_work_keeps_the_box() {
        let now = 10_000 * MIN;
        let p = policy(30);
        let mut b = idle_box(0);
        b.agents_busy = 1;
        assert_eq!(decide(&b, &p, now), Verdict::Keep { reason: KeepReason::AgentWorking });
        let mut b = idle_box(0);
        b.setup_active = true;
        assert_eq!(decide(&b, &p, now), Verdict::Keep { reason: KeepReason::SetupRunning });
        let mut b = idle_box(0);
        b.terminals_open = 2;
        assert_eq!(decide(&b, &p, now), Verdict::Keep { reason: KeepReason::TerminalOpen });
    }

    #[test]
    fn off_opted_out_and_not_running_are_left_alone() {
        let now = 10_000 * MIN;
        let b = idle_box(0);
        assert_eq!(decide(&b, &policy(0), now), Verdict::Keep { reason: KeepReason::GuardOff });
        let p = IdlePolicy { idle_minutes: 30, opted_out: vec!["vm_1".into()] };
        assert_eq!(decide(&b, &p, now), Verdict::Keep { reason: KeepReason::OptedOut });
        for s in [MachineStatus::Suspended, MachineStatus::Starting, MachineStatus::Hibernated, MachineStatus::Stopped] {
            let mut b = idle_box(0);
            b.status = s;
            assert_eq!(decide(&b, &policy(30), now), Verdict::Keep { reason: KeepReason::NotRunning });
        }
    }

    #[test]
    fn a_working_agent_holds_the_box_while_its_session_runs() {
        let live = |state, running| AgentLive { id: "a".into(), state, since: 0, running };
        assert!(agent_holds_box(&live(Some(AgentState::Working), true)));
        assert!(!agent_holds_box(&live(Some(AgentState::Working), false)), "the harness exited");
        assert!(!agent_holds_box(&live(Some(AgentState::Waiting), true)));
        assert!(!agent_holds_box(&live(None, true)));
    }

    #[test]
    fn idle_clock_starts_when_the_box_is_first_seen_running() {
        let mut w = IdleWatch::new();
        // Never observed: the clock starts now.
        assert_eq!(w.last_activity("vm_1", [], 50 * MIN), 50 * MIN);
        w.observe("vm_1", MachineStatus::Running, 100 * MIN);
        w.observe("vm_1", MachineStatus::Running, 120 * MIN);
        assert_eq!(w.last_activity("vm_1", [], 200 * MIN), 100 * MIN);
        // Older agent activity does not pull it back; newer moves it on.
        assert_eq!(w.last_activity("vm_1", [10 * MIN], 200 * MIN), 100 * MIN);
        assert_eq!(w.last_activity("vm_1", [150 * MIN], 200 * MIN), 150 * MIN);
        w.touch("vm_1", 170 * MIN);
        w.touch("vm_1", 160 * MIN);
        assert_eq!(w.last_activity("vm_1", [150 * MIN], 200 * MIN), 170 * MIN);
        // Standby and back: a fresh clock.
        w.observe("vm_1", MachineStatus::Suspended, 210 * MIN);
        w.observe("vm_1", MachineStatus::Running, 300 * MIN);
        assert_eq!(w.last_activity("vm_1", [150 * MIN], 301 * MIN), 300 * MIN);
    }

    #[test]
    fn spend_metered_and_projected() {
        let c = check_spend(SpendFigure::Metered { total_micro: 5_200_000, complete: false }, 5_000_000, "eur");
        assert!(c.over);
        assert_eq!(c.amount_micro, 5_200_000);
        let c = check_spend(SpendFigure::Projected { hourly_micro: 182_000, why: "admins only".into() }, 5_000_000, "eur");
        assert_eq!(c.amount_micro, 4_368_000);
        assert!(!c.over);
        let c = check_spend(SpendFigure::Metered { total_micro: 1, complete: true }, 0, "eur");
        assert!(!c.over, "a zero limit never warns");
        let v = serde_json::to_value(check_spend(SpendFigure::Metered { total_micro: 1, complete: true }, 2, "eur")).unwrap();
        assert_eq!(v["basis"], "metered");
        assert_eq!(v["totalMicro"], 1);
        assert_eq!(v["amountMicro"], 1);
    }
}
