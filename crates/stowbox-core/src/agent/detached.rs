//! Turns that keep running when nobody is watching.
//!
//! A turn is launched *detached* inside the box: its own session (`setsid`),
//! stdio redirected to files, so closing the exec stream (a laptop going to
//! sleep, a dropped network, Stowbox quitting) does not touch it. Each turn
//! gets a directory, `~/.stowbox/turns/<agent>/<turn>/`, holding:
//!
//! - `prompt`: the user's message, written from the exec's stdin, never on a
//!   command line;
//! - `out.jsonl` and `err.log`: the harness's stdout and stderr;
//! - `pid`: the session leader, whose pid is also the process group id;
//! - `exit`: the exit code, written once the harness has ended.
//!
//! The app *follows* a turn with a second exec that streams both files from
//! a byte offset until `exit` appears, then prints an end marker carrying a
//! per-follow nonce. The offsets saved on the agent count only complete
//! lines that were applied, so following again after a drop, or after a
//! restart, picks up exactly where the last applied line ended.

use super::{AgentEvent, LineParser};
use crate::boxd::shell_quote;
use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Turn directories live under `$HOME/<TURNS_DIR>/<agent id>/`.
pub const TURNS_DIR: &str = ".stowbox/turns";

/// Turn directories kept per agent; the launch script deletes older ones.
pub const KEEP_TURNS: usize = 5;

/// The turn an agent is running (or last ran) and how much of its output
/// has been applied. Offsets are bytes of complete lines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCursor {
    pub id: String,
    pub out_offset: u64,
    pub err_offset: u64,
}

impl TurnCursor {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), out_offset: 0, err_offset: 0 }
    }
}

/// Turn ids sort by start time, so the launch script can keep the newest.
pub fn new_turn_id(now_ms: i64) -> String {
    let rand = uuid::Uuid::new_v4().simple().to_string();
    format!("{:013}-{}", now_ms.max(0), &rand[..8])
}

fn marker(nonce: &str) -> String {
    format!("::stowbox-{nonce}::")
}

/// `d=...` for the turn directory, quoted so any id is safe.
fn dir_assign(agent_id: &str, turn_id: &str) -> String {
    format!("d=\"$HOME\"/{}\n", shell_quote(&format!("{TURNS_DIR}/{agent_id}/{turn_id}")))
}

/// The detached process: runs the harness with the prompt on stdin, then
/// completes a partial last line in each log and writes `exit` atomically.
fn runner_script(agent_id: &str, turn_id: &str, turn_command: &str) -> String {
    let mut s = dir_assign(agent_id, turn_id);
    s.push_str("echo $$ > \"$d/pid.tmp\" && mv \"$d/pid.tmp\" \"$d/pid\"\n");
    s.push_str("(\n");
    s.push_str(turn_command.trim_end());
    s.push_str("\n) < \"$d/prompt\" >> \"$d/out.jsonl\" 2>> \"$d/err.log\"\n");
    s.push_str("rc=$?\n");
    s.push_str("for f in out.jsonl err.log; do [ -n \"$(tail -c 1 \"$d/$f\")\" ] && echo >> \"$d/$f\"; done\n");
    s.push_str("echo \"$rc\" > \"$d/exit.tmp\" && mv \"$d/exit.tmp\" \"$d/exit\"\n");
    s
}

/// The exec that starts a turn. The prompt arrives on stdin. Prints
/// `<marker> started <pid>` once the detached process has written its pid.
pub fn launch_script(agent_id: &str, turn_id: &str, turn_command: &str, nonce: &str) -> String {
    let mark = marker(nonce);
    let mut s = String::from("set -u\n");
    s.push_str(&dir_assign(agent_id, turn_id));
    s.push_str("mkdir -p \"$d\"\n");
    // Keep the newest few turn directories of this agent, this one included.
    s.push_str(&format!(
        "( cd \"$d/..\" && ls -1 | sort -r | tail -n +{} | while read -r old; do rm -rf -- \"$old\"; done )\n",
        KEEP_TURNS + 1
    ));
    s.push_str("( umask 077; cat > \"$d/prompt\" )\n");
    s.push_str(": > \"$d/out.jsonl\"; : > \"$d/err.log\"\n");
    s.push_str(&format!(
        "setsid bash -c {} </dev/null >/dev/null 2>&1 &\n",
        shell_quote(&runner_script(agent_id, turn_id, turn_command))
    ));
    s.push_str("i=0; while [ ! -s \"$d/pid\" ] && [ \"$i\" -lt 100 ]; do sleep 0.05; i=$((i+1)); done\n");
    s.push_str(&format!(
        "if [ -s \"$d/pid\" ]; then printf '{mark} started %s\\n' \"$(cat \"$d/pid\")\"; else echo 'the turn did not start' >&2; exit 1; fi\n"
    ));
    s
}

