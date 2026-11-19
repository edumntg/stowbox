//! How an agent runs in its box: the harness's interactive interface, in a
//! terminal, on the agent's own session.
//!
//! When the box has tmux, the harness runs in a tmux session of its own on a
//! private socket (`tmux -L stowbox`), so closing the terminal, the window or
//! the network only detaches: the agent keeps working, and the next open
//! attaches to it again. Without tmux the harness lives as long as the
//! terminal does, and the next open resumes the session.
//!
//! Claude Code reports what it is doing through hooks passed with
//! `--settings`: a prompt or a tool call writes `working`, the end of a turn
//! or a notification (a permission prompt, a long wait) writes `waiting`, to
//! `~/.stowbox/agents/<id>/state`. A watch exec reads those files, and the
//! tmux sessions still alive, while the app has a terminal open in the box.

use super::{account, AgentMeta, Harness};
use crate::boxd::shell_quote;
use crate::git::capture;
use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Per-agent state files live under `$HOME/<STATE_DIR>/<agent id>/`.
pub const STATE_DIR: &str = ".stowbox/agents";

/// The private tmux socket, so Stowbox's sessions never mix with the person's own.
pub const TMUX_SOCKET: &str = "stowbox";

const TMUX_CONF: &[&str] = &[
    "set -g status off",
    "set -g mouse on",
    "set -g history-limit 50000",
    "set -s escape-time 0",
    "set -g default-terminal xterm-256color",
    "set -ga terminal-overrides ,xterm-256color:Tc",
    // Pass hyperlinks (OSC 8) through to Stowbox's terminal, which tmux
    // drops unless told the outer terminal takes them (tmux 3.4 and later).
    "%if #{>=:#{version},3.4}",
    HYPERLINKS,
    "%endif",
];

/// tmux's setting for hyperlinks, at a fixed index so setting it again on a
/// running server does not add it twice.
const HYPERLINKS: &str = "set -s 'terminal-features[90]' 'xterm*:hyperlinks'";

pub const DEADLINE: Duration = Duration::from_secs(20);

/// The agent's tmux session name.
pub fn tmux_name(agent_id: &str) -> String {
    let id: String = agent_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    format!("agent-{id}")
}

fn state_dir(agent_id: &str) -> String {
    format!("\"$HOME\"/{}", shell_quote(&format!("{STATE_DIR}/{agent_id}")))
}

/// What Claude Code is doing, as its hooks last wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentState {
    /// A prompt is being answered.
    Working,
    /// The turn ended, or the agent asks for something: the person's move.
    Waiting,
}

/// One agent as the box shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLive {
    pub id: String,
    pub state: Option<AgentState>,
    /// Unix seconds the state was written.
    pub since: i64,
    /// Its tmux session exists, so the harness is running.
    pub running: bool,
}

/// Everything one watch round saw in a box.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxAgents {
    /// The box has tmux, so agents outlive their terminal.
    pub tmux: bool,
    pub agents: Vec<AgentLive>,
}

fn hook(agent_id: &str, state: &str) -> serde_json::Value {
    let d = state_dir(agent_id);
    let command = format!("mkdir -p {d} && printf '%s %s\\n' {state} \"$(date +%s)\" > {d}/state.tmp && mv -f {d}/state.tmp {d}/state");
    serde_json::json!([{ "matcher": "*", "hooks": [{ "type": "command", "command": command }] }])
}

/// `--settings` JSON with the hooks that report the agent's state.
pub fn hooks_settings(agent_id: &str) -> String {
    serde_json::json!({
        "hooks": {
            "SessionStart": hook(agent_id, "waiting"),
            "UserPromptSubmit": hook(agent_id, "working"),
            "PostToolUse": hook(agent_id, "working"),
            "Notification": hook(agent_id, "waiting"),
            "Stop": hook(agent_id, "waiting"),
        }
    })
    .to_string()
}

