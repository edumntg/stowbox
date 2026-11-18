//! Agent37 (agent37.com) behind the `Provider` trait. See `docs/agent37.md`.
//!
//! The lifecycle is the Sandbox API (`https://api.agent37.com/v1`). Its
//! exec takes no stdin and has no TTY, so every exec is an SSH channel, as
//! with boat. An instance has no address of its own: SSH goes through a
//! WebSocket to `wss://{id}-22022.agent37.app` carrying the API key, and the
//! edge hands the stream to the instance (see [`crate::wstunnel`]). Stowbox
//! links its own ed25519 key to the workspace once (`/v1/ssh-keys`); a key
//! reaches running instances within about two minutes, so the first logins
//! after linking are retried. The host key is not checked: the instance
//! makes a new one whenever it moves, and TLS plus the API key at the edge
//! already say who answers.
//!
//! `agent37-base` ships no tmux and no harness, and its user has no sudo, so
//! a new instance gets tmux (as root) and Claude Code and Codex (as `node`,
//! into its own npm prefix, so they can update themselves) through the REST
//! exec before the create returns, in about half a minute, with a profile.d
//! script that puts npm's prefix on PATH for SSH logins. The install runs
//! in the background and is polled for: one request held open that long can
//! be dropped while the command carries on. The whole disk persists across
//! stop, sleep and restart, and a fork copies it.
//!
//! Auto-sleep is the idle stop here: an instance with no traffic for its
//! timeout is checkpointed (`sleeping`, disk billed alone) and wakes on the
//! next request. An open SSH session keeps it awake.

pub mod api;
pub mod convert;

use crate::capabilities::{Capabilities, SizeOption};
use crate::cost::{RateCard, SizeRate};
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
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const DEFAULT_API: &str = "https://api.agent37.com/v1";
pub const DEFAULT_SANDBOX_API: &str = "https://sandbox.agent37.app/v1";
pub const DEFAULT_EDGE: &str = "agent37.app";
pub const DEFAULT_CONSOLE_URL: &str = "https://www.agent37.com/dashboard/cloud";
/// The instance port the edge carries SSH on.
const SSH_PORT: u16 = 22022;
/// The image's own user, whom exec and the agents run as.
const USER: &str = "node";
/// A plain Linux sandbox with Node, Python, git and build tools.
const TEMPLATE: &str = "agent37-base";
/// Where the install's log and exit code land, in the user's home.
const BOOTSTRAP_DIR: &str = "/home/node/.stowbox";
/// Run as root in a new instance: what agents need that the image lacks.
///
/// The container's env (npm's prefix in the user's home, and its `bin` on
/// PATH) is not what an SSH login gets, so a profile.d script puts it back
/// for the `bash -lc` every exec runs in.
const BOOTSTRAP: &str = "mkdir -p /home/node/.stowbox && chown node:node /home/node/.stowbox && \
    printf '%s\\n' 'export NPM_CONFIG_PREFIX=\"$HOME/.npm-global\"' \
      'case \":$PATH:\" in *\":$HOME/.npm-global/bin:\"*) ;; *) PATH=\"$HOME/.local/bin:$HOME/.npm-global/bin:$PATH\" ;; esac' \
      > /etc/profile.d/agent37-npm.sh && \
    ( export DEBIAN_FRONTEND=noninteractive; \
      (command -v tmux >/dev/null || (apt-get update -qq && apt-get install -y -qq tmux)) && \
      runuser -u node -- npm install -g --no-fund --no-audit @anthropic-ai/claude-code @openai/codex \
    ) > /home/node/.stowbox/bootstrap.log 2>&1; echo $? > /home/node/.stowbox/bootstrap.exit";
