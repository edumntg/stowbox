//! Exec over the bridge's WebSocket at `/v1/boxes/{id}/exec`.
//!
//! The first message names the command and whether it gets a PTY. Then
//! input goes as binary messages and a resize or the end of input as a text
//! message. Output comes back as binary messages that lead with their
//! stream (1 stdout, 2 stderr), and the end is an `exit` text message with
//! the code.
//!
//! The bridge runs each command as the leader of a session of its own, and
//! ends that whole session when the socket closes or breaks. So the command
//! runs as plain `bash -lc`, without the process group marker SSH needs.

use super::api::Api;
use crate::sshexec::with_env;
use crate::types::*;
use crate::websocket::{self, Event, Message};
use crate::{Error, Result};
use serde_json::{json, Value};
use tokio::sync::mpsc;

const STDOUT: u8 = 1;
const STDERR: u8 = 2;

/// The first message on the socket.
pub fn start_message(spec: &ExecSpec) -> String {
    json!({
        "type": "start",
        "argv": ["bash", "-lc", with_env(spec)],
        "tty": spec.tty,
        "cols": spec.cols.max(20),
        "rows": spec.rows.max(5),
    })
    .to_string()
}

/// What one message from the bridge means, or nothing Stowbox uses.
pub fn output_of(ev: Event) -> Option<ExecOutput> {
    match ev {
        Event::Binary(b) => match b.split_first() {
            Some((&STDOUT, data)) => Some(ExecOutput::Stdout(data.to_vec())),
            Some((&STDERR, data)) => Some(ExecOutput::Stderr(data.to_vec())),
            _ => None,
        },
        Event::Text(t) => {
            let v: Value = serde_json::from_str(&t).ok()?;
            match v.get("type").and_then(Value::as_str)? {
                "exit" => Some(ExecOutput::Exit(v.get("code").and_then(Value::as_i64).and_then(|c| i32::try_from(c).ok()).unwrap_or(-1))),
                "error" => {
                    let why = v.get("message").and_then(Value::as_str).unwrap_or("no reason given");
                    Some(ExecOutput::Error(format!("the bridge ended the command: {}", why.chars().take(200).collect::<String>())))
                }
                _ => None,
            }
        }
        Event::Closed { code, .. } => Some(ExecOutput::Error(format!(
            "the bridge closed the command's socket without its exit{}",
            code.map(|c| format!(" (code {c})")).unwrap_or_default()
        ))),
    }
}

/// Runs `spec` over the socket at `path`, a checked `/v1/boxes/{id}/exec`.
pub async fn run(api: &Api, path: &str, spec: ExecSpec) -> Result<ExecHandle> {
    let url = api.ws_url(path);
    let mut ws = websocket::connect(&url, &api.header_pairs()).await?;
    ws.send.send(Message::Text(start_message(&spec))).await.map_err(|_| Error::Network("the bridge closed the command's socket before it started".into()))?;

    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    // The reader keeps its own sender: the socket stays open after the
    // input ends, until the command exits or the reader goes away.
    let input = ws.send.clone();
    tokio::spawn(async move {
        while let Some(msg) = in_rx.recv().await {
            let (m, last) = match msg {
                ExecInput::Data(d) => (Message::Binary(d), false),
                ExecInput::Resize { cols, rows } => (Message::Text(json!({ "type": "resize", "cols": cols.max(20), "rows": rows.max(5) }).to_string()), false),
                ExecInput::Close => (Message::Text(json!({ "type": "eof" }).to_string()), true),
            };
            if input.send(m).await.is_err() || last {
                return;
            }
        }
    });

    tokio::spawn(async move {
        let _keep_open = ws.send;
        loop {
            let ev = tokio::select! {
                // Dropping the socket makes the bridge end the session; the
                // live test checks that nothing is left.
                _ = out_tx.closed() => return,
                ev = ws.recv.recv() => ev,
            };
            let out = match ev {
                Some(Ok(ev)) => match output_of(ev) {
                    Some(o) => o,
                    None => continue,
                },
                Some(Err(e)) => ExecOutput::Error(e.to_string()),
                None => ExecOutput::Error("the bridge closed the command's socket".into()),
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
    fn the_start_names_a_login_shell_and_a_sane_size() {
        let spec = ExecSpec {
            script: "echo hi".into(),
            tty: true,
            cols: 0,
            rows: 0,
            env: vec![("A".into(), "x y".into()), ("1X".into(), "y".into()), ("B-C".into(), "z".into())],
        };
        let m: Value = serde_json::from_str(&start_message(&spec)).unwrap();
        assert_eq!(m["type"], "start");
        assert_eq!(m["argv"], json!(["bash", "-lc", "export A='x y' && echo hi"]));
        assert_eq!((m["tty"].as_bool(), m["cols"].as_u64(), m["rows"].as_u64()), (Some(true), Some(20), Some(5)));
    }

    #[test]
    fn messages_become_output() {
        assert!(matches!(output_of(Event::Binary(vec![1, b'h'])), Some(ExecOutput::Stdout(b)) if b == b"h"));
        assert!(matches!(output_of(Event::Binary(vec![2])), Some(ExecOutput::Stderr(b)) if b.is_empty()));
        assert!(output_of(Event::Binary(vec![3, 0])).is_none());
        assert!(output_of(Event::Binary(vec![])).is_none());
        assert!(matches!(output_of(Event::Text(r#"{"type":"exit","code":3}"#.into())), Some(ExecOutput::Exit(3))));
        assert!(matches!(output_of(Event::Text(r#"{"type":"exit","code":1e12}"#.into())), Some(ExecOutput::Exit(-1))));
        let Some(ExecOutput::Error(e)) = output_of(Event::Text(r#"{"type":"error","message":"no start"}"#.into())) else { panic!("not an error") };
        assert!(e.contains("no start"), "{e}");
        assert!(output_of(Event::Text("not json".into())).is_none());
        assert!(matches!(output_of(Event::Closed { code: Some(1011), reason: String::new() }), Some(ExecOutput::Error(e)) if e.contains("1011")));
    }
}
