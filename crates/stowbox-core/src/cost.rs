//! Live cost estimates. The provider meters with a lag of up to ~25 minutes,
//! so the sidebar shows an estimate from the rate card and the machine's
//! current state, and the Details view shows the metered figure.

use crate::types::{Machine, MachineStatus, Rates};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// boxd's published rate card (EUR, 2026-10): the fallback until the live
/// rates have been fetched once.
pub const BOXD_PUBLISHED_EUR: Rates = Rates {
    vcpu_hour_micro: 39_000,
    ram_gib_hour_micro: 12_000,
    disk_gib_hour_micro: 80,
};

/// Estimated cost per hour, in micro-units of the rate card's currency,
/// for the machine's *current* state.
///
/// - running: vCPU + resident RAM + disk
/// - suspended (standby): RAM + disk (memory stays resident, no vCPU)
/// - hibernated / stopped / anything else: disk only
///
/// Disk is billed on *used* GiB, which the API does not expose per machine,
/// so the allocation is used: the estimate is a ceiling for that component.
pub fn hourly_micro(m: &Machine, rates: &Rates) -> i64 {
    let ram_gib = m.memory_bytes as f64 / GIB;
    let disk_gib = m.disk_bytes as f64 / GIB;
    let disk = disk_gib * rates.disk_gib_hour_micro as f64;
    let total = match m.status {
        MachineStatus::Running | MachineStatus::Starting | MachineStatus::Migrating => {
            m.vcpu as f64 * rates.vcpu_hour_micro as f64 + ram_gib * rates.ram_gib_hour_micro as f64 + disk
        }
        MachineStatus::Suspended => ram_gib * rates.ram_gib_hour_micro as f64 + disk,
        _ => disk,
    };
    total.round() as i64
}

/// Formats micro-units as a short money string: `€0.17`, `$12.40`, `€0.0082`.
pub fn money(micro: i64, currency: &str) -> String {
    let symbol = match currency {
        "eur" | "EUR" => "€",
        "usd" | "USD" => "$",
        other => other,
    };
    let units = micro as f64 / 1_000_000.0;
    let abs = units.abs();
    let body = if abs >= 100.0 {
        format!("{abs:.0}")
    } else if abs >= 1.0 {
        format!("{abs:.2}")
    } else if abs >= 0.01 {
        format!("{abs:.3}")
    } else if abs == 0.0 {
        "0".to_string()
    } else {
        format!("{abs:.4}")
    };
    let sign = if units < 0.0 { "-" } else { "" };
    format!("{sign}{symbol}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(status: MachineStatus, vcpu: u32, mem_gib: u64) -> Machine {
        Machine {
            id: "vm_1".into(),
            name: "t".into(),
            status,
            vcpu,
            memory_bytes: mem_gib * 1024 * 1024 * 1024,
            disk_bytes: 100 * 1024 * 1024 * 1024,
            public_ip: String::new(),
            access_domain: "boxd.sh".into(),
            url: String::new(),
            desktop_url: String::new(),
            ssh_port: 0,
            org: String::new(),
            billing_org: String::new(),
            source: String::new(),
            isolated: false,
            created_at: 0,
            hibernated_at_ms: 0,
            boot_time_ms: 0,
            last_connected_ms: 0,
            auto_suspend_secs: 0,
            auto_hibernate_secs: 0,
            auto_destroy_secs: 0,
        }
    }

    #[test]
    fn default_machine_matches_published_example() {
        // boxd's FAQ: a 2 vCPU / 8 GiB machine is "about €0.17 per hour".
        let m = machine(MachineStatus::Running, 2, 8);
        let h = hourly_micro(&m, &BOXD_PUBLISHED_EUR);
        assert_eq!(h, 2 * 39_000 + 8 * 12_000 + 100 * 80);
        assert_eq!(money(h, "eur"), "€0.182");
    }

    #[test]
    fn standby_drops_vcpu_and_hibernate_drops_ram() {
        let standby = machine(MachineStatus::Suspended, 2, 8);
        let hib = machine(MachineStatus::Hibernated, 2, 8);
        assert_eq!(hourly_micro(&standby, &BOXD_PUBLISHED_EUR), 8 * 12_000 + 100 * 80);
        assert_eq!(hourly_micro(&hib, &BOXD_PUBLISHED_EUR), 100 * 80);
    }

    #[test]
    fn money_formats() {
        assert_eq!(money(5_000_000, "eur"), "€5.00");
        assert_eq!(money(174_000, "eur"), "€0.174");
        assert_eq!(money(8_000, "usd"), "$0.0080");
        assert_eq!(money(-2_500_000, "eur"), "-€2.50");
        assert_eq!(money(0, "eur"), "€0");
        assert_eq!(money(123_456_789_000, "eur"), "€123457");
    }
}
