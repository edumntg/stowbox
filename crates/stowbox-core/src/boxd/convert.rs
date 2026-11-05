//! Wire shapes → provider-neutral types.

use super::pb;
use crate::types::*;

pub fn machine(vm: pb::GetVmResponse, desktop_url: &dyn Fn(&str) -> String) -> Machine {
    let host = if vm.access_domain.is_empty() {
        format!("{}.boxd.sh", vm.name)
    } else {
        format!("{}.{}", vm.name, vm.access_domain)
    };
    Machine {
        desktop_url: desktop_url(&vm.name),
        url: format!("https://{host}"),
        id: vm.vm_id,
        name: vm.name,
        status: MachineStatus::parse(&vm.status),
        vcpu: vm.vcpu,
        memory_bytes: vm.memory_bytes,
        disk_bytes: vm.disk_bytes,
        public_ip: vm.public_ip,
        access_domain: vm.access_domain,
        ssh_port: vm.ssh_port,
        org: vm.org,
        billing_org: vm.billing_org,
        source: vm.source,
        isolated: vm.isolated,
        created_at: vm.created_at,
        hibernated_at_ms: vm.hibernated_at_ms,
        boot_time_ms: vm.boot_time_ms,
        last_connected_ms: vm.last_connected_ms,
        auto_suspend_secs: vm.auto_suspend_timeout_secs,
        auto_hibernate_secs: vm.auto_hibernate_timeout_secs,
        auto_destroy_secs: vm.auto_destroy_timeout_secs,
    }
}

pub fn account(r: pb::WhoamiResponse) -> Account {
    let b = r.billing.unwrap_or_default();
    Account {
        display_name: if r.display_name.is_empty() { r.user_id.clone() } else { r.display_name },
        user_id: r.user_id,
        billing: Billing {
            mode: b.billing_mode,
            currency: b.currency,
            balance_micro: b.balance_micro,
            accruing_micro: None,
            max_vms: b.max_vms,
            default_vcpu: b.vcpu,
            default_memory_bytes: b.memory_bytes,
            ceiling_vcpu: b.ceiling_vcpu,
            ceiling_memory_bytes: b.ceiling_memory_bytes,
            subscription_status: b.subscription_status,
        },
    }
}

pub fn rates(r: Option<pb::UsageRates>) -> Rates {
    let r = r.unwrap_or_default();
    Rates {
        vcpu_hour_micro: r.vcpu_hour_micro,
        ram_gib_hour_micro: r.ram_gib_hour_micro,
        disk_gib_hour_micro: r.disk_gib_hour_micro,
    }
}

pub fn usage(r: pb::GetVmUsageResponse) -> Usage {
    let u = r.usage.unwrap_or_default();
    let s = u.seconds.unwrap_or_default();
    Usage {
        period_start: r.period_start,
        period_end: r.period_end,
        currency: r.currency,
        rates: rates(r.rates),
        vcpu_hours: u.vcpu_hours,
        ram_gib_hours: u.ram_gib_hours,
        disk_gib_hours: u.disk_gib_hours,
        cost_micro: u.cost_micro,
        seconds: StateSeconds {
            running: s.running,
            standby: s.standby,
            hibernated: s.hibernated,
            stopped: s.stopped,
        },
        complete: u.complete,
    }
}

pub fn org_usage(r: pb::GetOrgUsageResponse) -> OrgUsage {
    OrgUsage { period_start: r.period_start, period_end: r.period_end, currency: r.currency, total_cost_micro: r.total_cost_micro, complete: r.complete }
}

pub fn org_billing(r: pb::GetOrgBillingResponse) -> OrgBilling {
    OrgBilling {
        id: r.id,
        name: r.name,
        is_admin: r.is_admin,
        currency: r.currency,
        mode: r.billing_mode,
        balance_micro: r.balance_micro,
        accruing_micro: r.accruing_micro,
        vm_count: r.vm_count,
        max_vms: r.max_vms,
    }
}
