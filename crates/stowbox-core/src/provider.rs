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
    /// The live rate card, used for the €/h estimate in the sidebar.
    async fn rates(&self) -> Result<(String, Rates)>;
}
