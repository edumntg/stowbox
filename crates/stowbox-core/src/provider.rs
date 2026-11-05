//! The surface the desktop shell programs against. boxd is the first
//! implementation; boat.dev and Cloudflare Sandboxes can slot in behind it.

use crate::ports::{HttpsRoute, PortForward};
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

    /// Writes `data` to `path` in the box, an absolute path whose directory
    /// exists, replacing any file there. Returns the bytes written. The
    /// default sends the bytes base64-encoded over exec stdin; providers
    /// with a file API override it.
    async fn upload_file(&self, id: &str, path: &str, data: Vec<u8>) -> Result<u64> {
        crate::upload::upload_over_exec(self, id, path, data).await
    }

    async fn metrics(&self, id: &str) -> Result<Option<crate::metrics::MachineMetrics>> {
        crate::metrics::sample(self, id).await
    }

    /// HTTPS routes into the box: the default one at the box's own address
    /// and any named ones. Reading them does not wake the box.
    async fn https_routes(&self, _id: &str) -> Result<Vec<HttpsRoute>> {
        Err(crate::Error::Provider(format!("{} has no HTTPS links", self.id())))
    }
    /// A named route to `port`. Returns the route as the provider made it.
    async fn add_https_route(&self, _id: &str, _name: &str, _port: u16) -> Result<HttpsRoute> {
        Err(crate::Error::Provider(format!("{} has no HTTPS links", self.id())))
    }
    async fn remove_https_route(&self, _id: &str, _name: &str) -> Result<()> {
        Err(crate::Error::Provider(format!("{} has no HTTPS links", self.id())))
    }
    /// Pins the default route to `port`, or `None` to detect it again.
    async fn set_default_route_port(&self, _id: &str, _port: Option<u16>) -> Result<()> {
        Err(crate::Error::Provider(format!("{} has no HTTPS links", self.id())))
    }
    /// Raw TCP/UDP forwards on the box's public address.
    async fn port_forwards(&self, _id: &str) -> Result<Vec<PortForward>> {
        Err(crate::Error::Provider(format!("{} has no port forwards", self.id())))
    }
    /// `protocol` is "tcp", "udp" or "both".
    async fn expose_port(&self, _id: &str, _port: u16, _protocol: &str) -> Result<PortForward> {
        Err(crate::Error::Provider(format!("{} has no port forwards", self.id())))
    }
    async fn unexpose_port(&self, _id: &str, _port: u16) -> Result<()> {
        Err(crate::Error::Provider(format!("{} has no port forwards", self.id())))
    }
}
