//! What a provider can do, as data. The UI turns its buttons, fields and
//! tabs on and off from this record, never from the provider's name.

use crate::types::PowerAction;
use serde::{Deserialize, Serialize};

/// One size a provider sells, for providers with a fixed menu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeOption {
    /// The provider's own name for it, sent back on create.
    pub id: String,
    pub vcpu: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    /// List price per running hour, in micro-units of `Capabilities::currency`.
    pub hourly_micro: i64,
    /// Plans below a threshold cannot create it; the UI says so.
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Short name shown next to boxes and in Settings: "boxd", "boat".
    pub label: String,
    /// The provider's web console, opened from Settings.
    pub console_url: String,
    /// Where keys are made, in the words of the onboarding screen.
    pub key_hint: String,
    /// Accepted key prefixes, for a quick check before the network.
    pub key_prefixes: Vec<String>,
    /// Lower-case ISO 4217 of prices and balances.
    pub currency: String,
    /// Power actions the provider accepts. Anything else is not offered.
    pub power: Vec<PowerAction>,
    pub fork: bool,
    /// Isolated boxes (no in-VM CLI, no org integrations).
    pub isolated: bool,
    /// Auto standby and auto hibernate timers.
    pub auto_suspend: bool,
    pub auto_hibernate: bool,
    /// An auto-stop deadline that can be set at create and pushed out later.
    pub auto_stop: bool,
    /// A stop after so long with no activity, set at create and changed
    /// later (`set_auto_stop` takes it in seconds).
    #[serde(default)]
    pub idle_stop: bool,
    /// Free vCPU and memory at create; otherwise `sizes` is the menu.
    pub custom_size: bool,
    pub sizes: Vec<SizeOption>,
    /// HTTPS links: whether they have names, can be listed, removed, and
    /// whether the box has a default link whose port can be pinned.
    pub https_links: bool,
    pub https_named: bool,
    pub https_list: bool,
    pub https_remove: bool,
    pub https_default_route: bool,
    /// Raw TCP/UDP forwards on a public address.
    pub port_forwards: bool,
    /// Opening a box in VS Code or Cursor over Remote-SSH.
    pub editor_ssh: bool,
    /// Largest file one upload may carry, in bytes.
    pub max_upload_bytes: u64,
    /// The Linux user commands run as in the box, and its home.
    pub remote_user: String,
    pub remote_home: String,
    /// What the metered usage figure lags by, in plain words, or "".
    pub metering_note: String,
    /// Billed usage per box can be read; otherwise only the estimate shows.
    #[serde(default = "yes")]
    pub metered_usage: bool,
}

fn yes() -> bool {
    true
}

impl Capabilities {
    pub fn supports(&self, action: PowerAction) -> bool {
        self.power.contains(&action)
    }

    /// The action that brings a box in `status` to running without a
    /// reboot, if the provider has one: what a terminal or an editor uses
    /// before it connects.
    pub fn wake_action(&self, status: crate::types::MachineStatus) -> Option<PowerAction> {
        use crate::types::MachineStatus as S;
        let want = match status {
            S::Suspended => PowerAction::Resume,
            S::Hibernated => PowerAction::Wake,
            _ => return None,
        };
        self.supports(want).then_some(want)
    }
}
