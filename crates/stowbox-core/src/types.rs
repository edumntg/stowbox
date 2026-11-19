//! Provider-neutral shapes. Serialized straight to the UI, so field names are
//! camelCase and every number is a plain JSON number.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineStatus {
    Pending,
    Starting,
    Running,
    /// boxd calls this "standby" in the CLI and "suspended" on the wire:
    /// memory kept warm, no vCPU billed.
    Suspended,
    Hibernated,
    /// Going down: boat saves the disk before it reports `stopped`.
    Stopping,
    Stopped,
    Failed,
    Migrating,
    Unknown,
}

impl MachineStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "suspended" | "standby" => Self::Suspended,
            "hibernated" => Self::Hibernated,
            "stopping" => Self::Stopping,
            "stopped" => Self::Stopped,
            "failed" => Self::Failed,
            "migrating" => Self::Migrating,
            _ => Self::Unknown,
        }
    }

    /// Whether an interactive exec can be opened without a wake first.
    pub fn is_reachable(self) -> bool {
        matches!(self, Self::Running | Self::Suspended | Self::Hibernated)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub id: String,
    /// The provider that owns the box: "boxd", "boat", "daytona" or "agent37". Filled by the
    /// provider, so the shell can route every call about the box.
    #[serde(default)]
    pub provider: String,
    /// The provider's size name for fixed-size providers ("default"), else "".
    #[serde(default)]
    pub size: String,
    pub name: String,
    pub status: MachineStatus,
    pub vcpu: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub public_ip: String,
    /// `<name>.<access_domain>` is the HTTPS and SSH display address.
    pub access_domain: String,
    pub url: String,
    pub desktop_url: String,
    pub ssh_port: u32,
    pub org: String,
    pub billing_org: String,
    pub source: String,
    pub isolated: bool,
    pub created_at: i64,
    pub hibernated_at_ms: i64,
    pub boot_time_ms: i64,
    pub last_connected_ms: i64,
    pub auto_suspend_secs: u32,
    pub auto_hibernate_secs: u32,
    pub auto_destroy_secs: u32,
    /// When the provider stops the box on its own, unix seconds; 0 is never
    /// or not reported.
    #[serde(default)]
    pub auto_stop_at: i64,
    /// The provider stops the box after this long with no activity; 0 is
    /// off or not that kind of provider.
    #[serde(default)]
    pub idle_stop_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub user_id: String,
    pub display_name: String,
    pub billing: Billing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Billing {
    /// "credits" (pay as you go) or "stripe" (subscription).
    pub mode: String,
    /// Lower-case ISO 4217, "eur" or "usd".
    pub currency: String,
    /// Balance in millionths of the currency unit.
    pub balance_micro: i64,
    /// Usage accrued but not yet charged, when the provider reports it.
    pub accruing_micro: Option<i64>,
    pub max_vms: u32,
    pub default_vcpu: u32,
    pub default_memory_bytes: u64,
    pub ceiling_vcpu: u32,
    pub ceiling_memory_bytes: u64,
    pub subscription_status: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    pub vcpu_hour_micro: i64,
    pub ram_gib_hour_micro: i64,
    pub disk_gib_hour_micro: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateSeconds {
    pub running: u64,
    pub standby: u64,
    pub hibernated: u64,
    pub stopped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub period_start: i64,
    pub period_end: i64,
    pub currency: String,
    pub rates: Rates,
    pub vcpu_hours: f64,
    pub ram_gib_hours: f64,
    pub disk_gib_hours: f64,
    pub cost_micro: i64,
    pub seconds: StateSeconds,
    /// Metering lags by up to ~25 minutes; false means the figure can grow.
    pub complete: bool,
}

/// What an org's machines cost over a window, from boxd's meter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgUsage {
    pub period_start: i64,
    pub period_end: i64,
    pub currency: String,
    pub total_cost_micro: i64,
    /// Metering lags by up to ~25 minutes; false means the total can grow.
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgBilling {
    pub id: String,
    pub name: String,
    pub is_admin: bool,
    pub currency: String,
    pub mode: String,
    pub balance_micro: i64,
    pub accruing_micro: Option<i64>,
    pub vm_count: u32,
    pub max_vms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Start,
    Stop,
    Reboot,
    Pause,
    Resume,
    Hibernate,
    Wake,
    Destroy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpec {
    /// Which connected provider makes the box; None is the only one.
    #[serde(default)]
    pub provider: Option<String>,
    pub name: Option<String>,
    /// A size from the provider's menu, for providers without free sizes.
    #[serde(default)]
    pub size: Option<String>,
    /// Auto-stop after this many seconds (of running for `auto_stop`, of
    /// no activity for `idle_stop`); Some(0) is never, None is the
    /// provider's default.
    #[serde(default)]
    pub auto_stop_secs: Option<u32>,
    pub vcpu: Option<u32>,
    pub memory_bytes: Option<u64>,
    pub isolated: bool,
    pub org: Option<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecSpec {
    /// Run through `bash -lc`, so pipes, `cd` and `&&` all work.
    pub script: String,
    pub tty: bool,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// Bytes going into a live exec session.
#[derive(Debug)]
pub enum ExecInput {
    Data(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Closes stdin; the remote process sees EOF.
    Close,
}

/// Bytes and lifecycle coming out of a live exec session.
#[derive(Debug, Clone)]
pub enum ExecOutput {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit(i32),
    Error(String),
}

pub struct ExecHandle {
    pub input: tokio::sync::mpsc::Sender<ExecInput>,
    pub output: tokio::sync::mpsc::Receiver<ExecOutput>,
}
