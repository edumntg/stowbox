//! Blaxel (blaxel.ai) sandboxes behind the `Provider` trait.
//!
//! The lifecycle is Blaxel's control plane at `api.blaxel.ai/v0`, scoped to
//! one workspace. Commands and terminals go through each sandbox's own API
//! at its URL (see [`exec`]), so no SSH is needed.
//!
//! A Blaxel sandbox goes to standby by itself seconds after its last
//! request, memory and processes kept, and wakes in milliseconds on the
//! next one; standby costs no compute. So there is no idle stop to set,
//! and no power action but delete: archive (a stop that keeps the disk)
//! and fork are preview features Blaxel leaves off for workspaces.

pub mod api;
pub mod convert;
pub mod exec;

use crate::capabilities::{Capabilities, CredentialField, SizeOption};
use crate::cost::RateCard;
use crate::daytona::api::seg;
use crate::provider::Provider;
use crate::providers::Credentials;
use crate::types::*;
use crate::{Error, Result};
use api::Api;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub const ID: &str = "blaxel";
pub const DEFAULT_API: &str = "https://api.blaxel.ai/v0";
pub const DEFAULT_CONSOLE_URL: &str = "https://app.blaxel.ai";
pub const WORKSPACE: &str = "workspace";
const IMAGE: &str = "blaxel/base-image:latest";
/// Memory sizes offered, in GiB; Blaxel derives the CPUs from memory.
pub const SIZES_GIB: [u64; 4] = [2, 4, 8, 16];
const DEFAULT_GIB: u64 = 4;
/// What `df` shows on the default size: the disk is a tmpfs sized from
/// memory.
const DEFAULT_DISK_GIB: u32 = 3;
/// The user commands run as, and its home, as the base image reports them.
/// Polls, two seconds apart, for a deploying sandbox before an exec gives up.
const START_POLLS: u32 = 30;
const USER: &str = "root";
const HOME: &str = "/blaxel";
/// Blaxel's list price: $0.0000115 a GB-second of memory while active,
/// CPU included, nothing in standby but snapshot storage.
const RATES: Rates = Rates { vcpu_hour_micro: 0, ram_gib_hour_micro: 41_400, disk_gib_hour_micro: 0 };

const KEY_HINT: &str = "Make one in the Blaxel console under Workspace settings, API keys. It starts with bl_.";

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind { id: ID, capabilities, connect: |credentials, _| connect(&credentials) };

fn connect(credentials: &Credentials) -> Result<Arc<dyn Provider>> {
    let workspace = credentials.get(WORKSPACE).ok_or_else(|| Error::Auth("Blaxel needs the workspace".into()))?;
    Ok(Arc::new(Blaxel::new(DEFAULT_API, &credentials.api_key()?, workspace)?))
}

pub fn size_id(memory_mib: u64) -> String {
    format!("{}-gib", memory_mib / 1024)
}

fn hourly_micro(gib: u64) -> i64 {
    gib as i64 * RATES.ram_gib_hour_micro
}

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        create: true,
        new_box_blurb: "A Blaxel sandbox microVM. It goes to standby seconds after its last use, memory and processes kept, costs nothing for compute there, and wakes in milliseconds.".into(),
        free_sizes: Vec::new(),
        default_size: size_id(DEFAULT_GIB * 1024),
        default_disk_gib: DEFAULT_DISK_GIB,
        ssh_hint: String::new(),
        credential_fields: vec![
            CredentialField::api_key(KEY_HINT),
            CredentialField::plain(WORKSPACE, "workspace", "The workspace the sandboxes belong to, as its name shows in the console URL.", "my-workspace"),
        ],
        icon: "BX".into(),
        label: "Blaxel".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: vec!["bl_".into()],
        currency: "usd".into(),
        power: vec![Destroy],
        fork: false,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: false,
        pauses_when_idle: true,
        custom_size: false,
        sizes: SIZES_GIB
            .iter()
            .map(|&g| SizeOption {
                id: size_id(g * 1024),
                vcpu: convert::vcpus_for(g * 1024),
                memory_bytes: g << 30,
                disk_bytes: 0,
                hourly_micro: hourly_micro(g),
                note: String::new(),
            })
            .collect(),
        https_links: false,
        https_named: false,
        https_list: false,
        https_remove: false,
        https_default_route: false,
        port_forwards: false,
        editor_ssh: false,
        max_upload_bytes: crate::upload::MAX_BYTES,
        remote_user: USER.into(),
        remote_home: HOME.into(),
        metering_note: "Blaxel's API does not report what a sandbox has cost; the console does. This is the list price while active; standby costs no compute.".into(),
        metered_usage: false,
    }
}

