//! Blaxel's JSON to Stowbox's types.

use crate::boat::convert::parse_time;
use crate::types::*;
use serde_json::Value;

const MIB: u64 = 1024 * 1024;
/// The domains a sandbox's own API may live under. The API key goes to that
/// URL, so a URL elsewhere is refused rather than trusted.
const SANDBOX_DOMAINS: [&str; 2] = [".bl.run", ".blaxel.ai"];

fn str_at<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A sandbox in standby keeps its memory and processes and costs nothing
/// but storage, which is what hibernation means here; any call to it wakes
/// it in milliseconds. An archived one kept only its disk.
pub fn status(status: &str, state: &str) -> MachineStatus {
    match status {
        "DEPLOYED" if state == "STANDBY" => MachineStatus::Hibernated,
        "DEPLOYED" => MachineStatus::Running,
        "DEPLOYING" | "UPLOADING" | "BUILDING" | "BUILT" | "UNARCHIVING" => MachineStatus::Starting,
        "DELETING" | "DEACTIVATING" | "ARCHIVING" => MachineStatus::Stopping,
        "ARCHIVED" | "DEACTIVATED" | "TERMINATED" => MachineStatus::Stopped,
        "FAILED" => MachineStatus::Failed,
        _ => MachineStatus::Unknown,
    }
}

/// A sandbox name: lower-case letters, digits and `-`, as Blaxel requires.
/// Anything else is refused before it reaches a URL.
pub fn valid_name(name: &str) -> bool {
    (1..=49).contains(&name.len()) && !name.starts_with('-') && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The name a new sandbox gets: what was asked for, lower-cased with runs
/// of anything else turned into `-` and room left for `suffix`.
pub fn name_for(wanted: Option<&str>, suffix: &str) -> String {
    let mut out = String::new();
    for c in wanted.unwrap_or("").trim().chars() {
        if out.len() >= 40 {
            break;
        }
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        format!("stowbox-{suffix}")
    } else {
        out.to_string()
    }
}

/// vCPUs Blaxel gives a memory size: one per 2 GiB up to 8 GiB, six at
/// 16 GiB (its docs; a 4 GiB sandbox shows two in `nproc`).
pub fn vcpus_for(memory_mib: u64) -> u32 {
    match memory_mib {
        0 => 0,
        m if m >= 16_384 => 6,
        m => (m / 2048).max(1) as u32,
    }
}

/// The sandbox's own API, when its URL is https on a Blaxel domain.
pub fn sandbox_url(s: &Value) -> Option<String> {
    let url = s.get("metadata").map(|m| str_at(m, "url")).unwrap_or("").trim_end_matches('/');
    let host = url.strip_prefix("https://")?;
    let ok = host.len() <= 253
        && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && SANDBOX_DOMAINS.iter().any(|d| host.ends_with(d) && host.len() > d.len());
    ok.then(|| url.to_string())
}

pub fn machine(s: &Value) -> Machine {
    let meta = s.get("metadata").unwrap_or(&Value::Null);
    let runtime = s.get("spec").and_then(|sp| sp.get("runtime")).unwrap_or(&Value::Null);
    let name = str_at(meta, "name").to_string();
    let memory_mib = runtime.get("memory").and_then(Value::as_u64).unwrap_or(0);
    let display = Some(str_at(meta, "displayName")).filter(|d| !d.is_empty());
    let size = super::size_id(memory_mib);
    Machine {
        id: name.clone(),
        name: display.map(str::to_string).unwrap_or(name),
        provider: super::ID.into(),
        status: status(str_at(s, "status"), str_at(s, "state")),
        vcpu: vcpus_for(memory_mib),
        memory_bytes: memory_mib.saturating_mul(MIB),
        // The disk is a tmpfs sized from memory; Blaxel does not report it.
        disk_bytes: 0,
        size: if super::SIZES_GIB.iter().any(|g| super::size_id(g * 1024) == size) { size } else { String::new() },
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: String::new(),
        source: str_at(runtime, "image").to_string(),
        isolated: false,
        created_at: parse_time(str_at(meta, "createdAt")),
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
    fn a_listed_sandbox_becomes_a_machine() {
        let s = json!({
            "metadata": { "name": "api-work", "url": "https://sbx-api-work-abc.us-pdx-1.bl.run", "createdAt": "2026-11-20T17:18:38.547317131Z" },
            "spec": { "runtime": { "image": "blaxel/base-image:latest", "memory": 4096 } },
            "status": "DEPLOYED", "state": "STANDBY"
        });
        let m = machine(&s);
        assert_eq!((m.id.as_str(), m.name.as_str(), m.provider.as_str(), m.size.as_str()), ("api-work", "api-work", "blaxel", "4-gib"));
        assert_eq!((m.status, m.vcpu, m.memory_bytes), (MachineStatus::Hibernated, 2, 4096 * MIB));
        assert!(m.created_at > 0);
        assert_eq!(sandbox_url(&s).as_deref(), Some("https://sbx-api-work-abc.us-pdx-1.bl.run"));
    }

    #[test]
    fn states_map_and_sizes_give_their_cpus() {
        assert_eq!(status("DEPLOYED", ""), MachineStatus::Running);
        assert_eq!(status("ARCHIVED", ""), MachineStatus::Stopped);
        assert_eq!(status("UNARCHIVING", ""), MachineStatus::Starting);
        assert_eq!(status("NEW", ""), MachineStatus::Unknown);
        assert_eq!([2048, 4096, 8192, 16_384, 1024, 0].map(vcpus_for), [1, 2, 4, 6, 1, 0]);
    }

    #[test]
    fn a_sandbox_url_off_blaxel_is_refused() {
        let at = |u: &str| sandbox_url(&json!({ "metadata": { "url": u } }));
        for bad in
            ["http://x.bl.run", "https://evil.com", "https://bl.run", "https://x.bl.run.evil.com", "https://x.bl.run/@evil.com", "https://x.bl.run:8443", ""]
        {
            assert_eq!(at(bad), None, "{bad}");
        }
        assert_eq!(at("https://x.bl.run/").as_deref(), Some("https://x.bl.run"));
    }

    #[test]
    fn names_are_made_safe_and_hostile_ones_refused() {
        assert_eq!(name_for(Some("API work / 2"), "x1"), "api-work-2");
        assert_eq!(name_for(None, "x1"), "stowbox-x1");
        assert!(name_for(Some(&"a".repeat(200)), "x").len() <= 49);
        assert!(valid_name("api-work-2"));
        for bad in ["", "A", "a_b", "a/b", "-a", &"a".repeat(50)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
