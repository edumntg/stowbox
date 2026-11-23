//! E2B (e2b.dev) behind the `Provider` trait.
//!
//! The lifecycle is E2B's REST API at `api.e2b.app`. Commands run through
//! envd, E2B's agent inside each sandbox, over its Connect API (see
//! [`envd`]); envd has a real PTY, stdin and exit codes, so no SSH is needed.
//!
//! A sandbox runs until a deadline and is then paused, never killed: every
//! sandbox Stowbox makes has `autoPause`. A pause snapshots memory and disk
//! and costs nothing to hold, so it is shown as hibernation, and opening a
//! terminal resumes it. While an exec is open Stowbox pushes the deadline
//! out every few minutes, which makes the deadline an idle stop: the
//! sandbox pauses that long after Stowbox last used it. The plan caps how
//! long a sandbox can run in one go (an hour on Hobby, a day on Pro); past
//! that it pauses even in use, and the next exec resumes it.

pub mod api;
pub mod convert;
pub mod envd;

/// envd's process messages, generated from `proto/e2b/process.proto`.
#[allow(clippy::all, clippy::pedantic)]
pub mod proto {
    tonic::include_proto!("process");
}

use crate::capabilities::{Capabilities, CredentialField, SizeOption};
use crate::cost::RateCard;
use crate::daytona::api::seg;
use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use api::Api;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub const ID: &str = "e2b";
pub const DEFAULT_API: &str = "https://api.e2b.app";
pub const DEFAULT_CONSOLE_URL: &str = "https://e2b.dev/dashboard";
/// The metadata key that keeps the idle stop a sandbox was made with.
pub const IDLE_KEY: &str = "stowbox-idle";
/// The public template every team can start, and its fixed size.
const TEMPLATE: &str = "base";
const BASE_VCPU: u32 = 2;
const BASE_MEMORY_MIB: u64 = 512;
const BASE_DISK_GIB: u64 = 22;
/// E2B's list prices: $0.000014 a vCPU-second and $0.0000045 a GiB-second
/// of memory while running. A paused sandbox is not charged.
/// A paused sandbox shows as hibernated, so the estimate charges it disk
/// only, and disk is free.
const RATES: Rates = Rates { vcpu_hour_micro: 50_400, ram_gib_hour_micro: 16_200, disk_gib_hour_micro: 0 };
/// How long a sandbox runs with no use before it pauses, when New box does
/// not say.
const DEFAULT_IDLE_STOP_SECS: u32 = 1800;
/// How often an open exec pushes the pause deadline out.
const ACTIVITY_EVERY: Duration = Duration::from_secs(4 * 60);

const KEY_HINT: &str = "Copy it from the E2B dashboard under Keys. It starts with e2b_.";

pub const KIND: crate::providers::ProviderKind =
    crate::providers::ProviderKind { id: ID, capabilities, connect: |credentials, _| Ok(Arc::new(E2b::new(DEFAULT_API, credentials.api_key()?)?)) };

fn base_hourly_micro() -> i64 {
    i64::from(BASE_VCPU) * RATES.vcpu_hour_micro + (BASE_MEMORY_MIB as i64 * RATES.ram_gib_hour_micro) / 1024
}

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        create: true,
        new_box_blurb: "An E2B sandbox from the base template, up in under a second. It pauses after a stretch with no use, memory and all, and costs nothing while paused; opening a terminal resumes it.".into(),
        free_sizes: Vec::new(),
        default_size: TEMPLATE.into(),
        default_disk_gib: BASE_DISK_GIB as u32,
        ssh_hint: String::new(),
        credential_fields: vec![CredentialField::api_key(KEY_HINT)],
        icon: "E2".into(),
        label: "e2b".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: vec!["e2b_".into()],
        currency: "usd".into(),
        power: vec![Hibernate, Wake, Destroy],
        fork: true,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: true,
        pauses_when_idle: false,
        custom_size: false,
        sizes: vec![SizeOption {
            id: TEMPLATE.into(),
            vcpu: BASE_VCPU,
            memory_bytes: BASE_MEMORY_MIB << 20,
            disk_bytes: BASE_DISK_GIB << 30,
            hourly_micro: base_hourly_micro(),
            note: String::new(),
        }],
        https_links: false,
        https_named: false,
        https_list: false,
        https_remove: false,
        https_default_route: false,
        port_forwards: false,
        editor_ssh: false,
        max_upload_bytes: crate::upload::MAX_BYTES,
        remote_user: envd::USER.into(),
        remote_home: format!("/home/{}", envd::USER),
        metering_note: "E2B's API does not report what a sandbox has cost; the dashboard does. This is the list-price estimate.".into(),
        metered_usage: false,
    }
}