/// The exec that streams a turn from `cursor`. Lines before the `begin`
/// markers are login-shell noise and are ignored. Ends with
/// `<marker> exit <code>`, `<marker> lost` (the process is gone without an
/// exit code, as after a reboot) or `<marker> missing` (no such turn).
pub fn follow_script(agent_id: &str, cursor: &TurnCursor, nonce: &str) -> String {
    let mark = marker(nonce);
    let mut s = dir_assign(agent_id, &cursor.id);
    s.push_str(&format!("printf '{mark} begin\\n'; printf '{mark} begin\\n' >&2\n"));
    s.push_str(&format!("[ -d \"$d\" ] || {{ printf '{mark} missing\\n'; exit 0; }}\n"));
    s.push_str("i=0; while [ ! -s \"$d/pid\" ] && [ ! -e \"$d/exit\" ] && [ \"$i\" -lt 100 ]; do sleep 0.05; i=$((i+1)); done\n");
    s.push_str("pid=$(cat \"$d/pid\" 2>/dev/null)\n");
    s.push_str("if [ -n \"$pid\" ] && [ ! -e \"$d/exit\" ] && kill -0 \"$pid\" 2>/dev/null; then follow=\"-f --pid=$pid\"; else follow=; fi\n");
    s.push_str(&format!(
        "tail -c +{} $follow \"$d/out.jsonl\" 2>/dev/null &\nt1=$!\n",
        cursor.out_offset + 1
    ));
    s.push_str(&format!(
        "{{ tail -c +{} $follow \"$d/err.log\" 2>/dev/null; }} >&2 &\nt2=$!\n",
        cursor.err_offset + 1
    ));
    s.push_str("wait \"$t1\" \"$t2\"\n");
    // `stop` writes `exit` right after the kill; give it a moment.
    s.push_str("i=0; while [ ! -e \"$d/exit\" ] && [ \"$i\" -lt 20 ]; do sleep 0.1; i=$((i+1)); done\n");
    // A leading newline ends any partial line; empty lines are skipped.
    s.push_str(&format!(
        "if [ -e \"$d/exit\" ]; then printf '\\n{mark} exit %s\\n' \"$(cat \"$d/exit\")\"; else printf '\\n{mark} lost\\n'; fi\n"
    ));
    s
}

/// Exit code recorded for a turn that was stopped (128 + SIGTERM).
pub const STOPPED_EXIT: i32 = 143;

/// The exec that stops a turn: TERM to the whole process group, KILL after
/// three seconds, then records the exit so followers finish.
pub fn stop_script(agent_id: &str, turn_id: &str) -> String {
    let mut s = dir_assign(agent_id, turn_id);
    s.push_str("pid=$(cat \"$d/pid\" 2>/dev/null)\n");
    s.push_str("if [ -n \"$pid\" ] && [ ! -e \"$d/exit\" ] && kill -0 \"$pid\" 2>/dev/null; then\n");
    s.push_str("  kill -TERM -- \"-$pid\" 2>/dev/null\n");
    s.push_str("  i=0; while kill -0 \"$pid\" 2>/dev/null && [ \"$i\" -lt 30 ]; do sleep 0.1; i=$((i+1)); done\n");
    s.push_str("  kill -KILL -- \"-$pid\" 2>/dev/null\n");
    s.push_str("fi\n");
    s.push_str(&format!(
        "[ ! -d \"$d\" ] || [ -e \"$d/exit\" ] || echo {STOPPED_EXIT} > \"$d/exit\"\n"
    ));
    s
}

