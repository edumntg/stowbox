//! Daytona (daytona.io) behind the `Provider` trait.
//!
//! The lifecycle is Daytona's REST API (`/api/sandbox…`). Commands run over
//! Daytona's SSH gateway, not its toolbox API: the toolbox takes stdin as
//! text with no end of input, and Stowbox needs bytes in and out, a TTY and
//! exit codes. `POST /sandbox/{id}/ssh-access` mints a token that is the SSH
//! user name; the gateway announces no host key, so the first one seen is
//! remembered and held to (see [`crate::sshexec`]).
//!
//! Daytona stops a sandbox after its auto-stop interval with no activity,
//! even with processes running in it. While an exec is open (a terminal,
//! an agent's session, a watch) Stowbox reports activity every few minutes,
//! so a box is not stopped under an agent that Stowbox is talking to.

pub mod api;
pub mod convert;

use crate::capabilities::{Capabilities, SizeOption};
use crate::cost::RateCard;
use crate::ports::HttpsRoute;
use crate::provider::Provider;
use crate::sshexec::{self, Authorize, Dial, Endpoint, HostCheck, Login};
use crate::types::*;
use crate::{Error, Result};
use api::{seg, Api};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub const DEFAULT_API: &str = "https://app.daytona.io/api";
pub const DEFAULT_CONSOLE_URL: &str = "https://app.daytona.io/dashboard";
/// Daytona's list prices, per hour: $0.0504 a vCPU, $0.0162 a GiB of
/// memory, $0.000108 a GiB of disk (the first 5 GiB are free, which the
/// estimate does not take off).
const RATES: Rates = Rates { vcpu_hour_micro: 50_400, ram_gib_hour_micro: 16_200, disk_gib_hour_micro: 108 };
/// Daytona's default snapshots: (name, vCPU, GiB of memory, GiB of disk).
/// A sandbox's size comes from its snapshot; the API refuses `cpu` and
/// `memory` next to one, and a create without a snapshot uses the first.
const SNAPSHOTS: [(&str, u32, u64, u64); 3] = [("daytona-small", 1, 1, 3), ("daytona-medium", 2, 4, 8), ("daytona-large", 4, 8, 10)];
/// How often an open exec tells Daytona the sandbox is in use.
const ACTIVITY_EVERY: Duration = Duration::from_secs(4 * 60);
/// The auto-stop interval of a sandbox made without one.
const DEFAULT_IDLE_STOP_SECS: u32 = 3600;
/// SSH access tokens live this long; Daytona's maximum is a day.
const TOKEN_MINUTES: u32 = 1440;
/// Signed preview links last this long.
const LINK_SECS: u32 = 24 * 3600;

const KEY_HINT: &str = "Make one on the Daytona dashboard under Keys, with permission to write and delete sandboxes. Keys are fenced to one organization.";

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind {
    id: "daytona",
    capabilities,
    connect: |credentials, context| Ok(Arc::new(Daytona::new(DaytonaConfig::new(context.ssh_file("daytona_known_hosts")), credentials.api_key()?)?)),
};

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        new_box_blurb: "A Daytona sandbox from the default image, up in about a second. Daytona stops it after a stretch with no activity, even with work running; Stowbox reports activity while a terminal or an agent is open.".into(),
        free_sizes: Vec::new(),
        default_size: SNAPSHOTS[0].0.into(),
        default_disk_gib: 3,
        ssh_hint: "daytona ssh {id}".into(),
        credential_fields: vec![crate::capabilities::CredentialField::api_key(KEY_HINT)],
        icon: "DT".into(),
        label: "daytona".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: vec!["dtn_".into()],
        currency: "usd".into(),
        // Pause and fork exist only for VM-class sandboxes; the default
        // class is a container, so neither is offered.
        power: vec![Start, Stop, Destroy],
        fork: false,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: true,
        custom_size: false,
        sizes: SNAPSHOTS
            .iter()
            .map(|&(id, vcpu, memory_gib, disk_gib)| SizeOption {
                id: id.into(),
                vcpu,
                memory_bytes: memory_gib << 30,
                disk_bytes: disk_gib << 30,
                hourly_micro: i64::from(vcpu) * RATES.vcpu_hour_micro + memory_gib as i64 * RATES.ram_gib_hour_micro + disk_gib as i64 * RATES.disk_gib_hour_micro,
                note: String::new(),
            })
            .collect(),
        https_links: true,
        https_named: false,
        https_list: false,
        https_remove: false,
        https_default_route: false,
        port_forwards: false,
        editor_ssh: false,
        max_upload_bytes: crate::upload::MAX_BYTES,
        remote_user: "daytona".into(),
        remote_home: "/home/daytona".into(),
        metering_note: "Daytona's API does not report what a sandbox has cost; the dashboard does. This is the list-price estimate.".into(),
        metered_usage: false,
    }
}

