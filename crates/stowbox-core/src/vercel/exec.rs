//! Exec in a Vercel sandbox session, two ways.
//!
//! Without a TTY, a command: `POST /v2/sandboxes/sessions/{sid}/cmd` with
//! `wait` streams NDJSON back, first the command and its id, then output
//! lines, last the command with its exit code. Stdin goes in order from one
//! task, one `POST …/cmd/{id}/stdin` per batch, and ends with `close`.
//!
//! A terminal goes over the session's interactive WebSocket instead, the
//! one Vercel's CLI uses: a `start` text message names the command and the
//! window size, output and input are binary messages, a resize is a text
//! message, and the end is an `exit` text message with the code.
//!
//! Both run the same command line SSH runs ([`remote_command`]), so a
//! command without a TTY reports its process group, and dropping the exec
//! kills the group as well as the shell.

use super::api::Api;
use super::convert::valid_name;
use crate::daytona::api::seg;
use crate::ndjson::Lines;
use crate::sshexec::{kill_group_command, remote_command, Marker};
use crate::types::*;
use crate::websocket::{self, Event, Message};
use crate::{Error, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// How long a command may take to be started and named.
const START: Duration = Duration::from_secs(60);
/// Most input bytes sent in one call; more waiting input goes in the next.
const MAX_INPUT_BATCH: usize = 64 << 10;
/// Ctrl-D: how a PTY is told its input ended.
const EOT: u8 = 0x04;

fn cmd_path(sid: &str, rest: &str) -> String {
    format!("/v2/sandboxes/sessions/{}/cmd{rest}", seg(sid))
}

/// What one streamed line means to the reader: output, the end, or nothing.
pub fn output_of(line: &Value) -> Option<ExecOutput> {
    if let Some(cmd) = line.get("command") {
        return cmd.get("exitCode").and_then(Value::as_i64).map(|c| ExecOutput::Exit(i32::try_from(c).unwrap_or(-1)));
    }
    let data = || line.get("data").and_then(Value::as_str).unwrap_or("").as_bytes().to_vec();
    match line.get("stream").and_then(Value::as_str)? {
        "stdout" => Some(ExecOutput::Stdout(data())),
        "stderr" => Some(ExecOutput::Stderr(data())),
        "error" => {
            let d = line.get("data");
            let why = d.and_then(|d| d.get("message")).or(d).and_then(Value::as_str).unwrap_or("no reason given");
            Some(ExecOutput::Error(format!("Vercel ended the command's output: {}", why.chars().take(200).collect::<String>())))
        }
        _ => None,
    }
}

/// A command without a TTY.
pub async fn command(api: Arc<Api>, sid: String, spec: ExecSpec) -> Result<ExecHandle> {
    let body = json!({
        "command": "/bin/sh",
        "args": ["-c", remote_command(&spec)],
        "env": {},
        "sudo": false,
        "wait": true,
        "attachStdin": true,
        "logs": true,
    });
    let resp = api.post_stream(&cmd_path(&sid, ""), body).await?;
    // Vercel sends no keepalives, and a quiet command is not a dead one.
    let mut lines = Lines::new(resp, None, "Vercel");
    let first = tokio::time::timeout(START, lines.next()).await.map_err(|_| Error::Network("Vercel did not start the command in time".into()))??;
    let id = first
        .as_ref()
        .and_then(|l| l.get("command"))
        .and_then(|c| c.get("id"))
        .and_then(Value::as_str)
        .filter(|id| valid_name(id))
        .ok_or_else(|| Error::Provider("Vercel started a command without naming it".into()))?
        .to_string();

    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    let input_api = api.clone();
    let stdin = cmd_path(&sid, &format!("/{}/stdin", seg(&id)));
    // Weak, so the output still ends when the reader task does.
    let input_errors = out_tx.downgrade();
    tokio::spawn(async move {
        let mut pending: Option<ExecInput> = None;
        loop {
            let msg = match pending.take() {
                Some(m) => m,
                None => match in_rx.recv().await {
                    Some(m) => m,
                    None => break,
                },
            };
            let ExecInput::Data(mut data) = msg else {
                // A command has no window to resize.
                if matches!(msg, ExecInput::Close) {
                    break;
                }
                continue;
            };
            // Keystrokes and upload chunks arrive in pieces; a call each would lag.
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
            let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
            if let Err(e) = input_api.post(&stdin, json!({ "data": encoded })).await {
                // The reader passes it on; the command may still be running.
                if let Some(tx) = input_errors.upgrade() {
                    let _ = tx.send(ExecOutput::Error(format!("input did not reach the sandbox: {e}"))).await;
                }
                return;
            }
        }
        let _ = input_api.post(&stdin, json!({ "close": true })).await;
    });

    tokio::spawn(async move {
        let mut marker = Marker::new(false);
        let mut pgid: Option<u32> = None;
        loop {
            let next = tokio::select! {
                _ = out_tx.closed() => {
                    kill(&api, &sid, &id, pgid).await;
                    return;
                }
                l = lines.next() => l,
            };
            let out = match next {
                Ok(Some(line)) => match output_of(&line) {
                    Some(ExecOutput::Stderr(data)) => {
                        let (found, rest) = marker.feed(&data);
                        pgid = pgid.or(found);
                        rest.filter(|r| !r.is_empty()).map(ExecOutput::Stderr)
                    }
                    other => other,
                },
                Ok(None) => Some(ExecOutput::Error("Vercel ended the stream without the command's exit".into())),
                Err(e) => Some(ExecOutput::Error(e.to_string())),
            };
            let Some(o) = out else { continue };
            let last = matches!(o, ExecOutput::Exit(_) | ExecOutput::Error(_));
            if out_tx.send(o).await.is_err() {
                if !last {
                    kill(&api, &sid, &id, pgid).await;
                }
                return;
            }
            if last {
                return;
            }
        }
    });

    Ok(ExecHandle { input: in_tx, output: out_rx })
}

/// Ends a command and what it left running: the process group from a
/// second, short command, then SIGKILL to the command itself.
async fn kill(api: &Api, sid: &str, id: &str, pgid: Option<u32>) {
    if let Some(cmd) = pgid.and_then(kill_group_command) {
        let body = json!({ "command": "/bin/sh", "args": ["-c", cmd], "env": {}, "wait": true });
        if let Ok(resp) = api.post_stream(&cmd_path(sid, ""), body).await {
            let mut lines = Lines::new(resp, Some(Duration::from_secs(5)), "Vercel");
            let _ = tokio::time::timeout(Duration::from_secs(5), async { while let Ok(Some(_)) = lines.next().await {} }).await;
        }
    }
    let _ = api.post(&cmd_path(sid, &format!("/{}/kill", seg(id))), json!({ "signal": 9 })).await;
}

/// The first message on the interactive socket.
pub fn start_message(spec: &ExecSpec, cwd: &str) -> String {
    let mut m = json!({
        "type": "start",
        "command": "/bin/sh",
        "args": ["-c", remote_command(spec)],
        "env": ["TERM=xterm-256color"],
        "cols": spec.cols.max(20),
        "rows": spec.rows.max(5),
    });
    if !cwd.is_empty() {
        if let Some(o) = m.as_object_mut() {
            o.insert("cwd".into(), json!(cwd));
        }
    }
    m.to_string()
}

/// What a text message on the interactive socket means: the exit, or
/// nothing Stowbox uses.
pub fn exit_of(text: &str) -> Option<i32> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("exit") {
        return None;
    }
    Some(v.get("code").and_then(Value::as_i64).and_then(|c| i32::try_from(c).ok()).unwrap_or(-1))
}

