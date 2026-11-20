//! Modal's sandbox messages to Stowbox's shapes, and the checks on what
//! Modal sends back before it is used.

use super::pb::client::{sandbox_status::State, SandboxInfo};
use crate::types::*;

const MIB: u64 = 1024 * 1024;

/// A sandbox id as Modal mints them (`sb-` and letters and digits). It
/// goes into requests only, never a URL, but anything else is refused.
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("sb-").is_some_and(|rest| (1..=64).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

/// A name Modal accepts for a sandbox: letters, digits, `-`, `_` and `.`,
/// up to 64 characters, starting with a letter or digit (so never `.` or
/// `..`).
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// What New box typed, made into a name Modal accepts.
pub fn name_from(typed: &str) -> String {
    let mut out = String::new();
    for c in typed.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(64).collect()
}

/// vCPUs for a CPU request in thousandths of a physical core. Modal counts
/// a physical core as two vCPUs.
pub fn vcpus(milli_cpu: u32) -> u32 {
    (milli_cpu.saturating_mul(2)).div_ceil(1000)
}

pub fn status(s: &SandboxInfo) -> MachineStatus {
    if let Some(st) = &s.status {
        match State::try_from(st.state) {
            Ok(State::Ready) => return MachineStatus::Running,
            Ok(State::Created | State::Scheduled) => return MachineStatus::Pending,
            Ok(State::Starting) => return MachineStatus::Starting,
            Ok(State::Terminating) => return MachineStatus::Stopping,
            Ok(State::Terminated) => return MachineStatus::Stopped,
            Ok(State::Unspecified) | Err(_) => {}
        }
    }
    // Older answers leave the status out; the task says as much.
    match &s.task_info {
        // A running task carries an empty result.
        Some(t) if t.finished_at > 0.0 || t.result.as_ref().is_some_and(|r| r.status != 0) => MachineStatus::Stopped,
        Some(t) if t.started_at > 0.0 => MachineStatus::Running,
        _ => MachineStatus::Pending,
    }
}

pub fn machine(s: &SandboxInfo) -> Machine {
    let res = s.resource_info.as_ref();
    let milli_cpu = res.and_then(|r| r.milli_cpu.as_ref()).map(|v| v.value).unwrap_or(0);
    let memory_mb = res.and_then(|r| r.memory_mb.as_ref()).map(|v| v.value).unwrap_or(0);
    let created_at = if s.created_at.is_finite() && s.created_at > 0.0 { s.created_at as i64 } else { 0 };
    let size = super::SIZES.iter().find(|z| z.milli_cpu == milli_cpu && z.memory_mib == memory_mb).map(|z| z.id()).unwrap_or_default();
    Machine {
        id: s.id.clone(),
        provider: super::ID.into(),
        size,
        name: if s.name.is_empty() { s.id.clone() } else { s.name.clone() },
        status: status(s),
        vcpu: vcpus(milli_cpu),
        memory_bytes: u64::from(memory_mb).saturating_mul(MIB),
        // Left out unless the sandbox asked for a size of its own.
        disk_bytes: res.map(|r| u64::from(r.ephemeral_disk_mb).saturating_mul(MIB)).filter(|&b| b > 0).unwrap_or(super::DISK_GIB << 30),
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: String::new(),
        source: String::new(),
        isolated: false,
        created_at,
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        // Modal ends a sandbox at its timeout, whatever it is doing.
        auto_stop_at: if created_at > 0 && s.timeout_secs > 0 { created_at.saturating_add(i64::from(s.timeout_secs)) } else { 0 },
        idle_stop_secs: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::super::pb::client::{resource_info::ResourceValue, ResourceInfo, SandboxStatus, TaskInfo};
    use super::*;

    fn info() -> SandboxInfo {
        SandboxInfo {
            id: "sb-dwXsvnH78BrCLTuZBFVKag".into(),
            created_at: 1_795_212_434.5,
            name: "api-work".into(),
            resource_info: Some(ResourceInfo {
                memory_mb: Some(ResourceValue { value: 4096, is_default: false }),
                milli_cpu: Some(ResourceValue { value: 1000, is_default: false }),
                ephemeral_disk_mb: 0,
            }),
            timeout_secs: 86_400,
            task_info: Some(TaskInfo { id: "ta-1".into(), started_at: 1_795_212_440.0, ..Default::default() }),
            ..Default::default()
        }
    }

    #[test]
    fn a_sandbox_maps_with_its_size_and_end() {
        let m = machine(&info());
        assert_eq!((m.status, m.vcpu, m.memory_bytes, m.size.as_str()), (MachineStatus::Running, 2, 4096 * MIB, "2-vcpu"));
        assert_eq!((m.name.as_str(), m.created_at, m.auto_stop_at), ("api-work", 1_795_212_434, 1_795_212_434 + 86_400));
        let unnamed = machine(&SandboxInfo { name: String::new(), ..info() });
        assert_eq!(unnamed.name, unnamed.id);
    }

    #[test]
    fn hostile_numbers_saturate_or_fall_back() {
        let mut s = info();
        s.created_at = f64::NAN;
        s.resource_info = Some(ResourceInfo {
            memory_mb: Some(ResourceValue { value: u32::MAX, is_default: false }),
            milli_cpu: Some(ResourceValue { value: u32::MAX, is_default: false }),
            ephemeral_disk_mb: u32::MAX,
        });
        let m = machine(&s);
        assert_eq!((m.created_at, m.auto_stop_at, m.size.as_str()), (0, 0, ""));
        assert_eq!(m.vcpu, u32::MAX.div_ceil(1000));
        assert_eq!(m.memory_bytes, u64::from(u32::MAX) * MIB);
        // Finite but past i64: the cast saturates and the end must too.
        let far = machine(&SandboxInfo { created_at: 1e300, ..info() });
        assert_eq!((far.created_at, far.auto_stop_at), (i64::MAX, i64::MAX));
    }

    #[test]
    fn the_status_comes_from_the_state_or_else_the_task() {
        let with = |state: State| SandboxInfo { status: Some(SandboxStatus { state: state as i32, ..Default::default() }), ..info() };
        assert_eq!(status(&with(State::Ready)), MachineStatus::Running);
        assert_eq!(status(&with(State::Scheduled)), MachineStatus::Pending);
        assert_eq!(status(&with(State::Terminating)), MachineStatus::Stopping);
        assert_eq!(status(&SandboxInfo { status: Some(SandboxStatus { state: 99, ..Default::default() }), ..info() }), MachineStatus::Running);
        assert_eq!(status(&SandboxInfo { task_info: None, ..info() }), MachineStatus::Pending);
        let done = SandboxInfo { task_info: Some(TaskInfo { started_at: 1.0, finished_at: 2.0, ..Default::default() }), ..info() };
        assert_eq!(status(&done), MachineStatus::Stopped);
        let running = SandboxInfo { task_info: Some(TaskInfo { started_at: 1.0, result: Some(Default::default()), ..Default::default() }), ..info() };
        assert_eq!(status(&running), MachineStatus::Running);
    }

    #[test]
    fn ids_and_names_are_checked() {
        assert!(valid_id("sb-dwXsvnH78BrCLTuZBFVKag"));
        for bad in ["", "sb-", "ta-123", "sb-a/b", "sb-a b", &format!("sb-{}", "a".repeat(65))] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert!(valid_name("api-work.v2_1"));
        assert!(!valid_name("a b") && !valid_name("") && !valid_name(&"a".repeat(65)));
        assert!(!valid_name(".") && !valid_name("..") && !valid_name("-x"));
        assert_eq!(name_from("  My Box / 2 "), "my-box-2");
        assert!(!valid_name(&name_from("..")));
        assert!(valid_name(&name_from(&"x".repeat(200))));
    }
}