/// How long the install may take before the create returns without it.
const BOOTSTRAP_WAIT: Duration = Duration::from_secs(300);
/// The idle timeout of an instance made without one; Agent37 takes 5 min to a day.
const DEFAULT_IDLE_SECS: u32 = 3600;
const IDLE_RANGE: (u32, u32) = (300, 86_400);
/// How long a newly linked key may take to reach running instances.
const KEY_SPREAD: Duration = Duration::from_secs(150);
/// Signed preview links last this long.
const LINK_SECS: u32 = 24 * 3600;
/// Agent37's list prices, per hour (its monthly rates over 730 hours):
/// $0.80 a vCPU, $0.70 a GB of memory, $0.09 a GB of disk.
const RATES: Rates = Rates { vcpu_hour_micro: 1_096, ram_gib_hour_micro: 959, disk_gib_hour_micro: 123 };
/// Compute multiples of the instance types above `default`.
const TYPES: &[(&str, f64)] = &[("vm", 2.5), ("performance", 4.0)];

/// One of the four shapes every type comes in, with its default disk.
pub struct Shape {
    pub id: &'static str,
    pub cpu: u32,
    pub memory_gb: u32,
    pub disk_gb: u32,
}

/// What a create without a size gets.
const DEFAULT_SHAPE: Shape = Shape { id: "default", cpu: 2, memory_gb: 4, disk_gb: 4 };

pub const SHAPES: &[Shape] = &[
    DEFAULT_SHAPE,
    Shape { id: "medium", cpu: 4, memory_gb: 8, disk_gb: 6 },
    Shape { id: "large", cpu: 4, memory_gb: 16, disk_gb: 12 },
    Shape { id: "xlarge", cpu: 4, memory_gb: 32, disk_gb: 24 },
];

fn running_micro(s: &Shape, multiple: f64) -> i64 {
    let compute = (s.cpu as i64 * RATES.vcpu_hour_micro + s.memory_gb as i64 * RATES.ram_gib_hour_micro) as f64 * multiple;
    compute.round() as i64 + s.disk_gb as i64 * RATES.disk_gib_hour_micro
}

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        label: "agent37".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_hint: "Make one on the Agent37 dashboard under Cloud API, API keys. Stowbox links its own SSH key to the workspace the first time it opens a terminal.".into(),
        key_prefixes: vec!["sk_live_".into()],
        currency: "usd".into(),
        power: vec![Start, Stop, Reboot, Hibernate, Wake, Destroy],
        fork: true,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: true,
        custom_size: false,
        sizes: SHAPES
            .iter()
            .map(|s| SizeOption {
                id: s.id.into(),
                vcpu: s.cpu,
                memory_bytes: s.memory_gb as u64 * 1024 * 1024 * 1024,
                disk_bytes: s.disk_gb as u64 * 1024 * 1024 * 1024,
                hourly_micro: running_micro(s, 1.0),
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
        remote_user: USER.into(),
        remote_home: format!("/home/{USER}"),
        metering_note: "Agent37 settles metered minutes at least hourly and reports them by UTC day; managed model spend is not counted here.".into(),
        metered_usage: true,
    }
}

#[derive(Debug, Clone)]
pub struct Agent37Config {
    pub api: String,
    pub sandbox_api: String,
    /// The domain instance URLs live under: `{id}-{port}.{edge}`.
    pub edge: String,
    /// Where Stowbox keeps its own SSH key for Agent37 instances.
    pub ssh_key: PathBuf,
}

impl Agent37Config {
    pub fn new(ssh_key: PathBuf) -> Self {
        Self { api: DEFAULT_API.into(), sandbox_api: DEFAULT_SANDBOX_API.into(), edge: DEFAULT_EDGE.into(), ssh_key }
    }
}

pub struct Agent37 {
    api: Arc<Api>,
    edge: String,
    ssh: sshexec::Pool,
    /// The key's SHA256 fingerprint, as `/v1/ssh-keys` lists it.
    fingerprint: String,
    /// When this Stowbox linked its key; None until it checked the list.
    linked: tokio::sync::Mutex<Option<Option<Instant>>>,
    /// Signed links made from this session, per instance: Agent37 has no
    /// call that lists them.
    links: Mutex<HashMap<String, Vec<HttpsRoute>>>,
}

/// Instance ids are ten lower-case letters and digits; anything else would
/// put the WebSocket somewhere other than the instance's own host.
fn check_id(id: &str) -> Result<()> {
    if !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(Error::NotFound(format!("{id} is not an Agent37 instance id")))
    }
}

