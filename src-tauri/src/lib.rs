//! The desktop shell: owns the provider connection, the on-disk store, the
//! live terminals and the agent turns, and exposes them to the UI as Tauri
//! commands. Nothing here knows about one provider specifically: boxd,
//! boat, Daytona and Agent37 live in core, and every call about a box goes to the provider the
//! registry says owns it.

mod secrets;

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use stowbox_core::agent::{self, session, AgentMeta, Harness};
use stowbox_core::audit::{AuditLog, Event as AuditEvent};
use stowbox_core::capabilities::Capabilities;
use stowbox_core::cost::{self, RateCard};
use stowbox_core::git::{self, GitReview, PrOutcome};
use stowbox_core::guard::{self, BoxActivity, IdlePolicy, IdleWatch, SpendCheck, SpendFigure, Verdict};
use stowbox_core::metrics::MachineMetrics;
use stowbox_core::notify::{Notice, Notifier};
use stowbox_core::ports;
use stowbox_core::provider::Provider;
use stowbox_core::providers::{self, ConnectContext, Credentials};
use stowbox_core::provision::{self, Prepared, Preset, ProvisionRun, RunEvent, Variable};
use stowbox_core::registry::Registry;
use stowbox_core::ssh;
use stowbox_core::store::Store;
use stowbox_core::types::*;
use stowbox_core::upload::{self, Attachment};
use stowbox_core::worktree;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::AbortHandle;

struct RunHandle {
    machine: String,
    /// The terminal shows an agent.
    agent: bool,
    input: mpsc::Sender<ExecInput>,
    abort: AbortHandle,
}

struct AppState {
    providers: Registry,
    /// The credential store of each provider in `providers::ALL`.
    secrets: HashMap<&'static str, secrets::Secrets>,
    secret_backend: RwLock<HashMap<String, secrets::Backend>>,
    store: Store,
    agents: RwLock<Vec<AgentMeta>>,
    /// What each agent's box last showed, by agent id.
    live: std::sync::Mutex<HashMap<String, session::AgentLive>>,
    /// The last snapshot per box, for a UI that loads after it came.
    live_pushes: std::sync::Mutex<HashMap<String, LivePush>>,
    /// Running state watches, by machine id.
    watches: Mutex<HashMap<String, AbortHandle>>,
    terminals: Mutex<HashMap<String, RunHandle>>,
    /// Each provider's rate card, by provider id, once read.
    rates: RwLock<HashMap<String, RateCard>>,
    presets: RwLock<Vec<Preset>>,
    preset_secrets: secrets::PresetSecrets,
    /// The latest setup run per box, finished or not.
    setup_runs: RwLock<Vec<ProvisionRun>>,
    /// Live setup runs by machine id: the run's id and its runner. The id
    /// says which run a handle belongs to, so cancelling or finishing one
    /// run never touches a newer one in the same box.
    setups: Mutex<HashMap<String, (String, AbortHandle)>>,
    /// Decides which pushes carry a desktop notification.
    notifier: std::sync::Mutex<Notifier>,
    /// When each box was last seen doing something, for the idle guard.
    idle: std::sync::Mutex<IdleWatch>,
    /// The app's data directory (agents, presets, this install's device id).
    data_dir: std::path::PathBuf,
    /// The person's home, where `~/.ssh` is; None when the OS reports none.
    home: Option<std::path::PathBuf>,
    audit: AuditLog,
}

type CmdResult<T> = Result<T, String>;

