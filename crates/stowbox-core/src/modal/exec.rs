//! Exec in a Modal sandbox through its task command router, the gRPC
//! service on the worker that runs it.
//!
//! `TaskExecStart` starts `sh -c` with the same command line SSH runs
//! ([`remote_command`]); output is read as one stream per file descriptor
//! from a byte offset, so a dropped stream picks up where it broke; input
//! is written at an offset too; `TaskExecWait` gives the exit.
//!
//! The router has no resize and no kill. A terminal records its shell's pid
//! and tty in a file named after the exec, and a resize is a second,
//! short-lived exec running `stty` on that tty, which sends the terminal
//! its SIGWINCH. Ending a command is also a second exec: a hang-up to the
//! terminal's session, or the kill of a script's process group.

use super::api::{self, Router};
use super::pb::client::{pty_info::PtyType, PtyInfo, SandboxGetTaskIdRequest, TaskGetCommandRouterAccessRequest};
use super::pb::router::*;
use crate::sshexec::{kill_group_command, remote_command, Marker};
use crate::types::*;
use crate::{Error, Result};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, watch};

/// Most input bytes sent in one call; more waiting input goes in the next.
const MAX_INPUT_BATCH: usize = 64 << 10;
/// Ctrl-D: how a PTY is told its input ended.
const EOT: u8 = 0x04;
/// How long one router call may take.
const CALL: Duration = Duration::from_secs(30);
/// How long a resize or a kill may take, `sleep 1` included.
const SIDE: Duration = Duration::from_secs(15);
/// Times an output stream that broke is opened again from where it was.
const READ_RETRIES: u32 = 3;
/// How long Modal is asked to wait for a sandbox to be ready.
const READY_WAIT_SECS: f32 = 50.0;
/// A router token is replaced this long before it expires.
const TOKEN_MARGIN: Duration = Duration::from_secs(120);
/// Kept when the token does not say when it expires.
const TOKEN_FALLBACK: Duration = Duration::from_secs(300);
/// A task id as Modal mints them; it goes into requests only.
const MAX_TASK_ID: usize = 64;

/// One sandbox's task and a client for its router.
#[derive(Clone)]
pub struct Task {
    pub id: String,
    pub router: Router,
}

struct Cached {
    task: Task,
    expires: SystemTime,
}

/// The router of each sandbox in use, with a token replaced before it runs
/// out, so a terminal open for hours keeps working.
pub struct Routers {
    client: api::Client,
    cache: Mutex<HashMap<String, Cached>>,
}

impl Routers {
    pub fn new(client: api::Client) -> Self {
        Self { client, cache: Default::default() }
    }

    pub fn forget(&self, sandbox_id: &str) {
        if let Ok(mut c) = self.cache.lock() {
            c.remove(sandbox_id);
        }
    }

    /// The sandbox's task, waiting for it to be ready.
    pub async fn task(&self, sandbox_id: &str) -> Result<Task> {
        let now = SystemTime::now();
        // SystemTime arithmetic is checked, so a hostile `exp` cannot wrap.
        let fresh = |a: &&Cached| now.checked_add(TOKEN_MARGIN).is_some_and(|t| a.expires > t);
        let cached = self.cache.lock().ok().and_then(|c| c.get(sandbox_id).filter(fresh).map(|a| a.task.clone()));
        if let Some(t) = cached {
            return Ok(t);
        }
        let mut client = self.client.clone();
        let req = SandboxGetTaskIdRequest { sandbox_id: sandbox_id.to_string(), timeout: Some(READY_WAIT_SECS), wait_until_ready: true };
        let got = client.sandbox_get_task_id(req).await?.into_inner();
        if got.task_result.is_some() {
            return Err(Error::Provider(format!("{sandbox_id} has ended, so it cannot run commands")));
        }
        let task_id =
            got.task_id.filter(|t| valid_task_id(t)).ok_or_else(|| Error::Provider(format!("{sandbox_id} is not ready yet; try again in a moment")))?;
        let access = client.task_get_command_router_access(TaskGetCommandRouterAccessRequest { task_id: task_id.clone() }).await?.into_inner();
        let router = api::router(&access.url, &access.jwt).await?;
        let expires = crate::boxd::auth::jwt_exp(&access.jwt).or_else(|| now.checked_add(TOKEN_FALLBACK)).unwrap_or(now);
        let task = Task { id: task_id, router };
        if let Ok(mut c) = self.cache.lock() {
            c.insert(sandbox_id.to_string(), Cached { task: task.clone(), expires });
        }
        Ok(task)
    }
}

