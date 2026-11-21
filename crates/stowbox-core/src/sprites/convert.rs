//! Sprites to Stowbox's shapes, and the checks on what the API sends back
//! before it is used.

use crate::boat::convert::parse_time;
use crate::types::*;
use serde_json::Value;

/// Longest name Stowbox gives a sprite. The name is also the front of the
/// sprite's host name (`{name}-{org}.sprites.app`), so it stays well under
/// a DNS label's 63.
pub const MAX_NAME: usize = 40;

/// A sprite name from the API, before it goes into a path: letters, digits,
/// `-` and `_`, up to 63. Sprites documents no rule of its own; a name
/// outside this is left out of the list instead of guessed at.
pub fn valid_name(name: &str) -> bool {
    (1..=63).contains(&name.len()) && !name.starts_with('-') && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// What New box typed, made into a name: lowercase letters, digits and
/// single hyphens, up to [`MAX_NAME`].
pub fn name_from(typed: &str) -> String {
    let mut out = String::new();
    for c in typed.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.chars().take(MAX_NAME).collect();
    out.trim_end_matches('-').to_string()
}

/// `running` is awake. `warm` is paused with memory kept and wakes in well
/// under a second; `cold` has dropped memory and keeps the disk. Both wake
/// on the next exec.
pub fn status(s: &str) -> MachineStatus {
    match s {
        "running" => MachineStatus::Running,
        "warm" => MachineStatus::Suspended,
        "cold" => MachineStatus::Hibernated,
        _ => MachineStatus::Unknown,
    }
}

pub fn machine(v: &Value) -> Machine {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let name = s("name");
    let url = s("url");
    Machine {
        id: name.clone(),
        name,
        provider: super::ID.into(),
        size: super::SIZE.into(),
        status: status(&s("status")),
        vcpu: super::VCPUS,
        memory_bytes: 0,
        disk_bytes: super::DISK_GB * 1_000_000_000,
        public_ip: String::new(),
        access_domain: String::new(),
        // Only an https URL from the API is passed on to the UI.
        url: if url.starts_with("https://") && url.len() < 512 { url } else { String::new() },
        desktop_url: String::new(),
        ssh_port: 0,
        org: s("organization"),
        billing_org: String::new(),
        source: String::new(),
        isolated: false,
        created_at: parse_time(&s("created_at")),
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        auto_stop_at: 0,
        idle_stop_secs: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_sprite_maps_with_its_state() {
        let m = machine(&json!({
            "id": "spr_01", "name": "api-work", "organization": "my-org", "status": "warm",
            "url": "https://api-work-abc.sprites.app", "created_at": "2026-11-21T10:00:00Z",
        }));
        assert_eq!((m.id.as_str(), m.name.as_str(), m.status, m.org.as_str()), ("api-work", "api-work", MachineStatus::Suspended, "my-org"));
        assert_eq!((m.vcpu, m.created_at, m.url.as_str()), (8, 1_795_255_200, "https://api-work-abc.sprites.app"));
        assert_eq!(status("cold"), MachineStatus::Hibernated);
        assert_eq!(status("running"), MachineStatus::Running);
        let odd = machine(&json!({ "name": 5, "status": "melting", "url": "javascript:alert(1)", "created_at": "soon" }));
        assert_eq!((odd.id.as_str(), odd.status, odd.url.as_str(), odd.created_at), ("", MachineStatus::Unknown, "", 0));
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_name("my-sprite") && valid_name("a_b-9"));
        for bad in ["", "-a", "a/b", "../x", "a b", "a?b", "é", &"a".repeat(64)] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert_eq!(name_from("  My Box / 2 "), "my-box-2");
        assert_eq!(name_from("..."), "");
        assert_eq!(name_from(&"a".repeat(200)).len(), MAX_NAME);
        assert!(!name_from(&format!("{}-b", "a".repeat(MAX_NAME - 1))).ends_with('-'));
    }
}