/// The harness command for `a`, run from its directory. `input` is the
/// first message, for a new session.
pub fn harness_command(a: &AgentMeta, input: Option<&str>) -> String {
    let mut s = String::new();
    let input = input.map(str::trim).filter(|i| !i.is_empty());
    if a.harness == Harness::ClaudeCode {
        s.push_str(&account::prelude());
        // Addresses come as hyperlinks (OSC 8) carrying the whole URL, so
        // one that Claude Code breaks over rows still opens and copies whole.
        s.push_str("\nexport FORCE_HYPERLINK=1\n");
    }
    if let Some(cwd) = a.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
        s.push_str(&format!("cd {} || exit 1\n", shell_quote(cwd)));
    }
    let model = a.model.as_deref().filter(|m| !m.trim().is_empty());
    let session = a.session_id.as_deref().filter(|s| !s.trim().is_empty());
    match a.harness {
        Harness::ClaudeCode => {
            match session {
                // A session that was never started has no transcript yet:
                // create it under this id, then resume it from then on.
                Some(sid) => s.push_str(&format!(
                    "sid={}\nif ls \"${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}\"/projects/*/\"$sid\".jsonl >/dev/null 2>&1; then set -- --resume \"$sid\"; else set -- --session-id \"$sid\"; fi\n",
                    shell_quote(sid)
                )),
                None => s.push_str("set --\n"),
            }
            s.push_str("exec claude \"$@\"");
            if let Some(m) = model {
                s.push_str(&format!(" --model {}", shell_quote(m)));
            }
            if a.skip_permissions {
                s.push_str(" --dangerously-skip-permissions");
            }
            s.push_str(&format!(" --settings {}", shell_quote(&hooks_settings(&a.id))));
            if let Some(i) = input {
                s.push_str(&format!(" {}", shell_quote(i)));
            }
        }
        Harness::Codex => {
            s.push_str("exec codex");
            if let Some(sid) = session {
                s.push_str(&format!(" resume {}", shell_quote(sid)));
            }
            if let Some(m) = model {
                s.push_str(&format!(" --model {}", shell_quote(m)));
            }
            if a.skip_permissions {
                s.push_str(" --dangerously-bypass-approvals-and-sandbox");
            }
            if let (Some(i), None) = (input, session) {
                s.push_str(&format!(" {}", shell_quote(i)));
            }
        }
    }
    s.push('\n');
    s
}

/// The terminal's script: attach to the agent's tmux session, starting it
/// with the harness if it is not running; without tmux, run the harness.
pub fn open_script(a: &AgentMeta, input: Option<&str>) -> String {
    let inner = format!("bash -lc {}", shell_quote(&harness_command(a, input)));
    let mut s = String::from("if command -v tmux >/dev/null 2>&1; then\n");
    s.push_str("  mkdir -p \"$HOME/.stowbox\"\n");
    s.push_str(&format!("  printf '%s\\n' {} > \"$HOME/.stowbox/tmux.conf\"\n", TMUX_CONF.iter().map(|l| shell_quote(l)).collect::<Vec<_>>().join(" ")));
    // The config is read when the server starts; a server already running
    // (older agents still open) gets the hyperlink setting here. Fails
    // quietly on a tmux too old for it, or when no server runs yet.
    s.push_str(&format!("  tmux -L {TMUX_SOCKET} {HYPERLINKS} 2>/dev/null\n"));
    s.push_str(&format!("  exec tmux -L {TMUX_SOCKET} -f \"$HOME/.stowbox/tmux.conf\" new-session -A -s {} {}\n", tmux_name(&a.id), shell_quote(&inner)));
    s.push_str("fi\n");
    s.push_str(&format!("exec {inner}\n"));
    s
}

/// Ends the agent's tmux session, if it runs. `forget` also removes its
/// state files.
pub fn stop_script(agent_id: &str, forget: bool) -> String {
    let mut s = format!("command -v tmux >/dev/null 2>&1 && tmux -L {TMUX_SOCKET} kill-session -t ={} 2>/dev/null\n", tmux_name(agent_id));
    if forget {
        s.push_str(&format!("rm -rf -- {}\n", state_dir(agent_id)));
    }
    s.push_str("true\n");
    s
}

fn marker(nonce: &str) -> String {
    format!("::stowbox-{nonce}::")
}

/// Prints one snapshot (tmux presence, states, running sessions), ended by
/// the marker, whenever it changes; checks every second until killed.
pub fn watch_script(nonce: &str) -> String {
    let m = marker(nonce);
    let mut s = format!("d=\"$HOME/{STATE_DIR}\"\nlast=x\n");
    s.push_str("while :; do\n");
    s.push_str("  cur=$(\n");
    s.push_str("    if command -v tmux >/dev/null 2>&1; then echo tmux; tmux -L stowbox ls -F 'running #S' 2>/dev/null; fi\n");
    s.push_str(
        "    for f in \"$d\"/*/state; do [ -f \"$f\" ] && printf 'state %s %s\\n' \"$(basename \"$(dirname \"$f\")\")\" \"$(cat \"$f\")\"; done 2>/dev/null\n",
    );
    s.push_str("  )\n");
    s.push_str(&format!("  if [ \"$cur\" != \"$last\" ]; then printf '%s\\n{m} end\\n' \"$cur\"; last=$cur; fi\n"));
    s.push_str("  sleep 1\n");
    s.push_str("done\n");
    s
}