fn idle_secs(secs: u32) -> u32 {
    secs.clamp(IDLE_RANGE.0, IDLE_RANGE.1)
}

impl Agent37 {
    pub fn new(api_key: String, config: Agent37Config) -> Result<Self> {
        let key = sshexec::load_or_make_key(&config.ssh_key)?;
        let fingerprint = key.public_key().fingerprint(russh::keys::HashAlg::Sha256).to_string();
        Ok(Self {
            api: Arc::new(Api::new(&config.api, &config.sandbox_api, api_key)?),
            edge: config.edge,
            ssh: sshexec::Pool::new(key)?,
            fingerprint,
            linked: Default::default(),
            links: Default::default(),
        })
    }

    /// The signed links made so far. A panic elsewhere while the lock was
    /// held leaves the map as it was, which is still a valid cache.
    fn links(&self) -> MutexGuard<'_, HashMap<String, Vec<HttpsRoute>>> {
        self.links.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Installs what agents need in a new instance and waits for it. The
    /// exit code of the install, or an error when it could not be started
    /// or did not end in time.
    async fn bootstrap(&self, id: &str) -> Result<i64> {
        self.api.exec(id, BOOTSTRAP, true, true).await?;
        let deadline = Instant::now() + BOOTSTRAP_WAIT;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_secs(3)).await;
            // A poll that fails (the instance settling) is tried again.
            if let Ok(v) = self.api.exec(id, &format!("cat {BOOTSTRAP_DIR}/bootstrap.exit 2>/dev/null"), false, false).await {
                if let Ok(code) = v.get("stdout").and_then(Value::as_str).unwrap_or("").trim().parse::<i64>() {
                    return Ok(code);
                }
            }
        }
        Err(Error::Network(format!("installing tmux and the agents in {id} did not finish within five minutes")))
    }

    async fn instance(&self, id: &str) -> Result<Value> {
        check_id(id)?;
        self.api.get(&format!("/instances/{}", seg(id))).await
    }

    /// Links Stowbox's key to the workspace unless it already is. Returns
    /// when it was linked from here, if that was recent enough to matter.
    async fn ensure_key(&self) -> Result<Option<Instant>> {
        let mut linked = self.linked.lock().await;
        if let Some(at) = *linked {
            return Ok(at.filter(|t| t.elapsed() < KEY_SPREAD));
        }
        let keys = self.api.list("/ssh-keys").await?;
        let at = if keys.iter().any(|k| k.get("fingerprint").and_then(Value::as_str) == Some(self.fingerprint.as_str())) {
            None
        } else {
            match self.api.post("/ssh-keys", Some(json!({ "public_key": self.ssh.public_line(), "name": "stowbox" }))).await {
                Ok(_) => Some(Instant::now()),
                // Linked from another window in the meantime.
                Err(Error::Provider(m)) if m.contains("ssh_key_exists") => None,
                Err(Error::Provider(m)) if m.contains("invalid_request") => {
                    return Err(Error::Provider(format!("Agent37 would not link Stowbox's SSH key: {m}. A workspace holds at most 20 keys; remove one on the dashboard")));
                }
                Err(e) => return Err(e),
            }
        };
        *linked = Some(at);
        Ok(at)
    }
}

/// How long a sleeping instance gets to leave `sleeping` after a start
/// before another one is sent.
const START_AGAIN_AFTER: Duration = Duration::from_secs(30);

/// Whether to send a start to an instance that still reports `sleeping`:
/// once, then again only if the last one has had time and did not take.
/// The status lags the start by a few polls, and every poll is 2 s.
fn start_due(last: Option<Instant>, now: Instant) -> bool {
    // Not `is_none_or`: it is newer than the crate's rust-version.
    last.map_or(true, |t| now.duration_since(t) >= START_AGAIN_AFTER)
}

/// Wakes a sleeping instance and says how to reach it through the edge.
struct EdgeAuth<'a>(&'a Agent37);

