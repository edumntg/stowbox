//! Exec in a Blaxel sandbox through its own API, two ways.
//!
//! Without a TTY, a process: `POST /process` starts the command with a
//! stdin pipe, `GET /process/{name}/logs/stream` streams its output as
//! NDJSON (a keepalive every 30 seconds), and `GET /process/{name}/wait`
//! gives the exit code once the stream ends. Stdin is `POST
//! /process/{name}/stdin` with the raw bytes, in order from one task, and
//! `DELETE` on the same path closes it.
//!
//! A terminal goes over `/terminal/ws`, a PTY whose program is given as a
//! path with no arguments. Stowbox therefore first writes a short script
//! (mode 700, in /tmp, removed by itself) that records its pid, runs the
//! command line and records the exit code, and opens the terminal on it.
//! Messages are JSON text: `input` and `resize` out, `output` in; the
//! socket closes when the program exits, and the code is read back after.
//!
//! Both run the same command line SSH runs ([`remote_command`]).

use super::api::{Api, Body};
use crate::boxd::shell_quote;
use crate::daytona::api::seg;
use crate::ndjson::Lines;
use crate::sshexec::{kill_group_command, remote_command, Marker};
use crate::types::*;
use crate::websocket::{self, Event, Message};
use crate::{Error, Result};
use base64::Engine;
use reqwest::Method;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Blaxel's log stream sends a keepalive every 30 seconds.
const IDLE: Duration = Duration::from_secs(90);
/// Most input bytes sent in one call; more waiting input goes in the next.
const MAX_INPUT_BATCH: usize = 64 << 10;
/// Ctrl-D: how a PTY is told its input ended.
const EOT: &str = "\u{4}";

fn new_tag() -> String {
    uuid::Uuid::new_v4().simple().to_string().chars().take(16).collect()
}

/// What one streamed log event means to the reader: output, an error, or
/// nothing. Output that is not valid UTF-8 comes base64-encoded.
pub fn output_of(event: &Value) -> Option<ExecOutput> {
    let data = || {
        let d = event.get("data").and_then(Value::as_str).unwrap_or("");
        if event.get("encoding").and_then(Value::as_str) == Some("base64") {
            base64::engine::general_purpose::STANDARD.decode(d).unwrap_or_default()
        } else {
            d.as_bytes().to_vec()
        }
    };
    match event.get("type").and_then(Value::as_str)? {
        "stdout" => Some(ExecOutput::Stdout(data())),
        "stderr" => Some(ExecOutput::Stderr(data())),
        "error" => {
            Some(ExecOutput::Error(format!("Blaxel ended the command's output: {}", String::from_utf8_lossy(&data()).chars().take(200).collect::<String>())))
        }
        _ => None,
    }
}

/// Runs a short command to its end and returns its stdout and exit code.
async fn run(api: &Api, base: &str, command: String) -> Result<(String, i64)> {
    let r = api.post(&format!("{base}/process"), json!({ "command": command, "waitForCompletion": true })).await?;
    Ok((r.get("stdout").and_then(Value::as_str).unwrap_or("").to_string(), r.get("exitCode").and_then(Value::as_i64).unwrap_or(-1)))
}