/// One snapshot's lines, without the end marker.
pub fn parse_snapshot(text: &str) -> BoxAgents {
    let mut b = BoxAgents::default();
    let mut agents: BTreeMap<String, AgentLive> = BTreeMap::new();
    let entry = |agents: &mut BTreeMap<String, AgentLive>, id: &str| -> AgentLive {
        agents.remove(id).unwrap_or(AgentLive { id: id.to_string(), state: None, since: 0, running: false })
    };
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("tmux") => b.tmux = true,
            Some("running") => {
                if let Some(id) = w.next().and_then(|n| n.strip_prefix("agent-")) {
                    let mut a = entry(&mut agents, id);
                    a.running = true;
                    agents.insert(id.to_string(), a);
                }
            }
            Some("state") => {
                let (Some(id), Some(state)) = (w.next(), w.next()) else { continue };
                let mut a = entry(&mut agents, id);
                a.state = match state {
                    "working" => Some(AgentState::Working),
                    "waiting" => Some(AgentState::Waiting),
                    _ => None,
                };
                a.since = w.next().and_then(|t| t.parse().ok()).unwrap_or(0);
                agents.insert(id.to_string(), a);
            }
            _ => {}
        }
    }
    b.agents = agents.into_values().collect();
    b
}

/// A running watch: yields a snapshot each time the box's agents change.
pub struct Watch {
    handle: ExecHandle,
    end: String,
    buf: String,
}

impl Watch {
    /// The next snapshot; `None` once the exec stream ends.
    pub async fn next(&mut self) -> Option<BoxAgents> {
        loop {
            if let Some(i) = self.buf.find(&self.end) {
                let snap = parse_snapshot(&self.buf[..i]);
                self.buf.drain(..i + self.end.len());
                return Some(snap);
            }
            match self.handle.output.recv().await? {
                ExecOutput::Stdout(b) => self.buf.push_str(&String::from_utf8_lossy(&b)),
                ExecOutput::Stderr(_) => {}
                ExecOutput::Exit(_) | ExecOutput::Error(_) => return None,
            }
        }
    }
}

pub async fn watch<P: Provider + ?Sized>(p: &P, machine_id: &str) -> Result<Watch> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let handle = p.exec(machine_id, ExecSpec { script: watch_script(&nonce), tty: false, cols: 0, rows: 0, env: vec![] }).await?;
    Ok(Watch { handle, end: format!("{} end\n", marker(&nonce)), buf: String::new() })
}