pub struct E2b {
    api: Arc<Api>,
    /// envd streams last as long as a terminal, so this client has no
    /// overall timeout; each call bounds its own wait.
    envd_http: reqwest::Client,
    /// The idle stop of each sandbox seen: E2B keeps only the deadline.
    idle: Mutex<HashMap<String, u32>>,
}

impl E2b {
    pub fn new(api: &str, key: String) -> Result<Self> {
        let envd_http =
            reqwest::Client::builder().user_agent(concat!("stowbox/", env!("CARGO_PKG_VERSION"))).connect_timeout(Duration::from_secs(10)).build()?;
        Ok(Self { api: Arc::new(Api::new(api, key)?), envd_http, idle: Default::default() })
    }

    fn checked(id: &str) -> Result<&str> {
        if convert::valid_id(id) {
            Ok(id)
        } else {
            Err(Error::NotFound(format!("{id:?} is not an E2B sandbox id")))
        }
    }

    /// A sandbox's idle stop: what this session set, else what it was made
    /// with, else the default.
    fn idle_of(&self, s: &Value) -> u32 {
        let id = s.get("sandboxID").and_then(Value::as_str).unwrap_or("");
        let known = self.idle.lock().ok().and_then(|m| m.get(id).copied());
        known.unwrap_or_else(|| Some(convert::machine(s).idle_stop_secs).filter(|&s| s > 0).unwrap_or(DEFAULT_IDLE_STOP_SECS))
    }

    fn machine(&self, s: &Value) -> Machine {
        let mut m = convert::machine(s);
        m.idle_stop_secs = self.idle_of(s);
        m
    }

    /// Resumes the sandbox if it is paused and pushes its deadline out by
    /// its idle stop. E2B only ever extends a deadline here.
    async fn keep_up(api: &Api, id: &str, idle: u32) -> Result<Value> {
        api.post(&format!("/v2/sandboxes/{}/connect", seg(id)), json!({ "timeout": idle })).await
    }
}