/// How a follow stream ended.
#[derive(Debug, Clone, PartialEq)]
pub enum FollowEnd {
    /// The harness ended with this code.
    Exited(i32),
    /// The process is gone and never wrote an exit code.
    Lost,
    /// The box has no directory for this turn.
    Missing,
    /// The stream broke while the turn may still be running: follow again.
    Dropped(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FollowItem {
    /// A complete stdout line, and the cursor once it is applied.
    Out { line: String, cursor: TurnCursor },
    /// A complete stderr line, and the cursor once it is applied.
    Err { line: String, cursor: TurnCursor },
    End(FollowEnd),
}

/// Turns the follow exec's chunks into lines and offsets.
pub struct Follow {
    mark: String,
    cursor: TurnCursor,
    out: Vec<u8>,
    err: Vec<u8>,
    out_begun: bool,
    err_begun: bool,
    ended: bool,
}

impl Follow {
    pub fn new(cursor: TurnCursor, nonce: &str) -> Self {
        Self {
            mark: marker(nonce),
            cursor,
            out: Vec::new(),
            err: Vec::new(),
            out_begun: false,
            err_begun: false,
            ended: false,
        }
    }

    pub fn cursor(&self) -> &TurnCursor {
        &self.cursor
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn push(&mut self, chunk: ExecOutput) -> Vec<FollowItem> {
        if self.ended {
            return Vec::new();
        }
        let mut items = Vec::new();
        match chunk {
            ExecOutput::Stdout(b) => {
                self.out.extend_from_slice(&b);
                while let Some(pos) = self.out.iter().position(|&c| c == b'\n') {
                    let raw: Vec<u8> = self.out.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&raw[..raw.len() - 1]).trim_end_matches('\r').to_string();
                    if let Some(rest) = line.strip_prefix(&self.mark) {
                        let end = match rest.split_whitespace().collect::<Vec<_>>().as_slice() {
                            ["begin"] => {
                                self.out_begun = true;
                                continue;
                            }
                            ["exit", c] => FollowEnd::Exited(c.parse().unwrap_or(-1)),
                            ["lost"] => FollowEnd::Lost,
                            ["missing"] => FollowEnd::Missing,
                            _ => continue,
                        };
                        self.ended = true;
                        items.push(FollowItem::End(end));
                        return items;
                    }
                    if !self.out_begun {
                        continue;
                    }
                    self.cursor.out_offset += raw.len() as u64;
                    if !line.trim().is_empty() {
                        items.push(FollowItem::Out { line, cursor: self.cursor.clone() });
                    }
                }
            }
            ExecOutput::Stderr(b) => {
                self.err.extend_from_slice(&b);
                while let Some(pos) = self.err.iter().position(|&c| c == b'\n') {
                    let raw: Vec<u8> = self.err.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&raw[..raw.len() - 1]).trim_end_matches('\r').to_string();
                    if !self.err_begun {
                        self.err_begun = line.strip_prefix(&self.mark).map(str::trim) == Some("begin");
                        continue;
                    }
                    self.cursor.err_offset += raw.len() as u64;
                    if !line.trim().is_empty() {
                        items.push(FollowItem::Err { line, cursor: self.cursor.clone() });
                    }
                }
            }
            ExecOutput::Exit(code) => {
                self.ended = true;
                items.push(FollowItem::End(FollowEnd::Dropped(format!(
                    "the follow stream exited with code {code} before the turn ended"
                ))));
            }
            ExecOutput::Error(e) => {
                self.ended = true;
                items.push(FollowItem::End(FollowEnd::Dropped(e)));
            }
        }
        items
    }
}

/// Normalized events for one followed line, as the attached runner used to
/// produce them: stdout through the harness parser, stderr verbatim.
pub fn line_events(parser: &mut dyn LineParser, item: &FollowItem) -> Vec<AgentEvent> {
    match item {
        FollowItem::Out { line, .. } => parser.parse_line(line),
        FollowItem::Err { line, .. } => vec![AgentEvent::Stderr { text: line.clone() }],
        FollowItem::End(_) => Vec::new(),
    }
}

fn plain(script: String, env: Vec<(String, String)>) -> ExecSpec {
    ExecSpec { script, tty: false, cols: 0, rows: 0, env }
}

/// Collects a short exec's output. Returns (exit code, stdout, stderr).
async fn run_to_end(handle: &mut ExecHandle) -> Result<(i32, String, String)> {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    while let Some(chunk) = handle.output.recv().await {
        match chunk {
            ExecOutput::Stdout(b) => out.extend_from_slice(&b),
            ExecOutput::Stderr(b) => err.extend_from_slice(&b),
            ExecOutput::Exit(code) => {
                return Ok((code, String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned()))
            }
            ExecOutput::Error(e) => return Err(Error::Provider(e)),
        }
    }
    Err(Error::Provider("the exec stream ended without an exit code".into()))
}

/// Starts a turn in the box and returns once it runs on its own. `env` is
/// exported for the harness (TERM, NO_COLOR).
pub async fn launch<P: Provider + ?Sized>(
    p: &P,
    machine_id: &str,
    agent_id: &str,
    turn_id: &str,
    turn_command: &str,
    prompt: &str,
    env: Vec<(String, String)>,
) -> Result<()> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut handle = p.exec(machine_id, plain(launch_script(agent_id, turn_id, turn_command, &nonce), env)).await?;
    let sent = handle.input.send(ExecInput::Data(prompt.as_bytes().to_vec())).await.is_ok()
        && handle.input.send(ExecInput::Close).await.is_ok();
    if !sent {
        return Err(Error::Provider("the box closed the shell before the prompt was sent".into()));
    }
    let (code, out, err) = run_to_end(&mut handle).await?;
    let started = format!("{} started", marker(&nonce));
    if code == 0 && out.lines().any(|l| l.starts_with(&started)) {
        return Ok(());
    }
    let why = err.trim();
    Err(Error::Provider(if why.is_empty() {
        format!("the turn did not start (exit code {code})")
    } else {
        format!("the turn did not start: {why}")
    }))
}

