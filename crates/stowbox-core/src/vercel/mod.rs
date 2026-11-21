//! Vercel Sandbox (vercel.com/docs/vercel-sandbox) behind the `Provider`
//! trait.
//!
//! The lifecycle is Vercel's REST API at `api.vercel.com`, scoped to one
//! team and one project: every sandbox in that project shows up. Commands
//! stream as NDJSON and terminals go over the session's interactive
//! WebSocket (see [`exec`]), so no SSH is needed.
//!
//! Stowbox makes persistent sandboxes. A session runs until its timeout and
//! then stops; stopping snapshots the disk (not memory or processes), and
//! starting the sandbox boots a new session from that snapshot. While an
//! exec is open Stowbox keeps the deadline at least the idle stop away,
//! which makes the timeout an idle stop. The plan caps one session (45
//! minutes on Hobby, a day on Pro); past that the sandbox stops even in
//! use, and the next exec starts it again.

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
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub const ID: &str = "vercel";
pub const DEFAULT_API: &str = "https://api.vercel.com";
pub const DEFAULT_CONSOLE_URL: &str = "https://vercel.com/dashboard";
pub const TEAM: &str = "team";
pub const PROJECT: &str = "project";
/// The user commands run as in Vercel's images, with passwordless sudo,
/// and its home, as a login shell there reports them.
const USER: &str = "ubuntu";
const HOME: &str = "/vercel";
/// Memory each vCPU comes with.
const MIB_PER_VCPU: u64 = 2048;
const VCPUS: [u32; 4] = [1, 2, 4, 8];
const DEFAULT_VCPUS: u32 = 2;
/// Vercel's list prices in iad1: $0.128 an hour of active CPU per vCPU and
/// $0.0212 a GB-hour of provisioned memory. CPU is billed only while busy,
/// so the estimate is what a fully busy sandbox costs. A stopped sandbox
/// costs only its snapshot's storage, which is not estimated.
const RATES: Rates = Rates { vcpu_hour_micro: 128_000, ram_gib_hour_micro: 21_200, disk_gib_hour_micro: 0 };
/// How long a session runs with no use before it stops, when New box does
/// not say.
const DEFAULT_IDLE_STOP_SECS: u32 = 1800;
/// How often an open exec pushes the stop deadline out.
const ACTIVITY_EVERY: Duration = Duration::from_secs(4 * 60);
/// How long a starting sandbox may take to run.
const START_WAIT: Duration = Duration::from_secs(90);

const KEY_HINT: &str = "Make a token on vercel.com under Account Settings, Tokens, scoped to the team below.";

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind { id: ID, capabilities, connect: |credentials, _| connect(&credentials) };

fn connect(credentials: &Credentials) -> Result<Arc<dyn Provider>> {
    let team = credentials.get(TEAM).ok_or_else(|| Error::Auth("Vercel needs the team id".into()))?;
    let project = credentials.get(PROJECT).ok_or_else(|| Error::Auth("Vercel needs the project".into()))?;
    Ok(Arc::new(Vercel::new(DEFAULT_API, credentials.api_key()?, team.into(), project.into())?))
}

fn size_id(vcpus: u32) -> String {
    format!("{vcpus}-vcpu")
}

fn hourly_micro(vcpus: u32) -> i64 {
    i64::from(vcpus) * RATES.vcpu_hour_micro + (i64::from(vcpus) * MIB_PER_VCPU as i64 * RATES.ram_gib_hour_micro) / 1024
}

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        create: true,
        new_box_blurb: "A Vercel Sandbox microVM with Node and Python, up in seconds. It stops after a stretch with no use and keeps its disk; starting it again boots from that.".into(),
        free_sizes: Vec::new(),
        default_size: size_id(DEFAULT_VCPUS),
        default_disk_gib: convert::DISK_GIB as u32,
        ssh_hint: String::new(),
        credential_fields: vec![
            CredentialField::api_key(KEY_HINT),
            CredentialField::plain(TEAM, "team id", "The team the sandboxes belong to, under the team's Settings, General.", "team_…"),
            CredentialField::plain(PROJECT, "project", "The project the sandboxes belong to, by name or id.", "my-project"),
        ],
        icon: "VC".into(),
        label: "Vercel".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: Vec::new(),
        currency: "usd".into(),
        power: vec![Stop, Start, Destroy],
        fork: true,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: true,
        custom_size: false,
        sizes: VCPUS
            .iter()
            .map(|&n| SizeOption {
                id: size_id(n),
                vcpu: n,
                memory_bytes: (u64::from(n) * MIB_PER_VCPU) << 20,
                disk_bytes: convert::DISK_GIB << 30,
                hourly_micro: hourly_micro(n),
                note: if n == 8 { "Pro and Enterprise".into() } else { String::new() },
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
        metering_note: "Vercel's API does not report what a sandbox has cost; the dashboard does. This is the list price with every vCPU busy.".into(),
        metered_usage: false,
    }
}

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