#[async_trait]
impl Provider for E2b {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        // The list proves the key. E2B's team call answers with API keys in
        // it, which Stowbox has no use for.
        self.api.get("/v2/sandboxes?limit=1").await?;
        Ok(Account {
            user_id: String::new(),
            display_name: "E2B team".into(),
            billing: Billing {
                mode: String::new(),
                currency: "usd".into(),
                balance_micro: 0,
                accruing_micro: None,
                max_vms: 0,
                default_vcpu: BASE_VCPU,
                default_memory_bytes: BASE_MEMORY_MIB << 20,
                ceiling_vcpu: BASE_VCPU,
                ceiling_memory_bytes: BASE_MEMORY_MIB << 20,
                subscription_status: String::new(),
            },
        })
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let all = self.api.list_all("/v2/sandboxes?state=running,paused").await?;
        Ok(all.iter().map(|s| self.machine(s)).collect())
    }

    async fn get_machine(&self, id: &str) -> Result<Machine> {
        let s = self.api.get(&format!("/sandboxes/{}", seg(Self::checked(id)?))).await?;
        Ok(self.machine(&s))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        if let Some(size) = spec.size.as_deref().filter(|s| !s.is_empty() && *s != TEMPLATE) {
            return Err(Error::Provider(format!("E2B has no size {size}; Stowbox starts the base template")));
        }
        let idle = match spec.auto_stop_secs {
            Some(0) => return Err(Error::Provider("an E2B sandbox always pauses once idle; pick how long it waits".into())),
            Some(s) => s,
            None => DEFAULT_IDLE_STOP_SECS,
        };
        let mut metadata = Map::new();
        metadata.insert("created-by".into(), json!("stowbox"));
        metadata.insert(IDLE_KEY.into(), json!(idle.to_string()));
        if let Some(n) = spec.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            metadata.insert(convert::NAME_KEY.into(), json!(n));
        }
        let env: Map<String, Value> = spec.env.into_iter().map(|(k, v)| (k, json!(v))).collect();
        let body = json!({ "templateID": TEMPLATE, "timeout": idle, "autoPause": true, "metadata": metadata, "envVars": env });
        let created = self.api.post("/v2/sandboxes", body).await?;
        let id = created.get("sandboxID").and_then(Value::as_str).unwrap_or("").to_string();
        if let Ok(mut m) = self.idle.lock() {
            m.insert(id.clone(), idle);
        }
        // The create answer leaves out the size and state; read it back.
        self.get_machine(&id).await
    }

    async fn fork_machine(&self, source: &str, _name: Option<String>) -> Result<Machine> {
        let source = Self::checked(source)?;
        let s = self.api.get(&format!("/sandboxes/{}", seg(source))).await?;
        let idle = self.idle_of(&s);
        let results = self.api.post(&format!("/sandboxes/{}/fork", seg(source)), json!({ "count": 1, "timeout": idle })).await?;
        let id = convert::forked(&results)?.get("sandboxID").and_then(Value::as_str).unwrap_or("").to_string();
        if let Ok(mut m) = self.idle.lock() {
            m.insert(id.clone(), idle);
        }
        self.get_machine(&id).await
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        let id = Self::checked(id)?;
        match action {
            PowerAction::Hibernate | PowerAction::Pause | PowerAction::Stop => {
                self.api.post(&format!("/sandboxes/{}/pause", seg(id)), json!({})).await?;
            }
            PowerAction::Wake | PowerAction::Resume | PowerAction::Start => {
                let s = self.api.get(&format!("/sandboxes/{}", seg(id))).await?;
                Self::keep_up(&self.api, id, self.idle_of(&s)).await?;
            }
            PowerAction::Destroy => {
                self.api.delete(&format!("/sandboxes/{}", seg(id))).await?;
                if let Ok(mut m) = self.idle.lock() {
                    m.remove(id);
                }
            }
            PowerAction::Reboot => return Err(Error::Provider("E2B cannot reboot a sandbox".into())),
        }
        Ok(())
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("E2B pauses a sandbox once idle; set how long it waits instead".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("E2B pauses a sandbox once idle; set how long it waits instead".into()))
    }

    /// How long the sandbox waits, idle, before it pauses. E2B has no
    /// "never": its plan caps how long a sandbox runs in one go.
    async fn set_auto_stop(&self, id: &str, secs: u32) -> Result<()> {
        let id = Self::checked(id)?;
        if secs == 0 {
            return Err(Error::Provider("an E2B sandbox always pauses once idle; pick how long it waits".into()));
        }
        self.api.post(&format!("/sandboxes/{}/timeout", seg(id)), json!({ "timeout": secs })).await?;
        if let Ok(mut m) = self.idle.lock() {
            m.insert(id.to_string(), secs);
        }
        Ok(())
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        let id = Self::checked(id)?.to_string();
        let s = self.api.get(&format!("/sandboxes/{}", seg(&id))).await?;
        let idle = self.idle_of(&s);
        // Resumes a paused sandbox, and says who may talk to its envd.
        let up = Self::keep_up(&self.api, &id, idle).await?;
        let token = up.get("envdAccessToken").or_else(|| s.get("envdAccessToken")).and_then(Value::as_str).unwrap_or("");
        let domain = convert::domain(if up.get("domain").is_some() { &up } else { &s });
        let rpc = envd::client(self.envd_http.clone(), &format!("https://{}-{id}.{domain}", envd::PORT), token)?;
        let inner = envd::exec(rpc, spec).await?;

        // Pass the output through, and keep the sandbox from pausing while
        // the reader is there. Dropping the reader still ends the exec.
        let (tx, rx) = mpsc::channel::<ExecOutput>(256);
        let api = self.api.clone();
        let mut out = inner.output;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(ACTIVITY_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
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
                        // Past the plan's cap this fails; the sandbox then
                        // pauses and the next exec resumes it.
                        let _ = Self::keep_up(&api, &id, idle).await;
                    }
                    _ = tx.closed() => return,
                }
            }
        });
        Ok(ExecHandle { input: inner.input, output: rx })
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("E2B's API does not report what a sandbox has cost; the E2B dashboard does".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("E2B's API does not report a balance".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard::resources("usd", RATES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_what_e2b_does() {
        let c = capabilities();
        assert!(c.create && c.fork && c.idle_stop && !c.auto_stop && !c.metered_usage);
        assert!(c.supports(PowerAction::Hibernate) && c.supports(PowerAction::Wake) && c.supports(PowerAction::Destroy));
        assert_eq!(c.wake_action(MachineStatus::Hibernated), Some(PowerAction::Wake));
        assert!(c.can_idle_down());
        assert_eq!(c.remote_home, "/home/user");
    }

    #[test]
    fn the_base_template_costs_its_list_price_only_while_running() {
        // 2 × 0.0504 + 0.5 × 0.0162 = 0.1089 a running hour.
        assert_eq!(base_hourly_micro(), 108_900);
        let card = RateCard::resources("usd", RATES);
        let mut m = crate::testing::machine("s1", MachineStatus::Running);
        m.vcpu = BASE_VCPU;
        m.memory_bytes = BASE_MEMORY_MIB << 20;
        m.disk_bytes = BASE_DISK_GIB << 30;
        assert_eq!(card.hourly_micro(&m), 108_900);
        m.status = MachineStatus::Hibernated;
        assert_eq!(card.hourly_micro(&m), 0);
    }

    /// Live: dropping an exec kills what its script left running, not only
    /// the shell. Needs `STOWBOX_LIVE_E2B_KEY`; makes and deletes a sandbox.
    #[tokio::test]
    #[ignore = "makes and deletes a real E2B sandbox"]
    async fn dropping_an_exec_kills_its_children() {
        let Ok(key) = std::env::var("STOWBOX_LIVE_E2B_KEY") else { return };
        let p = E2b::new(DEFAULT_API, key).unwrap();
        let m = p.create_machine(CreateSpec { name: Some("stowbox-live-kill".into()), auto_stop_secs: Some(600), ..Default::default() }).await.unwrap();
        let check = async {
            let mut h = p.exec(&m.id, ExecSpec { script: "sleep 777 & sleep 778; echo never".into(), ..Default::default() }).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            h.output.close();
            drop(h);
            tokio::time::sleep(Duration::from_secs(3)).await;
            let mut c = p.exec(&m.id, ExecSpec { script: "pgrep -c -f 'sleep 77[78]' || true".into(), ..Default::default() }).await?;
            let mut out = Vec::new();
            while let Some(o) = c.output.recv().await {
                if let ExecOutput::Stdout(b) = o {
                    out.extend(b);
                }
            }
            Ok::<_, Error>(String::from_utf8_lossy(&out).trim().to_string())
        };
        let left = check.await;
        p.power(&m.id, PowerAction::Destroy).await.unwrap();
        assert_eq!(left.unwrap(), "0");
    }

    #[test]
    fn ids_that_could_change_the_envd_host_are_refused() {
        assert!(E2b::checked("ik3zr4qbbz").is_ok());
        assert!(matches!(E2b::checked("x.evil.com/"), Err(Error::NotFound(_))));
    }
}