#[derive(Debug, Clone)]
pub struct DaytonaConfig {
    pub api: String,
    /// Where the SSH gateway's host key is remembered.
    pub known_hosts: PathBuf,
}

impl DaytonaConfig {
    pub fn new(known_hosts: PathBuf) -> Self {
        Self { api: DEFAULT_API.into(), known_hosts }
    }
}

pub struct Daytona {
    api: Arc<Api>,
    ssh: sshexec::Pool,
    known_hosts: PathBuf,
    /// Preview links made from this session, per sandbox: Daytona has no
    /// call that lists them.
    links: Mutex<HashMap<String, Vec<HttpsRoute>>>,
}

impl Daytona {
    pub fn new(config: DaytonaConfig, api_key: String) -> Result<Self> {
        Ok(Self {
            api: Arc::new(Api::new(&config.api, api_key)?),
            ssh: sshexec::Pool::without_key(),
            known_hosts: config.known_hosts,
            links: Default::default(),
        })
    }

    async fn sandbox(&self, id: &str) -> Result<Value> {
        self.api.get(&format!("/sandbox/{}", seg(id))).await
    }

    async fn signed_link(&self, id: &str, port: u16) -> Result<Value> {
        let path = format!("/sandbox/{}/ports/{port}/signed-preview-url", seg(id));
        match self.api.get(&format!("{path}?expiresInSeconds={LINK_SECS}")).await {
            Ok(v) => Ok(v),
            // An organization may cap the lifetime lower; an hour then.
            Err(Error::Provider(_)) => self.api.get(&format!("{path}?expiresInSeconds=3600")).await,
            Err(e) => Err(e),
        }
    }
}

/// `POST /sandbox/{id}/ssh-access` for a token, after checking that the
/// sandbox runs: the gateway would only fail later and less clearly.
struct GatewayAuth<'a> {
    api: &'a Api,
    known_hosts: &'a PathBuf,
}

#[async_trait]
impl Authorize for GatewayAuth<'_> {
    async fn authorize(&self, id: &str, _: &str) -> Result<Endpoint> {
        let s = self.api.get(&format!("/sandbox/{}", seg(id))).await?;
        let state = s.get("state").and_then(Value::as_str).unwrap_or("");
        if state != "started" {
            return Err(Error::Provider(format!("the sandbox is {}; start it first", if state.is_empty() { "not running" } else { state })));
        }
        let a = self.api.post(&format!("/sandbox/{}/ssh-access?expiresInMinutes={TOKEN_MINUTES}", seg(id)), None).await?;
        let token =
            a.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| Error::Provider("Daytona gave no SSH access token".into()))?;
        let (host, port) = a.get("sshCommand").and_then(Value::as_str).and_then(convert::ssh_target).unwrap_or_else(|| ("ssh.app.daytona.io".into(), 22));
        Ok(Endpoint {
            host,
            port,
            user: token.to_string(),
            login: Login::Token,
            check: HostCheck::Remembered(self.known_hosts.clone()),
            dial: Dial::Tcp,
            provider: "Daytona",
        })
    }
}

