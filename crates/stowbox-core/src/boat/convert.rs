//! boat's JSON → provider-neutral types. Every field is read leniently: a
//! missing or null one becomes the type's empty value, never an error.

use super::SIZES;
use crate::types::*;
use serde_json::Value;

const GIB: u64 = 1024 * 1024 * 1024;

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// boat's states → Stowbox's. The API says `archived` for what the CLI
/// calls stopped.
pub fn status(state: &str) -> MachineStatus {
    match state {
        "init" | "provisioning" | "provisioned" | "cloning" => MachineStatus::Starting,
        "ready" | "idle" | "running" => MachineStatus::Running,
        "archiving" => MachineStatus::Stopping,
        "archived" => MachineStatus::Stopped,
        "error" | "cancelled" => MachineStatus::Failed,
        _ => MachineStatus::Unknown,
    }
}

pub fn machine(s: &Value) -> Machine {
    let size = str_at(s, "type").to_string();
    let menu = SIZES.iter().find(|z| z.id == size);
    let vcpu = s.get("vcpu").and_then(Value::as_u64).map(|n| n as u32).or(menu.map(|z| z.vcpu)).unwrap_or(0);
    let memory_bytes = s.get("memoryGB").and_then(Value::as_u64).map(|g| g * GIB).or(menu.map(|z| z.memory_gib * GIB)).unwrap_or(0);
    let state = status(str_at(s, "state"));
    let team = s.get("team").cloned().unwrap_or(Value::Null);
    // A deadline only means something while the sandbox runs.
    let auto_stop_at = match state {
        MachineStatus::Running | MachineStatus::Starting => parse_time(str_at(s, "archiveAfter")),
        _ => 0,
    };
    Machine {
        id: str_at(s, "id").to_string(),
        provider: "boat".into(),
        name: Some(str_at(s, "name")).filter(|n| !n.is_empty()).unwrap_or(str_at(s, "id")).to_string(),
        status: state,
        vcpu,
        memory_bytes,
        disk_bytes: menu.map(|z| z.disk_gb * 1_000_000_000).unwrap_or(0),
        size,
        public_ip: str_at(s, "ip").to_string(),
        access_domain: "on.boat.dev".into(),
        url: str_at(s, "url").to_string(),
        desktop_url: str_at(s, "desktopUrl").to_string(),
        ssh_port: 0,
        org: str_at(&team, "name").to_string(),
        billing_org: str_at(&team, "id").to_string(),
        source: String::new(),
        isolated: false,
        created_at: parse_time(str_at(s, "createdAt")),
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
        auto_stop_at,
    }
}

/// `GET /me` and `GET /limits` together.
pub fn account(me: &Value, limits: &Value) -> Account {
    let user = me.get("user").cloned().unwrap_or(Value::Null);
    let id = Some(str_at(&user, "id")).filter(|s| !s.is_empty()).unwrap_or(str_at(&user, "login")).to_string();
    let name = [str_at(&user, "displayName"), str_at(&user, "login"), str_at(&user, "email")].into_iter().find(|s| !s.is_empty()).unwrap_or(&id).to_string();
    let balance = limits.pointer("/wallet/balanceDollars").and_then(Value::as_f64);
    Account {
        user_id: id,
        display_name: name,
        billing: Billing {
            // A wallet in dollars that pays past the plan's included time.
            mode: if balance.is_some() { "wallet".into() } else { String::new() },
            currency: "usd".into(),
            balance_micro: dollars_micro(balance.unwrap_or(0.0)),
            accruing_micro: None,
            max_vms: u64_at(limits, "maxActiveSandboxes") as u32,
            default_vcpu: 4,
            default_memory_bytes: 8 * GIB,
            ceiling_vcpu: 0,
            ceiling_memory_bytes: 0,
            subscription_status: str_at(limits, "accessTier").to_string(),
        },
    }
}

pub fn org_billing(limits: &Value) -> OrgBilling {
    let balance = limits.pointer("/wallet/balanceDollars").and_then(Value::as_f64);
    OrgBilling {
        id: str_at(limits, "teamId").to_string(),
        name: String::new(),
        is_admin: false,
        currency: "usd".into(),
        mode: if balance.is_some() { "wallet".into() } else { String::new() },
        balance_micro: dollars_micro(balance.unwrap_or(0.0)),
        accruing_micro: None,
        vm_count: u64_at(limits, "activeSandboxes") as u32,
        max_vms: u64_at(limits, "maxActiveSandboxes") as u32,
    }
}

/// `GET /sandboxes/{id}/usage`: billable seconds and their list price.
pub fn usage(u: &Value) -> Usage {
    let seconds = u64_at(u, "seconds");
    let dollars = u.get("dollars").and_then(Value::as_f64).unwrap_or(0.0);
    Usage {
        period_start: parse_time(str_at(u, "since")),
        period_end: parse_time(str_at(u, "until")),
        currency: "usd".into(),
        rates: Rates::default(),
        vcpu_hours: 0.0,
        ram_gib_hours: 0.0,
        disk_gib_hours: 0.0,
        cost_micro: dollars_micro(dollars),
        seconds: StateSeconds { running: seconds, standby: 0, hibernated: 0, stopped: 0 },
        // boat counts by the second; nothing arrives late.
        complete: true,
    }
}

pub fn dollars_micro(d: f64) -> i64 {
    if d.is_finite() {
        (d * 1_000_000.0).round() as i64
    } else {
        0
    }
}