pub struct Blaxel {
    api: Arc<Api>,
    workspace: String,
}

impl Blaxel {
    pub fn new(base: &str, key: &str, workspace: &str) -> Result<Self> {
        Ok(Self { api: Arc::new(Api::new(base, key, workspace)?), workspace: workspace.to_string() })
    }

    fn checked(id: &str) -> Result<&str> {
        if convert::valid_name(id) {
            Ok(id)
        } else {
            Err(Error::NotFound(format!("{id:?} is not a Blaxel sandbox name")))
        }
    }

    fn sandbox(&self, name: &str, rest: &str) -> Result<String> {
        Ok(self.api.control(&format!("/sandboxes/{}{rest}", seg(Self::checked(name)?))))
    }

    fn new_suffix() -> String {
        uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect()
    }
}

#[async_trait]
impl Provider for Blaxel {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        // The list proves the key and the workspace together.
        self.api.get(&self.api.control("/sandboxes?limit=1")).await?;
        Ok(Account {
            user_id: String::new(),
            display_name: format!("Blaxel {}", self.workspace),
            billing: Billing {
                mode: String::new(),
                currency: "usd".into(),
                balance_micro: 0,
                accruing_micro: None,
                max_vms: 0,
                default_vcpu: convert::vcpus_for(DEFAULT_GIB * 1024),
                default_memory_bytes: DEFAULT_GIB << 30,
                ceiling_vcpu: 0,
                ceiling_memory_bytes: 0,
                subscription_status: String::new(),
            },
        })
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        for _ in 0..50 {
            let mut path = "/sandboxes?limit=200".to_string();
            if !cursor.is_empty() {
                path.push_str(&format!("&cursor={}", seg(&cursor)));
            }
            let page = self.api.get(&self.api.control(&path)).await?;
            // A bare array from older API versions, a page from newer ones.
            let items = page.as_array().or_else(|| page.get("data").and_then(Value::as_array));
            if let Some(items) = items {
                out.extend(items.iter().map(convert::machine).filter(|m| convert::valid_name(&m.id)));
            }
            let meta = page.get("meta");
            match meta.and_then(|m| m.get("nextCursor")).and_then(Value::as_str) {
                Some(next) if !next.is_empty() && meta.and_then(|m| m.get("hasMore")).and_then(Value::as_bool) != Some(false) => cursor = next.to_string(),
                _ => break,
            }
        }
        Ok(out)
    }

    async fn get_machine(&self, id: &str) -> Result<Machine> {
        Ok(convert::machine(&self.api.get(&self.sandbox(id, "")?).await?))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let gib = match spec.size.as_deref().filter(|s| !s.is_empty()) {
            None => DEFAULT_GIB,
            Some(s) => SIZES_GIB.into_iter().find(|&g| size_id(g * 1024) == s).ok_or_else(|| Error::Provider(format!("Blaxel has no size {s}")))?,
        };
        let name = convert::name_for(spec.name.as_deref(), &Self::new_suffix());
        let envs: Vec<Value> = spec.env.into_iter().map(|(k, v)| json!({ "name": k, "value": v })).collect();
        let body = json!({
            "metadata": { "name": name, "labels": { "created-by": "stowbox" } },
            "spec": { "runtime": { "image": IMAGE, "memory": gib * 1024, "envs": envs } },
        });
        let created = self.api.post(&self.api.control("/sandboxes"), body).await?;
        Ok(convert::machine(&created))
    }

    /// Blaxel's fork is a preview feature, off for workspaces by default.
    async fn fork_machine(&self, _source: &str, _name: Option<String>) -> Result<Machine> {
        Err(Error::Provider("Blaxel's fork is in preview and not offered here yet".into()))
    }

    /// Standby is automatic and any call wakes it; archive (stop) is a
    /// preview feature, off for workspaces by default. Only delete is left.
    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        match action {
            PowerAction::Destroy => {
                self.api.delete(&self.sandbox(id, "")?).await?;
                Ok(())
            }
            _ => Err(Error::Provider(
                "a Blaxel sandbox goes to standby by itself seconds after its last use and wakes on the next; it can only be deleted".into(),
            )),
        }
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a Blaxel sandbox goes to standby by itself seconds after its last use".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a Blaxel sandbox goes to standby by itself seconds after its last use".into()))
    }

    async fn set_auto_stop(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a Blaxel sandbox goes to standby by itself seconds after its last use".into()))
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        let path = self.sandbox(id, "")?;
        let mut s = self.api.get(&path).await?;
        // A new sandbox deploys for a few seconds; wait for it rather than
        // refuse, since Blaxel has no start for the person to press.
        for _ in 0..START_POLLS {
            if !matches!(convert::machine(&s).status, MachineStatus::Pending | MachineStatus::Starting) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            s = self.api.get(&path).await?;
        }
        let status = convert::machine(&s).status;
        if !matches!(status, MachineStatus::Running | MachineStatus::Hibernated) {
            return Err(Error::Provider(format!("the Blaxel sandbox {id} is {}, so it cannot run commands", format!("{status:?}").to_lowercase())));
        }
        let base = convert::sandbox_url(&s).ok_or_else(|| Error::Provider("Blaxel gave the sandbox no usable URL".into()))?;
        if spec.tty {
            exec::terminal(self.api.clone(), base, spec).await
        } else {
            exec::command(self.api.clone(), base, spec).await
        }
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("Blaxel's API does not report what a sandbox has cost; the Blaxel console does".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("Blaxel's API does not report a balance".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard::resources("usd", RATES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn capabilities_match_what_blaxel_does() {
        let c = capabilities();
        assert!(c.create && !c.fork && !c.idle_stop && !c.auto_stop && !c.metered_usage);
        assert!(c.supports(PowerAction::Destroy) && !c.supports(PowerAction::Stop));
        assert_eq!(c.default_size, "4-gib");
        assert_eq!(c.sizes.iter().map(|s| s.vcpu).collect::<Vec<_>>(), [1, 2, 4, 6]);
    }

    #[test]
    fn sizes_cost_their_memory_only_while_active() {
        // 4 GiB × $0.0414 = $0.1656 an active hour.
        assert_eq!(hourly_micro(4), 165_600);
        let card = RateCard::resources("usd", RATES);
        assert!(card.is_priced());
        let mut m = crate::testing::machine("s1", MachineStatus::Running);
        m.vcpu = 2;
        m.memory_bytes = 4 << 30;
        m.disk_bytes = 0;
        assert_eq!(card.hourly_micro(&m), 165_600);
        m.status = MachineStatus::Hibernated;
        assert_eq!(card.hourly_micro(&m), 0);
    }

    #[test]
    fn names_that_could_change_a_url_are_refused() {
        let b = Blaxel::new(DEFAULT_API, "bl_x", "ws").unwrap();
        assert!(b.sandbox("api-work", "/archive").is_ok());
        assert!(matches!(b.sandbox("../x", ""), Err(Error::NotFound(_))));
    }

    /// Live: commands run as the declared user and home, a quiet command
    /// outlives the idle limit, and dropping a terminal hangs up what it
    /// runs. Needs `STOWBOX_LIVE_BLAXEL_KEY` and `_WORKSPACE`.
    #[tokio::test]
    #[ignore = "makes and deletes a real Blaxel sandbox"]
    async fn runs_as_its_user_and_dropping_a_terminal_hangs_it_up() {
        let (Ok(key), Ok(ws)) = (std::env::var("STOWBOX_LIVE_BLAXEL_KEY"), std::env::var("STOWBOX_LIVE_BLAXEL_WORKSPACE")) else { return };
        let p = Blaxel::new(DEFAULT_API, &key, &ws).unwrap();
        let m = p.create_machine(CreateSpec { name: Some("stowbox-live-tty".into()), ..Default::default() }).await.unwrap();
        let run = |script: &'static str| {
            let p = &p;
            let id = m.id.clone();
            async move {
                let mut h = p.exec(&id, ExecSpec { script: script.into(), ..Default::default() }).await?;
                let mut out = Vec::new();
                while let Some(o) = h.output.recv().await {
                    if let ExecOutput::Stdout(b) = o {
                        out.extend(b);
                    }
                }
                Ok::<_, Error>(String::from_utf8_lossy(&out).trim().to_string())
            }
        };
        let check = async {
            let who = run("id -un; echo $HOME").await?;
            let quiet = run("sleep 100; echo late").await?;
            let h = p.exec(&m.id, ExecSpec { script: "sleep 777".into(), tty: true, cols: 80, rows: 24, env: vec![] }).await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop(h);
            tokio::time::sleep(Duration::from_secs(4)).await;
            Ok::<_, Error>((who, quiet, run("ps -eo args | grep -c '^sleep 777' || true").await?))
        };
        let result = check.await;
        p.power(&m.id, PowerAction::Destroy).await.unwrap();
        let (who, quiet, left) = result.unwrap();
        assert_eq!(who, format!("{USER}\n{HOME}"));
        assert_eq!(quiet, "late");
        assert_eq!(left, "0");
    }

    #[test]
    fn a_connection_needs_the_workspace() {
        let without = Credentials::new([("api_key".into(), "bl_0123456789abcdefghijklmn".into())]);
        assert!(matches!(connect(&without), Err(Error::Auth(_))));
    }
}
