//! The desktop shell: owns the provider connection, the on-disk store, the
//! live terminals and the agent turns, and exposes them to the UI as Tauri
//! commands. Nothing here knows about boxd specifically; that lives in core.

mod secrets;

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use stowbox_core::agent::detached::{self, FollowEnd, FollowItem, TurnCursor};
use stowbox_core::agent::{self, AgentEvent, AgentMeta, AgentStatus, Block, Harness, TurnSpec};
use stowbox_core::boxd::{Boxd, BoxdConfig};
use stowbox_core::cost;
use stowbox_core::git::{self, GitReview, PrOutcome};
use stowbox_core::metrics::MachineMetrics;
use stowbox_core::notify::{Notice, Notifier};
use stowbox_core::provider::Provider;
use stowbox_core::provision::{self, Preset, ProvisionRun, RunEvent};
use stowbox_core::store::Store;
use stowbox_core::types::*;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::AbortHandle;

struct RunHandle {
    input: mpsc::Sender<ExecInput>,
    abort: AbortHandle,
}

struct AppState {
    provider: RwLock<Option<Arc<dyn Provider>>>,
    secrets: secrets::Secrets,
    secret_backend: RwLock<Option<secrets::Backend>>,
    store: Store,
    agents: RwLock<Vec<AgentMeta>>,
    /// Live followers of detached agent turns, by agent id.
    runs: Mutex<HashMap<String, AbortHandle>>,
    terminals: Mutex<HashMap<String, RunHandle>>,
    rates: RwLock<Option<(String, Rates)>>,
    presets: RwLock<Vec<Preset>>,
    preset_secrets: secrets::PresetSecrets,
    /// The latest setup run per box, finished or not.
    setup_runs: RwLock<Vec<ProvisionRun>>,
    /// Live setup runs by machine id.
    setups: Mutex<HashMap<String, AbortHandle>>,
    /// Decides which pushes carry a desktop notification.
    notifier: std::sync::Mutex<Notifier>,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl AppState {
    async fn provider(&self) -> CmdResult<Arc<dyn Provider>> {
        self.provider.read().await.clone().ok_or_else(|| stowbox_core::Error::NotConfigured.to_string())
    }

    async fn rates(&self, p: &Arc<dyn Provider>) -> (String, Rates) {
        if let Some(r) = self.rates.read().await.clone() {
            return r;
        }
        let fetched = p.rates().await.ok().filter(|(_, r)| r.vcpu_hour_micro > 0);
        let r = fetched.unwrap_or_else(|| ("eur".to_string(), cost::BOXD_PUBLISHED_EUR));
        *self.rates.write().await = Some(r.clone());
        r
    }

    async fn view(&self, p: &Arc<dyn Provider>, m: Machine) -> MachineView {
        let (currency, rates) = self.rates(p).await;
        MachineView { est_hourly_micro: cost::hourly_micro(&m, &rates), currency, machine: m }
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
struct ProviderStatus {
    configured: bool,
    provider: &'static str,
    secret_backend: Option<secrets::Backend>,
    console_url: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentPush {
    agent_id: String,
    event: Option<AgentEvent>,
    meta: AgentMeta,
    /// Set when this change is an outcome worth a desktop notification.
    notice: Option<Notice>,
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
}

fn yes() -> bool {
    true
}

// ---------------------------------------------------------------- provider

#[tauri::command]
async fn provider_status(state: State<'_, AppState>) -> CmdResult<ProviderStatus> {
    Ok(ProviderStatus {
        configured: state.provider.read().await.is_some(),
        provider: "boxd",
        secret_backend: *state.secret_backend.read().await,
        console_url: BoxdConfig::default().console_url,
    })
}

#[tauri::command]
async fn set_api_key(state: State<'_, AppState>, key: String) -> CmdResult<Account> {
    let key = key.trim().to_string();
    if !key.starts_with("bxd_") || key.len() < 20 {
        return Err("That does not look like a boxd API key. Keys start with bxd_ and come from app.boxd.sh or `boxd auth keys create`.".into());
    }
    let provider: Arc<dyn Provider> = Arc::new(Boxd::new(key.clone(), BoxdConfig::default()).map_err(err)?);
    let account = provider.whoami().await.map_err(err)?;
    let backend = state.secrets.save(&key).map_err(|e| format!("could not store the key: {e}"))?;
    *state.secret_backend.write().await = Some(backend);
    *state.provider.write().await = Some(provider);
    *state.rates.write().await = None;
    Ok(account)
}

#[tauri::command]
async fn clear_api_key(state: State<'_, AppState>) -> CmdResult<()> {
    state.secrets.clear();
    *state.secret_backend.write().await = None;
    *state.provider.write().await = None;
    *state.rates.write().await = None;
    Ok(())
}

#[tauri::command]
async fn whoami(state: State<'_, AppState>) -> CmdResult<Account> {
    state.provider().await?.whoami().await.map_err(err)
}

// ---------------------------------------------------------------- machines

#[tauri::command]
async fn list_machines(state: State<'_, AppState>) -> CmdResult<Vec<MachineView>> {
    let p = state.provider().await?;
    let machines = p.list_machines().await.map_err(err)?;
    let mut out = Vec::with_capacity(machines.len());
    for m in machines {
        out.push(state.view(&p, m).await);
    }
    out.sort_by(|a, b| a.machine.name.cmp(&b.machine.name));
    Ok(out)
}

#[tauri::command]
async fn get_machine(state: State<'_, AppState>, id: String) -> CmdResult<MachineView> {
    let p = state.provider().await?;
    let m = p.get_machine(&id).await.map_err(err)?;
    Ok(state.view(&p, m).await)
}

/// Creates the box, then, when a setup preset is picked, starts running it
/// in the background. A setup that cannot start (a missing secret value)
/// fails before the box is created, so nothing is billed for nothing.
#[tauri::command]
async fn create_machine(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: CreateSpec,
    preset_id: Option<String>,
) -> CmdResult<MachineView> {
    let p = state.provider().await?;
    let setup = match preset_id.filter(|id| !id.is_empty()) {
        Some(id) => Some(preset_with_secrets(&state, &id).await?),
        None => None,
    };
    let m = p.create_machine(spec).await.map_err(err)?;
    if let Some((preset, secrets)) = setup {
        start_setup(&app, &state, p.clone(), &m, preset, secrets).await?;
    }
    Ok(state.view(&p, m).await)
}

#[tauri::command]
async fn fork_machine(state: State<'_, AppState>, source: String, name: Option<String>) -> CmdResult<MachineView> {
    let p = state.provider().await?;
    let m = p.fork_machine(&source, name).await.map_err(err)?;
    Ok(state.view(&p, m).await)
}

#[tauri::command]
async fn machine_power(state: State<'_, AppState>, id: String, action: PowerAction) -> CmdResult<()> {
    let p = state.provider().await?;
    p.power(&id, action).await.map_err(err)?;
    if action == PowerAction::Destroy {
        if let Some(h) = state.setups.lock().await.remove(&id) {
            h.abort();
        }
        state.setup_runs.write().await.retain(|r| r.machine_id != id);
        state.persist_setup_runs().await?;
        // Agents that lived there are kept as history, but marked stopped.
        let mut agents = state.agents.write().await;
        let mut runs = state.runs.lock().await;
        for a in agents.iter_mut().filter(|a| a.machine_id == id) {
            if is_busy(&a.status) {
                a.status = AgentStatus::Stopped;
            }
            a.turn = None;
            if let Some(h) = runs.remove(&a.id) {
                h.abort();
            }
        }
        drop(runs);
        drop(agents);
        state.persist_agents().await?;
    }
    Ok(())
}

#[tauri::command]
async fn machine_usage(state: State<'_, AppState>, id: String) -> CmdResult<Usage> {
    state.provider().await?.machine_usage(&id).await.map_err(err)
}

/// One live sample from inside the box, or `None` when the box is not
/// running and was left alone. Called per box by the fleet view.
#[tauri::command]
async fn machine_metrics(state: State<'_, AppState>, id: String) -> CmdResult<Option<MachineMetrics>> {
    state.provider().await?.metrics(&id).await.map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RateCard {
    currency: String,
    rates: Rates,
}

#[tauri::command]
async fn rates(state: State<'_, AppState>) -> CmdResult<RateCard> {
    let p = state.provider().await?;
    let (currency, rates) = state.rates(&p).await;
    Ok(RateCard { currency, rates })
}

#[tauri::command]
async fn org_billing(state: State<'_, AppState>, org: String) -> CmdResult<OrgBilling> {
    state.provider().await?.org_billing(&org).await.map_err(err)
}

// ---------------------------------------------------------------- git review

// The repository that holds `cwd` in the box. Each call is one exec; the
// scripts and the parsing live in stowbox_core::git. `cwd` None is $HOME.

#[tauri::command]
async fn git_review(state: State<'_, AppState>, machine_id: String, cwd: Option<String>) -> CmdResult<GitReview> {
    let p = state.provider().await?;
    git::review(p.as_ref(), &machine_id, cwd.as_deref()).await.map_err(err)
}

#[tauri::command]
async fn git_commit(state: State<'_, AppState>, machine_id: String, cwd: Option<String>, message: String) -> CmdResult<String> {
    let p = state.provider().await?;
    git::commit(p.as_ref(), &machine_id, cwd.as_deref(), &message).await.map_err(err)
}

#[tauri::command]
async fn git_push(state: State<'_, AppState>, machine_id: String, cwd: Option<String>) -> CmdResult<String> {
    let p = state.provider().await?;
    git::push(p.as_ref(), &machine_id, cwd.as_deref()).await.map_err(err)
}

#[tauri::command]
async fn git_open_pr(state: State<'_, AppState>, machine_id: String, cwd: Option<String>) -> CmdResult<PrOutcome> {
    let p = state.provider().await?;
    git::open_pr(p.as_ref(), &machine_id, cwd.as_deref()).await.map_err(err)
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
    let now = now_ms();
    let meta = AgentMeta {
        id: uuid::Uuid::new_v4().to_string(),
        title: spec.title.filter(|t| !t.trim().is_empty()).unwrap_or_default(),
        harness: spec.harness,
        machine_id: spec.machine_id,
        machine_name: spec.machine_name,
        cwd: spec.cwd.filter(|c| !c.trim().is_empty()),
        model: spec.model.filter(|m| !m.trim().is_empty()),
        session_id: None,
        status: AgentStatus::Idle,
        skip_permissions: spec.skip_permissions,
        created_at: now,
        updated_at: now,
        total_cost_usd: 0.0,
        turns: 0,
        input_tokens: 0,
        output_tokens: 0,
        turn: None,
    };
    state.agents.write().await.push(meta.clone());
    state.persist_agents().await?;
    Ok(meta)
}

#[tauri::command]
async fn agent_events(state: State<'_, AppState>, id: String) -> CmdResult<Vec<AgentEvent>> {
    state.store.read_events(&id).await.map_err(err)
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

#[tauri::command]
async fn delete_agent(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(h) = state.runs.lock().await.remove(&id) {
        h.abort();
    }
    // A turn still running in the box is killed on the way out, best effort.
    let gone = state.agents.read().await.iter().find(|a| a.id == id).cloned();
    if let (Some(a), Ok(p)) = (gone, state.provider().await) {
        if let Some(turn) = a.turn {
            tauri::async_runtime::spawn(async move {
                let _ = detached::stop(p.as_ref(), &a.machine_id, &a.id, &turn.id).await;
            });
        }
    }
    state.agents.write().await.retain(|a| a.id != id);
    state.notifier.lock().unwrap_or_else(|e| e.into_inner()).forget_agent(&id);
    state.persist_agents().await?;
    state.store.delete_transcript(&id).await.map_err(err)
}

#[tauri::command]
async fn agent_stop(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let follower = state.runs.lock().await.remove(&id);
    if let Some(h) = &follower {
        h.abort();
    }
    let meta = state.agents.read().await.iter().find(|a| a.id == id).cloned().ok_or("agent not found")?;
    let Some(turn) = meta.turn.clone() else {
        if follower.is_none() {
            return Ok(());
        }
        let meta = update_meta(&state, &id, |m| m.status = AgentStatus::Stopped).await?;
        push(&app, &state, &id, Some(AgentEvent::Exit { code: -1 }), meta).await;
        return Ok(());
    };
    // The turn runs on its own in the box, so stopping it means killing it there.
    let killed = match state.provider().await {
        Ok(p) => detached::stop(p.as_ref(), &meta.machine_id, &id, &turn.id).await.map_err(err),
        Err(e) => Err(e),
    };
    let meta = update_meta(&state, &id, |m| {
        m.status = AgentStatus::Stopped;
        m.turn = None;
    })
    .await?;
    push(&app, &state, &id, Some(AgentEvent::Exit { code: -1 }), meta).await;
    killed.map_err(|e| format!("marked stopped, but the turn may still be running in the box: {e}"))
}

fn is_busy(s: &AgentStatus) -> bool {
    matches!(s, AgentStatus::Starting | AgentStatus::Thinking | AgentStatus::Tool { .. } | AgentStatus::Reconnecting { .. })
}

async fn update_meta(state: &AppState, id: &str, f: impl FnOnce(&mut AgentMeta)) -> CmdResult<AgentMeta> {
    let meta = {
        let mut agents = state.agents.write().await;
        let a = agents.iter_mut().find(|a| a.id == id).ok_or("agent not found")?;
        f(a);
        a.updated_at = now_ms();
        a.clone()
    };
    state.persist_agents().await?;
    Ok(meta)
}

/// Appends to the transcript (except live deltas) and tells the UI.
async fn push(app: &AppHandle, state: &AppState, id: &str, event: Option<AgentEvent>, meta: AgentMeta) {
    if let Some(ev) = &event {
        if !matches!(ev, AgentEvent::TextDelta { .. }) {
            let _ = state.store.append_event(id, ev).await;
        }
    }
    let notice = state.notifier.lock().unwrap_or_else(|e| e.into_inner()).agent(&meta, now_ms());
    let _ = app.emit("agent-event", AgentPush { agent_id: id.to_string(), event, meta, notice });
}

#[tauri::command]
async fn agent_send(app: AppHandle, state: State<'_, AppState>, id: String, text: String) -> CmdResult<()> {
    let text = text.trim_end().to_string();
    if text.is_empty() {
        return Err("write something for the agent first".into());
    }
    if state.runs.lock().await.contains_key(&id) {
        return Err("this agent is still working on the previous message".into());
    }
    let p = state.provider().await?;
    let meta = state.agents.read().await.iter().find(|a| a.id == id).cloned().ok_or("agent not found")?;

    let spec = TurnSpec {
        harness: Some(meta.harness),
        cwd: meta.cwd.clone(),
        model: meta.model.clone(),
        resume_session: meta.session_id.clone(),
        skip_permissions: meta.skip_permissions,
        max_turns: None,
        max_budget_usd: None,
    };
    let command = agent::turn_command(meta.harness, &spec);
    let turn_id = detached::new_turn_id(now_ms());

    let title_from_prompt = meta.title.is_empty();
    let meta = update_meta(&state, &id, |m| {
        if title_from_prompt {
            m.title = title_from(&text);
        }
        m.status = AgentStatus::Starting;
        m.turn = Some(TurnCursor::new(turn_id.clone()));
    })
    .await?;
    push(&app, &state, &id, Some(AgentEvent::UserMessage { text: text.clone() }), meta.clone()).await;

    // The turn runs detached in the box and writes to files there; a second
    // exec follows those files, so a dropped stream or a closed app does
    // not end the turn.
    let env = vec![("TERM".into(), "dumb".into()), ("NO_COLOR".into(), "1".into())];
    let prompt = format!("{text}\n");
    if let Err(e) = detached::launch(p.as_ref(), &meta.machine_id, &id, &turn_id, &command, &prompt, env).await {
        let msg = e.to_string();
        let meta = update_meta(&state, &id, |m| {
            m.status = AgentStatus::Failed { message: msg.clone() };
            m.turn = None;
        })
        .await?;
        push(&app, &state, &id, Some(AgentEvent::Stderr { text: msg.clone() }), meta).await;
        return Err(msg);
    }
    spawn_follower(&app, &state, id).await;
    Ok(())
}

/// Starts following the agent's turn in the background.
async fn spawn_follower(app: &AppHandle, state: &AppState, id: String) {
    // The lock is held across the spawn so the task's own removal at the end
    // cannot run before this insert.
    let mut runs = state.runs.lock().await;
    let task = tauri::async_runtime::spawn(follow_turn(app.clone(), id.clone()));
    if let Some(old) = runs.insert(id, task.inner().abort_handle()) {
        old.abort();
    }
}

/// Shows that the turn is not attached, unless a result already came in.
async fn set_reconnecting(app: &AppHandle, state: &AppState, id: &str, message: String) {
    let still_running = state.agents.read().await.iter().any(|a| a.id == id && is_busy(&a.status));
    if !still_running {
        return;
    }
    if let Ok(meta) = update_meta(state, id, |m| m.status = AgentStatus::Reconnecting { message }).await {
        push(app, state, id, None, meta).await;
    }
}

/// Ends a turn that will produce nothing more. Without an exit code the
/// failure itself goes into the transcript.
async fn end_turn(app: &AppHandle, state: &AppState, id: &str, code: Option<i32>, failure: Option<String>) {
    let meta = update_meta(state, id, |m| {
        if let Some(msg) = &failure {
            if !matches!(m.status, AgentStatus::Done | AgentStatus::Failed { .. } | AgentStatus::Stopped) {
                m.status = AgentStatus::Failed { message: msg.clone() };
            }
        }
        m.turn = None;
    })
    .await;
    let event = match (code, failure) {
        (Some(code), _) => Some(AgentEvent::Exit { code }),
        (None, Some(text)) => Some(AgentEvent::Stderr { text }),
        (None, None) => None,
    };
    if let Ok(meta) = meta {
        push(app, state, id, event, meta).await;
    }
}

/// Follows the agent's detached turn until it ends, following again from
/// the saved offsets whenever the stream drops or the box is not running.
async fn follow_turn(app: AppHandle, id: String) {
    let st = app.state::<AppState>();
    let Some(harness) = st.agents.read().await.iter().find(|a| a.id == id).map(|a| a.harness) else { return };
    let mut parser = agent::parser_for(harness);
    let mut saw_result = false;
    let mut attempt = 0u32;
    // What to show again once the stream is back.
    let mut resume: Option<AgentStatus> = None;
    loop {
        let Some(meta) = st.agents.read().await.iter().find(|a| a.id == id).cloned() else { break };
        let Some(cursor) = meta.turn.clone() else { break };
        if !matches!(meta.status, AgentStatus::Reconnecting { .. }) {
            resume = Some(meta.status.clone());
        }
        let wait = match st.provider().await {
            Err(e) => Some((e, detached::backoff_secs(attempt))),
            Ok(p) => match p.get_machine(&meta.machine_id).await {
                Err(e) => Some((format!("cannot reach the box: {e}"), detached::backoff_secs(attempt))),
                Ok(m) => match m.status {
                    MachineStatus::Running => match detached::follow(p.as_ref(), &meta.machine_id, &id, &cursor).await {
                        Err(e) => Some((format!("cannot follow the turn: {e}"), detached::backoff_secs(attempt))),
                        Ok(mut stream) => {
                            if matches!(meta.status, AgentStatus::Reconnecting { .. }) {
                                let back = resume.clone().unwrap_or(AgentStatus::Thinking);
                                if let Ok(meta) = update_meta(&st, &id, |m| {
                                    if matches!(m.status, AgentStatus::Reconnecting { .. }) {
                                        m.status = back;
                                    }
                                })
                                .await
                                {
                                    push(&app, &st, &id, None, meta).await;
                                }
                            }
                            let end = loop {
                                let item = stream.next().await;
                                let cursor = match &item {
                                    FollowItem::End(end) => break end.clone(),
                                    FollowItem::Out { cursor, .. } | FollowItem::Err { cursor, .. } => cursor.clone(),
                                };
                                attempt = 0;
                                let events = detached::line_events(parser.as_mut(), &item);
                                let last = events.len().saturating_sub(1);
                                for (i, ev) in events.into_iter().enumerate() {
                                    saw_result |= matches!(ev, AgentEvent::Result { .. });
                                    // The cursor moves with the line's last event, in the same write.
                                    apply_event(&app, &st, &id, ev, (i == last).then_some(&cursor)).await;
                                }
                            };
                            match end {
                                FollowEnd::Exited(code) => {
                                    let failure = (!saw_result).then(|| format!("the agent process exited with code {code} before reporting a result"));
                                    end_turn(&app, &st, &id, Some(code), failure).await;
                                    break;
                                }
                                FollowEnd::Lost => {
                                    let msg = "the turn's process ended without an exit code; the box may have restarted".to_string();
                                    end_turn(&app, &st, &id, None, Some(msg)).await;
                                    break;
                                }
                                FollowEnd::Missing => {
                                    end_turn(&app, &st, &id, None, Some("the turn's files are no longer in the box".into())).await;
                                    break;
                                }
                                FollowEnd::Dropped(e) => Some((format!("the stream dropped ({e})"), detached::backoff_secs(attempt))),
                            }
                        }
                    },
                    // The turn is frozen with the box; exec would wake it, so wait instead.
                    MachineStatus::Suspended | MachineStatus::Hibernated => {
                        Some((format!("the box is {}; the turn continues when it runs again", if m.status == MachineStatus::Suspended { "in standby" } else { "hibernated" }), 5))
                    }
                    MachineStatus::Stopped | MachineStatus::Failed => {
                        end_turn(&app, &st, &id, None, Some("the box stopped while the turn was running".into())).await;
                        break;
                    }
                    _ => Some(("waiting for the box to start".to_string(), 3)),
                },
            },
        };
        if let Some((why, secs)) = wait {
            set_reconnecting(&app, &st, &id, why).await;
            attempt += 1;
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        }
    }
    st.runs.lock().await.remove(&id);
}

fn title_from(prompt: &str) -> String {
    let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut t: String = first.chars().take(64).collect();
    if first.chars().count() > 64 {
        t.push('…');
    }
    t
}

/// Folds one parsed event into the agent's metadata, then pushes it.
async fn apply_event(app: &AppHandle, state: &AppState, id: &str, ev: AgentEvent, cursor: Option<&TurnCursor>) {
    let Ok(meta) = update_meta(state, id, |m| {
        if let Some(c) = cursor.filter(|c| m.turn.as_ref().is_some_and(|t| t.id == c.id)) {
            m.turn = Some(c.clone());
        }
        match &ev {
            AgentEvent::Init { session_id, model, .. } => {
                if !session_id.is_empty() {
                    m.session_id = Some(session_id.clone());
                }
                if m.model.is_none() {
                    m.model = model.clone();
                }
                m.status = AgentStatus::Thinking;
            }
            AgentEvent::Assistant { blocks } => {
                if let Some(Block::ToolUse { name, .. }) = blocks.iter().find(|b| matches!(b, Block::ToolUse { .. })) {
                    m.status = AgentStatus::Tool { name: name.clone() };
                } else {
                    m.status = AgentStatus::Thinking;
                }
            }
            AgentEvent::ToolResult { .. } => m.status = AgentStatus::Thinking,
            AgentEvent::TextDelta { .. } => {
                if matches!(m.status, AgentStatus::Starting | AgentStatus::Reconnecting { .. }) {
                    m.status = AgentStatus::Thinking;
                }
            }
            AgentEvent::Result { ok, subtype, message, cost_usd, turns, session_id, input_tokens, output_tokens, .. } => {
                if let Some(c) = cost_usd {
                    m.total_cost_usd += c;
                }
                m.turns += turns.unwrap_or(1);
                m.input_tokens += input_tokens.unwrap_or(0);
                m.output_tokens += output_tokens.unwrap_or(0);
                if let Some(s) = session_id.as_ref().filter(|s| !s.is_empty()) {
                    m.session_id = Some(s.clone());
                }
                m.status = if *ok {
                    AgentStatus::Done
                } else {
                    AgentStatus::Failed { message: message.clone().unwrap_or_else(|| subtype.replace('_', " ")) }
                };
            }
            AgentEvent::UserMessage { .. } | AgentEvent::Stderr { .. } | AgentEvent::Exit { .. } => {}
        }
    })
    .await
    else {
        return;
    };
    push(app, state, id, Some(ev), meta).await;
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
        secrets: p
            .secret_names
            .iter()
            .map(|n| SecretView { name: n.clone(), has_value: values.get(n).is_some_and(|v| !v.is_empty()) })
            .collect(),
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
    let commands: Vec<String> =
        input.commands.iter().map(|c| c.trim_end().to_string()).filter(|c| !c.trim().is_empty()).collect();
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
    for (s, name) in input.secrets.iter().zip(&secret_names) {
        match s.value.as_ref().filter(|v| !v.is_empty()) {
            Some(v) => {
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

    let preset = Preset {
        id: id.clone(),
        name,
        commands,
        secret_names,
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
    let all = {
        let mut presets = state.presets.write().await;
        presets.retain(|p| p.id != id);
        presets.clone()
    };
    state.store.save_presets(&all).await.map_err(err)?;
    state.preset_secrets.delete(&id);
    Ok(())
}

/// The preset and its secret values in the preset's order, or an error
/// naming the first secret that has no value.
async fn preset_with_secrets(state: &AppState, id: &str) -> CmdResult<(Preset, Vec<(String, String)>)> {
    let preset = state.presets.read().await.iter().find(|p| p.id == id).cloned().ok_or("setup preset not found")?;
    let values = state.preset_secrets.load(id);
    let mut secrets = Vec::with_capacity(preset.secret_names.len());
    for n in &preset.secret_names {
        match values.get(n).filter(|v| !v.is_empty()) {
            Some(v) => secrets.push((n.clone(), v.clone())),
            None => return Err(format!("the secret {n} in \"{}\" has no value; edit the preset first", preset.name)),
        }
    }
    Ok((preset, secrets))
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
        state.setups.lock().await.remove(&run.machine_id);
        let _ = state.persist_setup_runs().await;
    }
}

async fn start_setup(
    app: &AppHandle,
    state: &AppState,
    p: Arc<dyn Provider>,
    machine: &Machine,
    preset: Preset,
    secrets: Vec<(String, String)>,
) -> CmdResult<ProvisionRun> {
    let mut setups = state.setups.lock().await;
    if setups.contains_key(&machine.id) {
        return Err(format!("a setup is already running in {}", machine.name));
    }
    let run = ProvisionRun::new(uuid::Uuid::new_v4().to_string(), machine, &preset, now_ms());
    {
        let mut runs = state.setup_runs.write().await;
        runs.retain(|r| r.machine_id != machine.id);
        runs.push(run.clone());
    }
    let _ = app.emit("setup-event", SetupPush { run: run.clone(), append: String::new(), notice: None });

    let (tx, mut rx) = mpsc::unbounded_channel::<RunEvent>();
    let machine_id = machine.id.clone();
    let commands = preset.commands.clone();
    let runner = tokio::spawn(async move {
        provision::run(&*p, &machine_id, &commands, &secrets, |ev| {
            let _ = tx.send(ev);
        })
        .await;
    });
    setups.insert(machine.id.clone(), runner.abort_handle());
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
async fn setup_start(app: AppHandle, state: State<'_, AppState>, machine_id: String, preset_id: String) -> CmdResult<ProvisionRun> {
    let p = state.provider().await?;
    let (preset, secrets) = preset_with_secrets(&state, &preset_id).await?;
    let m = p.get_machine(&machine_id).await.map_err(err)?;
    start_setup(&app, &state, p, &m, preset, secrets).await
}

#[tauri::command]
async fn setup_cancel(app: AppHandle, state: State<'_, AppState>, machine_id: String) -> CmdResult<()> {
    let Some(h) = state.setups.lock().await.remove(&machine_id) else { return Ok(()) };
    // Mark it first, so the runner's own "stopped" report after the abort
    // finds the run already over and is dropped.
    let run_id = state.setup_runs.read().await.iter().find(|r| r.machine_id == machine_id).map(|r| r.id.clone());
    if let Some(id) = run_id {
        apply_setup_event(&app, &state, &id, RunEvent::Cancelled).await;
    }
    // Aborting drops the exec stream, which kills the remote process group.
    h.abort();
    Ok(())
}

#[tauri::command]
async fn setup_runs(state: State<'_, AppState>) -> CmdResult<Vec<ProvisionRun>> {
    Ok(state.setup_runs.read().await.clone())
}

// ---------------------------------------------------------------- terminals

#[tauri::command]
async fn terminal_open(
    state: State<'_, AppState>,
    machine: String,
    cols: u16,
    rows: u16,
    on_data: Channel<TermEvent>,
) -> CmdResult<String> {
    let p = state.provider().await?;
    let handle = p
        .exec(
            &machine,
            ExecSpec {
                script: "exec \"${SHELL:-bash}\" -l".into(),
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
    state.terminals.lock().await.insert(id.clone(), RunHandle { input, abort: pump.abort_handle() });
    Ok(id)
}

#[tauri::command]
async fn terminal_write(state: State<'_, AppState>, id: String, data: String) -> CmdResult<()> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(err)?;
    let terms = state.terminals.lock().await;
    let t = terms.get(&id).ok_or("terminal is closed")?;
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
    }
    Ok(())
}

// ---------------------------------------------------------------- app

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            let secrets = secrets::Secrets::new(data.join("secrets"));
            let store = Store::new(data.join("state"));
            let preset_secrets = secrets::PresetSecrets::new(data.join("secrets"));

            let (provider, backend) = match secrets.load() {
                Some((key, backend)) => match Boxd::new(key, BoxdConfig::default()) {
                    Ok(p) => (Some(Arc::new(p) as Arc<dyn Provider>), Some(backend)),
                    Err(e) => {
                        eprintln!("stowbox: could not initialise the boxd client: {e}");
                        (None, Some(backend))
                    }
                },
                None => (None, None),
            };
            let agents = tauri::async_runtime::block_on(store.load_agents()).unwrap_or_else(|e| {
                eprintln!("stowbox: could not read agents.json: {e}");
                Vec::new()
            });
            // Turns run detached in the box, so one that was running when the
            // app closed may still be; follow it again from the saved offsets.
            // A busy agent without a turn record cannot be found again.
            let agents: Vec<AgentMeta> = agents
                .into_iter()
                .map(|mut a| {
                    if is_busy(&a.status) && a.turn.is_none() {
                        a.status = AgentStatus::Stopped;
                    }
                    a
                })
                .collect();
            let notifier = Notifier::new(&agents, now_ms());
            let reattach: Vec<String> = agents.iter().filter(|a| a.turn.is_some()).map(|a| a.id.clone()).collect();

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
                provider: RwLock::new(provider),
                secrets,
                secret_backend: RwLock::new(backend),
                store,
                agents: RwLock::new(agents),
                runs: Mutex::new(HashMap::new()),
                terminals: Mutex::new(HashMap::new()),
                rates: RwLock::new(None),
                presets: RwLock::new(presets),
                preset_secrets,
                setup_runs: RwLock::new(setup_runs),
                setups: Mutex::new(HashMap::new()),
                notifier: std::sync::Mutex::new(notifier),
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let st = handle.state::<AppState>();
                for id in reattach {
                    spawn_follower(&handle, &st, id).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            provider_status,
            set_api_key,
            clear_api_key,
            whoami,
            list_machines,
            get_machine,
            create_machine,
            fork_machine,
            machine_power,
            machine_usage,
            machine_metrics,
            org_billing,
            rates,
            list_agents,
            create_agent,
            agent_events,
            rename_agent,
            delete_agent,
            agent_stop,
            agent_send,
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
            terminal_open,
            terminal_write,
            terminal_resize,
            terminal_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running stowbox");
}