/// A command without a TTY.
pub async fn command(api: Arc<Api>, base: String, spec: ExecSpec) -> Result<ExecHandle> {
    let name = format!("stowbox-{}", new_tag());
    api.post(&format!("{base}/process"), json!({ "command": remote_command(&spec), "name": name, "stdin": true })).await?;
    let proc = format!("{base}/process/{}", seg(&name));
    let mut lines = Lines::new(api.get_stream(&format!("{proc}/logs/stream")).await?, Some(IDLE), "Blaxel");

    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    let input_api = api.clone();
    let stdin = format!("{proc}/stdin");
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
            if let Err(e) = input_api.call(Method::POST, &stdin, Body::Bytes(data)).await {
                // The reader passes it on; the command may still be running.
                if let Some(tx) = input_errors.upgrade() {
                    let _ = tx.send(ExecOutput::Error(format!("input did not reach the sandbox: {e}"))).await;
                }
                return;
            }
        }
        let _ = input_api.delete(&stdin).await;
    });

    tokio::spawn(async move {
        let mut marker = Marker::new(false);
        let mut pgid: Option<u32> = None;
        loop {
            let next = tokio::select! {
                _ = out_tx.closed() => {
                    kill(&api, &base, &proc, pgid).await;
                    return;
                }
                l = lines.next() => l,
            };
            let out = match next {
                Ok(Some(event)) => match output_of(&event) {
                    Some(ExecOutput::Stderr(data)) => {
                        let (found, rest) = marker.feed(&data);
                        pgid = pgid.or(found);
                        rest.filter(|r| !r.is_empty()).map(ExecOutput::Stderr)
                    }
                    other => other,
                },
                // The stream ends when the process does; ask how.
                Ok(None) => Some(match api.get(&format!("{proc}/wait?timeout=30")).await {
                    Ok(p) => match p.get("exitCode").and_then(Value::as_i64) {
                        Some(c) if p.get("status").and_then(Value::as_str) != Some("running") => ExecOutput::Exit(i32::try_from(c).unwrap_or(-1)),
                        _ => ExecOutput::Error("Blaxel ended the output before the command ended".into()),
                    },
                    Err(e) => ExecOutput::Error(format!("the command's exit code could not be read: {e}")),
                }),
                Err(e) => Some(ExecOutput::Error(e.to_string())),
            };
            let Some(o) = out else { continue };
            let ended = matches!(o, ExecOutput::Exit(_));
            // A broken stream leaves the command running with nothing to
            // read it, so it is ended as if the reader had gone.
            let broke = matches!(o, ExecOutput::Error(_));
            let delivered = out_tx.send(o).await.is_ok();
            if broke || (!ended && !delivered) {
                kill(&api, &base, &proc, pgid).await;
            }
            if ended || broke || !delivered {
                return;
            }
        }
    });

    Ok(ExecHandle { input: in_tx, output: out_rx })
}

/// Ends a process and what it left running: the process group from a
/// second, short process, then a kill of the process itself.
async fn kill(api: &Api, base: &str, proc: &str, pgid: Option<u32>) {
    if let Some(cmd) = pgid.and_then(kill_group_command) {
        let _ = run(api, base, cmd).await;
    }
    let _ = api.delete(&format!("{proc}/kill")).await;
}

/// The script a terminal runs: it removes itself, records its pid so a
/// dropped terminal can be hung up, and records the command's exit code.
pub fn terminal_script(spec: &ExecSpec) -> String {
    format!("#!/bin/sh\nrm -f \"$0\"\necho $$ > \"$0.pid\"\n/bin/sh -c {}\nc=$?\necho $c > \"$0.rc\"\nexit $c\n", shell_quote(&remote_command(spec)))
}

/// A message from the terminal: output, or nothing Stowbox uses.
pub fn terminal_output(text: &str) -> Option<ExecOutput> {
    let v: Value = serde_json::from_str(text).ok()?;
    let data = v.get("data").and_then(Value::as_str).unwrap_or("");
    match v.get("type").and_then(Value::as_str)? {
        "output" => Some(ExecOutput::Stdout(data.as_bytes().to_vec())),
        "error" => Some(ExecOutput::Error(format!("the Blaxel terminal failed: {}", data.chars().take(200).collect::<String>()))),
        _ => None,
    }
}