pub struct Vercel {
    api: Arc<Api>,
    project: String,
    /// For the terminal's health check, which bounds each request itself.
    http: reqwest::Client,
}

impl Vercel {
    pub fn new(base: &str, token: String, team: String, project: String) -> Result<Self> {
        let http = reqwest::Client::builder().user_agent(concat!("stowbox/", env!("CARGO_PKG_VERSION"))).connect_timeout(Duration::from_secs(10)).build()?;
        Ok(Self { api: Arc::new(Api::new(base, token, team)?), project, http })
    }

    fn checked(id: &str) -> Result<&str> {
        if convert::valid_name(id) {
            Ok(id)
        } else {
            Err(Error::NotFound(format!("{id:?} is not a Vercel sandbox name")))
        }
    }

    fn sandbox_path(&self, name: &str, query: &str) -> String {
        format!("/v2/sandboxes/{}?projectId={}{query}", seg(name), seg(&self.project))
    }

    /// The sandbox and its current session, started if `resume`.
    async fn fetch(&self, id: &str, resume: bool) -> Result<Value> {
        self.api.get(&self.sandbox_path(Self::checked(id)?, if resume { "&resume=true" } else { "&resume=false" })).await
    }

    fn machine_of(answer: &Value) -> Result<Machine> {
        let sandbox = answer.get("sandbox").filter(|s| s.is_object()).ok_or_else(|| Error::Provider("Vercel answered without the sandbox".into()))?;
        Ok(convert::machine(sandbox, answer.get("session")))
    }

    /// A running session of the sandbox, started if it was stopped: its id
    /// and the directory commands start in.
    async fn running_session(&self, id: &str) -> Result<(String, String)> {
        let answer = self.fetch(id, true).await?;
        let sid = convert::session_id(&answer).ok_or_else(|| Error::Provider("Vercel gave the sandbox no session".into()))?;
        let mut session = answer.get("session").cloned().unwrap_or(Value::Null);
        let deadline = tokio::time::Instant::now() + START_WAIT;
        while convert::status(session.get("status").and_then(Value::as_str).unwrap_or("")) != MachineStatus::Running {
            if tokio::time::Instant::now() > deadline {
                return Err(Error::Network(format!("the Vercel sandbox {id} did not start within {} s", START_WAIT.as_secs())));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            let s = self.api.get(&format!("/v2/sandboxes/sessions/{}", seg(&sid))).await?;
            session = s.get("session").cloned().unwrap_or(Value::Null);
            if matches!(convert::status(session.get("status").and_then(Value::as_str).unwrap_or("")), MachineStatus::Stopped | MachineStatus::Failed) {
                return Err(Error::Provider(format!("the Vercel sandbox {id} stopped while starting")));
            }
        }
        let cwd = session.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
        Ok((sid, cwd))
    }

    /// Pushes the session's stop deadline out to at least `idle` from now.
    /// Vercel only adds to a timeout, so this adds what is missing.
    async fn keep_up(api: &Api, sid: &str, idle: u32) -> Result<()> {
        let s = api.get(&format!("/v2/sandboxes/sessions/{}", seg(sid))).await?;
        // Without a deadline there is nothing to measure from; asking for
        // more than `idle` would keep the box up past its idle stop.
        let Some(deadline) = s.get("session").map(convert::deadline_ms).filter(|d| *d > 0.0) else {
            return Ok(());
        };
        let idle_ms = f64::from(idle) * 1000.0;
        let missing = (now_ms() + idle_ms - deadline).min(idle_ms);
        if missing > 1000.0 {
            api.post(&format!("/v2/sandboxes/sessions/{}/extend-timeout", seg(sid)), json!({ "duration": missing.ceil() as u64 })).await?;
        }
        Ok(())
    }

    fn new_suffix() -> String {
        uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect()
    }
}

#[async_trait]
impl Provider for Vercel {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        // The list proves the token, the team and the project at once.
        self.api.get(&format!("/v2/sandboxes?project={}&limit=1", seg(&self.project))).await?;
        Ok(Account {
            user_id: String::new(),
            display_name: format!("Vercel {}", self.project),
            billing: Billing {
                mode: String::new(),
                currency: "usd".into(),
                balance_micro: 0,
                accruing_micro: None,
                max_vms: 0,
                default_vcpu: DEFAULT_VCPUS,
                default_memory_bytes: (u64::from(DEFAULT_VCPUS) * MIB_PER_VCPU) << 20,
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
            let mut path = format!("/v2/sandboxes?project={}&limit=50", seg(&self.project));
            if !cursor.is_empty() {
                path.push_str(&format!("&cursor={}", seg(&cursor)));
            }
            let page = self.api.get(&path).await?;
            if let Some(items) = page.get("sandboxes").and_then(Value::as_array) {
                out.extend(
                    items.iter().filter(|s| convert::valid_name(s.get("name").and_then(Value::as_str).unwrap_or(""))).map(|s| convert::machine(s, None)),
                );
            }
            match page.get("pagination").and_then(|p| p.get("next")).and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = next.to_string(),
                _ => break,
            }
        }
        Ok(out)
    }