fn valid_task_id(id: &str) -> bool {
    (1..=MAX_TASK_ID).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn within<T>(what: &str, f: impl Future<Output = std::result::Result<T, tonic::Status>>) -> Result<T> {
    match tokio::time::timeout(CALL, f).await {
        Ok(r) => Ok(r?),
        Err(_) => Err(Error::Network(format!("Modal's command router did not answer {what} in time"))),
    }
}

/// Where a terminal records `<pid> <tty>`. The exec id is a UUID, so the
/// path needs no quoting.
fn tty_file(exec_id: &str) -> String {
    format!("/tmp/.stowbox-{exec_id}.tty")
}

pub fn start_request(task_id: &str, exec_id: &str, spec: &ExecSpec) -> TaskExecStartRequest {
    let command = if spec.tty {
        format!("printf '%s %s\\n' \"$$\" \"$(tty)\" > {} 2>/dev/null; exec {}", tty_file(exec_id), remote_command(spec))
    } else {
        remote_command(spec)
    };
    let pty_info = spec.tty.then(|| PtyInfo {
        enabled: true,
        winsz_rows: u32::from(spec.rows.max(5)),
        winsz_cols: u32::from(spec.cols.max(20)),
        env_term: "xterm-256color".into(),
        env_colorterm: "truecolor".into(),
        env_term_program: String::new(),
        pty_type: PtyType::Shell as i32,
        // A terminal can sit with no input for hours.
        no_terminate_on_idle_stdin: true,
    });
    TaskExecStartRequest {
        task_id: task_id.to_string(),
        exec_id: exec_id.to_string(),
        command_args: vec!["/bin/sh".into(), "-c".into(), command],
        stdout_config: TaskExecStdoutConfig::Pipe as i32,
        // A PTY has one output, so its stderr stream stays empty and is not
        // read; the router refuses STDOUT here.
        stderr_config: TaskExecStderrConfig::Pipe as i32,
        timeout_secs: None,
        workdir: None,
        pty_info,
        env: HashMap::new(),
    }
}

/// Waits up to two seconds for the shell to have written its record, since
/// the first resize can come before it has.
fn resize_script(exec_id: &str, cols: u16, rows: u16) -> String {
    let f = tty_file(exec_id);
    format!(
        "for i in 1 2 3 4 5 6 7 8 9 10; do [ -s {f} ] && break; sleep 0.2; done; read p t < {f} && stty -F \"$t\" rows {} cols {}",
        rows.max(5),
        cols.max(20)
    )
}

/// SIGHUP to the terminal's session, as closing a real terminal sends, and
/// the record removed.
fn hangup_script(exec_id: &str) -> String {
    let f = tty_file(exec_id);
    format!("read p t < {f} 2>/dev/null; [ \"$p\" -gt 1 ] 2>/dev/null && kill -HUP -\"$p\" \"$p\" 2>/dev/null; rm -f {f}")
}

/// Runs `script` as root with no output and waits for it, best effort.
async fn side(routers: &Routers, sandbox_id: &str, script: String) {
    let run = async {
        let task = routers.task(sandbox_id).await?;
        let exec_id = uuid::Uuid::new_v4().to_string();
        let req = TaskExecStartRequest {
            task_id: task.id.clone(),
            exec_id: exec_id.clone(),
            command_args: vec!["/bin/sh".into(), "-c".into(), script],
            stdout_config: TaskExecStdoutConfig::Devnull as i32,
            stderr_config: TaskExecStderrConfig::Devnull as i32,
            timeout_secs: Some(SIDE.as_secs() as u32),
            workdir: None,
            pty_info: None,
            env: HashMap::new(),
        };
        task.router.clone().task_exec_start(req).await?;
        task.router.clone().task_exec_wait(TaskExecWaitRequest { task_id: task.id, exec_id }).await?;
        Ok::<_, Error>(())
    };
    let _ = tokio::time::timeout(SIDE, run).await;
}

enum Chunk {
    Out(TaskExecStdioFileDescriptor, Vec<u8>),
    End,
    Broke(String),
}

/// Reads one file descriptor to its end into `tx`, opening the stream again
/// from the last byte read when it breaks.
async fn read_fd(routers: Arc<Routers>, sandbox_id: String, exec_id: String, fd: TaskExecStdioFileDescriptor, tx: mpsc::Sender<Chunk>) {
    let mut offset: u64 = 0;
    let mut tries = 0;
    let end = loop {
        let opened = async {
            let task = routers.task(&sandbox_id).await?;
            let req = TaskExecStdioReadRequest { task_id: task.id, exec_id: exec_id.clone(), offset, file_descriptor: fd as i32 };
            within("a read", task.router.clone().task_exec_stdio_read(req)).await
        };
        let mut stream = match opened.await {
            Ok(r) => r.into_inner(),
            // Only the last failure is reported.
            Err(_) if tries < READ_RETRIES => {
                tries += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            Err(e) => break Chunk::Broke(e.to_string()),
        };
        loop {
            match stream.message().await {
                Ok(Some(m)) => {
                    tries = 0;
                    offset = offset.saturating_add(m.data.len() as u64);
                    if !m.data.is_empty() && tx.send(Chunk::Out(fd, m.data)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => return drop(tx.send(Chunk::End).await),
                Err(_) if tries < READ_RETRIES => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    break;
                }
                Err(e) => return drop(tx.send(Chunk::Broke(Error::from(e).to_string())).await),
            }
        }
    };
    let _ = tx.send(end).await;
}

pub async fn exec(routers: Arc<Routers>, sandbox_id: String, spec: ExecSpec) -> Result<ExecHandle> {
    let tty = spec.tty;
    let task = routers.task(&sandbox_id).await?;
    let exec_id = uuid::Uuid::new_v4().to_string();
    within("the start", task.router.clone().task_exec_start(start_request(&task.id, &exec_id, &spec))).await?;

    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    // A resize is a second exec, which takes a round trip or two: it runs
    // off the input path so typing does not wait on it, and only the latest
    // size is applied, so dragging a window does not queue dozens.
    let (size_tx, mut size_rx) = watch::channel::<Option<(u16, u16)>>(None);
    if tty {
        let (routers, sandbox_id, exec_id) = (routers.clone(), sandbox_id.clone(), exec_id.clone());
        tokio::spawn(async move {
            while size_rx.changed().await.is_ok() {
                let latest = *size_rx.borrow_and_update();
                if let Some((cols, rows)) = latest {
                    side(&routers, &sandbox_id, resize_script(&exec_id, cols, rows)).await;
                }
            }
        });
    }

    let input_routers = routers.clone();
    let (input_sandbox, input_exec) = (sandbox_id.clone(), exec_id.clone());
    // Weak, so the output still ends when the reader task does.
    let input_errors = out_tx.downgrade();
    tokio::spawn(async move {
        let mut offset: u64 = 0;
        let write = |offset: u64, data: Vec<u8>, eof: bool| {
            let (routers, sandbox, exec_id) = (input_routers.clone(), input_sandbox.clone(), input_exec.clone());
            async move {
                let task = routers.task(&sandbox).await?;
                let req = TaskExecStdinWriteRequest { task_id: task.id, exec_id, offset, data, eof };
                within("the input", task.router.clone().task_exec_stdin_write(req)).await.map(drop)
            }
        };
        let mut pending: Option<ExecInput> = None;
        loop {
            let msg = match pending.take() {
                Some(m) => m,
                None => match in_rx.recv().await {
                    Some(m) => m,
                    None => break,
                },
            };
            let sent = match msg {
                ExecInput::Data(mut data) => {
                    // Keystrokes arrive one at a time; a call each would lag.
                    while data.len() < MAX_INPUT_BATCH {
                        match in_rx.try_recv() {
                            Ok(ExecInput::Data(more)) => data.extend_from_slice(&more),
                            Ok(other) => {
                                pending = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    let n = data.len() as u64;
                    let r = write(offset, data, false).await;
                    offset = offset.saturating_add(n);
                    r
                }
                ExecInput::Resize { cols, rows } => {
                    // Without a TTY nothing receives it, and nothing is lost.
                    let _ = size_tx.send(Some((cols, rows)));
                    Ok(())
                }
                ExecInput::Close => break,
            };
            if let Err(e) = sent {
                // The reader passes it on; the command may still be running.
                if let Some(tx) = input_errors.upgrade() {
                    let _ = tx.send(ExecOutput::Error(format!("input did not reach the sandbox: {e}"))).await;
                }
                return;
            }
        }
        let _ = if tty { write(offset, vec![EOT], false).await } else { write(offset, Vec::new(), true).await };
    });

    let (chunk_tx, mut chunks) = mpsc::channel::<Chunk>(64);
    let mut open = 1;
    tokio::spawn(read_fd(routers.clone(), sandbox_id.clone(), exec_id.clone(), TaskExecStdioFileDescriptor::Stdout, chunk_tx.clone()));
    if !tty {
        open += 1;
        tokio::spawn(read_fd(routers.clone(), sandbox_id.clone(), exec_id.clone(), TaskExecStdioFileDescriptor::Stderr, chunk_tx.clone()));
    }
    drop(chunk_tx);

    tokio::spawn(async move {
        let mut marker = Marker::new(tty);
        let mut pgid: Option<u32> = None;
        let end = |pgid: Option<u32>| kill(&routers, &sandbox_id, &exec_id, tty, pgid);
        while open > 0 {
            let next = tokio::select! {
                _ = out_tx.closed() => return end(pgid).await,
                c = chunks.recv() => c,
            };
            let out = match next {
                Some(Chunk::Out(TaskExecStdioFileDescriptor::Stderr, data)) => {
                    let (found, rest) = marker.feed(&data);
                    pgid = pgid.or(found);
                    match rest.filter(|r| !r.is_empty()) {
                        Some(r) => ExecOutput::Stderr(r),
                        None => continue,
                    }
                }
                Some(Chunk::Out(_, data)) => ExecOutput::Stdout(data),
                Some(Chunk::End) => {
                    open -= 1;
                    continue;
                }
                Some(Chunk::Broke(e)) => {
                    // Nothing reads the command any more, so it is ended.
                    let _ = out_tx.send(ExecOutput::Error(format!("the output from Modal broke off: {e}"))).await;
                    return end(pgid).await;
                }
                None => break,
            };
            if out_tx.send(out).await.is_err() {
                return end(pgid).await;
            }
        }
        // Both streams ended: the command has exited, or closed its output
        // and goes on. The wait is asked again until it answers.
        let exit = loop {
            let wait = async {
                let task = routers.task(&sandbox_id).await?;
                within("the wait", task.router.clone().task_exec_wait(TaskExecWaitRequest { task_id: task.id, exec_id: exec_id.clone() })).await
            };
            tokio::select! {
                _ = out_tx.closed() => return end(pgid).await,
                r = wait => match r {
                    Ok(w) => break exit_of(w.into_inner()),
                    // A router that cannot be reached fails at once; the
                    // pause keeps that from spinning.
                    Err(Error::Network(_)) => tokio::time::sleep(Duration::from_secs(1)).await,
                    Err(e) => break ExecOutput::Error(e.to_string()),
                },
            }
        };
        let _ = out_tx.send(exit).await;
        // The output ends now, not after the clean-up's round trip.
        drop(out_tx);
        if tty {
            side(&routers, &sandbox_id, format!("rm -f {}", tty_file(&exec_id))).await;
        }
    });

    Ok(ExecHandle { input: in_tx, output: out_rx })
}

pub fn exit_of(w: TaskExecWaitResponse) -> ExecOutput {
    match w.exit_status {
        Some(task_exec_wait_response::ExitStatus::Code(c)) => ExecOutput::Exit(c),
        // As a shell reports a death by signal.
        Some(task_exec_wait_response::ExitStatus::Signal(s)) => ExecOutput::Exit(128_i32.saturating_add(s)),
        None => ExecOutput::Error("Modal did not say how the command ended".into()),
    }
}

/// Ends the command and what it left running: a hang-up for a terminal, the
/// process group for a script once its marker said which.
async fn kill(routers: &Routers, sandbox_id: &str, exec_id: &str, tty: bool, pgid: Option<u32>) {
    let script = if tty { Some(hangup_script(exec_id)) } else { pgid.and_then(kill_group_command) };
    if let Some(s) = script {
        side(routers, sandbox_id, s).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_records_its_tty_and_a_script_its_group() {
        let spec = ExecSpec { script: "echo hi".into(), tty: true, cols: 100, rows: 30, env: vec![] };
        let r = start_request("ta-1", "e1", &spec);
        assert_eq!(r.command_args[..2], ["/bin/sh".to_string(), "-c".to_string()]);
        assert!(r.command_args[2].starts_with("printf '%s %s\\n' \"$$\" \"$(tty)\" > /tmp/.stowbox-e1.tty 2>/dev/null; exec bash -lc "));
        let pty = r.pty_info.unwrap();
        assert_eq!((pty.winsz_cols, pty.winsz_rows, pty.no_terminate_on_idle_stdin), (100, 30, true));

        let plain = start_request("ta-1", "e2", &ExecSpec { script: "cat".into(), ..Default::default() });
        assert!(plain.pty_info.is_none());
        assert_eq!(plain.command_args[2], remote_command(&ExecSpec { script: "cat".into(), ..Default::default() }));
    }

    #[test]
    fn resize_and_hangup_read_the_record() {
        assert!(resize_script("e1", 0, 0).ends_with("; read p t < /tmp/.stowbox-e1.tty && stty -F \"$t\" rows 5 cols 20"));
        let h = hangup_script("e1");
        assert!(h.contains("kill -HUP -\"$p\" \"$p\"") && h.ends_with("rm -f /tmp/.stowbox-e1.tty"));
    }

    #[test]
    fn an_exit_by_signal_reads_as_a_shell_reports_it() {
        use task_exec_wait_response::ExitStatus;
        assert!(matches!(exit_of(TaskExecWaitResponse { exit_status: Some(ExitStatus::Code(3)) }), ExecOutput::Exit(3)));
        assert!(matches!(exit_of(TaskExecWaitResponse { exit_status: Some(ExitStatus::Signal(9)) }), ExecOutput::Exit(137)));
        assert!(matches!(exit_of(TaskExecWaitResponse { exit_status: None }), ExecOutput::Error(_)));
    }

    #[test]
    fn task_ids_are_checked() {
        assert!(valid_task_id("ta-01M7XG2J6HN1M6KT5HC62DJFFS"));
        assert!(!valid_task_id("") && !valid_task_id("ta/1") && !valid_task_id(&"a".repeat(65)));
    }
}
