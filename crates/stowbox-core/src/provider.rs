//! The surface the desktop shell programs against. boxd is the first
//! implementation; boat.dev and Cloudflare Sandboxes can slot in behind it.

use crate::types::*;
use crate::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    async fn whoami(&self) -> Result<Account>;
    async fn list_machines(&self) -> Result<Vec<Machine>>;
    async fn get_machine(&self, id_or_name: &str) -> Result<Machine>;
    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine>;
    async fn fork_machine(&self, source: &str, name: Option<String>) -> Result<Machine>;
    async fn power(&self, id: &str, action: PowerAction) -> Result<()>;
    async fn set_auto_hibernate(&self, id: &str, secs: u32) -> Result<()>;
    async fn set_auto_suspend(&self, id: &str, secs: u32) -> Result<()>;
    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle>;

    async fn machine_usage(&self, id: &str) -> Result<Usage>;
    async fn org_billing(&self, org: &str) -> Result<OrgBilling>;
    /// Metered cost of the org's machines over `[since, until)` (unix
    /// seconds, 0 = now). `org` "" is the key's own org.
    async fn org_usage(&self, _org: &str, _since: i64, _until: i64) -> Result<OrgUsage> {
        Err(crate::Error::Provider(format!("{} does not report usage", self.id())))
    }
    /// The live rate card, used for the €/h estimate in the sidebar.
    async fn rates(&self) -> Result<(String, Rates)>;

    /// Live CPU, memory, disk and uptime, read inside the box. `None` when
    /// the box is not running: sampling would wake it. Providers with a
    /// telemetry API can override this instead of paying for an exec.
    /// Links an OpenSSH public key (`ssh-ed25519 AAAA… comment`) to the
    /// account so SSH to its boxes accepts it. `device_id` names this
    /// computer: linking again from it replaces the key linked before.
    async fn link_ssh_key(&self, _pubkey: &str, _device_id: &str, _label: &str) -> Result<()> {
        Err(crate::Error::Provider(format!("{} has no SSH access", self.id())))
    }

    /// The host key boxes present over SSH, as an OpenSSH public-key line,
    /// when the provider publishes it.
    async fn ssh_host_key(&self) -> Result<Option<String>> {
        Ok(None)
    }

    async fn metrics(&self, id: &str) -> Result<Option<crate::metrics::MachineMetrics>> {
        crate::metrics::sample(self, id).await
    }
}