/// A live follow exec. Dropping it closes the stream; the turn keeps going.
pub struct FollowStream {
    handle: ExecHandle,
    follow: Follow,
    queue: VecDeque<FollowItem>,
}

impl FollowStream {
    /// The next line or the end. After `End`, keeps returning `Dropped`.
    pub async fn next(&mut self) -> FollowItem {
        loop {
            if let Some(item) = self.queue.pop_front() {
                return item;
            }
            if self.follow.ended() {
                return FollowItem::End(FollowEnd::Dropped("the follow stream already ended".into()));
            }
            match self.handle.output.recv().await {
                Some(chunk) => self.queue.extend(self.follow.push(chunk)),
                None => {
                    self.follow.ended = true;
                    return FollowItem::End(FollowEnd::Dropped("the exec stream closed".into()));
                }
            }
        }
    }

    pub fn cursor(&self) -> &TurnCursor {
        self.follow.cursor()
    }
}

/// Opens a follow stream for `cursor`.
pub async fn follow<P: Provider + ?Sized>(p: &P, machine_id: &str, agent_id: &str, cursor: &TurnCursor) -> Result<FollowStream> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let handle = p.exec(machine_id, plain(follow_script(agent_id, cursor, &nonce), vec![])).await?;
    // Nothing to send; EOF so nothing waits on stdin.
    let _ = handle.input.send(ExecInput::Close).await;
    Ok(FollowStream { handle, follow: Follow::new(cursor.clone(), &nonce), queue: VecDeque::new() })
}

/// Kills the turn's process group and records it as stopped.
pub async fn stop<P: Provider + ?Sized>(p: &P, machine_id: &str, agent_id: &str, turn_id: &str) -> Result<()> {
    let mut handle = p.exec(machine_id, plain(stop_script(agent_id, turn_id), vec![])).await?;
    let _ = handle.input.send(ExecInput::Close).await;
    let (code, _, err) = run_to_end(&mut handle).await?;
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Provider(format!("could not stop the turn (exit code {code}): {}", err.trim())))
    }
}

