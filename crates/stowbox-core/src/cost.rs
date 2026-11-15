//! Live cost estimates. The provider meters with a lag of up to ~25 minutes,
//! so the sidebar shows an estimate from the rate card and the machine's
//! current state, and the Details view shows the metered figure.

use crate::types::{Machine, MachineStatus, Rates};
use serde::{Deserialize, Serialize};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// boxd's published rate card (EUR, 2026-10): the fallback until the live
/// rates have been fetched once.
pub const BOXD_PUBLISHED_EUR: Rates = Rates {
    vcpu_hour_micro: 39_000,
    ram_gib_hour_micro: 12_000,
    disk_gib_hour_micro: 80,
};

/// A provider's prices: per resource (boxd) and, for providers that sell a
/// fixed menu, per size (boat). `sizes` wins for a machine whose size it
/// names.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateCard {
    pub currency: String,
    pub rates: Rates,
    #[serde(default)]
    pub sizes: Vec<SizeRate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeRate {
    pub size: String,
    /// Per running hour; nothing is billed in any other state.
    pub hourly_micro: i64,
}

impl RateCard {
    pub fn resources(currency: &str, rates: Rates) -> Self {
        Self { currency: currency.into(), rates, sizes: Vec::new() }
    }

    /// Whether the card has a price for anything at all.
    pub fn is_priced(&self) -> bool {
        self.rates.vcpu_hour_micro > 0 || self.sizes.iter().any(|s| s.hourly_micro > 0)
    }

    /// The estimate for `m` in its current state, from whichever part of
    /// the card applies to it.
    pub fn hourly_micro(&self, m: &Machine) -> i64 {
        if let Some(s) = self.sizes.iter().find(|s| !m.size.is_empty() && s.size == m.size) {
            return match m.status {
                MachineStatus::Running | MachineStatus::Starting | MachineStatus::Migrating => s.hourly_micro,
                _ => 0,
            };
        }
        hourly_micro(m, &self.rates)
    }
}

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
            provider: "boxd".into(),
            size: String::new(),
            auto_stop_at: 0,
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
    fn sized_machines_pay_their_size_while_running_and_nothing_after() {
        let card = RateCard { currency: "usd".into(), rates: Rates::default(), sizes: vec![SizeRate { size: "default".into(), hourly_micro: 36_000 }] };
        let mut m = machine(MachineStatus::Running, 4, 8);
        m.size = "default".into();
        assert_eq!(card.hourly_micro(&m), 36_000);
        m.status = MachineStatus::Stopped;
        assert_eq!(card.hourly_micro(&m), 0);
        // A size the card does not know falls back to the resource rates (none here).
        m.status = MachineStatus::Running;
        m.size = "huge".into();
        assert_eq!(card.hourly_micro(&m), 0);
        assert!(card.is_priced());
        assert!(!RateCard::default().is_priced());
        // A boxd machine on a resource card is unchanged.
        let boxd = RateCard::resources("eur", BOXD_PUBLISHED_EUR);
        assert_eq!(boxd.hourly_micro(&machine(MachineStatus::Running, 2, 8)), 2 * 39_000 + 8 * 12_000 + 100 * 80);
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
