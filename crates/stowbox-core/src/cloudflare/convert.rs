//! The bridge's boxes to Stowbox's shapes, and the checks on what it sends
//! back before it is used.

use crate::types::*;
use serde_json::Value;

/// A size the bridge deploys: a Cloudflare instance type, as its limits
/// page lists it.
pub struct Size {
    pub id: &'static str,
    pub vcpu: u32,
    pub memory_gib: u64,
    /// Cloudflare counts disk in GB.
    pub disk_gb: u64,
}

pub const SIZES: [Size; 3] = [
    Size { id: "standard-2", vcpu: 1, memory_gib: 6, disk_gb: 12 },
    Size { id: "standard-3", vcpu: 2, memory_gib: 8, disk_gb: 16 },
    Size { id: "standard-4", vcpu: 4, memory_gib: 12, disk_gb: 20 },
];

pub fn size(id: &str) -> Option<&'static Size> {
    SIZES.iter().find(|s| s.id == id)
}

/// A box id as the bridge mints them: `box-` and 20 lowercase letters or
/// digits. It goes into a URL path, so anything else is refused.
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("box-").is_some_and(|rest| rest.len() == 20 && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

/// What New box typed, made into a name the bridge accepts: 1 to 63
/// lowercase letters, digits or hyphens, starting with a letter or digit.
pub fn name_from(typed: &str) -> String {
    let mut out = String::new();
    for c in typed.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.chars().take(63).collect();
    out.trim_end_matches('-').to_string()
}

pub fn machine(v: &Value) -> Machine {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str| v.get(k).and_then(Value::as_i64).filter(|&t| t > 0).unwrap_or(0);
    let id = s("id");
    let size_id = s("size");
    let known = size(&size_id);
    let name = s("name");
    Machine {
        name: if name.is_empty() { id.clone() } else { name },
        id,
        provider: super::ID.into(),
        size: known.map(|z| z.id.to_string()).unwrap_or_default(),
        status: match v.get("status").and_then(Value::as_str) {
            Some("running") => MachineStatus::Running,
            Some("stopped") => MachineStatus::Stopped,
            _ => MachineStatus::Unknown,
        },
        vcpu: known.map(|z| z.vcpu).unwrap_or(0),
        memory_bytes: known.map(|z| z.memory_gib << 30).unwrap_or(0),
        disk_bytes: known.map(|z| z.disk_gb * 1_000_000_000).unwrap_or(0),
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: String::new(),
        source: String::new(),
        isolated: false,
        created_at: n("created_at"),
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        auto_stop_at: 0,
        idle_stop_secs: u32::try_from(n("idle_secs")).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_box_maps_with_its_size() {
        let m = machine(&json!({
            "id": "box-abcdefghij0123456789", "name": "api-work", "size": "standard-3",
            "created_at": 1_795_252_111, "status": "running", "started_at": 1_795_252_111, "idle_secs": 1800,
        }));
        assert_eq!((m.status, m.vcpu, m.memory_bytes, m.disk_bytes, m.size.as_str()), (MachineStatus::Running, 2, 8 << 30, 16_000_000_000, "standard-3"));
        assert_eq!((m.name.as_str(), m.created_at, m.idle_stop_secs), ("api-work", 1_795_252_111, 1800));
        let odd = machine(&json!({ "id": "box-abcdefghij0123456789", "size": "huge", "status": "melting", "created_at": -5, "idle_secs": 1e20 }));
        assert_eq!((odd.status, odd.vcpu, odd.size.as_str(), odd.created_at, odd.idle_stop_secs), (MachineStatus::Unknown, 0, "", 0, 0));
        assert_eq!(odd.name, odd.id);
        assert_eq!(machine(&json!({ "status": "stopped" })).status, MachineStatus::Stopped);
    }

    #[test]
    fn ids_and_names_are_checked() {
        assert!(valid_id("box-abcdefghij0123456789"));
        for bad in ["", "box-", "box-ABCDEFGHIJ0123456789", "box-abcdefghij012345678", "box-abcdefghij0123456789x", "box-abcdefghij01234567/9", "../registry"] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert_eq!(name_from("  My Box / 2 "), "my-box-2");
        assert_eq!(name_from("--x--"), "x");
        assert_eq!(name_from("..."), "");
        assert_eq!(name_from(&"a".repeat(200)).len(), 63);
        assert!(!name_from(&format!("{}-b", "a".repeat(62))).ends_with('-'));
    }
}