/// Seconds to wait before following again after the `attempt`-th drop.
pub fn backoff_secs(attempt: u32) -> u64 {
    [1, 2, 4, 8, 15, 30][attempt.min(5) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn out(b: &str) -> ExecOutput {
        ExecOutput::Stdout(b.as_bytes().to_vec())
    }

    #[test]
    fn turn_ids_sort_by_time() {
        let a = new_turn_id(999);
        let b = new_turn_id(1_700_000_000_000);
        assert!(a < b, "{a} {b}");
        assert_eq!(b.len(), 13 + 1 + 8);
    }

    #[test]
    fn scripts_quote_the_turn_dir_and_keep_the_prompt_off_the_command_line() {
        let s = launch_script("ag'1", "t1", "cd '/w' && claude -p", "n");
        assert!(s.contains("d=\"$HOME\"/'.stowbox/turns/ag'\\''1/t1'"));
        assert!(s.contains("cat > \"$d/prompt\""));
        assert!(s.contains("setsid bash -c "));
        assert!(s.contains("tail -n +6"));
        assert!(follow_script("a", &TurnCursor { id: "t".into(), out_offset: 10, err_offset: 3 }, "n").contains("tail -c +11 "));
        assert!(stop_script("a", "t").contains("kill -TERM -- \"-$pid\""));
    }

    #[test]
    fn follow_counts_complete_lines_only_and_needs_the_nonce() {
        let mut f = Follow::new(TurnCursor::new("t"), "n1");
        // Login noise before `begin` is ignored and not counted.
        assert!(f.push(out("motd\n::stowbox-n1:: begin\n{\"a\"")).is_empty());
        let items = f.push(out(":1}\n\n{\"b\":2}\r\n::stowbox-xx:: exit 0\npart"));
        assert_eq!(
            items,
            vec![
                FollowItem::Out { line: "{\"a\":1}".into(), cursor: TurnCursor { id: "t".into(), out_offset: 8, err_offset: 0 } },
                FollowItem::Out { line: "{\"b\":2}".into(), cursor: TurnCursor { id: "t".into(), out_offset: 18, err_offset: 0 } },
                FollowItem::Out { line: "::stowbox-xx:: exit 0".into(), cursor: TurnCursor { id: "t".into(), out_offset: 40, err_offset: 0 } },
            ]
        );
        // A stderr line before its own `begin` is noise too.
        assert!(f.push(ExecOutput::Stderr(b"warn\n::stowbox-n1:: begin\n".to_vec())).is_empty());
        let items = f.push(ExecOutput::Stderr(b"oops\n".to_vec()));
        assert_eq!(items, vec![FollowItem::Err { line: "oops".into(), cursor: TurnCursor { id: "t".into(), out_offset: 40, err_offset: 5 } }]);
        // The partial "part" is not counted; the end marker is not either.
        assert_eq!(f.push(out("\n::stowbox-n1:: exit 3\n")), vec![
            FollowItem::Out { line: "part".into(), cursor: TurnCursor { id: "t".into(), out_offset: 45, err_offset: 5 } },
            FollowItem::End(FollowEnd::Exited(3)),
        ]);
        assert_eq!(f.cursor().out_offset, 45);
        assert!(f.push(out("late\n")).is_empty());
    }

    #[test]
    fn follow_reports_a_broken_stream_as_dropped() {
        let mut f = Follow::new(TurnCursor::new("t"), "n");
        f.push(out("::stowbox-n:: begin\nhalf"));
        assert!(matches!(f.push(ExecOutput::Error("reset".into())).as_slice(), [FollowItem::End(FollowEnd::Dropped(m))] if m == "reset"));
        assert_eq!(f.cursor().out_offset, 0);
        let mut f = Follow::new(TurnCursor::new("t"), "n");
        assert!(matches!(f.push(ExecOutput::Exit(255)).as_slice(), [FollowItem::End(FollowEnd::Dropped(_))]));
        let mut f = Follow::new(TurnCursor::new("t"), "n");
        assert_eq!(f.push(out("::stowbox-n:: begin\n::stowbox-n:: missing\n")), vec![FollowItem::End(FollowEnd::Missing)]);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(0), 1);
        assert_eq!(backoff_secs(3), 8);
        assert_eq!(backoff_secs(40), 30);
    }

    // ------------------------------------------------ end to end with local bash

    /// Runs exec scripts with local bash. Like boxd, dropping the session
    /// kills its whole process group, so a detached turn must escape it.
    struct LocalBox {
        home: std::path::PathBuf,
    }

    fn kill_group(pgid: u32) {
        let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{pgid}")]).stderr(std::process::Stdio::null()).status();
    }

    #[async_trait::async_trait]
    impl Provider for LocalBox {
        fn id(&self) -> &'static str {
            "local"
        }
        async fn whoami(&self) -> Result<Account> {
            unimplemented!()
        }
        async fn list_machines(&self) -> Result<Vec<Machine>> {
            unimplemented!()
        }
        async fn get_machine(&self, _: &str) -> Result<Machine> {
            unimplemented!()
        }
        async fn create_machine(&self, _: CreateSpec) -> Result<Machine> {
            unimplemented!()
        }
        async fn fork_machine(&self, _: &str, _: Option<String>) -> Result<Machine> {
            unimplemented!()
        }
        async fn power(&self, _: &str, _: PowerAction) -> Result<()> {
            unimplemented!()
        }
        async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
            unimplemented!()
        }
        async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
            unimplemented!()
        }
        async fn machine_usage(&self, _: &str) -> Result<Usage> {
            unimplemented!()
        }
        async fn org_billing(&self, _: &str) -> Result<OrgBilling> {
            unimplemented!()
        }
        async fn rates(&self) -> Result<(String, Rates)> {
            unimplemented!()
        }
        async fn exec(&self, _: &str, spec: ExecSpec) -> Result<ExecHandle> {
            use std::io::{Read, Write};
            use std::os::unix::process::CommandExt;
            let (in_tx, mut in_rx) = tokio::sync::mpsc::channel::<ExecInput>(8);
            let (out_tx, out_rx) = tokio::sync::mpsc::channel::<ExecOutput>(64);
            let mut script = String::new();
            for (k, v) in &spec.env {
                script.push_str(&format!("export {}={} && ", k, shell_quote(v)));
            }
            script.push_str(&spec.script);
            let mut child = std::process::Command::new("bash")
                .arg("-c")
                .arg(&script)
                .env("HOME", &self.home)
                .current_dir(&self.home)
                .process_group(0)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            let pgid = child.id();
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();
            tokio::spawn(async move {
                while let Some(ExecInput::Data(d)) = in_rx.recv().await {
                    let _ = stdin.write_all(&d);
                }
            });
            let watch = out_tx.clone();
            tokio::spawn(async move {
                watch.closed().await;
                kill_group(pgid);
            });
            let err_tx = out_tx.clone();
            let err_thread = std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 || err_tx.blocking_send(ExecOutput::Stderr(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            });
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stdout.read(&mut buf) {
                    if n == 0 || out_tx.blocking_send(ExecOutput::Stdout(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                let _ = err_thread.join();
                let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
                let _ = out_tx.blocking_send(ExecOutput::Exit(code));
            });
            Ok(ExecHandle { input: in_tx, output: out_rx })
        }
    }

    fn have_tools() -> bool {
        std::process::Command::new("bash").arg("-c").arg("command -v setsid && tail --version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn local_box() -> LocalBox {
        let home = std::env::temp_dir().join(format!("stowbox-turn-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        // A fake harness: echoes the prompt, then prints JSONL slowly, a
        // line split across writes, a warning on stderr, and exits 3.
        std::fs::write(
            home.join("harness.sh"),
            concat!(
                "read -r prompt\n",
                "printf '{\"n\":0,\"prompt\":\"%s\"}\\n' \"$prompt\"\n",
                "for i in 1 2 3 4 5; do sleep 0.15; printf '{\"n\":%d,' \"$i\"; sleep 0.05; printf '\"ok\":true}\\n'; done\n",
                "echo 'warning: slow' >&2\n",
                "sleep 0.2; printf '{\"n\":6}\\n'\n",
                "exit 3\n",
            ),
        )
        .unwrap();
        LocalBox { home }
    }

    fn pid_alive(home: &std::path::Path, agent: &str, turn: &str) -> bool {
        let pid = std::fs::read_to_string(home.join(TURNS_DIR).join(agent).join(turn).join("pid")).unwrap();
        std::process::Command::new("kill").args(["-0", pid.trim()]).stderr(std::process::Stdio::null()).status().unwrap().success()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_turn_survives_a_dropped_follow_and_every_line_arrives_once() {
        if !have_tools() {
            return;
        }
        let b = local_box();
        let turn = new_turn_id(1);
        launch(&b, "vm", "ag", &turn, "bash \"$HOME/harness.sh\"", "fix the bug\n", vec![("NO_COLOR".into(), "1".into())]).await.unwrap();

        let mut cursor = TurnCursor::new(turn.clone());
        let mut lines = Vec::new();
        let mut errs = Vec::new();
        // First follow: take two lines, then drop the stream mid-turn.
        let mut s = follow(&b, "vm", "ag", &cursor).await.unwrap();
        while lines.len() < 2 {
            match s.next().await {
                FollowItem::Out { line, cursor: c } => {
                    lines.push(line);
                    cursor = c;
                }
                FollowItem::Err { line, cursor: c } => {
                    errs.push(line);
                    cursor = c;
                }
                FollowItem::End(e) => panic!("ended early: {e:?}"),
            }
        }
        drop(s);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(pid_alive(&b.home, "ag", &turn), "the turn died with its follow stream");

        // Follow again from the saved cursor, through to the end.
        let mut s = follow(&b, "vm", "ag", &cursor).await.unwrap();
        let end = loop {
            match s.next().await {
                FollowItem::Out { line, cursor: c } => {
                    lines.push(line);
                    cursor = c;
                }
                FollowItem::Err { line, cursor: c } => {
                    errs.push(line);
                    cursor = c;
                }
                FollowItem::End(e) => break e,
            }
        };
        assert_eq!(end, FollowEnd::Exited(3));
        let mut want = vec!["{\"n\":0,\"prompt\":\"fix the bug\"}".to_string()];
        want.extend((1..=5).map(|i| format!("{{\"n\":{i},\"ok\":true}}")));
        want.push("{\"n\":6}".into());
        assert_eq!(lines, want);
        assert_eq!(errs, vec!["warning: slow".to_string()]);
        let dir = b.home.join(TURNS_DIR).join("ag").join(&turn);
        assert_eq!(cursor.out_offset, std::fs::metadata(dir.join("out.jsonl")).unwrap().len());
        assert_eq!(cursor.err_offset, std::fs::metadata(dir.join("err.log")).unwrap().len());

        // Following a finished turn from its end yields only the exit.
        let mut s = follow(&b, "vm", "ag", &cursor).await.unwrap();
        assert_eq!(s.next().await, FollowItem::End(FollowEnd::Exited(3)));
        let _ = std::fs::remove_dir_all(&b.home);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stop_kills_the_turn_and_a_killed_turn_reads_as_lost() {
        if !have_tools() {
            return;
        }
        let b = local_box();
        let cmd = "sleep 30 & sleep 30; echo never";
        let t1 = new_turn_id(1);
        launch(&b, "vm", "ag", &t1, cmd, "x\n", vec![]).await.unwrap();
        assert!(pid_alive(&b.home, "ag", &t1));
        stop(&b, "vm", "ag", &t1).await.unwrap();
        assert!(!pid_alive(&b.home, "ag", &t1));
        let mut s = follow(&b, "vm", "ag", &TurnCursor::new(t1.clone())).await.unwrap();
        assert_eq!(s.next().await, FollowItem::End(FollowEnd::Exited(STOPPED_EXIT)));

        // Killed from outside (a reboot, the OOM killer): no exit code.
        let t2 = new_turn_id(2);
        launch(&b, "vm", "ag", &t2, cmd, "x\n", vec![]).await.unwrap();
        let mut s = follow(&b, "vm", "ag", &TurnCursor::new(t2.clone())).await.unwrap();
        let pid: u32 = std::fs::read_to_string(b.home.join(TURNS_DIR).join("ag").join(&t2).join("pid")).unwrap().trim().parse().unwrap();
        kill_group(pid);
        assert_eq!(tokio::time::timeout(Duration::from_secs(10), s.next()).await.unwrap(), FollowItem::End(FollowEnd::Lost));

        let mut s = follow(&b, "vm", "ag", &TurnCursor::new("nope")).await.unwrap();
        assert_eq!(s.next().await, FollowItem::End(FollowEnd::Missing));
        let _ = std::fs::remove_dir_all(&b.home);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn launch_keeps_only_the_newest_turn_dirs() {
        if !have_tools() {
            return;
        }
        let b = local_box();
        let ids: Vec<String> = (0..KEEP_TURNS as i64 + 2).map(new_turn_id).collect();
        for id in &ids {
            launch(&b, "vm", "ag", id, "true", "x\n", vec![]).await.unwrap();
        }
        let mut left: Vec<String> = std::fs::read_dir(b.home.join(TURNS_DIR).join("ag")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ids[2..].to_vec());
        let _ = std::fs::remove_dir_all(&b.home);
    }
}
