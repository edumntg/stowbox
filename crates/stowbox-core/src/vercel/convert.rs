//! Vercel's JSON to Stowbox's types.
//!
//! A Stowbox box is a Vercel *sandbox*: a name that outlives its sessions.
//! Each start of it is a *session* (one VM boot); stopping a persistent
//! sandbox snapshots its disk, and starting it again boots a new session
//! from that snapshot.

use crate::types::*;
use serde_json::Value;

const MIB: u64 = 1024 * 1024;
/// Every sandbox's disk, whatever its size: Vercel's limits say 64 GB, and
/// `df` in one shows 64 GiB. The API does not report it.
pub const DISK_GIB: u64 = 64;
/// Longest name Stowbox gives a sandbox.
const MAX_NAME: usize = 48;

fn str_at<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn num_at(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Milliseconds since the epoch to seconds.
fn secs(ms: f64) -> i64 {
    if ms.is_finite() && ms > 0.0 {
        (ms / 1000.0) as i64
    } else {
        0
    }
}

pub fn status(state: &str) -> MachineStatus {
    match state {
        "running" => MachineStatus::Running,
        "pending" => MachineStatus::Starting,
        "stopping" | "snapshotting" => MachineStatus::Stopping,
        "stopped" | "aborted" => MachineStatus::Stopped,
        "failed" => MachineStatus::Failed,
        _ => MachineStatus::Unknown,
    }
}

/// A sandbox name as Stowbox makes them and accepts them back: letters,
/// digits, `-` and `_`. Anything else is refused before it reaches a URL.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The name a new sandbox gets: what was asked for, lower-cased with runs
/// of anything else turned into `-`, or `stowbox-` and `suffix` when that
/// leaves nothing.
pub fn name_for(wanted: Option<&str>, suffix: &str) -> String {
    let mut out = String::new();
    for c in wanted.unwrap_or("").trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= MAX_NAME {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        format!("stowbox-{suffix}")
    } else {
        out
    }
}

/// A sandbox, with its current session when the API gave one. The list
/// gives sandboxes alone; then a running box shows no deadline.
pub fn machine(sandbox: &Value, session: Option<&Value>) -> Machine {
    let name = str_at(sandbox, "name").to_string();
    let state = session.map(|s| str_at(s, "status")).filter(|s| !s.is_empty()).unwrap_or(str_at(sandbox, "status"));
    let status = status(state);
    let from = |k: &str| session.map(|s| num_at(s, k)).filter(|n| *n > 0.0).unwrap_or(num_at(sandbox, k));
    let deadline = session.filter(|_| status == MachineStatus::Running).map(|s| {
        let start = Some(num_at(s, "startedAt")).filter(|n| *n > 0.0).unwrap_or(num_at(s, "createdAt"));
        secs(start + num_at(s, "timeout"))
    });
    let image = Some(str_at(sandbox, "image")).filter(|i| !i.is_empty()).unwrap_or(str_at(sandbox, "runtime"));
    Machine {
        id: name.clone(),
        name,
        provider: super::ID.into(),
        status,
        vcpu: from("vcpus") as u32,
        memory_bytes: from("memory") as u64 * MIB,
        disk_bytes: DISK_GIB << 30,
        size: String::new(),
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: String::new(),
        source: if image.is_empty() { "universal".into() } else { image.to_string() },
        isolated: false,
        created_at: secs(num_at(sandbox, "createdAt")),
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        auto_stop_at: deadline.unwrap_or(0),
        idle_stop_secs: secs(num_at(sandbox, "timeout")) as u32,
    }
}

/// The current session's id, when the sandbox has one.
pub fn session_id(answer: &Value) -> Option<String> {
    let id = answer.get("session").map(|s| str_at(s, "id")).filter(|s| !s.is_empty()).or_else(|| answer.get("sandbox").map(|s| str_at(s, "currentSessionId")));
    id.filter(|id| valid_name(id)).map(str::to_string)
}

/// When the session stops unless its timeout is extended, in ms.
pub fn deadline_ms(session: &Value) -> f64 {
    let start = Some(num_at(session, "startedAt")).filter(|n| *n > 0.0).unwrap_or(num_at(session, "createdAt"));
    start + num_at(session, "timeout")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_sandbox_with_its_session_becomes_a_machine() {
        let sandbox = json!({ "name": "api-work", "status": "running", "createdAt": 1_763_632_800_000u64, "timeout": 1_800_000, "currentSessionId": "sbx_abc", "persistent": true });
        let session = json!({ "id": "sbx_abc", "status": "running", "vcpus": 2, "memory": 4096, "createdAt": 1_763_632_800_000u64, "startedAt": 1_763_632_801_000u64, "timeout": 1_800_000 });
        let m = machine(&sandbox, Some(&session));
        assert_eq!((m.id.as_str(), m.name.as_str(), m.provider.as_str(), m.source.as_str()), ("api-work", "api-work", "vercel", "universal"));
        assert_eq!((m.status, m.vcpu, m.memory_bytes, m.idle_stop_secs), (MachineStatus::Running, 2, 4096 * MIB, 1800));
        assert_eq!(m.auto_stop_at, 1_763_632_801 + 1800);
        assert_eq!(session_id(&json!({ "sandbox": sandbox, "session": session })).as_deref(), Some("sbx_abc"));
    }

    #[test]
    fn a_stopped_sandbox_has_no_deadline() {
        let m = machine(&json!({ "name": "old", "status": "stopped", "vcpus": 1, "memory": 2048 }), None);
        assert_eq!((m.status, m.auto_stop_at, m.vcpu), (MachineStatus::Stopped, 0, 1));
        assert_eq!(status("snapshotting"), MachineStatus::Stopping);
        assert_eq!(status("new-state"), MachineStatus::Unknown);
    }

    #[test]
    fn names_are_made_safe_and_hostile_ones_refused() {
        assert_eq!(name_for(Some("API work / 2"), "x1"), "api-work-2");
        assert_eq!(name_for(Some("  ---  "), "x1"), "stowbox-x1");
        assert_eq!(name_for(None, "x1"), "stowbox-x1");
        assert!(name_for(Some(&"a".repeat(200)), "x").len() <= MAX_NAME);
        assert!(valid_name("api-work_2"));
        for bad in ["", "a/b", "a?b", "a b", "../x", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert_eq!(session_id(&json!({ "session": { "id": "../evil" } })), None);
    }
}
