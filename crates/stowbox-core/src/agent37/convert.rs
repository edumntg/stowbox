//! Agent37's JSON to Stowbox's types.

use crate::boat::convert::dollars_micro;
use crate::types::*;
use serde_json::Value;

const GIB: u64 = 1024 * 1024 * 1024;

fn str_at<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn i64_at(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

/// Agent37's instance states. A sleeping instance is checkpointed with its
/// disk kept and wakes on the next connection, as a hibernated box does.
pub fn status(s: &str) -> MachineStatus {
    match s {
        "running" => MachineStatus::Running,
        "provisioning" | "starting" | "restarting" | "waking" => MachineStatus::Starting,
        "updating" => MachineStatus::Migrating,
        "stopping" | "deleting" => MachineStatus::Stopping,
        "stopped" => MachineStatus::Stopped,
        "sleeping" => MachineStatus::Hibernated,
        "failed" => MachineStatus::Failed,
        _ => MachineStatus::Unknown,
    }
}

/// An instance that is gone or on its way out, which the list leaves out.
pub fn is_gone(i: &Value) -> bool {
    matches!(str_at(i, "status"), "deleted" | "deleting")
}

/// The rate-card size of an instance whose compute costs a multiple of the
/// default rate (`vm`, `performance`); "" for a default instance.
pub fn size_key(kind: &str, cpu: i64, memory_gb: i64) -> String {
    match kind {
        "" | "default" => String::new(),
        k => format!("{k}-{cpu}x{memory_gb}"),
    }
}

pub fn machine(i: &Value) -> Machine {
    let id = str_at(i, "id").to_string();
    let res = i.get("resources").cloned().unwrap_or(Value::Null);
    let (cpu, mem, disk) = (i64_at(&res, "cpu"), i64_at(&res, "memory"), i64_at(&res, "disk"));
    let auto_sleep = i.get("auto_sleep").and_then(Value::as_bool).unwrap_or(false);
    Machine {
        name: Some(str_at(i, "name")).filter(|n| !n.is_empty()).unwrap_or(&id).to_string(),
        provider: "agent37".into(),
        status: status(str_at(i, "status")),
        vcpu: cpu.max(0) as u32,
        // Agent37 says GB; `free` in the box shows them as GiB.
        memory_bytes: mem.max(0) as u64 * GIB,
        disk_bytes: disk.max(0) as u64 * GIB,
        size: size_key(str_at(i, "type"), cpu, mem),
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: String::new(),
        source: str_at(i, "template").to_string(),
        isolated: false,
        created_at: i64_at(i, "created"),
        hibernated_at_ms: i64_at(i, "slept_at") * 1000,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        auto_stop_at: 0,
        idle_stop_secs: if auto_sleep { i64_at(i, "idle_timeout_seconds").max(0) as u32 } else { 0 },
        id,
    }
}

/// `GET /v1/billing`. Agent37 has no account name a key can read; the
/// workspace is the key's own.
pub fn account(b: &Value) -> Account {
    Account {
        user_id: String::new(),
        display_name: "Agent37 workspace".into(),
        billing: Billing {
            mode: "wallet".into(),
            currency: "usd".into(),
            balance_micro: dollars_micro(b.get("balance_usd").and_then(Value::as_f64).unwrap_or(0.0)),
            accruing_micro: None,
            max_vms: i64_at(b, "instance_limit").max(0) as u32,
            default_vcpu: 2,
            default_memory_bytes: 4 * GIB,
            ceiling_vcpu: 4,
            ceiling_memory_bytes: 32 * GIB,
            subscription_status: if b.get("past_due").and_then(Value::as_bool).unwrap_or(false) { "past_due".into() } else { String::new() },
        },
    }
}

pub fn org_billing(b: &Value, awake: u32) -> OrgBilling {
    OrgBilling {
        id: String::new(),
        name: "Agent37 workspace".into(),
        is_admin: true,
        currency: "usd".into(),
        mode: "wallet".into(),
        balance_micro: dollars_micro(b.get("balance_usd").and_then(Value::as_f64).unwrap_or(0.0)),
        accruing_micro: None,
        vm_count: awake,
        max_vms: i64_at(b, "instance_limit").max(0) as u32,
    }
}

/// One instance's line of `GET /v1/usage`: compute only, since managed
/// models and tools are not the box's running cost.
pub fn machine_usage(u: &Value, id: &str, cpu: u32, memory_gib: f64, since: i64, until: i64) -> Usage {
    let line = u.get("instances").and_then(Value::as_array).and_then(|a| a.iter().find(|l| str_at(l, "id") == id)).cloned().unwrap_or(Value::Null);
    let minutes = i64_at(&line, "awake_minutes").max(0) as u64;
    let hours = minutes as f64 / 60.0;
    Usage {
        period_start: since,
        period_end: until,
        currency: "usd".into(),
        rates: Rates::default(),
        vcpu_hours: hours * cpu as f64,
        ram_gib_hours: hours * memory_gib,
        disk_gib_hours: 0.0,
        cost_micro: i64_at(&line, "compute_micros"),
        seconds: StateSeconds { running: minutes * 60, standby: 0, hibernated: 0, stopped: 0 },
        // Metered minutes settle at least hourly.
        complete: false,
    }
}

/// The workspace's compute over the days of `GET /v1/usage`.
pub fn org_usage(u: &Value, since: i64, until: i64) -> OrgUsage {
    let total = u.get("days").and_then(Value::as_array).map(|d| d.iter().map(|day| i64_at(day, "compute_micros")).sum()).unwrap_or(0);
    OrgUsage { period_start: since, period_end: until, currency: "usd".into(), total_cost_micro: total, complete: false }
}

/// Unix seconds as the UTC day `YYYY-MM-DD`.
pub fn utc_day(unix: i64) -> String {
    // Howard Hinnant's days-to-civil.
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An instance as `POST /v1/instances` answered it (2026-11-16).
    fn created() -> Value {
        json!({"id":"uwop3g494x","status":"running","status_reason":null,"template":"agent37-base","template_revision":null,"image_ref":"ghcr.io/agent37-platform/base:2026.10.24a","type":"default","region":"us","mount_path":"/home/node","resources":{"cpu":2,"memory":4,"disk":4},"url":"https://uwop3g494x.agent37.app","domain_urls":[],"public_ports":[],"name":"stowbox-probe","metadata":null,"auto_sleep":true,"idle_timeout_seconds":600,"network":"allow-all","past_due":false,"created":1794859624})
    }

    #[test]
    fn an_instance_becomes_a_machine() {
        let m = machine(&created());
        assert_eq!((m.id.as_str(), m.name.as_str(), m.provider.as_str()), ("uwop3g494x", "stowbox-probe", "agent37"));
        assert_eq!((m.status, m.vcpu, m.memory_bytes, m.disk_bytes), (MachineStatus::Running, 2, 4 * GIB, 4 * GIB));
        assert_eq!((m.size.as_str(), m.source.as_str(), m.idle_stop_secs, m.created_at), ("", "agent37-base", 600, 1794859624));

        let mut v = created();
        v["status"] = json!("sleeping");
        v["slept_at"] = json!(1794860000);
        v["auto_sleep"] = json!(false);
        v["type"] = json!("performance");
        v["name"] = Value::Null;
        let m = machine(&v);
        assert_eq!((m.status, m.hibernated_at_ms, m.idle_stop_secs), (MachineStatus::Hibernated, 1794860000000, 0));
        assert_eq!((m.name.as_str(), m.size.as_str()), ("uwop3g494x", "performance-2x4"));
    }

    #[test]
    fn states_map_and_deleted_ones_are_gone() {
        assert_eq!(status("waking"), MachineStatus::Starting);
        assert_eq!(status("updating"), MachineStatus::Migrating);
        assert_eq!(status("stopped"), MachineStatus::Stopped);
        assert!(is_gone(&json!({"status":"deleting"})) && !is_gone(&created()));
    }

    #[test]
    fn the_wallet_becomes_the_balance() {
        let b = json!({"balance_usd":5,"tier":0,"memory_limit_gb":32,"instance_limit":8,"auto_top_up":null,"past_due":false});
        let a = account(&b);
        assert_eq!((a.billing.balance_micro, a.billing.max_vms, a.billing.currency.as_str()), (5_000_000, 8, "usd"));
        assert_eq!(org_billing(&b, 2).vm_count, 2);
    }

    #[test]
    fn usage_takes_the_instance_line_and_compute_only() {
        let u = json!({"days":[{"date":"2026-11-15","compute_micros":1200,"llm_micros":900},{"date":"2026-11-16","compute_micros":300}],
            "instances":[{"id":"other","compute_micros":7},{"id":"uwop3g494x","compute_micros":1500,"llm_micros":900,"awake_minutes":90}]});
        let m = machine_usage(&u, "uwop3g494x", 2, 4.0, 10, 20);
        assert_eq!((m.cost_micro, m.seconds.running), (1500, 5400));
        assert_eq!((m.vcpu_hours, m.ram_gib_hours), (3.0, 6.0));
        assert_eq!(machine_usage(&u, "nope", 2, 4.0, 0, 0).cost_micro, 0);
        assert_eq!(org_usage(&u, 0, 0).total_cost_micro, 1500);
    }

    #[test]
    fn days_are_utc() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(1794859624), "2026-11-16");
        assert_eq!(utc_day(951_782_400), "2000-02-29");
    }
}
