//! Exec in a sandbox through envd, the agent E2B runs inside every sandbox,
//! over its Connect API at `https://49983-{id}.{domain}`.
//!
//! `process.Process/Start` starts `bash -lc <script>` (with a PTY for a
//! terminal) and streams its events: the pid, output chunks, the end with
//! its exit code. Input goes as one `SendInput` call per batch, made in
//! order from a single task; a resize is an `Update`. Dropping the output
//! receiver kills the process, as with SSH; dropping the input sender
//! closes stdin.
//!
//! envd takes the sandbox's access token in `X-Access-Token` and the Linux
//! user as HTTP basic auth with no password.

use super::proto;
use crate::connectrpc::{Client, Stream};
use crate::sshexec::{kill_group_command, remote_command, Marker};
use crate::types::*;
use crate::{Error, Result};
use base64::Engine;
use proto::process_event::{data_event::Output, Event};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

pub const PORT: u16 = 49983;
/// The user commands run as; E2B's images give it passwordless sudo.
pub const USER: &str = "user";
/// envd sends a keepalive this often when asked, so a quiet terminal is
/// told apart from a dead one.
const KEEPALIVE_SECS: u32 = 30;
const IDLE: Duration = Duration::from_secs(90);
/// How long the process may take to report its pid.
const START: Duration = Duration::from_secs(30);
/// Most input bytes sent in one call; more waiting input goes in the next.
const MAX_INPUT_BATCH: usize = 64 << 10;
/// Ctrl-D: how a PTY is told its input ended.
const EOT: u8 = 0x04;

pub fn client(http: reqwest::Client, base: &str, access_token: &str) -> Result<Client> {
    let basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{USER}:")));
    let mut headers = vec![("authorization", basic)];
    if !access_token.is_empty() {
        headers.push(("x-access-token", access_token.to_string()));
    }
    Client::new(http, base, &headers, "E2B envd")
}

/// What envd is asked to start for `spec`: the same command line SSH runs
/// ([`remote_command`]), so the environment, the login shell and, without a
/// TTY, the process group and its marker on stderr all match.
pub fn start_request(spec: &ExecSpec) -> proto::StartRequest {
    let mut envs = HashMap::new();
    let pty = spec.tty.then(|| {
        envs.insert("TERM".to_string(), "xterm-256color".to_string());
        proto::Pty { size: Some(proto::pty::Size { cols: u32::from(spec.cols.max(20)), rows: u32::from(spec.rows.max(5)) }) }
    });
    proto::StartRequest {
        process: Some(proto::ProcessConfig { cmd: "/bin/sh".into(), args: vec!["-c".into(), remote_command(spec)], envs, cwd: None }),
        pty,
        tag: None,
        // Without it a non-TTY process has no stdin to close.
        stdin: Some(!spec.tty),
    }
}

fn selector(pid: u32) -> Option<proto::ProcessSelector> {
    Some(proto::ProcessSelector { selector: Some(proto::process_selector::Selector::Pid(pid)) })
}

/// What one stream event means to the reader: output, the end, or nothing.
pub fn output_of(event: proto::ProcessEvent) -> Option<ExecOutput> {
    match event.event? {
        Event::Data(d) => match d.output? {
            Output::Stdout(b) | Output::Pty(b) => Some(ExecOutput::Stdout(b)),
            Output::Stderr(b) => Some(ExecOutput::Stderr(b)),
        },
        Event::End(e) if e.exited => Some(ExecOutput::Exit(e.exit_code)),
        Event::End(e) => Some(ExecOutput::Error(format!(
            "the command ended without exiting: {}",
            e.error.filter(|s| !s.is_empty()).unwrap_or(e.status).chars().take(200).collect::<String>()
        ))),
        Event::Start(_) | Event::Keepalive(_) => None,
    }
}