#[async_trait]
impl Provider for Daytona {
    fn id(&self) -> &'static str {
        "daytona"
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        // Proves the key first: listing sandboxes is what every key can do.
        self.api.get("/sandbox?limit=1").await?;
        let orgs = self.api.get("/organizations").await.unwrap_or(Value::Null);
        Ok(convert::account(&orgs))
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let all = self.api.list_all("/sandbox").await?;
        Ok(all.iter().filter(|s| !convert::is_gone(s)).map(convert::machine).collect())
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        Ok(convert::machine(&self.sandbox(id_or_name).await?))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let mut body = Map::new();
        if let Some(n) = spec.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            body.insert("name".into(), json!(n));
        }
        if let Some(size) = spec.size.filter(|s| !s.is_empty()) {
            if !SNAPSHOTS.iter().any(|s| s.0 == size) {
                return Err(Error::Provider(format!("Daytona has no size {size}")));
            }
            body.insert("snapshot".into(), json!(size));
        }
        // Daytona's own default is 15 minutes, short for an agent's turn.
        let idle = spec.auto_stop_secs.unwrap_or(DEFAULT_IDLE_STOP_SECS);
        body.insert("autoStopInterval".into(), json!(idle.div_ceil(60)));
        if !spec.env.is_empty() {
            body.insert("env".into(), Value::Object(spec.env.into_iter().map(|(k, v)| (k, json!(v))).collect()));
        }
        body.insert("labels".into(), json!({ "created-by": "stowbox" }));
        let s = self.api.post("/sandbox", Some(Value::Object(body))).await?;
        Ok(convert::machine(&s))
    }

    async fn fork_machine(&self, _source: &str, _name: Option<String>) -> Result<Machine> {
        Err(Error::Provider("Daytona forks only VM-class sandboxes, which Stowbox does not create yet".into()))
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        let path = format!("/sandbox/{}", seg(id));
        match action {
            PowerAction::Start | PowerAction::Resume | PowerAction::Wake => {
                self.api.post(&format!("{path}/start"), None).await?;
            }
            PowerAction::Stop => {
                self.api.post(&format!("{path}/stop"), None).await?;
            }
            PowerAction::Destroy => {
                self.api.delete(&path).await?;
                self.links.lock().unwrap().remove(id);
            }
            PowerAction::Pause | PowerAction::Hibernate | PowerAction::Reboot => {
                return Err(Error::Provider(format!("Stowbox cannot {} a Daytona sandbox; stop it instead", format!("{action:?}").to_lowercase())));
            }
        }
        // A stopped sandbox comes back on another runner.
        self.ssh.drop_conn(id);
        if action != PowerAction::Destroy {
            // Preview links end with the run that made them.
            self.links.lock().unwrap().remove(id);
        }
        Ok(())
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Daytona has no auto hibernate; set its auto-stop".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Daytona has no auto standby; set its auto-stop".into()))
    }

    /// The idle interval, in seconds; 0 turns it off.
    async fn set_auto_stop(&self, id: &str, secs: u32) -> Result<()> {
        self.api.post(&format!("/sandbox/{}/autostop/{}", seg(id), secs.div_ceil(60)), None).await?;
        Ok(())
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        let auth = GatewayAuth { api: &self.api, known_hosts: &self.known_hosts };
        let inner = self.ssh.exec(&auth, id, spec).await?;
        // Pass the output through, and say the sandbox is in use while the
        // reader is there. Dropping the reader still ends the exec.
        let (tx, rx) = mpsc::channel::<ExecOutput>(256);
        let api = self.api.clone();
        let path = format!("/sandbox/{}/last-activity", seg(id));
        let mut out = inner.output;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(ACTIVITY_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    m = out.recv() => match m {
                        Some(o) => {
                            if tx.send(o).await.is_err() {
                                return;
                            }
                        }
                        None => return,
                    },
                    _ = tick.tick() => {
                        let _ = api.post(&path, None).await;
                    }
                    _ = tx.closed() => return,
                }
            }
        });
        Ok(ExecHandle { input: inner.input, output: rx })
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("Daytona's API does not report what a sandbox has cost; the Daytona dashboard does".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("Daytona's API does not report a balance".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard::resources("usd", RATES))
    }

    async fn https_routes(&self, id: &str) -> Result<Vec<HttpsRoute>> {
        Ok(self.links.lock().unwrap().get(id).cloned().unwrap_or_default())
    }

    async fn add_https_route(&self, id: &str, _name: &str, port: u16) -> Result<HttpsRoute> {
        let r = self.signed_link(id, port).await?;
        let url = r.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let domain = url.trim_start_matches("https://").split(['/', '?']).next().unwrap_or("").to_string();
        let route = HttpsRoute {
            name: port.to_string(),
            domain,
            url: Some(url).filter(|u| !u.is_empty()),
            port: port as u32,
            port_mode: "locked".into(),
            is_default: false,
        };
        let mut links = self.links.lock().unwrap();
        let list = links.entry(id.to_string()).or_default();
        list.retain(|l| l.port != route.port);
        list.push(route.clone());
        list.sort_by_key(|l| l.port);
        Ok(route)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_offer_only_what_every_sandbox_class_has() {
        let c = capabilities();
        assert!(c.supports(PowerAction::Start) && c.supports(PowerAction::Stop) && c.supports(PowerAction::Destroy));
        assert!(!c.supports(PowerAction::Pause) && !c.fork && c.idle_stop && !c.auto_stop && !c.metered_usage);
    }

    #[test]
    fn the_estimate_uses_the_list_prices() {
        let card = RateCard::resources("usd", RATES);
        let mut m = crate::testing::machine("d1", MachineStatus::Running);
        m.vcpu = 2;
        m.memory_bytes = 4 * 1024 * 1024 * 1024;
        m.disk_bytes = 10 * 1024 * 1024 * 1024;
        // 2 × 0.0504 + 4 × 0.0162 + 10 × 0.000108 = 0.16668 a running hour.
        assert_eq!(card.hourly_micro(&m), 166_680);
        m.status = MachineStatus::Stopped;
        assert_eq!(card.hourly_micro(&m), 1_080);
    }
}