/// Ends the agent's harness in the box; `forget` also drops its state.
pub async fn stop<P: Provider + ?Sized>(p: &P, machine_id: &str, agent_id: &str, forget: bool) -> Result<()> {
    let c = capture(p, machine_id, stop_script(agent_id, forget), None, DEADLINE, "stopping the agent").await?;
    if c.code != 0 {
        return Err(Error::Provider(format!("stopping the agent exited with code {}", c.code)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{has_bash, temp_dir, LocalBash};

    fn meta(harness: Harness, session: Option<&str>) -> AgentMeta {
        AgentMeta {
            id: "a1".into(),
            title: String::new(),
            harness,
            machine_id: "vm".into(),
            machine_name: "box".into(),
            cwd: Some("/w d".into()),
            model: Some("opus".into()),
            session_id: session.map(Into::into),
            skip_permissions: true,
            created_at: 0,
            updated_at: 0,
            worktree: None,
        }
    }

    #[test]
    fn claude_starts_or_resumes_its_own_session() {
        let c = harness_command(&meta(Harness::ClaudeCode, Some("1b4e28ba-2fa1-11d2-883f-0016d3cca427")), Some("Add a test"));
        assert!(c.starts_with(&account::prelude()));
        assert!(c.contains("cd '/w d' || exit 1\n"));
        assert!(c.contains("set -- --resume \"$sid\"; else set -- --session-id \"$sid\""));
        assert!(c.contains("exec claude \"$@\" --model opus --dangerously-skip-permissions --settings '{\"hooks\""));
        assert!(c.trim_end().ends_with(" 'Add a test'"), "{c}");
    }

    #[test]
    fn claude_sends_whole_addresses_as_hyperlinks_through_tmux() {
        let a = meta(Harness::ClaudeCode, Some("s1"));
        assert!(harness_command(&a, None).contains("\nexport FORCE_HYPERLINK=1\n"));
        assert!(!harness_command(&meta(Harness::Codex, None), None).contains("FORCE_HYPERLINK"));
        let open = open_script(&a, None);
        assert!(open.contains(&format!("  tmux -L stowbox {HYPERLINKS} 2>/dev/null\n")), "{open}");
        assert!(open.contains(&shell_quote(HYPERLINKS)), "{open}");
    }

    /// The config loads without errors and turns hyperlinks on where tmux
    /// has them; only where tmux is installed.
    #[test]
    fn tmux_takes_the_config() {
        let tmux = |args: &[&str]| std::process::Command::new("tmux").args(args).env_remove("TMUX").output();
        let Ok(v) = tmux(&["-V"]) else { return };
        let version = String::from_utf8_lossy(&v.stdout).trim().trim_start_matches("tmux ").to_string();
        let dir = std::env::temp_dir().join(format!("stowbox-tmux-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("tmux.conf");
        std::fs::write(&conf, TMUX_CONF.join("\n") + "\n").unwrap();
        let sock = format!("stowbox-test-{}", uuid::Uuid::new_v4().simple());
        let conf = conf.to_str().unwrap();
        let started = tmux(&["-L", &sock, "-f", conf, "new-session", "-d", "-s", "t", "sleep 5"]).unwrap();
        let features = tmux(&["-L", &sock, "show", "-s", "terminal-features"]).unwrap();
        let _ = tmux(&["-L", &sock, "kill-server"]);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(started.status.success(), "{}", String::from_utf8_lossy(&started.stderr));
        let features = String::from_utf8_lossy(&features.stdout);
        if version.as_str() >= "3.4" {
            assert!(features.contains("xterm*:hyperlinks"), "tmux {version}: {features}");
        }
    }

    #[test]
    fn codex_resumes_by_id_and_takes_a_prompt_only_when_new() {
        let c = harness_command(&meta(Harness::Codex, Some("t1")), Some("hi"));
        assert!(c.ends_with("exec codex resume t1 --model opus --dangerously-bypass-approvals-and-sandbox\n"), "{c}");
        let c = harness_command(&meta(Harness::Codex, None), Some("hi"));
        assert!(c.ends_with(" hi\n"), "{c}");
    }

    #[test]
    fn hooks_are_valid_settings() {
        let v: serde_json::Value = serde_json::from_str(&hooks_settings("a1")).unwrap();
        for event in ["SessionStart", "UserPromptSubmit", "PostToolUse", "Notification", "Stop"] {
            let cmd = v.pointer(&format!("/hooks/{event}/0/hooks/0/command")).and_then(|c| c.as_str()).unwrap();
            assert!(cmd.contains(".stowbox/agents/a1"), "{event}: {cmd}");
        }
    }

    #[test]
    fn snapshots_merge_states_and_sessions() {
        let b = parse_snapshot("tmux\nrunning agent-a1\nrunning other\nstate a1 working 1700000000\nstate a2 waiting 1700000100\nstate a3 weird 5\n");
        assert!(b.tmux);
        assert_eq!(
            b.agents,
            vec![
                AgentLive { id: "a1".into(), state: Some(AgentState::Working), since: 1_700_000_000, running: true },
                AgentLive { id: "a2".into(), state: Some(AgentState::Waiting), since: 1_700_000_100, running: false },
                AgentLive { id: "a3".into(), state: None, since: 5, running: false },
            ]
        );
        assert_eq!(parse_snapshot(""), BoxAgents::default());
    }

    #[cfg_attr(not(unix), ignore = "box-side script, needs a Linux box's bash; see testing.rs")]
    #[tokio::test]
    async fn hooks_write_the_state_the_watch_reads() {
        if !has_bash() {
            return;
        }
        let home = temp_dir("stowbox-session");
        let p = LocalBash { home: home.clone(), env: vec![] };
        let v: serde_json::Value = serde_json::from_str(&hooks_settings("a1")).unwrap();
        let stop = v.pointer("/hooks/Stop/0/hooks/0/command").and_then(|c| c.as_str()).unwrap();
        capture(&p, "vm", stop.to_string(), None, DEADLINE, "hook").await.unwrap();
        assert!(std::fs::read_to_string(home.join(".stowbox/agents/a1/state")).unwrap().starts_with("waiting "));

        let mut w = watch(&p, "vm").await.unwrap();
        let snap = tokio::time::timeout(Duration::from_secs(10), w.next()).await.unwrap().unwrap();
        let a = snap.agents.iter().find(|a| a.id == "a1").unwrap();
        assert_eq!(a.state, Some(AgentState::Waiting));

        let work = v.pointer("/hooks/UserPromptSubmit/0/hooks/0/command").and_then(|c| c.as_str()).unwrap();
        capture(&p, "vm", work.to_string(), None, DEADLINE, "hook").await.unwrap();
        let snap = tokio::time::timeout(Duration::from_secs(10), w.next()).await.unwrap().unwrap();
        assert_eq!(snap.agents.iter().find(|a| a.id == "a1").unwrap().state, Some(AgentState::Working));

        stop_script("a1", true);
        capture(&p, "vm", stop_script("a1", true), None, DEADLINE, "stop").await.unwrap();
        assert!(!home.join(".stowbox/agents/a1").exists());
    }
}