pub async fn exec(rpc: Client, spec: ExecSpec) -> Result<ExecHandle> {
    let tty = spec.tty;
    let mut stream: Stream<proto::StartResponse> =
        rpc.server_stream("process.Process/Start", &start_request(&spec), &[("keepalive-ping-interval", KEEPALIVE_SECS.to_string())], IDLE).await?;
    let pid = tokio::time::timeout(START, async {
        loop {
            match stream.next().await? {
                Some(proto::StartResponse { event: Some(proto::ProcessEvent { event: Some(Event::Start(s)) }) }) => return Ok(s.pid),
                Some(_) => continue,
                None => return Err(Error::Provider("envd ended the command before it started".into())),
            }
        }
    })
    .await
    .map_err(|_| Error::Network("envd did not start the command in time".into()))??;

    let rpc = std::sync::Arc::new(rpc);
    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    let input_rpc = rpc.clone();
    // Weak, so the output still ends when the reader task does.
    let input_errors = out_tx.downgrade();
    tokio::spawn(async move {
        let send = |bytes: Vec<u8>| {
            let input = if tty { proto::process_input::Input::Pty(bytes) } else { proto::process_input::Input::Stdin(bytes) };
            proto::SendInputRequest { process: selector(pid), input: Some(proto::ProcessInput { input: Some(input) }) }
        };
        let close = || async {
            if tty {
                let _ = input_rpc.unary::<_, proto::SendInputResponse>("process.Process/SendInput", &send(vec![EOT])).await;
            } else {
                let _ =
                    input_rpc.unary::<_, proto::CloseStdinResponse>("process.Process/CloseStdin", &proto::CloseStdinRequest { process: selector(pid) }).await;
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
                    input_rpc.unary::<_, proto::SendInputResponse>("process.Process/SendInput", &send(data)).await.map(drop)
                }
                ExecInput::Resize { cols, rows } => {
                    let pty = proto::Pty { size: Some(proto::pty::Size { cols: u32::from(cols.max(20)), rows: u32::from(rows.max(5)) }) };
                    input_rpc
                        .unary::<_, proto::UpdateResponse>("process.Process/Update", &proto::UpdateRequest { process: selector(pid), pty: Some(pty) })
                        .await
                        .map(drop)
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
        close().await;
    });

    tokio::spawn(async move {
        let mut marker = Marker::new(tty);
        let mut pgid: Option<u32> = None;
        loop {
            let next = tokio::select! {
                _ = out_tx.closed() => {
                    kill(&rpc, pid, pgid).await;
                    return;
                }
                m = stream.next() => m,
            };
            let out = match next {
                Ok(Some(r)) => match r.event.and_then(output_of) {
                    Some(ExecOutput::Stderr(data)) => {
                        let (found, rest) = marker.feed(&data);
                        pgid = pgid.or(found);
                        rest.filter(|r| !r.is_empty()).map(ExecOutput::Stderr)
                    }
                    other => other,
                },
                Ok(None) => Some(ExecOutput::Error("envd ended the stream without the command's exit".into())),
                Err(e) => Some(ExecOutput::Error(e.to_string())),
            };
            let Some(o) = out else { continue };
            let ended = matches!(o, ExecOutput::Exit(_));
            // A broken stream leaves the command running with nothing to
            // read it, so it is ended as if the reader had gone.
            let broke = matches!(o, ExecOutput::Error(_));
            let delivered = out_tx.send(o).await.is_ok();
            if broke || (!ended && !delivered) {
                kill(&rpc, pid, pgid).await;
            }
            if ended || broke || !delivered {
                return;
            }
        }
    });

    Ok(ExecHandle { input: in_tx, output: out_rx })
}

/// Ends the command and what it left running. SIGKILL to the pid stops
/// only the shell envd started, so a command without a TTY, which runs in a
/// process group of its own, also gets the group killed by a second,
/// short-lived process.
async fn kill(rpc: &Client, pid: u32, pgid: Option<u32>) {
    if let Some(cmd) = pgid.and_then(kill_group_command) {
        let req = proto::StartRequest {
            process: Some(proto::ProcessConfig { cmd: "/bin/sh".into(), args: vec!["-c".into(), cmd], envs: HashMap::new(), cwd: None }),
            pty: None,
            tag: None,
            stdin: Some(false),
        };
        // Read to its end (about a second): envd may end a process whose
        // stream goes away, which would skip the KILL after the TERM.
        if let Ok(mut s) = rpc.server_stream::<_, proto::StartResponse>("process.Process/Start", &req, &[], START).await {
            let _ = tokio::time::timeout(Duration::from_secs(5), async { while let Ok(Some(_)) = s.next().await {} }).await;
        }
    }
    let req = proto::SendSignalRequest { process: selector(pid), signal: proto::Signal::Sigkill as i32 };
    let _ = rpc.unary::<_, proto::SendSignalResponse>("process.Process/SendSignal", &req).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_gets_a_pty_and_a_script_gets_stdin() {
        let spec = ExecSpec { script: "echo hi".into(), tty: true, cols: 100, rows: 30, env: vec![("A".into(), "1".into()), ("BAD KEY".into(), "x".into())] };
        let tty = start_request(&spec);
        let p = tty.process.unwrap();
        // The same line SSH runs, environment and all.
        assert_eq!((p.cmd.as_str(), p.args), ("/bin/sh", vec!["-c".to_string(), remote_command(&spec)]));
        assert_eq!(p.envs.get("TERM").map(String::as_str), Some("xterm-256color"));
        let size = tty.pty.unwrap().size.unwrap();
        assert_eq!((size.cols, size.rows, tty.stdin), (100, 30, Some(false)));

        let plain = start_request(&ExecSpec { script: "cat".into(), tty: false, cols: 0, rows: 0, env: vec![] });
        assert!(plain.pty.is_none());
        assert_eq!(plain.stdin, Some(true));
        let p = plain.process.unwrap();
        assert!(p.envs.is_empty());
        assert!(p.args[1].contains("stowbox-pgid"), "a script reports its process group");
    }

    #[test]
    fn a_tiny_window_is_raised_to_a_usable_one() {
        let r = start_request(&ExecSpec { script: String::new(), tty: true, cols: 0, rows: 0, env: vec![] });
        let s = r.pty.unwrap().size.unwrap();
        assert_eq!((s.cols, s.rows), (20, 5));
    }

    #[test]
    fn events_map_to_exec_output() {
        let data = |o| proto::ProcessEvent { event: Some(Event::Data(proto::process_event::DataEvent { output: Some(o) })) };
        assert!(matches!(output_of(data(Output::Pty(b"x".to_vec()))), Some(ExecOutput::Stdout(b)) if b == b"x"));
        assert!(matches!(output_of(data(Output::Stderr(b"e".to_vec()))), Some(ExecOutput::Stderr(_))));
        let end = |exited, code| proto::ProcessEvent {
            event: Some(Event::End(proto::process_event::EndEvent { exit_code: code, exited, status: "exit status 3".into(), error: None })),
        };
        assert!(matches!(output_of(end(true, 3)), Some(ExecOutput::Exit(3))));
        assert!(matches!(output_of(end(false, 0)), Some(ExecOutput::Error(m)) if m.contains("exit status 3")));
        assert!(output_of(proto::ProcessEvent { event: Some(Event::Keepalive(proto::process_event::KeepAlive {})) }).is_none());
        assert!(output_of(proto::ProcessEvent { event: None }).is_none());
    }
}
