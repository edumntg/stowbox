//! The desktop shell: owns the provider connection, the on-disk store, the
//! live terminals and the agent turns, and exposes them to the UI as Tauri
//! commands. Nothing here knows about boxd specifically; that lives in core.

mod secrets;

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use stowbox_core::agent::{self, AgentEvent, AgentMeta, AgentStatus, Block, Harness, LineBuffer, TurnSpec};
use stowbox_core::boxd::{Boxd, BoxdConfig};
use stowbox_core::cost;
use stowbox_core::provider::Provider;
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
    runs: Mutex<HashMap<String, RunHandle>>,
    terminals: Mutex<HashMap<String, RunHandle>>,
    rates: RwLock<Option<(String, Rates)>>,
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

#[tauri::command]
async fn create_machine(state: State<'_, AppState>, spec: CreateSpec) -> CmdResult<MachineView> {
    let p = state.provider().await?;
    let m = p.create_machine(spec).await.map_err(err)?;
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
        // Agents that lived there are kept as history, but marked stopped.
        let mut agents = state.agents.write().await;
        for a in agents.iter_mut().filter(|a| a.machine_id == id) {
            if matches!(a.status, AgentStatus::Starting | AgentStatus::Thinking | AgentStatus::Tool { .. }) {
                a.status = AgentStatus::Stopped;
            }
        }
        drop(agents);
        state.persist_agents().await?;
    }
    Ok(())
}

#[tauri::command]
async fn machine_usage(state: State<'_, AppState>, id: String) -> CmdResult<Usage> {
    state.provider().await?.machine_usage(&id).await.map_err(err)
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
    if let Some(run) = state.runs.lock().await.remove(&id) {
        run.abort.abort();
    }
    state.agents.write().await.retain(|a| a.id != id);
    state.persist_agents().await?;
    state.store.delete_transcript(&id).await.map_err(err)
}

#[tauri::command]
async fn agent_stop(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let Some(run) = state.runs.lock().await.remove(&id) else { return Ok(()) };
    run.abort.abort();
    drop(run.input);
    let meta = update_meta(&state, &id, |m| m.status = AgentStatus::Stopped).await?;
    push(&app, &state, &id, Some(AgentEvent::Exit { code: -1 }), meta).await;
    Ok(())
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
    let _ = app.emit("agent-event", AgentPush { agent_id: id.to_string(), event, meta });
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
    let script = agent::turn_command(meta.harness, &spec);

    let title_from_prompt = meta.title.is_empty();
    let meta = update_meta(&state, &id, |m| {
        if title_from_prompt {
            m.title = title_from(&text);
        }
        m.status = AgentStatus::Starting;
    })
    .await?;
    push(&app, &state, &id, Some(AgentEvent::UserMessage { text: text.clone() }), meta.clone()).await;

    let handle = match p
        .exec(
            &meta_machine(&state, &id).await?,
            ExecSpec {
                script,
                tty: false,
                cols: 0,
                rows: 0,
                env: vec![("TERM".into(), "dumb".into()), ("NO_COLOR".into(), "1".into())],
            },
        )
        .await
    {
        Ok(h) => h,
        Err(e) => {
            let msg = e.to_string();
            let meta = update_meta(&state, &id, |m| m.status = AgentStatus::Failed { message: msg.clone() }).await?;
            push(&app, &state, &id, Some(AgentEvent::Stderr { text: msg.clone() }), meta).await;
            return Err(msg);
        }
    };

    let ExecHandle { input, mut output } = handle;
    let prompt = format!("{text}\n");
    input.send(ExecInput::Data(prompt.into_bytes())).await.map_err(|_| "could not send the prompt")?;
    input.send(ExecInput::Close).await.map_err(|_| "could not close stdin")?;

    let app2 = app.clone();
    let agent_id = id.clone();
    let harness = meta.harness;
    let reader = tokio::spawn(async move {
        let st = app2.state::<AppState>();
        let mut parser = agent::parser_for(harness);
        let mut out_buf = LineBuffer::default();
        let mut err_buf = LineBuffer::default();
        let mut saw_result = false;

        while let Some(chunk) = output.recv().await {
            let (lines, is_err) = match chunk {
                ExecOutput::Stdout(b) => (out_buf.push(&b), false),
                ExecOutput::Stderr(b) => (err_buf.push(&b), true),
                ExecOutput::Exit(code) => {
                    let tail: Vec<AgentEvent> = out_buf
                        .flush()
                        .map(|l| parser.parse_line(&l))
                        .unwrap_or_default()
                        .into_iter()
                        .chain(err_buf.flush().map(|t| AgentEvent::Stderr { text: t }))
                        .collect();
                    for ev in tail {
                        saw_result |= matches!(ev, AgentEvent::Result { .. });
                        apply_event(&app2, &st, &agent_id, ev).await;
                    }
                    if !saw_result {
                        let msg = format!("the agent process exited with code {code} before reporting a result");
                        let _ = update_meta(&st, &agent_id, |m| {
                            if !matches!(m.status, AgentStatus::Done | AgentStatus::Failed { .. } | AgentStatus::Stopped) {
                                m.status = AgentStatus::Failed { message: msg.clone() };
                            }
                        })
                        .await;
                    }
                    if let Ok(meta) = update_meta(&st, &agent_id, |_| {}).await {
                        push(&app2, &st, &agent_id, Some(AgentEvent::Exit { code }), meta).await;
                    }
                    break;
                }
                ExecOutput::Error(message) => {
                    if let Ok(meta) =
                        update_meta(&st, &agent_id, |m| m.status = AgentStatus::Failed { message: message.clone() }).await
                    {
                        push(&app2, &st, &agent_id, Some(AgentEvent::Stderr { text: message }), meta).await;
                    }
                    break;
                }
            };
            for line in lines {
                let events = if is_err {
                    if line.trim().is_empty() {
                        vec![]
                    } else {
                        vec![AgentEvent::Stderr { text: line }]
                    }
                } else {
                    parser.parse_line(&line)
                };
                for ev in events {
                    saw_result |= matches!(ev, AgentEvent::Result { .. });
                    apply_event(&app2, &st, &agent_id, ev).await;
                }
            }
        }
        st.runs.lock().await.remove(&agent_id);
    });

    state.runs.lock().await.insert(id, RunHandle { input, abort: reader.abort_handle() });
    Ok(())
}

async fn meta_machine(state: &AppState, id: &str) -> CmdResult<String> {
    state
        .agents
        .read()
        .await
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.machine_id.clone())
        .ok_or_else(|| "agent not found".to_string())
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
async fn apply_event(app: &AppHandle, state: &AppState, id: &str, ev: AgentEvent) {
    let Ok(meta) = update_meta(state, id, |m| match &ev {
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
            if matches!(m.status, AgentStatus::Starting) {
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
    })
    .await
    else {
        return;
    };
    push(app, state, id, Some(ev), meta).await;
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
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            let secrets = secrets::Secrets::new(data.join("secrets"));
            let store = Store::new(data.join("state"));

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
            // Nothing is running right after launch; turns do not survive restarts.
            let agents = agents
                .into_iter()
                .map(|mut a| {
                    if matches!(a.status, AgentStatus::Starting | AgentStatus::Thinking | AgentStatus::Tool { .. }) {
                        a.status = AgentStatus::Stopped;
                    }
                    a
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
            org_billing,
            rates,
            list_agents,
            create_agent,
            agent_events,
            rename_agent,
            delete_agent,
            agent_stop,
            agent_send,
            terminal_open,
            terminal_write,
            terminal_resize,
            terminal_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running stowbox");
}