    async fn get_machine(&self, id: &str) -> Result<Machine> {
        Self::machine_of(&self.fetch(id, false).await?)
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let vcpus = match spec.size.as_deref().filter(|s| !s.is_empty()) {
            None => DEFAULT_VCPUS,
            Some(s) => VCPUS.into_iter().find(|&n| size_id(n) == s).ok_or_else(|| Error::Provider(format!("Vercel has no size {s}")))?,
        };
        let idle = match spec.auto_stop_secs {
            Some(0) => return Err(Error::Provider("a Vercel sandbox always stops once idle; pick how long it waits".into())),
            Some(s) => s,
            None => DEFAULT_IDLE_STOP_SECS,
        };
        let env: Map<String, Value> = spec.env.into_iter().map(|(k, v)| (k, json!(v))).collect();
        let body = json!({
            "projectId": self.project,
            "name": convert::name_for(spec.name.as_deref(), &Self::new_suffix()),
            "persistent": true,
            "resources": { "vcpus": vcpus },
            "timeout": u64::from(idle) * 1000,
            "env": env,
            "tags": { "created-by": "stowbox" },
        });
        Self::machine_of(&self.api.post("/v3/sandboxes", body).await?)
    }

    async fn fork_machine(&self, source: &str, name: Option<String>) -> Result<Machine> {
        let source = Self::checked(source)?;
        let src = Self::machine_of(&self.fetch(source, false).await?)?;
        let name = convert::name_for(Some(name.as_deref().unwrap_or(&format!("{source}-fork"))), &Self::new_suffix());
        let name = if name == source { format!("{name}-{}", Self::new_suffix()) } else { name };
        let body = json!({ "name": name, "persistent": true, "timeout": u64::from(src.idle_stop_secs.max(60)) * 1000 });
        Self::machine_of(&self.api.post(&format!("/v2/sandboxes/{}/fork?projectId={}", seg(source), seg(&self.project)), body).await?)
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        let id = Self::checked(id)?;
        match action {
            PowerAction::Stop => {
                let answer = self.fetch(id, false).await?;
                if let Some(sid) = convert::session_id(&answer) {
                    self.api.post(&format!("/v2/sandboxes/sessions/{}/stop", seg(&sid)), json!({})).await?;
                }
            }
            PowerAction::Start | PowerAction::Wake | PowerAction::Resume => {
                self.fetch(id, true).await?;
            }
            PowerAction::Destroy => {
                self.api.delete(&self.sandbox_path(id, "")).await?;
            }
            PowerAction::Hibernate | PowerAction::Pause => {
                return Err(Error::Provider("Vercel keeps a stopped sandbox's disk, not its memory; stop it instead".into()));
            }
            PowerAction::Reboot => return Err(Error::Provider("Vercel cannot reboot a sandbox; stop and start it".into())),
        }
        Ok(())
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Vercel stops a sandbox once idle; set how long it waits instead".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Vercel stops a sandbox once idle; set how long it waits instead".into()))
    }

    /// How long a session waits, idle, before it stops. Vercel has no
    /// "never": its plan caps how long one session runs.
    async fn set_auto_stop(&self, id: &str, secs: u32) -> Result<()> {
        let id = Self::checked(id)?;
        if secs == 0 {
            return Err(Error::Provider("a Vercel sandbox always stops once idle; pick how long it waits".into()));
        }
        self.api.patch(&self.sandbox_path(id, ""), json!({ "timeout": u64::from(secs) * 1000 })).await?;
        Ok(())
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        let id = Self::checked(id)?.to_string();
        let idle = Some(Self::machine_of(&self.fetch(&id, false).await?)?.idle_stop_secs).filter(|s| *s > 0).unwrap_or(DEFAULT_IDLE_STOP_SECS);
        let (sid, cwd) = self.running_session(&id).await?;
        // A session that sat idle may be close to its deadline. Past the
        // plan's cap Vercel refuses to extend it, but the session still runs
        // until then, so that refusal does not fail the exec.
        let _ = Self::keep_up(&self.api, &sid, idle).await;
        let inner =
            if spec.tty { exec::terminal(&self.api, &self.http, &sid, &cwd, spec).await? } else { exec::command(self.api.clone(), sid.clone(), spec).await? };

        // Pass the output through, and keep the session from stopping while
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
                        // Past the plan's cap this fails; the session then
                        // stops and the next exec starts it again.
                        let _ = Self::keep_up(&api, &sid, idle).await;
                    }
                    _ = tx.closed() => return,
                }
            }
        });
        Ok(ExecHandle { input: inner.input, output: rx })
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("Vercel's API does not report what a sandbox has cost; the Vercel dashboard does".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("Vercel's API does not report a balance".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard::resources("usd", RATES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_what_vercel_does() {
        let c = capabilities();
        assert!(c.create && c.fork && c.idle_stop && !c.auto_stop && !c.metered_usage);
        assert!(c.supports(PowerAction::Stop) && c.supports(PowerAction::Start) && c.supports(PowerAction::Destroy));
        assert!(!c.supports(PowerAction::Hibernate));
        assert_eq!(c.default_size, "2-vcpu");
        assert_eq!(c.credential_fields.len(), 3);
    }

    #[test]
    fn sizes_cost_their_list_price_only_while_running() {
        // 2 × 0.128 + 4 GiB × 0.0212 = 0.3408 a running hour.
        assert_eq!(hourly_micro(2), 340_800);
        let card = RateCard::resources("usd", RATES);
        let mut m = crate::testing::machine("s1", MachineStatus::Running);
        m.vcpu = 2;
        m.memory_bytes = 4096 << 20;
        m.disk_bytes = convert::DISK_GIB << 30;
        assert_eq!(card.hourly_micro(&m), 340_800);
        m.status = MachineStatus::Stopped;
        assert_eq!(card.hourly_micro(&m), 0);
    }

    #[test]
    fn names_that_could_change_a_url_are_refused() {
        assert!(Vercel::checked("api-work").is_ok());
        assert!(matches!(Vercel::checked("x/../y"), Err(Error::NotFound(_))));
    }

    /// Live: commands run as the declared user and home, and dropping an
    /// exec or a terminal kills what it left running, not only the shell. Needs
    /// the `STOWBOX_LIVE_VERCEL_*` variables; makes and deletes a sandbox.
    #[tokio::test]
    #[ignore = "makes and deletes a real Vercel sandbox"]
    async fn runs_as_its_user_and_dropping_an_exec_kills_its_children() {
        let (Ok(key), Ok(team), Ok(project)) =
            (std::env::var("STOWBOX_LIVE_VERCEL_KEY"), std::env::var("STOWBOX_LIVE_VERCEL_TEAM"), std::env::var("STOWBOX_LIVE_VERCEL_PROJECT"))
        else {
            return;
        };
        let p = Vercel::new(DEFAULT_API, key, team, project).unwrap();
        let m = p.create_machine(CreateSpec { name: Some("stowbox-live-kill".into()), auto_stop_secs: Some(600), ..Default::default() }).await.unwrap();
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
            let mut h = p.exec(&m.id, ExecSpec { script: "sleep 777 & sleep 778; echo never".into(), ..Default::default() }).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            h.output.close();
            drop(h);
            tokio::time::sleep(Duration::from_secs(4)).await;
            let left = run("pgrep -c -f 'sleep 77[78]' || true").await?;
            let t = p.exec(&m.id, ExecSpec { script: "sleep 779".into(), tty: true, cols: 80, rows: 24, env: vec![] }).await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop(t);
            tokio::time::sleep(Duration::from_secs(4)).await;
            // The bracket keeps pgrep from matching the shell that runs it.
            let after_tty = run("pgrep -c -f 'sleep 77[9]' || true").await?;
            Ok::<_, Error>((who, left, after_tty))
        };
        let result = check.await;
        p.power(&m.id, PowerAction::Destroy).await.unwrap();
        let (who, left, after_tty) = result.unwrap();
        let caps = capabilities();
        assert_eq!(who, format!("{}\n{}", caps.remote_user, caps.remote_home));
        assert_eq!(left, "0");
        assert_eq!(after_tty, "0");
    }

    #[test]
    fn a_connection_needs_every_field() {
        let without_project = Credentials::new([("api_key".into(), "vca_0123456789abcdefghij".into()), (TEAM.into(), "team_x".into())]);
        assert!(matches!(connect(&without_project), Err(Error::Auth(_))));
    }
}
