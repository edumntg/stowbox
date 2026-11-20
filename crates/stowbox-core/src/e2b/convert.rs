//! E2B's JSON to Stowbox's types.

use crate::boat::convert::parse_time;
use crate::types::*;
use serde_json::Value;

const MIB: u64 = 1024 * 1024;
/// The metadata key that holds the name Stowbox gave a sandbox: E2B
/// sandboxes have an id and no name.
pub const NAME_KEY: &str = "stowbox-name";

fn str_at<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn num_at(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A paused sandbox keeps its memory in a snapshot and costs nothing to
/// hold, which is what hibernation means here; resuming restores it.
pub fn status(state: &str) -> MachineStatus {
    match state {
        "running" => MachineStatus::Running,
        "paused" => MachineStatus::Hibernated,
        "pausing" => MachineStatus::Stopping,
        "resuming" | "creating" => MachineStatus::Starting,
        _ => MachineStatus::Unknown,
    }
}

/// An E2B sandbox id: lower-case letters and digits. Anything else is
/// refused before it becomes part of a host name.
pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// E2B's own domains. The sandbox's access token and every keystroke go to
/// the domain the API names, so it must be one of these.
const DOMAINS: [&str; 2] = ["e2b.app", "e2b.dev"];

/// The base domain a sandbox's traffic goes to, when E2B names one of its
/// own; `e2b.app` otherwise.
pub fn domain(s: &Value) -> String {
    let d = str_at(s, "domain");
    let own = DOMAINS.iter().any(|o| d == *o || d.strip_suffix(o).is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')));
    let ok = own
        && d.len() <= 253
        && !d.starts_with(['.', '-'])
        && !d.contains("..")
        && d.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    if ok {
        d.to_string()
    } else {
        "e2b.app".into()
    }
}

pub fn machine(s: &Value) -> Machine {
    let id = str_at(s, "sandboxID").to_string();
    let named = s.get("metadata").and_then(|m| m.get(NAME_KEY)).and_then(Value::as_str).filter(|n| !n.is_empty());
    let template = Some(str_at(s, "alias")).filter(|a| !a.is_empty()).unwrap_or(str_at(s, "templateID"));
    Machine {
        name: named.map(str::to_string).unwrap_or_else(|| id.clone()),
        provider: super::ID.into(),
        status: status(str_at(s, "state")),
        vcpu: num_at(s, "cpuCount") as u32,
        memory_bytes: (num_at(s, "memoryMB") as u64).saturating_mul(MIB),
        disk_bytes: (num_at(s, "diskSizeMB") as u64).saturating_mul(MIB),
        size: String::new(),
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: String::new(),
        source: template.to_string(),
        isolated: false,
        created_at: parse_time(str_at(s, "startedAt")),
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        // A running sandbox pauses at `endAt` unless something pushes it out.
        auto_stop_at: if str_at(s, "state") == "running" { parse_time(str_at(s, "endAt")) } else { 0 },
        idle_stop_secs: s.get("metadata").and_then(|m| m.get(super::IDLE_KEY)).and_then(Value::as_str).and_then(|v| v.parse().ok()).unwrap_or(0),
        id,
    }
}

/// The sandbox a fork made, or the reason it did not start. E2B answers a
/// list with one entry per fork asked for.
pub fn forked(results: &Value) -> crate::Result<&Value> {
    let first = results.as_array().and_then(|a| a.first()).ok_or_else(|| crate::Error::Provider("E2B made no fork".into()))?;
    if let Some(s) = first.get("sandbox").filter(|s| s.is_object()) {
        return Ok(s);
    }
    let why = first.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("no reason given");
    Err(crate::Error::Provider(format!("E2B could not start the fork: {why}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_listed_sandbox_becomes_a_machine() {
        let s = json!({
            "sandboxID": "ik3zr4qbbz", "templateID": "rki5dems9wqfm4r03t7g", "alias": "base", "state": "running",
            "cpuCount": 2, "memoryMB": 512, "diskSizeMB": 23670, "startedAt": "2026-11-20T10:00:00Z", "endAt": "2026-11-20T11:00:00Z",
            "metadata": { "stowbox-name": "api-work", "stowbox-idle": "3600" }
        });
        let m = machine(&s);
        assert_eq!((m.id.as_str(), m.name.as_str(), m.provider.as_str(), m.source.as_str()), ("ik3zr4qbbz", "api-work", "e2b", "base"));
        assert_eq!((m.status, m.vcpu, m.memory_bytes), (MachineStatus::Running, 2, 512 * MIB));
        assert_eq!(m.auto_stop_at - m.created_at, 3600);
        assert_eq!(m.idle_stop_secs, 3600);
    }

    #[test]
    fn a_paused_sandbox_is_hibernated_with_no_deadline() {
        let m = machine(&json!({ "sandboxID": "abc", "state": "paused", "endAt": "2026-11-20T11:00:00Z" }));
        assert_eq!((m.status, m.auto_stop_at, m.name.as_str()), (MachineStatus::Hibernated, 0, "abc"));
        assert_eq!(status("something-new"), MachineStatus::Unknown);
    }

    #[test]
    fn hostile_ids_and_domains_never_reach_a_host_name() {
        assert!(valid_id("ik3zr4qbbz"));
        for bad in ["", "a.b", "a/b", "A", "x@evil", "id with space", &"a".repeat(65)] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert_eq!(domain(&json!({ "domain": "e2b.dev" })), "e2b.dev");
        assert_eq!(domain(&json!({ "domain": "eu.e2b.app" })), "eu.e2b.app");
        for bad in ["", "evil", ".e2b.app", "e2b.app/x", "e2b.app:1", "E2B.APP", "attacker.com", "evile2b.app", "e2b.app.evil.com", "a..e2b.app"] {
            assert_eq!(domain(&json!({ "domain": bad })), "e2b.app", "{bad}");
        }
    }

    #[test]
    fn a_fork_answer_gives_its_sandbox_or_its_reason() {
        let ok = json!([{ "sandbox": { "sandboxID": "new1" } }]);
        assert_eq!(str_at(forked(&ok).unwrap(), "sandboxID"), "new1");
        let failed = json!([{ "error": { "code": 503, "message": "no capacity" } }]);
        assert!(forked(&failed).unwrap_err().to_string().contains("no capacity"));
        assert!(forked(&json!([])).is_err());
    }
}