#[async_trait]
impl Authorize for EdgeAuth<'_> {
    async fn authorize(&self, id: &str, _: &str) -> Result<Endpoint> {
        let p = self.0;
        p.ensure_key().await?;
        let mut i = p.instance(id).await?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut started: Option<Instant> = None;
        loop {
            match i.get("status").and_then(Value::as_str).unwrap_or("") {
                "running" => break,
                "sleeping" if start_due(started, Instant::now()) => {
                    p.api.post(&format!("/instances/{}/start", seg(id)), None).await?;
                    started = Some(Instant::now());
                }
                "sleeping" => {}
                "waking" | "starting" | "restarting" | "provisioning" | "updating" => {}
                "stopped" | "stopping" => return Err(Error::Provider("the instance is stopped; start it first".into())),
                s => return Err(Error::Provider(format!("the instance is {}; it cannot take a terminal", if s.is_empty() { "in an unknown state" } else { s }))),
            }
            if Instant::now() > deadline {
                return Err(Error::Network("the instance did not come up within two minutes".into()));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            i = p.instance(id).await?;
        }
        Ok(Endpoint {
            host: format!("{id}-{SSH_PORT}.{}", p.edge),
            port: 443,
            user: USER.into(),
            login: Login::Key,
            check: HostCheck::Edge,
            dial: Dial::WebSocket(vec![("X-Agent37-Key".into(), p.api.key().to_string())]),
            provider: "Agent37",
        })
    }
}