/// A terminal over the session's interactive WebSocket.
pub async fn terminal(api: &Api, http: &reqwest::Client, sid: &str, cwd: &str, spec: ExecSpec) -> Result<ExecHandle> {
    let inter = api.post(&format!("/v2/sandboxes/sessions/{}/interactive", seg(sid)), json!({})).await?;
    let url = inter.get("url").and_then(Value::as_str).unwrap_or("");
    let token = inter.get("token").and_then(Value::as_str).unwrap_or("");
    let (host, _, _) = websocket::parse_url(url)?;
    // The PTY server comes up with the session; until then the socket is
    // refused. Its health check says when, as the CLI does it.
    for wait in [0u64, 100, 200, 400, 800, 1600] {
        tokio::time::sleep(Duration::from_millis(wait)).await;
        let ok = http.get(format!("https://{host}/health")).timeout(Duration::from_secs(5)).send().await.map(|r| r.status().is_success());
        if matches!(ok, Ok(true)) {
            break;
        }
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    let mut ws = websocket::connect(&format!("{url}{sep}token={}", seg(token)), &[]).await?;
    ws.send.send(Message::Text(start_message(&spec, cwd))).await.map_err(|_| Error::Network("the Vercel terminal closed before it started".into()))?;

    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    // The reader keeps its own sender: the socket stays open after the
    // input ends, until the command exits or the reader goes away.
    let input = ws.send.clone();
    tokio::spawn(async move {
        while let Some(msg) = in_rx.recv().await {
            let m = match msg {
                ExecInput::Data(d) => Message::Binary(d),
                ExecInput::Resize { cols, rows } => Message::Text(json!({ "type": "resize", "cols": cols.max(20), "rows": rows.max(5) }).to_string()),
                ExecInput::Close => break,
            };
            if input.send(m).await.is_err() {
                return;
            }
        }
        let _ = input.send(Message::Binary(vec![EOT])).await;
    });

    tokio::spawn(async move {
        let _keep_open = ws.send;
        loop {
            let ev = tokio::select! {
                _ = out_tx.closed() => return,
                ev = ws.recv.recv() => ev,
            };
            let out = match ev {
                Some(Ok(Event::Binary(b))) => ExecOutput::Stdout(b),
                Some(Ok(Event::Text(t))) => match exit_of(&t) {
                    Some(code) => ExecOutput::Exit(code),
                    None => continue,
                },
                Some(Ok(Event::Closed { code, .. })) => ExecOutput::Error(format!(
                    "the Vercel terminal closed without the command's exit{}",
                    code.map(|c| format!(" (code {c})")).unwrap_or_default()
                )),
                Some(Err(e)) => ExecOutput::Error(e.to_string()),
                None => ExecOutput::Error("the Vercel terminal closed".into()),
            };
            let last = matches!(out, ExecOutput::Exit(_) | ExecOutput::Error(_));
            if out_tx.send(out).await.is_err() || last {
                return;
            }
        }
    });

    Ok(ExecHandle { input: in_tx, output: out_rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_lines_become_output() {
        assert!(matches!(output_of(&json!({ "stream": "stdout", "data": "hi\n" })), Some(ExecOutput::Stdout(b)) if b == b"hi\n"));
        assert!(matches!(output_of(&json!({ "stream": "stderr", "data": "x" })), Some(ExecOutput::Stderr(b)) if b == b"x"));
        assert!(matches!(output_of(&json!({ "command": { "id": "cmd_1", "exitCode": 3 } })), Some(ExecOutput::Exit(3))));
        assert!(output_of(&json!({ "command": { "id": "cmd_1", "exitCode": null } })).is_none());
        assert!(output_of(&json!({ "command": { "exitCode": 1e12 } })).is_none());
        let Some(ExecOutput::Error(e)) = output_of(&json!({ "stream": "error", "data": { "code": "x", "message": "gone" } })) else { panic!("not an error") };
        assert!(e.contains("gone"), "{e}");
        assert!(output_of(&json!({ "stream": "other" })).is_none());
        assert!(output_of(&json!(42)).is_none());
    }

    #[test]
    fn the_terminal_starts_the_ssh_command_line_with_a_sane_size() {
        let spec = ExecSpec { script: "echo hi".into(), tty: true, cols: 0, rows: 0, env: vec![] };
        let m: Value = serde_json::from_str(&start_message(&spec, "/vercel/sandbox")).unwrap();
        assert_eq!(m["type"], "start");
        assert_eq!(m["command"], "/bin/sh");
        assert_eq!(m["args"][1], remote_command(&spec));
        assert_eq!((m["cols"].as_u64(), m["rows"].as_u64()), (Some(20), Some(5)));
        assert_eq!(m["cwd"], "/vercel/sandbox");
        let m: Value = serde_json::from_str(&start_message(&spec, "")).unwrap();
        assert!(m.get("cwd").is_none());
    }

    #[test]
    fn only_an_exit_message_ends_the_terminal() {
        assert_eq!(exit_of(r#"{"type":"exit","code":3}"#), Some(3));
        assert_eq!(exit_of(r#"{"type":"exit"}"#), Some(-1));
        assert_eq!(exit_of(r#"{"type":"ready"}"#), None);
        assert_eq!(exit_of("not json"), None);
    }
}