/// Reverses `encodeURIComponent`; a malformed escape is kept as typed.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push(h << 4 | l);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl AppState {
    /// Adds to the audit log. A write that fails never fails the action.
    fn audit(&self, event: AuditEvent) {
        if let Err(e) = self.audit.record(&event, now_ms()) {
            eprintln!("stowbox: could not write {}: {e}", self.audit.path().display());
        }
    }

    /// The provider that owns the box.
    async fn owner(&self, machine_id: &str) -> CmdResult<Arc<dyn Provider>> {
        self.providers.owner(machine_id).await.map_err(err)
    }

    /// A connected provider by id; `None` or "" is the only one connected.
    fn named(&self, provider: Option<&str>) -> CmdResult<Arc<dyn Provider>> {
        match provider.filter(|p| !p.is_empty()) {
            Some(id) => self.providers.get(id).ok_or_else(|| format!("{id} is not connected")),
            None => self.providers.for_create(None).map_err(err),
        }
    }

    async fn card(&self, p: &Arc<dyn Provider>) -> RateCard {
        if let Some(r) = self.rates.read().await.get(p.id()).cloned() {
            return r;
        }
        let fetched = p.rates().await.ok().filter(RateCard::is_priced);
        let r = match fetched {
            Some(r) => r,
            // boxd's published card until the live one is readable.
            None if p.id() == "boxd" => RateCard::resources("eur", cost::BOXD_PUBLISHED_EUR),
            None => return RateCard { currency: p.capabilities().currency, ..Default::default() },
        };
        self.rates.write().await.insert(p.id().to_string(), r.clone());
        r
    }

    async fn view(&self, p: &Arc<dyn Provider>, m: Machine) -> MachineView {
        let card = self.card(p).await;
        MachineView { est_hourly_micro: card.hourly_micro(&m), currency: card.currency, machine: m }
    }

    async fn configured(&self) -> bool {
        !self.providers.is_empty()
    }

    /// Something the person did in the box restarts its idle clock.
    fn touch(&self, machine_id: &str) {
        self.idle.lock().unwrap().touch(machine_id, now_ms());
    }

    fn observe(&self, machines: &[Machine]) {
        let now = now_ms();
        let mut w = self.idle.lock().unwrap();
        for m in machines {
            w.observe(&m.id, m.status, now);
        }
    }

    /// Everything the idle guard weighs for one box.
    async fn activity(&self, m: &Machine) -> BoxActivity {
        let now = now_ms();
        let mut signals = Vec::new();
        let mut agents_busy = 0;
        let ids: Vec<(String, i64)> = self.agents.read().await.iter().filter(|a| a.machine_id == m.id).map(|a| (a.id.clone(), a.updated_at)).collect();
        {
            let live = self.live.lock().unwrap();
            for (id, updated_at) in ids {
                signals.push(updated_at);
                if let Some(l) = live.get(&id) {
                    signals.push(l.since * 1000);
                    if guard::agent_holds_box(l) {
                        agents_busy += 1;
                    }
                }
            }
        }
        let mut setup_active = false;
        for r in self.setup_runs.read().await.iter().filter(|r| r.machine_id == m.id) {
            signals.push(r.finished_at.unwrap_or(r.started_at));
            setup_active |= r.is_active();
        }
        let terminals_open = self.terminals.lock().await.values().filter(|t| t.machine == m.id && !t.abort.is_finished()).count() as u32;
        let last_activity_ms = self.idle.lock().unwrap().last_activity(&m.id, signals, now);
        BoxActivity { machine_id: m.id.clone(), status: m.status, agents_busy, setup_active, terminals_open, last_activity_ms }
    }

    async fn persist_agents(&self) -> CmdResult<()> {
        let agents = self.agents.read().await.clone();
        self.store.save_agents(&agents).await.map_err(err)
    }

    async fn persist_setup_runs(&self) -> CmdResult<()> {
        let runs = self.setup_runs.read().await.clone();
        self.store.save_runs(&runs).await.map_err(err)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineView {
    #[serde(flatten)]
    machine: Machine,
    est_hourly_micro: i64,
    currency: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInfo {
    id: &'static str,
    connected: bool,
    secret_backend: Option<secrets::Backend>,
    capabilities: Capabilities,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderStatus {
    /// At least one provider is connected.
    configured: bool,
    providers: Vec<ProviderInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderError {
    provider: String,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineList {
    machines: Vec<MachineView>,
    /// Providers whose listing failed; their boxes are missing from `machines`.
    errors: Vec<ProviderError>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TermEvent {
    Data { b64: String },
    Exit { code: i32 },
    Error { message: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewAgent {
    machine_id: String,
    machine_name: String,
    harness: Harness,
    title: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    #[serde(default = "yes")]
    skip_permissions: bool,
    /// Give the agent its own worktree on a new branch, made from the
    /// repository that contains `cwd`, before it first opens.
    #[serde(default)]
    own_branch: bool,
    /// Text to name the branch after (the first task), when there is no title.
    #[serde(default)]
    branch_hint: Option<String>,
}

fn yes() -> bool {
    true
}

// ---------------------------------------------------------------- provider

#[tauri::command]
async fn provider_status(state: State<'_, AppState>) -> CmdResult<ProviderStatus> {
    let backends = state.secret_backend.read().await.clone();
    let providers = providers::ALL
        .iter()
        .map(|kind| ProviderInfo {
            id: kind.id,
            connected: state.providers.get(kind.id).is_some(),
            secret_backend: backends.get(kind.id).copied(),
            capabilities: (kind.capabilities)(),
        })
        .collect();
    Ok(ProviderStatus { configured: state.configured().await, providers })
}

/// Checks the credentials with the provider, then keeps them. Connecting a
/// second provider leaves the first one connected.
#[tauri::command]
async fn set_credentials(state: State<'_, AppState>, provider: String, values: HashMap<String, String>) -> CmdResult<Account> {
    let kind = providers::find(&provider).map_err(err)?;
    let id = kind.id.to_string();
    let credentials = Credentials::new(values);
    credentials.validate(&(kind.capabilities)()).map_err(err)?;
    let encoded = credentials.encode().map_err(err)?;
    let p = (kind.connect)(credentials, &ConnectContext::new(&state.data_dir)).map_err(err)?;
    let account = p.whoami().await.map_err(err)?;
    let store = state.secrets.get(kind.id).ok_or("no key store for this provider")?.clone();
    // The keychain can block on the OS, for a prompt or a locked session.
    let backend =
        tauri::async_runtime::spawn_blocking(move || store.save(&encoded)).await.map_err(err)?.map_err(|e| format!("could not store the key: {e}"))?;
    state.secret_backend.write().await.insert(id.clone(), backend);
    state.audit(AuditEvent::KeySaved { provider: id.clone(), backend: format!("{backend:?}").to_lowercase() });
    state.providers.set(p);
    state.rates.write().await.remove(&id);
    Ok(account)
}

/// Forgets one provider's key, or every key when `provider` is None.
#[tauri::command]
async fn clear_api_key(state: State<'_, AppState>, provider: Option<String>) -> CmdResult<()> {
    let ids: Vec<String> = match provider {
        Some(id) => vec![id],
        None => providers::ALL.iter().map(|k| k.id.to_string()).collect(),
    };
    for id in ids {
        if let Some(s) = state.secrets.get(id.as_str()) {
            s.clear();
            state.audit(AuditEvent::KeyCleared { provider: id.clone() });
        }
        state.secret_backend.write().await.remove(&id);
        state.providers.remove(&id);
        state.rates.write().await.remove(&id);
    }
    Ok(())
}

#[tauri::command]
async fn whoami(state: State<'_, AppState>, provider: Option<String>) -> CmdResult<Account> {
    state.named(provider.as_deref())?.whoami().await.map_err(err)
}

// ---------------------------------------------------------------- machines

/// Every connected provider's boxes. A provider that fails is reported in
/// `errors` and its boxes are left out; only when all of them fail is the
/// whole call an error.
#[tauri::command]
async fn list_machines(state: State<'_, AppState>) -> CmdResult<MachineList> {
    if !state.configured().await {
        return Err(stowbox_core::Error::NotConfigured.to_string());
    }
    let listing = state.providers.list_machines().await;
    if listing.machines.is_empty() && !listing.errors.is_empty() && listing.errors.len() == state.providers.all().len() {
        let msgs: Vec<String> = listing.errors.iter().map(|(id, e)| format!("{id}: {e}")).collect();
        return Err(msgs.join("; "));
    }
    state.observe(&listing.machines);
    let mut out = Vec::with_capacity(listing.machines.len());
    for m in listing.machines {
        let Some(p) = state.providers.get(&m.provider) else { continue };
        out.push(state.view(&p, m).await);
    }
    out.sort_by(|a, b| a.machine.name.cmp(&b.machine.name).then_with(|| a.machine.provider.cmp(&b.machine.provider)));
    let errors = listing.errors.into_iter().map(|(id, e)| ProviderError { provider: id.to_string(), message: e.to_string() }).collect();
    Ok(MachineList { machines: out, errors })
}

#[tauri::command]
async fn get_machine(state: State<'_, AppState>, id: String) -> CmdResult<MachineView> {
    let (p, m) = state.providers.get_machine(&id).await.map_err(err)?;
    Ok(state.view(&p, m).await)
}

/// Creates the box, then, when a setup preset is picked, starts running it
/// in the background. A setup that cannot start (a missing secret value)
/// or a variable without a value) fails before the box is created, so
/// nothing is billed for nothing.
#[tauri::command]
async fn create_machine(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: CreateSpec,
    preset_id: Option<String>,
    variables: Option<HashMap<String, String>>,
) -> CmdResult<MachineView> {
    let p = state.providers.for_create(spec.provider.as_deref()).map_err(err)?;
    let setup = match preset_id.filter(|id| !id.is_empty()) {
        Some(id) => Some(prepare_setup(&state, &id, &variables.unwrap_or_default()).await?),
        None => None,
    };
    let m = state.providers.adopt(&p, p.create_machine(spec).await.map_err(err)?);
    state.audit(AuditEvent::BoxCreated { provider: m.provider.clone(), machine_id: m.id.clone(), machine_name: m.name.clone() });
    if let Some(setup) = setup {
        start_setup(&app, &state, p.clone(), &m, setup).await?;
    }
    Ok(state.view(&p, m).await)
}

#[tauri::command]
async fn fork_machine(state: State<'_, AppState>, source: String, name: Option<String>) -> CmdResult<MachineView> {
    let p = state.owner(&source).await?;
    let m = state.providers.adopt(&p, p.fork_machine(&source, name).await.map_err(err)?);
    state.audit(AuditEvent::BoxForked { source, machine_id: m.id.clone(), machine_name: m.name.clone() });
    Ok(state.view(&p, m).await)
}

#[tauri::command]
async fn machine_power(state: State<'_, AppState>, id: String, action: PowerAction) -> CmdResult<()> {
    let p = state.owner(&id).await?;
    if !p.capabilities().supports(action) {
        return Err(format!("{} boxes cannot {}", p.capabilities().label, format!("{action:?}").to_lowercase()));
    }
    p.power(&id, action).await.map_err(err)?;
    state.audit(AuditEvent::Power { machine_id: id.clone(), power: action });
    if action == PowerAction::Destroy {
        state.providers.forget(&id);
        if let Some((_, h)) = state.setups.lock().await.remove(&id) {
            h.abort();
        }
        state.setup_runs.write().await.retain(|r| r.machine_id != id);
        state.persist_setup_runs().await?;
        // Agents that lived there are kept as history; their sessions went
        // with the box.
        if let Some(h) = state.watches.lock().await.remove(&id) {
            h.abort();
        }
        state.live_pushes.lock().unwrap().remove(&id);
        let ids: Vec<String> = state.agents.read().await.iter().filter(|a| a.machine_id == id).map(|a| a.id.clone()).collect();
        let mut live = state.live.lock().unwrap();
        for a in ids {
            live.remove(&a);
        }
    }
    Ok(())
}

#[tauri::command]
async fn machine_usage(state: State<'_, AppState>, id: String) -> CmdResult<Usage> {
    state.owner(&id).await?.machine_usage(&id).await.map_err(err)
}

/// One live sample from inside the box, or `None` when the box is not
/// running and was left alone. Called per box by the fleet view.
#[tauri::command]
async fn machine_metrics(state: State<'_, AppState>, id: String) -> CmdResult<Option<MachineMetrics>> {
    state.owner(&id).await?.metrics(&id).await.map_err(err)
}

/// Stops the box `secs` from now, or never with 0, for providers with auto-stop.
#[tauri::command]
async fn set_auto_stop(state: State<'_, AppState>, id: String, secs: u32) -> CmdResult<MachineView> {
    let p = state.owner(&id).await?;
    p.set_auto_stop(&id, secs).await.map_err(err)?;
    let m = state.providers.adopt(&p, p.get_machine(&id).await.map_err(err)?);
    Ok(state.view(&p, m).await)
}

#[tauri::command]
async fn rates(state: State<'_, AppState>, provider: Option<String>) -> CmdResult<RateCard> {
    let p = state.named(provider.as_deref())?;
    Ok(state.card(&p).await)
}

#[tauri::command]
async fn org_billing(state: State<'_, AppState>, provider: Option<String>, org: String) -> CmdResult<OrgBilling> {
    state.named(provider.as_deref())?.org_billing(&org).await.map_err(err)
}

// ---------------------------------------------------------------- cost guards

// The idle guard runs from the UI, which shows a countdown with an Undo
// before anything is suspended; `guard_suspend` decides again from fresh
// state at the end of it. The decisions live in stowbox_core::guard.

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuardEntry {
    machine_id: String,
    machine_name: String,
    verdict: Verdict,
}

#[tauri::command]
async fn guard_check(state: State<'_, AppState>, policy: IdlePolicy) -> CmdResult<Vec<GuardEntry>> {
    let machines = state.providers.list_machines().await.machines;
    state.observe(&machines);
    let now = now_ms();
    let mut out = Vec::new();
    for m in machines {
        let verdict = guard::decide(&state.activity(&m).await, &policy, now);
        out.push(GuardEntry { machine_id: m.id, machine_name: m.name, verdict });
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuardOutcome {
    /// The verdict at the moment of acting; anything but `suspend` means
    /// the box was left running.
    verdict: Verdict,
    suspended: bool,
    /// The provider has no standby, so the box was stopped instead.
    stopped: bool,
    machine_name: String,
    /// The estimate before and after, from the rate card.
    hourly_before_micro: i64,
    hourly_after_micro: i64,
    currency: String,
}

#[tauri::command]
async fn guard_suspend(state: State<'_, AppState>, machine_id: String, policy: IdlePolicy) -> CmdResult<GuardOutcome> {
    let (p, m) = state.providers.get_machine(&machine_id).await.map_err(err)?;
    state.observe(std::slice::from_ref(&m));
    let verdict = guard::decide(&state.activity(&m).await, &policy, now_ms());
    let card = state.card(&p).await;
    let hourly_before_micro = card.hourly_micro(&m);
    let suspended = matches!(verdict, Verdict::Suspend { .. });
    // Standby where the provider has it, else hibernate (Agent37's sleep,
    // which wakes on the next connection); a provider with neither (boat,
    // Daytona) is stopped.
    let caps = p.capabilities();
    let (action, after) = if caps.supports(PowerAction::Pause) {
        (PowerAction::Pause, MachineStatus::Suspended)
    } else if caps.supports(PowerAction::Hibernate) {
        (PowerAction::Hibernate, MachineStatus::Hibernated)
    } else {
        (PowerAction::Stop, MachineStatus::Stopped)
    };
    let hourly_after_micro = if suspended {
        p.power(&m.id, action).await.map_err(err)?;
        card.hourly_micro(&Machine { status: after, ..m.clone() })
    } else {
        hourly_before_micro
    };
    Ok(GuardOutcome {
        verdict,
        suspended,
        stopped: suspended && action == PowerAction::Stop,
        machine_name: m.name,
        hourly_before_micro,
        hourly_after_micro,
        currency: card.currency,
    })
}

/// Undo on the countdown: the box counts as used just now.
#[tauri::command]
async fn guard_touch(state: State<'_, AppState>, machine_id: String) -> CmdResult<()> {
    state.touch(&machine_id);
    Ok(())
}

/// Today's spend against `limit_micro`, in `currency` (the limit's). `since`
/// is local midnight in unix seconds, which only the UI knows. A provider's
/// meter counts when it is readable (boxd's needs an org admin); otherwise
/// its boxes' current hourly estimate counts, projected over a day.
/// Providers that bill in another currency are named in `excluded`.
#[tauri::command]
async fn spend_today(state: State<'_, AppState>, since: i64, limit_micro: i64, currency: Option<String>) -> CmdResult<SpendCheck> {
    let providers = state.providers.all();
    if providers.is_empty() {
        return Err(stowbox_core::Error::NotConfigured.to_string());
    }
    let mut cards = Vec::new();
    for p in &providers {
        cards.push(state.card(p).await);
    }
    let currency = currency.filter(|c| !c.is_empty()).unwrap_or_else(|| cards[0].currency.clone());
    let mut figures = Vec::new();
    let mut excluded = Vec::new();
    for (p, card) in providers.iter().zip(&cards) {
        if !card.currency.eq_ignore_ascii_case(&currency) {
            excluded.push(p.capabilities().label);
            continue;
        }
        let figure = match p.org_usage("", since, 0).await {
            Ok(u) => SpendFigure::Metered { total_micro: u.total_cost_micro, complete: u.complete },
            Err(e) => {
                let machines = p.list_machines().await.map_err(err)?;
                SpendFigure::Projected { hourly_micro: machines.iter().map(|m| card.hourly_micro(m)).sum(), why: e.to_string() }
            }
        };
        figures.push(figure);
    }
    let mut check = guard::check_spend(guard::combine(figures), limit_micro, &currency);
    check.excluded = excluded;
    Ok(check)
}

// ---------------------------------------------------------------- git review

// The repository that holds `cwd` in the box. Each call is one exec; the
// scripts and the parsing live in stowbox_core::git. `cwd` None is $HOME.

#[tauri::command]
async fn git_review(state: State<'_, AppState>, machine_id: String, cwd: Option<String>) -> CmdResult<GitReview> {
    state.touch(&machine_id);
    let p = state.owner(&machine_id).await?;
    git::review(p.as_ref(), &machine_id, cwd.as_deref()).await.map_err(err)
}

#[tauri::command]
async fn git_commit(state: State<'_, AppState>, machine_id: String, cwd: Option<String>, message: String) -> CmdResult<String> {
    state.touch(&machine_id);
    let p = state.owner(&machine_id).await?;
    git::commit(p.as_ref(), &machine_id, cwd.as_deref(), &message).await.map_err(err)
}

#[tauri::command]
async fn git_push(state: State<'_, AppState>, machine_id: String, cwd: Option<String>) -> CmdResult<String> {
    state.touch(&machine_id);
    let p = state.owner(&machine_id).await?;
    git::push(p.as_ref(), &machine_id, cwd.as_deref()).await.map_err(err)
}

#[tauri::command]
async fn git_open_pr(state: State<'_, AppState>, machine_id: String, cwd: Option<String>) -> CmdResult<PrOutcome> {
    state.touch(&machine_id);
    let p = state.owner(&machine_id).await?;
    git::open_pr(p.as_ref(), &machine_id, cwd.as_deref()).await.map_err(err)
}

// ---------------------------------------------------------------- ports

// HTTPS links and raw forwards are provider calls that do not wake the box;
// reading what listens inside it is one exec, which does, so it only runs
// on a sleeping box when `wake` is set. Parsing lives in stowbox_core::ports.

#[tauri::command]
async fn https_routes(state: State<'_, AppState>, machine_id: String) -> CmdResult<Vec<ports::HttpsRoute>> {
    state.owner(&machine_id).await?.https_routes(&machine_id).await.map_err(err)
}

#[tauri::command]
async fn add_https_route(state: State<'_, AppState>, machine_id: String, name: String, port: u32) -> CmdResult<ports::HttpsRoute> {
    let port = ports::validate_port(port).map_err(err)?;
    ports::validate_route_name(&name).map_err(err)?;
    state.touch(&machine_id);
    state.owner(&machine_id).await?.add_https_route(&machine_id, &name, port).await.map_err(err)
}

#[tauri::command]
async fn remove_https_route(state: State<'_, AppState>, machine_id: String, name: String) -> CmdResult<()> {
    state.touch(&machine_id);
    state.owner(&machine_id).await?.remove_https_route(&machine_id, &name).await.map_err(err)
}

/// `port` None goes back to detecting the port.
#[tauri::command]
async fn set_default_route_port(state: State<'_, AppState>, machine_id: String, port: Option<u32>) -> CmdResult<()> {
    let port = port.map(ports::validate_port).transpose().map_err(err)?;
    state.touch(&machine_id);
    state.owner(&machine_id).await?.set_default_route_port(&machine_id, port).await.map_err(err)
}

#[tauri::command]
async fn port_forwards(state: State<'_, AppState>, machine_id: String) -> CmdResult<Vec<ports::PortForward>> {
    state.owner(&machine_id).await?.port_forwards(&machine_id).await.map_err(err)
}

#[tauri::command]
async fn expose_port(state: State<'_, AppState>, machine_id: String, port: u32, protocol: String) -> CmdResult<ports::PortForward> {
    let port = ports::validate_port(port).map_err(err)?;
    ports::validate_protocol(&protocol).map_err(err)?;
    state.touch(&machine_id);
    state.owner(&machine_id).await?.expose_port(&machine_id, port, &protocol).await.map_err(err)
}

#[tauri::command]
async fn unexpose_port(state: State<'_, AppState>, machine_id: String, port: u32) -> CmdResult<()> {
    let port = ports::validate_port(port).map_err(err)?;
    state.touch(&machine_id);
    state.owner(&machine_id).await?.unexpose_port(&machine_id, port).await.map_err(err)
}

/// `None` when the box is not running and was left alone.
#[tauri::command]
async fn listening_ports(state: State<'_, AppState>, machine_id: String, wake: Option<bool>) -> CmdResult<Option<ports::Listening>> {
    let p = state.owner(&machine_id).await?;
    ports::listening(p.as_ref(), &machine_id, wake.unwrap_or(false)).await.map_err(err)
}

// ---------------------------------------------------------------- editor over SSH

// Opening a box in VS Code or Cursor. Only `open_in_editor` writes: the
// Stowbox key, the managed block in ~/.ssh/config and Stowbox's
// known_hosts, all in stowbox_core::ssh. `ssh_plan` only reads.

// The home directory comes from the OS (the profile folder on Windows,
// %USERPROFILE%, never $HOME), which is where Windows' OpenSSH and VS Code's
// Remote-SSH read `.ssh\config`. The config itself names files as `~/.ssh/...`.
fn ssh_files(state: &AppState) -> CmdResult<ssh::Files> {
    let home = state.home.as_ref().ok_or("the OS reports no home directory, so there is no ~/.ssh to write")?;
    Ok(ssh::Files::new(home.join(".ssh"), "~/.ssh"))
}

/// This install's id for `LinkSshKey`, so linking again replaces Stowbox's
/// earlier key on this computer instead of adding one per click.
fn ssh_device(state: &AppState) -> CmdResult<ssh::Device> {
    let path = state.data_dir.join("ssh-device-id");
    let id = match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            let id = format!("stowbox-{}", uuid::Uuid::new_v4().simple());
            std::fs::create_dir_all(&state.data_dir).map_err(err)?;
            std::fs::write(&path, &id).map_err(err)?;
            id
        }
    };
    Ok(ssh::Device { id, label: ssh::computer_name() })
}

#[tauri::command]
async fn ssh_plan(state: State<'_, AppState>, machine_id: String) -> CmdResult<ssh::Plan> {
    let (p, m) = state.providers.get_machine(&machine_id).await.map_err(err)?;
    if !p.capabilities().editor_ssh {
        return Err(format!("Opening a {} box in an editor is not supported yet", p.capabilities().label));
    }
    ssh::plan(&ssh_files(&state)?, &m).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorOpened {
    url: String,
    alias: String,
    changes: Vec<String>,
}

#[tauri::command]
async fn open_in_editor(app: AppHandle, state: State<'_, AppState>, machine_id: String, editor: ssh::Editor, dir: Option<String>) -> CmdResult<EditorOpened> {
    let (p, m) = state.providers.get_machine(&machine_id).await.map_err(err)?;
    if !p.capabilities().editor_ssh {
        return Err(format!("Opening a {} box in an editor is not supported yet", p.capabilities().label));
    }
    let files = ssh_files(&state)?;
    if let Some(blocker) = ssh::plan(&files, &m).map_err(err)?.blocker {
        return Err(blocker);
    }
    let device = ssh_device(&state)?;
    state.touch(&m.id);
    let mut applied = ssh::setup(p.as_ref(), &files, &m, &device, &ssh::keygen_program()).await.map_err(err)?;
    // The editor connects as soon as it opens; wake the box first, as the
    // terminal does, rather than leave the editor waiting on a sleeping one.
    if let Some(action) = p.capabilities().wake_action(m.status) {
        p.power(&m.id, action).await.map_err(err)?;
        applied.changes.push(format!("Woke {}", m.name));
    }
    let url = ssh::editor_url(editor, &applied.alias, &ssh::remote_path(dir.as_deref()));
    app.opener().open_url(&url, None::<&str>).map_err(|e| format!("Could not hand the link to {}: {e}. Is it installed?", editor.label()))?;
    Ok(EditorOpened { url, alias: applied.alias, changes: applied.changes })
}

// ---------------------------------------------------------------- agents

#[tauri::command]
async fn list_agents(state: State<'_, AppState>) -> CmdResult<Vec<AgentMeta>> {
    let mut agents = state.agents.read().await.clone();
    agents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(agents)
}

#[tauri::command]
async fn create_agent(state: State<'_, AppState>, spec: NewAgent) -> CmdResult<AgentMeta> {
    let id = uuid::Uuid::new_v4().to_string();
    let title = spec.title.filter(|t| !t.trim().is_empty()).unwrap_or_default();
    let mut cwd = spec.cwd.filter(|c| !c.trim().is_empty());
    let mut own = None;
    if spec.own_branch {
        let p = state.owner(&spec.machine_id).await?;
        let hint = if title.is_empty() { spec.branch_hint.unwrap_or_default() } else { title.clone() };
        let added = worktree::add(p.as_ref(), &spec.machine_id, cwd.as_deref(), &worktree::slug(&hint, &id)).await.map_err(err)?;
        cwd = Some(added.cwd);
        own = Some(added.worktree);
    }
    let now = now_ms();
    let meta = AgentMeta {
        id,
        title,
        harness: spec.harness,
        machine_id: spec.machine_id,
        machine_name: spec.machine_name,
        cwd,
        model: spec.model.filter(|m| !m.trim().is_empty()),
        // Claude Code takes the id up front, so every open resumes this session.
        session_id: (spec.harness == Harness::ClaudeCode).then(|| uuid::Uuid::new_v4().to_string()),
        skip_permissions: spec.skip_permissions,
        created_at: now,
        updated_at: now,
        worktree: own,
    };
    state.agents.write().await.push(meta.clone());
    state.persist_agents().await?;
    Ok(meta)
}

#[tauri::command]
async fn rename_agent(state: State<'_, AppState>, id: String, title: String) -> CmdResult<AgentMeta> {
    let meta = {
        let mut agents = state.agents.write().await;
        let a = agents.iter_mut().find(|a| a.id == id).ok_or("agent not found")?;
        a.title = title.trim().to_string();
        a.updated_at = now_ms();
        a.clone()
    };
    state.persist_agents().await?;
    Ok(meta)
}

/// Ends the agent's harness in the box and forgets the agent.
/// `remove_worktree` also removes its worktree (its branch stays) first; if
/// git refuses, for example over uncommitted files without `force`, the
/// agent is kept and the error carries git's words.
#[tauri::command]
async fn delete_agent(state: State<'_, AppState>, id: String, remove_worktree: Option<bool>, force: Option<bool>) -> CmdResult<()> {
    let gone = state.agents.read().await.iter().find(|a| a.id == id).cloned();
    if let (true, Some(a)) = (remove_worktree.unwrap_or(false), gone.as_ref()) {
        if let Some(wt) = &a.worktree {
            let p = state.owner(&a.machine_id).await?;
            session::stop(p.as_ref(), &a.machine_id, &a.id, false).await.map_err(err)?;
            worktree::remove(p.as_ref(), &a.machine_id, wt, force.unwrap_or(false)).await.map_err(err)?;
        }
    }
    // The harness may still run in its tmux session; end it on the way out,
    // best effort, unless the box is gone.
    let owner = match &gone {
        Some(a) => state.owner(&a.machine_id).await.ok(),
        None => None,
    };
    if let (Some(a), Some(p)) = (gone, owner) {
        tauri::async_runtime::spawn(async move {
            let _ = session::stop(p.as_ref(), &a.machine_id, &a.id, true).await;
        });
    }
    state.agents.write().await.retain(|a| a.id != id);
    state.live.lock().unwrap().remove(&id);
    state.notifier.lock().unwrap_or_else(|e| e.into_inner()).forget_agent(&id);
    state.persist_agents().await
}

/// Ends the agent's harness in the box. The agent stays; opening it again
/// resumes its session.
#[tauri::command]
async fn agent_stop(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let a = state.agents.read().await.iter().find(|a| a.id == id).cloned().ok_or("agent not found")?;
    let p = state.owner(&a.machine_id).await?;
    state.touch(&a.machine_id);
    session::stop(p.as_ref(), &a.machine_id, &id, false).await.map_err(err)
}

/// What the watches last saw: per box whether it has tmux, per agent its state.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LivePush {
    machine_id: String,
    tmux: bool,
    agents: Vec<session::AgentLive>,
    /// Agents that just went from working to waiting.
    notices: Vec<Notice>,
}

#[tauri::command]
async fn agents_live(state: State<'_, AppState>) -> CmdResult<Vec<LivePush>> {
    Ok(state.live_pushes.lock().unwrap().values().cloned().collect())
}

/// Copies a file into the box for the agent; the UI types its path at the
/// prompt. The body is the file's raw bytes, so a large file does not cross
/// as a base64 string; the agent id and the file name (percent-encoded) come
/// as `x-agent` and `x-name` headers. Errors carry the provider's message.
#[tauri::command]
async fn agent_upload(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> CmdResult<Attachment> {
    let header = |k: &str| request.headers().get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let id = header("x-agent");
    let name = percent_decode(&header("x-name"));
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("the file did not arrive as bytes".into());
    };
    let machine_id = state.agents.read().await.iter().find(|a| a.id == id).map(|a| a.machine_id.clone()).ok_or("agent not found")?;
    let bytes = bytes.clone();
    let p = state.owner(&machine_id).await?;
    upload::check_size(&name, bytes.len() as u64, p.capabilities().max_upload_bytes).map_err(err)?;
    state.touch(&machine_id);
    upload::upload(p.as_ref(), &machine_id, &id, &name, bytes).await.map_err(err)
}

/// Whether the box's watch should go on: an agent terminal is open there,
/// or an agent was last seen working, so its end is worth catching.
async fn keep_watching(state: &AppState, machine_id: &str) -> bool {
    let terminals = state.terminals.lock().await.values().any(|t| t.machine == machine_id && t.agent && !t.abort.is_finished());
    if terminals {
        return true;
    }
    let ids: Vec<String> = state.agents.read().await.iter().filter(|a| a.machine_id == machine_id).map(|a| a.id.clone()).collect();
    let live = state.live.lock().unwrap();
    ids.iter().any(|id| live.get(id).is_some_and(guard::agent_holds_box))
}

/// Starts reading the box's agent states, unless that already runs.
async fn ensure_watch(app: &AppHandle, state: &AppState, machine_id: &str) {
    let mut watches = state.watches.lock().await;
    if watches.get(machine_id).is_some_and(|h| !h.is_finished()) {
        return;
    }
    let task = tauri::async_runtime::spawn(watch_box(app.clone(), machine_id.to_string()));
    watches.insert(machine_id.to_string(), task.inner().abort_handle());
}

async fn watch_box(app: AppHandle, machine_id: String) {
    let state = app.state::<AppState>();
    let mut attempt = 0u32;
    loop {
        let Ok(p) = state.owner(&machine_id).await else { break };
        if let Ok(mut w) = session::watch(p.as_ref(), &machine_id).await {
            while let Some(snap) = w.next().await {
                attempt = 0;
                apply_live(&app, &state, &machine_id, snap).await;
                if !keep_watching(&state, &machine_id).await {
                    state.watches.lock().await.remove(&machine_id);
                    return;
                }
            }
        }
        // The stream dropped. Only come back while there is a reason to;
        // a box in standby is not woken for it.
        attempt += 1;
        let running = p.get_machine(&machine_id).await.map(|m| m.status == MachineStatus::Running).unwrap_or(false);
        if !running || attempt > 5 || !keep_watching(&state, &machine_id).await {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2 * attempt as u64)).await;
    }
    state.watches.lock().await.remove(&machine_id);
}

/// Records a box's snapshot and tells the UI, with a notice for each agent
/// that just finished working.
async fn apply_live(app: &AppHandle, state: &AppState, machine_id: &str, snap: session::BoxAgents) {
    let agents: Vec<AgentMeta> = state.agents.read().await.iter().filter(|a| a.machine_id == machine_id).cloned().collect();
    let mut notices = Vec::new();
    let mut seen = Vec::new();
    {
        let mut live = state.live.lock().unwrap();
        let mut notifier = state.notifier.lock().unwrap_or_else(|e| e.into_inner());
        for a in &agents {
            let l =
                snap.agents.iter().find(|l| l.id == a.id).cloned().unwrap_or(session::AgentLive { id: a.id.clone(), state: None, since: 0, running: false });
            if let Some(n) = notifier.agent(a, &l) {
                notices.push(n);
            }
            live.insert(a.id.clone(), l.clone());
            seen.push(l);
        }
    }
    let push = LivePush { machine_id: machine_id.to_string(), tmux: snap.tmux, agents: seen, notices };
    state.live_pushes.lock().unwrap().insert(machine_id.to_string(), LivePush { notices: Vec::new(), ..push.clone() });
    let _ = app.emit("agents-live", push);
}

// ---------------------------------------------------------------- setup presets

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretView {
    name: String,
    has_value: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PresetView {
    #[serde(flatten)]
    preset: Preset,
    secrets: Vec<SecretView>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretInput {
    name: String,
    /// `None` or empty keeps the stored value.
    value: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresetInput {
    id: Option<String>,
    name: String,
    commands: Vec<String>,
    secrets: Vec<SecretInput>,
    /// Defaults by name; the names themselves come from the commands.
    #[serde(default)]
    variables: Vec<Variable>,
}

/// A run as the UI sees it: the record without its output, plus the text
/// that arrived with this change. The full output comes from `setup_runs`.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SetupPush {
    run: ProvisionRun,
    append: String,
    notice: Option<Notice>,
}

fn preset_view(state: &AppState, p: &Preset) -> PresetView {
    let values = state.preset_secrets.load(&p.id);
    PresetView {
        secrets: p.secret_names.iter().map(|n| SecretView { name: n.clone(), has_value: values.get(n).is_some_and(|v| !v.is_empty()) }).collect(),
        preset: p.clone(),
    }
}

#[tauri::command]
async fn list_presets(state: State<'_, AppState>) -> CmdResult<Vec<PresetView>> {
    let presets = state.presets.read().await.clone();
    Ok(presets.iter().map(|p| preset_view(&state, p)).collect())
}

#[tauri::command]
async fn save_preset(state: State<'_, AppState>, input: PresetInput) -> CmdResult<PresetView> {
    let name = input.name.trim().to_string();
    let commands: Vec<String> = input.commands.iter().map(|c| c.trim_end().to_string()).filter(|c| !c.trim().is_empty()).collect();
    let secret_names: Vec<String> = input.secrets.iter().map(|s| s.name.trim().to_string()).collect();
    provision::validate(&name, &commands, &secret_names).map_err(err)?;

    let now = now_ms();
    let existing = match &input.id {
        Some(id) => Some(state.presets.read().await.iter().find(|p| &p.id == id).cloned().ok_or("preset not found")?),
        None => None,
    };
    let id = existing.as_ref().map(|p| p.id.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let old = state.preset_secrets.load(&id);
    let mut values = HashMap::new();
    let mut changed = Vec::new();
    for (s, name) in input.secrets.iter().zip(&secret_names) {
        match s.value.as_ref().filter(|v| !v.is_empty()) {
            Some(v) => {
                if old.get(name) != Some(v) {
                    changed.push(name.clone());
                }
                values.insert(name.clone(), v.clone());
            }
            None => match old.get(name) {
                Some(v) => {
                    values.insert(name.clone(), v.clone());
                }
                None => return Err(format!("give {name} a value")),
            },
        }
    }
    state.preset_secrets.save(&id, &values).map_err(|e| format!("could not store the secrets: {e}"))?;
    state.audit(AuditEvent::PresetSaved { preset_id: id.clone(), preset_name: name.clone(), secret_names: secret_names.clone(), changed });

    let preset = Preset {
        id: id.clone(),
        name,
        commands: commands.clone(),
        secret_names,
        variables: provision::preset_variables(&commands, &input.variables),
        created_at: existing.as_ref().map(|p| p.created_at).unwrap_or(now),
        updated_at: now,
    };
    let all = {
        let mut presets = state.presets.write().await;
        match presets.iter_mut().find(|p| p.id == id) {
            Some(p) => *p = preset.clone(),
            None => presets.push(preset.clone()),
        }
        presets.clone()
    };
    state.store.save_presets(&all).await.map_err(err)?;
    Ok(preset_view(&state, &preset))
}

#[tauri::command]
async fn delete_preset(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let (all, gone) = {
        let mut presets = state.presets.write().await;
        let gone = presets.iter().find(|p| p.id == id).map(|p| p.name.clone());
        presets.retain(|p| p.id != id);
        (presets.clone(), gone)
    };
    state.store.save_presets(&all).await.map_err(err)?;
    state.preset_secrets.delete(&id);
    if let Some(preset_name) = gone {
        state.audit(AuditEvent::PresetDeleted { preset_id: id, preset_name });
    }
    Ok(())
}

/// Everything one setup run needs, checked before anything starts.
struct Setup {
    preset: Preset,
    prepared: Prepared,
    secrets: Vec<(String, String)>,
}

/// The preset with its variables filled in and its secret values in the
/// preset's order, or an error naming the first thing that has no value.
async fn prepare_setup(state: &AppState, id: &str, variables: &HashMap<String, String>) -> CmdResult<Setup> {
    let preset = state.presets.read().await.iter().find(|p| p.id == id).cloned().ok_or("setup preset not found")?;
    let values = state.preset_secrets.load(id);
    let mut secrets = Vec::with_capacity(preset.secret_names.len());
    for n in &preset.secret_names {
        match values.get(n).filter(|v| !v.is_empty()) {
            Some(v) => secrets.push((n.clone(), v.clone())),
            None => return Err(format!("the secret {n} in \"{}\" has no value; edit the preset first", preset.name)),
        }
    }
    let prepared = provision::prepare(&preset, variables).map_err(err)?;
    Ok(Setup { preset, prepared, secrets })
}

/// Folds one event into the box's run, tells the UI, and persists the run
/// once it is over. Events for a run that already ended are dropped.
async fn apply_setup_event(app: &AppHandle, state: &AppState, run_id: &str, ev: RunEvent) {
    let (run, done) = {
        let mut runs = state.setup_runs.write().await;
        let Some(r) = runs.iter_mut().find(|r| r.id == run_id) else { return };
        if !r.is_active() {
            return;
        }
        r.apply(&ev, now_ms());
        (r.clone(), !r.is_active())
    };
    let append = if let RunEvent::Output(t) = &ev { t.clone() } else { String::new() };
    let notice = if done { state.notifier.lock().unwrap_or_else(|e| e.into_inner()).setup(&run) } else { None };
    let _ = app.emit("setup-event", SetupPush { run: ProvisionRun { output: String::new(), ..run.clone() }, append, notice });
    if done {
        state.audit(AuditEvent::SetupEnded {
            run_id: run.id.clone(),
            machine_id: run.machine_id.clone(),
            status: run.status,
            secrets_written: run.secrets_written,
        });
        let mut setups = state.setups.lock().await;
        if setups.get(&run.machine_id).is_some_and(|(id, _)| *id == run.id) {
            setups.remove(&run.machine_id);
        }
        drop(setups);
        let _ = state.persist_setup_runs().await;
    }
}

async fn start_setup(app: &AppHandle, state: &AppState, p: Arc<dyn Provider>, machine: &Machine, setup: Setup) -> CmdResult<ProvisionRun> {
    let Setup { preset, prepared, secrets } = setup;
    let mut setups = state.setups.lock().await;
    if setups.contains_key(&machine.id) {
        return Err(format!("a setup is already running in {}", machine.name));
    }
    let run = ProvisionRun::new(uuid::Uuid::new_v4().to_string(), machine, &preset, &prepared, now_ms());
    {
        let mut runs = state.setup_runs.write().await;
        runs.retain(|r| r.machine_id != machine.id);
        runs.push(run.clone());
    }
    let _ = app.emit("setup-event", SetupPush { run: run.clone(), append: String::new(), notice: None });
    state.audit(AuditEvent::SetupStarted {
        run_id: run.id.clone(),
        machine_id: machine.id.clone(),
        machine_name: machine.name.clone(),
        preset_id: preset.id.clone(),
        preset_name: preset.name.clone(),
        secret_names: secrets.iter().map(|(n, _)| n.clone()).collect(),
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<RunEvent>();
    let machine_id = machine.id.clone();
    let commands = prepared.commands;
    let runner = tokio::spawn(async move {
        provision::run(&*p, &machine_id, &commands, &secrets, |ev| {
            let _ = tx.send(ev);
        })
        .await;
    });
    setups.insert(machine.id.clone(), (run.id.clone(), runner.abort_handle()));
    drop(setups);

    let app2 = app.clone();
    let run_id = run.id.clone();
    tokio::spawn(async move {
        let st = app2.state::<AppState>();
        while let Some(ev) = rx.recv().await {
            apply_setup_event(&app2, &st, &run_id, ev).await;
        }
        // The runner is gone. If nothing ended the run, it was interrupted.
        apply_setup_event(&app2, &st, &run_id, RunEvent::Failed("the setup stopped without reporting an exit".into())).await;
    });
    Ok(run)
}

#[tauri::command]
async fn setup_start(
    app: AppHandle,
    state: State<'_, AppState>,
    machine_id: String,
    preset_id: String,
    variables: Option<HashMap<String, String>>,
) -> CmdResult<ProvisionRun> {
    let p = state.owner(&machine_id).await?;
    let setup = prepare_setup(&state, &preset_id, &variables.unwrap_or_default()).await?;
    let m = p.get_machine(&machine_id).await.map_err(err)?;
    start_setup(&app, &state, p, &m, setup).await
}

#[tauri::command]
async fn setup_cancel(app: AppHandle, state: State<'_, AppState>, machine_id: String) -> CmdResult<()> {
    let Some((run_id, h)) = state.setups.lock().await.remove(&machine_id) else { return Ok(()) };
    // Mark it first, so the runner's own "stopped" report after the abort
    // finds the run already over and is dropped. By id: a setup started in
    // this box since the handle was taken is another run.
    apply_setup_event(&app, &state, &run_id, RunEvent::Cancelled).await;
    // Aborting drops the exec stream, which kills the remote process group.
    h.abort();
    Ok(())
}

#[tauri::command]
async fn setup_runs(state: State<'_, AppState>) -> CmdResult<Vec<ProvisionRun>> {
    Ok(state.setup_runs.read().await.clone())
}

// ---------------------------------------------------------------- terminals

/// What a terminal runs: a login shell, an agent (its harness on its own
/// session, in its tmux session when the box has tmux), or Claude Code's
/// sign-in.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum TermTarget {
    Shell,
    Agent { id: String, input: Option<String> },
    ClaudeLogin,
}

#[tauri::command]
async fn claude_account(state: State<'_, AppState>, machine: String) -> CmdResult<agent::account::ClaudeAccount> {
    let p = state.owner(&machine).await?;
    agent::account::status(&*p, &machine).await.map_err(err)
}

#[tauri::command]
async fn set_claude_account(state: State<'_, AppState>, machine: String, own: bool) -> CmdResult<agent::account::ClaudeAccount> {
    let p = state.owner(&machine).await?;
    state.touch(&machine);
    agent::account::set(&*p, &machine, own).await.map_err(err)
}

#[tauri::command]
async fn terminal_open(
    app: AppHandle,
    state: State<'_, AppState>,
    machine: String,
    cols: u16,
    rows: u16,
    target: Option<TermTarget>,
    on_data: Channel<TermEvent>,
) -> CmdResult<String> {
    let target = target.unwrap_or(TermTarget::Shell);
    let is_agent = matches!(target, TermTarget::Agent { .. });
    let script = match target {
        TermTarget::Shell => "exec \"${SHELL:-bash}\" -l".to_string(),
        TermTarget::ClaudeLogin => agent::account::login_command(),
        TermTarget::Agent { id, input } => {
            let a = {
                let mut agents = state.agents.write().await;
                let a = agents.iter_mut().find(|a| a.id == id).ok_or("no such agent")?;
                a.updated_at = now_ms();
                a.clone()
            };
            if a.machine_id != machine {
                return Err("the agent lives in another box".into());
            }
            state.persist_agents().await?;
            session::open_script(&a, input.as_deref())
        }
    };
    let p = state.owner(&machine).await?;
    let handle = p
        .exec(
            &machine,
            ExecSpec {
                script,
                tty: true,
                cols: cols.max(20),
                rows: rows.max(5),
                env: vec![("TERM".into(), "xterm-256color".into()), ("COLORTERM".into(), "truecolor".into())],
            },
        )
        .await
        .map_err(err)?;
    let ExecHandle { input, mut output } = handle;
    let id = uuid::Uuid::new_v4().to_string();
    let b64 = base64::engine::general_purpose::STANDARD;
    let pump = tokio::spawn(async move {
        while let Some(chunk) = output.recv().await {
            let ev = match chunk {
                ExecOutput::Stdout(b) | ExecOutput::Stderr(b) => TermEvent::Data { b64: b64.encode(b) },
                ExecOutput::Exit(code) => {
                    let _ = on_data.send(TermEvent::Exit { code });
                    break;
                }
                ExecOutput::Error(message) => {
                    let _ = on_data.send(TermEvent::Error { message });
                    break;
                }
            };
            if on_data.send(ev).is_err() {
                break;
            }
        }
    });
    state.touch(&machine);
    state.terminals.lock().await.insert(id.clone(), RunHandle { machine: machine.clone(), agent: is_agent, input, abort: pump.abort_handle() });
    if is_agent {
        ensure_watch(&app, &state, &machine).await;
    }
    Ok(id)
}

#[tauri::command]
async fn terminal_write(state: State<'_, AppState>, id: String, data: String) -> CmdResult<()> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(err)?;
    let terms = state.terminals.lock().await;
    let t = terms.get(&id).ok_or("terminal is closed")?;
    state.touch(&t.machine);
    t.input.send(ExecInput::Data(bytes)).await.map_err(|_| "terminal is closed".to_string())
}

#[tauri::command]
async fn terminal_resize(state: State<'_, AppState>, id: String, cols: u16, rows: u16) -> CmdResult<()> {
    let terms = state.terminals.lock().await;
    let t = terms.get(&id).ok_or("terminal is closed")?;
    t.input.send(ExecInput::Resize { cols, rows }).await.map_err(|_| "terminal is closed".to_string())
}

#[tauri::command]
async fn terminal_close(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(t) = state.terminals.lock().await.remove(&id) {
        t.abort.abort();
        state.touch(&t.machine);
    }
    Ok(())
}

// ---------------------------------------------------------------- app

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            let home = app.path().home_dir().ok();
            let secrets: HashMap<&'static str, secrets::Secrets> =
                providers::ALL.iter().map(|k| (k.id, secrets::Secrets::new(data.join("secrets"), k.id))).collect();
            let store = Store::new(data.join("state"));
            let preset_secrets = secrets::PresetSecrets::new(data.join("secrets"));

            // `setup` runs on the main thread, outside Tokio; the boxd client
            // binds its lazy channel to the runtime it is created in.
            let _runtime = tauri::async_runtime::handle().inner().enter();
            let providers = Registry::new();
            let mut backends = HashMap::new();
            let context = ConnectContext::new(&data);
            for kind in providers::ALL {
                let Some((stored, backend)) = secrets.get(kind.id).and_then(secrets::Secrets::load) else { continue };
                backends.insert(kind.id.to_string(), backend);
                match (kind.connect)(Credentials::decode(&stored), &context) {
                    Ok(p) => providers.set(p),
                    Err(e) => eprintln!("stowbox: could not initialise the {} client: {e}", kind.id),
                }
            }
            let agents = tauri::async_runtime::block_on(store.load_agents()).unwrap_or_else(|e| {
                eprintln!("stowbox: could not read agents.json: {e}");
                Vec::new()
            });
            let presets = tauri::async_runtime::block_on(store.load_presets()).unwrap_or_else(|e| {
                eprintln!("stowbox: could not read presets.json: {e}");
                Vec::new()
            });
            // Setup runs do not survive a restart either; say so on the record.
            let setup_runs = tauri::async_runtime::block_on(store.load_runs())
                .unwrap_or_default()
                .into_iter()
                .map(|mut r| {
                    if r.is_active() {
                        r.apply(&RunEvent::Failed("Stowbox was closed while the setup was running".into()), now_ms());
                    }
                    r
                })
                .collect();

            app.manage(AppState {
                providers,
                secrets,
                secret_backend: RwLock::new(backends),
                store,
                agents: RwLock::new(agents),
                live: std::sync::Mutex::new(HashMap::new()),
                live_pushes: std::sync::Mutex::new(HashMap::new()),
                watches: Mutex::new(HashMap::new()),
                terminals: Mutex::new(HashMap::new()),
                rates: RwLock::new(HashMap::new()),
                presets: RwLock::new(presets),
                preset_secrets,
                setup_runs: RwLock::new(setup_runs),
                setups: Mutex::new(HashMap::new()),
                notifier: std::sync::Mutex::new(Notifier::default()),
                idle: std::sync::Mutex::new(IdleWatch::new()),
                data_dir: data.clone(),
                home,
                audit: AuditLog::new(data.join("state")),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            provider_status,
            set_credentials,
            clear_api_key,
            whoami,
            list_machines,
            get_machine,
            create_machine,
            fork_machine,
            machine_power,
            machine_usage,
            machine_metrics,
            set_auto_stop,
            org_billing,
            rates,
            guard_check,
            guard_suspend,
            guard_touch,
            spend_today,
            list_agents,
            create_agent,
            rename_agent,
            delete_agent,
            agent_stop,
            agents_live,
            agent_upload,
            list_presets,
            save_preset,
            delete_preset,
            setup_start,
            setup_cancel,
            setup_runs,
            git_review,
            git_commit,
            git_push,
            git_open_pr,
            https_routes,
            add_https_route,
            remove_https_route,
            set_default_route_port,
            port_forwards,
            expose_port,
            unexpose_port,
            listening_ports,
            ssh_plan,
            open_in_editor,
            terminal_open,
            claude_account,
            set_claude_account,
            terminal_write,
            terminal_resize,
            terminal_close,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = app {
        eprintln!("stowbox: could not run: {e}");
        std::process::exit(1);
    }
}