#[async_trait]
impl Provider for Agent37 {
    fn id(&self) -> &'static str {
        "agent37"
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        Ok(convert::account(&self.api.get("/billing").await?))
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let all = self.api.list("/instances").await?;
        Ok(all.iter().filter(|i| !convert::is_gone(i)).map(convert::machine).collect())
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        Ok(convert::machine(&self.instance(id_or_name).await?))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let shape = SHAPES.iter().find(|s| Some(s.id) == spec.size.as_deref()).unwrap_or(&DEFAULT_SHAPE);
        let mut body = Map::new();
        body.insert("template".into(), json!(TEMPLATE));
        body.insert("resources".into(), json!({ "cpu": shape.cpu, "memory": shape.memory_gb, "disk": shape.disk_gb }));
        if let Some(n) = spec.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            body.insert("name".into(), json!(n));
        }
        match spec.auto_stop_secs.unwrap_or(DEFAULT_IDLE_SECS) {
            0 => {
                body.insert("auto_sleep".into(), json!(false));
            }
            s => {
                body.insert("auto_sleep".into(), json!(true));
                body.insert("idle_timeout_seconds".into(), json!(idle_secs(s)));
            }
        }
        if !spec.env.is_empty() {
            body.insert("env".into(), Value::Object(spec.env.into_iter().map(|(k, v)| (k, json!(v))).collect()));
        }
        body.insert("metadata".into(), json!({ "created_by": "stowbox" }));
        let i = self.api.post("/instances", Some(Value::Object(body))).await?;
        let m = convert::machine(&i);
        // The box is made either way; without the tools an agent's terminal
        // says what is missing, and the log is in ~/.stowbox/bootstrap.log.
        let _ = self.bootstrap(&m.id).await;
        Ok(m)
    }

    async fn fork_machine(&self, source: &str, name: Option<String>) -> Result<Machine> {
        check_id(source)?;
        let mut body = Map::new();
        if let Some(n) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            body.insert("name".into(), json!(n));
        }
        let i = self.api.post(&format!("/instances/{}/fork", seg(source)), Some(Value::Object(body))).await?;
        Ok(convert::machine(&i))
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        check_id(id)?;
        let path = format!("/instances/{}", seg(id));
        match action {
            PowerAction::Start | PowerAction::Resume | PowerAction::Wake => {
                self.api.post(&format!("{path}/start"), None).await?;
            }
            PowerAction::Stop => {
                self.api.post(&format!("{path}/stop"), None).await?;
            }
            PowerAction::Reboot => {
                self.api.post(&format!("{path}/restart"), None).await?;
            }
            PowerAction::Hibernate => {
                self.api.post(&format!("{path}/sleep"), None).await?;
            }
            PowerAction::Destroy => {
                self.api.delete(&path).await?;
                self.links().remove(id);
            }
            PowerAction::Pause => return Err(Error::Provider("Agent37 instances have no standby; put it to sleep instead".into())),
        }
        // A wake or a start can bring the instance back on another host.
        self.ssh.drop_conn(id);
        Ok(())
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Agent37's auto-sleep is set as its auto-stop".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Agent37 has no auto standby; set its auto-stop".into()))
    }

    /// The idle timeout before the instance sleeps, in seconds; 0 turns it off.
    async fn set_auto_stop(&self, id: &str, secs: u32) -> Result<()> {
        check_id(id)?;
        let body = if secs == 0 { json!({ "auto_sleep": false }) } else { json!({ "auto_sleep": true, "idle_timeout_seconds": idle_secs(secs) }) };
        self.api.patch(&format!("/instances/{}", seg(id)), body).await?;
        Ok(())
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        check_id(id)?;
        loop {
            match self.ssh.exec(&EdgeAuth(self), id, spec.clone()).await {
                // A key linked a moment ago has not reached the instance yet.
                Err(Error::Auth(m)) => match self.ensure_key().await? {
                    Some(_) => tokio::time::sleep(Duration::from_secs(10)).await,
                    None => return Err(Error::Auth(m)),
                },
                r => return r,
            }
        }
    }

    async fn machine_usage(&self, id: &str) -> Result<Usage> {
        let i = self.instance(id).await?;
        let m = convert::machine(&i);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        // The API answers up to 90 days at a time.
        let since = m.created_at.max(now - 89 * 86_400);
        let u = self.api.get(&format!("/usage?from={}&to={}", convert::utc_day(since), convert::utc_day(now))).await?;
        Ok(convert::machine_usage(&u, id, m.vcpu, m.memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0), since, now))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        let b = self.api.get("/billing").await?;
        let awake = self.api.list("/instances").await?.iter().filter(|i| i.get("status").and_then(Value::as_str) == Some("running")).count() as u32;
        Ok(convert::org_billing(&b, awake))
    }

    async fn org_usage(&self, _org: &str, since: i64, until: i64) -> Result<OrgUsage> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let until = if until == 0 { now } else { until };
        let since = since.max(until - 89 * 86_400);
        let u = self.api.get(&format!("/usage?from={}&to={}", convert::utc_day(since), convert::utc_day(until))).await?;
        Ok(convert::org_usage(&u, since, until))
    }

    async fn rates(&self) -> Result<RateCard> {
        let sizes = TYPES
            .iter()
            .flat_map(|(kind, multiple)| SHAPES.iter().map(move |s| SizeRate { size: convert::size_key(kind, s.cpu as i64, s.memory_gb as i64), hourly_micro: running_micro(s, *multiple) }))
            .collect();
        Ok(RateCard { currency: "usd".into(), rates: RATES, sizes })
    }

    async fn https_routes(&self, id: &str) -> Result<Vec<HttpsRoute>> {
        Ok(self.links().get(id).cloned().unwrap_or_default())
    }

    /// A signed link to `port`: every port has a URL at the edge, but a
    /// browser can open it only with a token.
    async fn add_https_route(&self, id: &str, _name: &str, port: u16) -> Result<HttpsRoute> {
        check_id(id)?;
        let path = format!("/instances/{}/signed-url", seg(id));
        let r = match self.api.post(&path, Some(json!({ "port": port, "ttl_seconds": LINK_SECS }))).await {
            Ok(v) => v,
            Err(Error::Provider(_)) => self.api.post(&path, Some(json!({ "port": port, "ttl_seconds": 3600 }))).await?,
            Err(e) => return Err(e),
        };
        let url = r.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let domain = url.trim_start_matches("https://").split(['/', '?']).next().unwrap_or("").to_string();
        let route = HttpsRoute { name: port.to_string(), domain, url: Some(url).filter(|u| !u.is_empty()), port: port as u32, port_mode: "locked".into(), is_default: false };
        let mut links = self.links();
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
    fn the_menu_matches_agent37s_monthly_prices() {
        let caps = capabilities();
        assert_eq!(caps.sizes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["default", "medium", "large", "xlarge"]);
        // Agent37's pricing page, for 730 hours at the default disk.
        let month = |s: &SizeOption| crate::cost::money(s.hourly_micro * 730, "usd");
        assert_eq!(caps.sizes.iter().map(month).collect::<Vec<_>>(), vec!["$4.76", "$9.34", "$15.48", "$27.76"]);
        assert!(caps.supports(PowerAction::Hibernate) && caps.supports(PowerAction::Reboot) && !caps.supports(PowerAction::Pause));
        assert!(caps.fork && caps.idle_stop && !caps.auto_stop && !caps.editor_ssh);
    }

    #[test]
    fn a_sleeping_box_bills_its_disk_alone() {
        let card = RateCard::resources("usd", RATES);
        let mut m = crate::testing::machine("uwop3g494x", MachineStatus::Running);
        m.vcpu = 2;
        m.memory_bytes = 4 * 1024 * 1024 * 1024;
        m.disk_bytes = 4 * 1024 * 1024 * 1024;
        assert_eq!(card.hourly_micro(&m), 2 * 1_096 + 4 * 959 + 4 * 123);
        m.status = MachineStatus::Hibernated;
        assert_eq!(card.hourly_micro(&m), 4 * 123);
    }

    #[tokio::test]
    async fn vm_and_performance_types_have_their_multiple() {
        let dir = std::env::temp_dir().join(format!("stowbox-a37-{}", uuid::Uuid::new_v4()));
        let p = Agent37::new("sk_live_test".into(), Agent37Config::new(dir.join("key"))).unwrap();
        let card = p.rates().await.unwrap();
        let perf = card.sizes.iter().find(|s| s.size == "performance-2x4").unwrap();
        // 4 × (2 × 1096 + 4 × 959) + 4 × 123
        assert_eq!(perf.hourly_micro, 4 * (2 * 1_096 + 4 * 959) + 4 * 123);
        assert!(card.sizes.iter().any(|s| s.size == "vm-4x32"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_sleeping_instance_gets_one_start_per_window() {
        let t0 = Instant::now();
        assert!(start_due(None, t0));
        assert!(!start_due(Some(t0), t0 + Duration::from_secs(2)));
        assert!(!start_due(Some(t0), t0 + START_AGAIN_AFTER - Duration::from_millis(1)));
        assert!(start_due(Some(t0), t0 + START_AGAIN_AFTER));
    }

    #[test]
    fn ids_cannot_point_the_tunnel_elsewhere() {
        assert!(check_id("uwop3g494x").is_ok());
        for bad in ["", "evil.example.com#", "UPPER", "a-b", "x/../y"] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
        assert_eq!((idle_secs(60), idle_secs(3600), idle_secs(10 * 86_400)), (300, 3600, 86_400));
    }

    /// Against a real instance: `AGENT37_API_KEY=sk_live_… AGENT37_INSTANCE=<id>
    /// cargo test -p stowbox-core agent37::tests::live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs an Agent37 key and instance; run with --ignored"]
    async fn live_exec_over_the_edge() {
        let (Ok(key), Ok(id)) = (std::env::var("AGENT37_API_KEY"), std::env::var("AGENT37_INSTANCE")) else { panic!("set AGENT37_API_KEY and AGENT37_INSTANCE") };
        let dir = std::env::temp_dir().join("stowbox-a37-live");
        let p = Agent37::new(key, Agent37Config::new(dir.join("agent37_ed25519"))).unwrap();
        assert!(p.list_machines().await.unwrap().iter().any(|m| m.id == id));

        // stdin in, stdout and the exit code out.
        let mut h = p.exec(&id, ExecSpec { script: "id -un; cat; exit 3".into(), ..Default::default() }).await.unwrap();
        h.input.send(ExecInput::Data(b"through the tunnel\n".to_vec())).await.unwrap();
        h.input.send(ExecInput::Close).await.unwrap();
        let (mut out, mut code) = (Vec::new(), None);
        while let Some(o) = h.output.recv().await {
            match o {
                ExecOutput::Stdout(b) => out.extend(b),
                ExecOutput::Exit(c) => code = Some(c),
                ExecOutput::Error(e) => panic!("{e}"),
                ExecOutput::Stderr(_) => {}
            }
        }
        assert_eq!((String::from_utf8_lossy(&out).as_ref(), code), ("node\nthrough the tunnel\n", Some(3)));

        // A TTY, on the same connection.
        let mut h = p.exec(&id, ExecSpec { script: "tty; stty size".into(), tty: true, cols: 100, rows: 30, ..Default::default() }).await.unwrap();
        let mut out = Vec::new();
        while let Some(o) = h.output.recv().await {
            if let ExecOutput::Stdout(b) = o {
                out.extend(b);
            }
        }
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains("/dev/pts/") && out.contains("30 100"), "{out}");

        // A file over the default upload, larger than one WebSocket frame.
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(p.upload_file(&id, "/tmp/stowbox-live.bin", data.clone()).await.unwrap(), data.len() as u64);
        let mut h = p.exec(&id, ExecSpec { script: "sha1sum < /tmp/stowbox-live.bin".into(), ..Default::default() }).await.unwrap();
        let mut out = Vec::new();
        while let Some(o) = h.output.recv().await {
            if let ExecOutput::Stdout(b) = o {
                out.extend(b);
            }
        }
        use sha1::Digest;
        let want = format!("{:x}", sha1::Sha1::digest(&data));
        assert!(String::from_utf8_lossy(&out).starts_with(&want), "{}", String::from_utf8_lossy(&out));
    }

    /// Makes a box, checks the tools went in, forks it and deletes both:
    /// `AGENT37_API_KEY=sk_live_… cargo test -p stowbox-core agent37::tests::live_lifecycle -- --ignored`.
    /// Costs a few minutes of the smallest shape.
    #[tokio::test]
    #[ignore = "makes and deletes Agent37 instances; run with --ignored"]
    async fn live_lifecycle() {
        let key = std::env::var("AGENT37_API_KEY").expect("set AGENT37_API_KEY");
        let p = Agent37::new(key, Agent37Config::new(std::env::temp_dir().join("stowbox-a37-live").join("agent37_ed25519"))).unwrap();
        let m = p.create_machine(CreateSpec { name: Some("stowbox-live-test".into()), auto_stop_secs: Some(900), ..Default::default() }).await.unwrap();
        // Checked after both boxes are deleted, so a failure leaves nothing running.
        let (tools, idle_after, fork) = async {
            let mut h = p.exec(&m.id, ExecSpec { script: "tmux -V && claude --version && codex --version".into(), ..Default::default() }).await.unwrap();
            let (mut out, mut code) = (Vec::new(), None);
            while let Some(o) = h.output.recv().await {
                match o {
                    ExecOutput::Stdout(b) | ExecOutput::Stderr(b) => out.extend(b),
                    ExecOutput::Exit(c) => code = Some(c),
                    ExecOutput::Error(e) => out.extend(e.into_bytes()),
                }
            }
            let idle = match p.set_auto_stop(&m.id, 0).await {
                Ok(()) => p.get_machine(&m.id).await.map(|x| x.idle_stop_secs).ok(),
                Err(_) => None,
            };
            (code.map(|c| (c, String::from_utf8_lossy(&out).to_string())), idle, p.fork_machine(&m.id, Some("stowbox-live-fork".into())).await)
        }
        .await;
        if let Ok(f) = &fork {
            p.power(&f.id, PowerAction::Destroy).await.unwrap();
        }
        p.power(&m.id, PowerAction::Destroy).await.unwrap();
        assert_eq!((m.status, m.vcpu, m.idle_stop_secs), (MachineStatus::Running, 2, 900));
        let (code, out) = tools.expect("the exec ended without an exit code");
        assert_eq!(code, 0, "{out}");
        assert_eq!(idle_after, Some(0));
        let fork = fork.unwrap();
        assert!(!p.list_machines().await.unwrap().iter().any(|x| x.id == m.id || x.id == fork.id));
    }
}
