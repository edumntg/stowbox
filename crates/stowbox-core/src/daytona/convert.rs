//! Daytona's JSON to Stowbox's types.

use crate::boat::convert::parse_time;
use crate::types::*;
use serde_json::Value;

const GIB: u64 = 1024 * 1024 * 1024;

fn str_at<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn num_at(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Daytona's sandbox states. Snapshots, forks and resizes keep the box busy
/// but not down, as a migration does on boxd.
pub fn status(state: &str) -> MachineStatus {
    match state {
        "started" => MachineStatus::Running,
        "creating" | "restoring" | "starting" | "pulling_snapshot" | "building_snapshot" | "pending_build" | "resuming" => MachineStatus::Starting,
        "snapshotting" | "forking" | "resizing" => MachineStatus::Migrating,
        "stopping" | "archiving" | "pausing" | "destroying" => MachineStatus::Stopping,
        "stopped" | "archived" => MachineStatus::Stopped,
        "paused" => MachineStatus::Suspended,
        "error" | "build_failed" => MachineStatus::Failed,
        _ => MachineStatus::Unknown,
    }
}

/// A sandbox that is gone or on its way out, which the list leaves out.
pub fn is_gone(s: &Value) -> bool {
    matches!(str_at(s, "state"), "destroyed" | "destroying") || str_at(s, "desiredState") == "destroyed"
}

pub fn machine(s: &Value) -> Machine {
    let id = str_at(s, "id").to_string();
    Machine {
        name: Some(str_at(s, "name")).filter(|n| !n.is_empty()).unwrap_or(&id).to_string(),
        provider: "daytona".into(),
        status: status(str_at(s, "state")),
        vcpu: num_at(s, "cpu") as u32,
        // Daytona counts memory and disk in GiB.
        memory_bytes: (num_at(s, "memory") * GIB as f64) as u64,
        disk_bytes: (num_at(s, "disk") * GIB as f64) as u64,
        size: String::new(),
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: str_at(s, "organizationId").to_string(),
        source: str_at(s, "snapshot").to_string(),
        isolated: false,
        created_at: parse_time(str_at(s, "createdAt")),
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: parse_time(str_at(s, "lastActivityAt")) * 1000,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        auto_stop_at: 0,
        idle_stop_secs: (num_at(s, "autoStopInterval") * 60.0) as u32,
        id,
    }
}

/// The organization the key belongs to, from `GET /organizations`. Daytona
/// keeps no balance an API key can read, so the billing part says only
/// what the organization allows.
pub fn account(orgs: &Value) -> Account {
    let org = orgs.as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
    let name = Some(str_at(&org, "name")).filter(|n| !n.is_empty()).unwrap_or("Daytona organization").to_string();
    let ceiling = |k: &str, fallback: f64| {
        let n = num_at(&org, k);
        if n > 0.0 {
            n
        } else {
            fallback
        }
    };
    Account {
        user_id: str_at(&org, "id").to_string(),
        display_name: name,
        billing: Billing {
            mode: String::new(),
            currency: "usd".into(),
            balance_micro: 0,
            accruing_micro: None,
            max_vms: 0,
            default_vcpu: 1,
            default_memory_bytes: GIB,
            ceiling_vcpu: ceiling("maxCpuPerSandbox", 4.0) as u32,
            ceiling_memory_bytes: (ceiling("maxMemoryPerSandbox", 8.0) * GIB as f64) as u64,
            subscription_status: String::new(),
        },
    }
}

/// Host, port and user from the `sshCommand` Daytona hands out with a
/// token (`ssh <token>@ssh.app.daytona.io`, maybe with `-p <port>`).
pub fn ssh_target(command: &str) -> Option<(String, u16)> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let host = words.iter().find_map(|w| w.split_once('@').map(|(_, h)| h.to_string())).filter(|h| !h.is_empty())?;
    let port = words.windows(2).find(|w| w[0] == "-p").and_then(|w| w[1].parse::<u16>().ok()).unwrap_or(22);
    Some((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_sandbox_becomes_a_machine() {
        let s = json!({
            "id": "a1b2c3", "name": "api", "state": "started", "cpu": 2, "memory": 4, "disk": 10,
            "organizationId": "org_1", "snapshot": "daytonaio/sandbox:0.4.3", "autoStopInterval": 15,
            "createdAt": "2026-11-14T10:20:30.123Z", "lastActivityAt": "2026-11-14T10:20:30Z", "user": "daytona"
        });
        let m = machine(&s);
        assert_eq!((m.id.as_str(), m.name.as_str(), m.provider.as_str(), m.status), ("a1b2c3", "api", "daytona", MachineStatus::Running));
        assert_eq!((m.vcpu, m.memory_bytes, m.disk_bytes), (2, 4 * GIB, 10 * GIB));
        assert_eq!((m.idle_stop_secs, m.created_at, m.last_connected_ms), (900, 1_794_651_630, 1_794_651_630_000));
        // No name: the id stands in.
        assert_eq!(machine(&json!({ "id": "x9", "state": "stopped" })).name, "x9");
    }

    #[test]
    fn states_map_to_what_the_ui_knows() {
        assert_eq!(status("archived"), MachineStatus::Stopped);
        assert_eq!(status("paused"), MachineStatus::Suspended);
        assert_eq!(status("pulling_snapshot"), MachineStatus::Starting);
        assert_eq!(status("build_failed"), MachineStatus::Failed);
        assert_eq!(status("archiving"), MachineStatus::Stopping);
        assert_eq!(status("something_new"), MachineStatus::Unknown);
        assert!(is_gone(&json!({ "state": "destroyed" })) && is_gone(&json!({ "state": "stopped", "desiredState": "destroyed" })));
        assert!(!is_gone(&json!({ "state": "stopped", "desiredState": "stopped" })));
    }

    #[test]
    fn the_account_is_the_organization() {
        let a = account(&json!([{ "id": "org_1", "name": "acme", "maxCpuPerSandbox": 4, "maxMemoryPerSandbox": 8 }]));
        assert_eq!((a.user_id.as_str(), a.display_name.as_str(), a.billing.ceiling_vcpu, a.billing.ceiling_memory_bytes), ("org_1", "acme", 4, 8 * GIB));
        assert_eq!(account(&Value::Null).display_name, "Daytona organization");
    }

    #[test]
    fn the_ssh_command_names_the_gateway() {
        assert_eq!(ssh_target("ssh tok123@ssh.app.daytona.io"), Some(("ssh.app.daytona.io".into(), 22)));
        assert_eq!(ssh_target("ssh -p 2222 tok123@gw.example"), Some(("gw.example".into(), 2222)));
        assert_eq!(ssh_target("nothing here"), None);
    }
}