/// RFC 3339 (`2026-11-14T10:20:30.123Z`, or with an offset) → unix seconds;
/// 0 for anything else.
pub fn parse_time(s: &str) -> i64 {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return 0;
    }
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|t| t.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (num(0..4), num(5..7), num(8..10), num(11..13), num(14..16), num(17..19)) else {
        return 0;
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return 0;
    }
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "Z" | "z" | "" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && &o[3..4] == ":" => {
            let (Some(oh), Some(om)) = (o[1..3].parse::<i64>().ok(), o[4..6].parse::<i64>().ok()) else { return 0 };
            let v = oh * 3600 + om * 60;
            if o.starts_with('-') {
                -v
            } else {
                v
            }
        }
        _ => return 0,
    };
    days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn times_parse_with_fractions_and_offsets() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), 0);
        assert_eq!(parse_time("2026-11-14T10:20:30.123Z"), 1_794_651_630);
        assert_eq!(parse_time("2026-11-14T12:20:30+02:00"), 1_794_651_630);
        assert_eq!(parse_time("2000-02-29T00:00:00Z"), 951_782_400);
        assert_eq!(parse_time(""), 0);
        assert_eq!(parse_time("yesterday"), 0);
        assert_eq!(parse_time("2026-13-01T00:00:00Z"), 0);
    }

    #[test]
    fn states_map_to_stowbox_states() {
        assert_eq!(status("provisioning"), MachineStatus::Starting);
        assert_eq!(status("idle"), MachineStatus::Running);
        assert_eq!(status("running"), MachineStatus::Running);
        assert_eq!(status("archiving"), MachineStatus::Stopping);
        assert_eq!(status("archived"), MachineStatus::Stopped);
        assert_eq!(status("error"), MachineStatus::Failed);
        assert_eq!(status("something-new"), MachineStatus::Unknown);
    }

    #[test]
    fn a_sandbox_becomes_a_machine() {
        let s = json!({
            "id": "bx_f7k2q9hd", "name": "api", "state": "ready", "type": "default",
            "vcpu": 4, "memoryGB": 8, "ip": "203.0.113.10", "sshEndpoint": null,
            "createdAt": "2026-11-14T10:20:30Z", "archiveAfter": "2026-11-14T11:20:30Z",
            "ttlSeconds": 3600, "desktopUrl": null, "team": { "id": "team_1", "name": "Acme" },
            "url": "https://api-x.on.boat.dev"
        });
        let m = machine(&s);
        assert_eq!((m.id.as_str(), m.name.as_str(), m.provider.as_str(), m.size.as_str()), ("bx_f7k2q9hd", "api", "boat", "default"));
        assert_eq!((m.status, m.vcpu, m.memory_bytes), (MachineStatus::Running, 4, 8 * GIB));
        assert_eq!(m.disk_bytes, 51_000_000_000);
        assert_eq!((m.public_ip.as_str(), m.org.as_str(), m.billing_org.as_str()), ("203.0.113.10", "Acme", "team_1"));
        assert_eq!(m.created_at, 1_794_651_630);
        assert_eq!(m.auto_stop_at, 1_794_651_630 + 3600);
        assert_eq!(m.desktop_url, "");
    }

    #[test]
    fn a_stopped_sandbox_has_no_deadline_and_a_bare_one_still_reads() {
        let m = machine(&json!({ "id": "bx_1", "state": "archived", "type": "small", "archiveAfter": "2026-11-14T11:20:30Z" }));
        assert_eq!((m.status, m.auto_stop_at, m.name.as_str()), (MachineStatus::Stopped, 0, "bx_1"));
        // Sizes come from the menu when the sandbox does not say.
        assert_eq!((m.vcpu, m.memory_bytes), (2, 4 * GIB));
        let m = machine(&json!({}));
        assert_eq!((m.status, m.vcpu), (MachineStatus::Unknown, 0));
    }

    #[test]
    fn account_takes_the_wallet_from_limits() {
        let me = json!({ "ok": true, "user": { "id": "u_1", "login": "ana", "displayName": "" } });
        let limits = json!({ "ok": true, "maxActiveSandboxes": 100, "activeSandboxes": 3, "accessTier": "paid", "wallet": { "balanceDollars": 52.4 } });
        let a = account(&me, &limits);
        assert_eq!((a.user_id.as_str(), a.display_name.as_str()), ("u_1", "ana"));
        assert_eq!((a.billing.mode.as_str(), a.billing.currency.as_str(), a.billing.balance_micro, a.billing.max_vms), ("wallet", "usd", 52_400_000, 100));
        // An unlimited account has no wallet: no balance is invented.
        let a = account(&me, &json!({ "wallet": null }));
        assert_eq!((a.billing.mode.as_str(), a.billing.balance_micro), ("", 0));
        assert_eq!(org_billing(&limits).vm_count, 3);
    }

    #[test]
    fn usage_is_seconds_and_dollars() {
        let u = usage(&json!({ "seconds": 4980, "dollars": 0.0498, "since": "2026-11-01T00:00:00Z", "until": "2026-11-14T00:00:00Z" }));
        assert_eq!((u.seconds.running, u.cost_micro, u.currency.as_str(), u.complete), (4980, 49_800, "usd", true));
        assert!(u.period_end > u.period_start);
    }
}