/// A terminal over the sandbox's `/terminal/ws`.
pub async fn terminal(api: Arc<Api>, base: String, spec: ExecSpec) -> Result<ExecHandle> {
    let tag = new_tag();
    let path = format!("/tmp/.stowbox-tty-{tag}");
    let (_, code) = run(&api, &base, format!("umask 077 && printf '%s' {} > {path} && chmod 700 {path}", shell_quote(&terminal_script(&spec)))).await?;
    if code != 0 {
        return Err(Error::Provider(format!("the Blaxel sandbox could not prepare the terminal (exit {code})")));
    }
    let wss = base.replacen("https://", "wss://", 1);
    let url = format!("{wss}/terminal/ws?sessionId={tag}&cols={}&rows={}&shell={}", spec.cols.max(20), spec.rows.max(5), seg(&path));
    let mut ws = websocket::connect(&url, &api.header_pairs()).await?;

    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    // The reader keeps its own sender: the socket stays open after the
    // input ends, until the program exits or the reader goes away.
    let input = ws.send.clone();
    tokio::spawn(async move {
        while let Some(msg) = in_rx.recv().await {
            let m = match msg {
                ExecInput::Data(d) => json!({ "type": "input", "data": String::from_utf8_lossy(&d) }),
                ExecInput::Resize { cols, rows } => json!({ "type": "resize", "cols": cols.max(20), "rows": rows.max(5) }),
                ExecInput::Close => break,
            };
            if input.send(Message::Text(m.to_string())).await.is_err() {
                return;
            }
        }
        let _ = input.send(Message::Text(json!({ "type": "input", "data": EOT }).to_string())).await;
    });

    tokio::spawn(async move {
        let _keep_open = ws.send;
        loop {
            let ev = tokio::select! {
                _ = out_tx.closed() => {
                    // Hang up what the terminal runs; Blaxel would keep the
                    // session ten minutes for a reconnect. A pid file that is
                    // empty or names init must not turn into `kill -1`.
                    let hang_up = format!("p=$(cat {path}.pid 2>/dev/null); [ \"$p\" -gt 1 ] 2>/dev/null && kill -HUP -\"$p\" \"$p\" 2>/dev/null; rm -f {path}.pid {path}.rc");
                    let _ = run(&api, &base, hang_up).await;
                    return;
                }
                ev = ws.recv.recv() => ev,
            };
            let out = match ev {
                Some(Ok(Event::Text(t))) => match terminal_output(&t) {
                    Some(o) => o,
                    None => continue,
                },
                Some(Ok(Event::Binary(b))) => ExecOutput::Stdout(b),
                Some(Ok(Event::Closed { .. })) | None => match run(&api, &base, format!("cat {path}.rc 2>/dev/null; rm -f {path}.pid {path}.rc")).await {
                    Ok((rc, _)) => match rc.trim().parse::<i32>() {
                        Ok(code) => ExecOutput::Exit(code),
                        Err(_) => ExecOutput::Error("the Blaxel terminal closed before the command ended".into()),
                    },
                    Err(e) => ExecOutput::Error(format!("the command's exit code could not be read: {e}")),
                },
                Some(Err(e)) => ExecOutput::Error(e.to_string()),
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
    fn log_events_become_output() {
        assert!(matches!(output_of(&json!({ "type": "stdout", "data": "hi\n" })), Some(ExecOutput::Stdout(b)) if b == b"hi\n"));
        assert!(matches!(output_of(&json!({ "type": "stderr", "data": "/w==", "encoding": "base64" })), Some(ExecOutput::Stderr(b)) if b == [0xff]));
        assert!(matches!(output_of(&json!({ "type": "error", "data": "gone" })), Some(ExecOutput::Error(e)) if e.contains("gone")));
        assert!(output_of(&json!({ "type": "keepalive" })).is_none());
        assert!(output_of(&json!({ "type": "truncated" })).is_none());
        assert!(output_of(&json!(1)).is_none());
    }

    #[test]
    fn the_terminal_script_quotes_the_command_line() {
        let spec = ExecSpec { script: "echo 'it''s'; exit 3".into(), tty: true, cols: 80, rows: 24, env: vec![] };
        let s = terminal_script(&spec);
        assert!(s.starts_with("#!/bin/sh\nrm -f \"$0\"\n"), "{s}");
        assert!(s.contains(&shell_quote(&remote_command(&spec))), "{s}");
        assert!(s.ends_with("exit $c\n"));
    }

    #[test]
    fn terminal_messages_become_output() {
        assert!(matches!(terminal_output(r#"{"type":"output","data":"$ "}"#), Some(ExecOutput::Stdout(b)) if b == b"$ "));
        assert!(matches!(terminal_output(r#"{"type":"error","data":"no shell"}"#), Some(ExecOutput::Error(_))));
        assert!(terminal_output(r#"{"type":"exit"}"#).is_none());
        assert!(terminal_output("nope").is_none());
    }
}
