//! Exec over Sprites' WebSocket at `/v1/sprites/{name}/exec`.
//!
//! The program is the `cmd` query parameters, one per argument. Without a
//! TTY every binary message leads with its stream: 0 stdin, 1 stdout,
//! 2 stderr, 3 the exit code (one byte), 4 the end of stdin. With a TTY
//! binary messages are the terminal's raw bytes both ways and a resize is a
//! JSON text message. Text messages from Sprites are JSON: `session_info`
//! names the session, `exit` carries `exit_code`, the rest is ignored.
//!
//! The command line itself never goes in the URL: it can hold an agent's
//! prompt and run to many kilobytes. A short first exec writes it to a
//! private file in `/tmp` from its stdin, and the real exec runs a fixed
//! wrapper that reads the file, removes it and runs it. Only fixed
//! wrappers, file paths and process group numbers reach the URL.
//!
//! Sprites keeps a command running after its socket closes, so a socket
//! Stowbox drops or loses ends what it started itself: the process group a
//! command reports on stderr ([`remote_command`]), or the group a terminal
//! wrote to a pid file, and then the session through Sprites' kill call.

use super::api::Api;
use crate::daytona::api::seg;
use crate::sshexec::{kill_group_command, remote_command, with_env, Marker};
use crate::types::*;
use crate::websocket::{self, Event, Message, Socket};
use crate::{Error, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const STDIN: u8 = 0;
const STDOUT: u8 = 1;
const STDERR: u8 = 2;
const EXIT: u8 = 3;
const STDIN_EOF: u8 = 4;
/// Ctrl-D: how a PTY is told its input ended.
const EOT: u8 = 4;
/// Largest message of input sent; more goes in the next one.
const CHUNK: usize = 64 << 10;
/// How long a short helper command may take.
const SHORT: Duration = Duration::from_secs(30);

/// Writes stdin to the file named by `$1`, readable only by the user.
const STAGE: &str = "umask 077 && cat > \"$1\"";
/// Runs the command line staged in `$1`, removing it first.
const RUN_STAGED: &str = "c=$(cat \"$1\") && rm -f \"$1\" && eval \"$c\"";
/// A terminal: records its pid, which leads the terminal's process group,
/// so a dropped terminal can be hung up; runs the script staged in `$2`;
/// removes the record when the script ends.
const TERMINAL: &str = "umask 077; echo $$ > \"$1\"; s=$(cat \"$2\"); rm -f \"$2\"; bash -lc \"$s\"; c=$?; rm -f \"$1\"; exit $c";
/// Hangs up the group a terminal recorded in `$1`. A pid file that is empty
/// or names init must not turn into `kill -1`.
const HANG_UP: &str =
    "p=$(cat \"$1\" 2>/dev/null); rm -f \"$1\"; [ \"$p\" -gt 1 ] 2>/dev/null || exit 0; kill -HUP -\"$p\" \"$p\" 2>/dev/null; sleep 1; kill -KILL -\"$p\" 2>/dev/null; true";

/// What one message from Sprites means.
pub enum Frame {
    Output(ExecOutput),
    /// The session's id, for the kill call.
    Session(String),
    Nothing,
}

/// A session id from Sprites, before it goes into a path.
fn valid_session(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn frame_of(ev: Event, tty: bool) -> Frame {
    match ev {
        Event::Binary(b) if tty => Frame::Output(ExecOutput::Stdout(b)),
        Event::Binary(b) => match b.split_first() {
            Some((&STDOUT, data)) => Frame::Output(ExecOutput::Stdout(data.to_vec())),
            Some((&STDERR, data)) => Frame::Output(ExecOutput::Stderr(data.to_vec())),
            // An exit without its code byte is not taken as success.
            Some((&EXIT, code)) => Frame::Output(ExecOutput::Exit(code.first().copied().map(i32::from).unwrap_or(-1))),
            _ => Frame::Nothing,
        },
        Event::Text(t) => {
            let Ok(v) = serde_json::from_str::<Value>(&t) else { return Frame::Nothing };
            match v.get("type").and_then(Value::as_str) {
                Some("session_info") => match v.get("session_id") {
                    Some(Value::String(id)) if valid_session(id) => Frame::Session(id.clone()),
                    Some(Value::Number(n)) => n.as_u64().map(|n| Frame::Session(n.to_string())).unwrap_or(Frame::Nothing),
                    _ => Frame::Nothing,
                },
                Some("exit") => Frame::Output(ExecOutput::Exit(v.get("exit_code").and_then(Value::as_i64).and_then(|c| i32::try_from(c).ok()).unwrap_or(-1))),
                Some("error") => {
                    let why = v.get("error").or_else(|| v.get("message")).and_then(Value::as_str).unwrap_or("no reason given");
                    Frame::Output(ExecOutput::Error(format!("Sprites ended the command: {}", why.chars().take(200).collect::<String>())))
                }
                _ => Frame::Nothing,
            }
        }
        Event::Closed { code, .. } => Frame::Output(ExecOutput::Error(format!(
            "Sprites closed the command's socket without its exit{}",
            code.map(|c| format!(" (code {c})")).unwrap_or_default()
        ))),
    }
}

/// The exec path and query for `argv`.
pub fn exec_path(sprite: &str, argv: &[String], tty: Option<(u16, u16)>) -> String {
    let mut q = format!("/v1/sprites/{}/exec?path={}", seg(sprite), seg(argv.first().map(String::as_str).unwrap_or("sh")));
    for a in argv {
        q.push_str("&cmd=");
        q.push_str(&seg(a));
    }
    q.push_str("&stdin=true");
    if let Some((cols, rows)) = tty {
        q.push_str(&format!("&tty=true&cols={}&rows={}", cols.max(20), rows.max(5)));
    }
    q
}

/// `sh -c script stowbox args…`: the script is fixed, the arguments are
/// `$1` and on.
fn sh(script: &str, args: &[&str]) -> Vec<String> {
    ["sh", "-c", script, "stowbox"].iter().chain(args).map(|s| s.to_string()).collect()
}

/// A new private file name in `/tmp`.
fn temp_path(kind: &str) -> String {
    format!("/tmp/.stowbox-{kind}-{}", uuid::Uuid::new_v4().simple())
}

/// Input as stdin messages of at most [`CHUNK`] bytes.
fn stdin_messages(data: &[u8]) -> impl Iterator<Item = Message> + '_ {
    data.chunks(CHUNK).map(|c| Message::Binary([STDIN].iter().chain(c).copied().collect()))
}

async fn open(api: &Api, sprite: &str, argv: &[String], tty: Option<(u16, u16)>) -> Result<Socket> {
    websocket::connect(&api.ws_url(&exec_path(sprite, argv, tty)), &api.header_pairs()).await
}

/// Runs a short command without a TTY, with `input` as its stdin, and
/// returns its stdout and exit code.
async fn short(api: &Api, sprite: &str, argv: &[String], input: &[u8]) -> Result<(String, i32)> {
    let run = async {
        let mut ws = open(api, sprite, argv, None).await?;
        let gone = || Error::Network("Sprites closed a helper command's socket".into());
        for m in stdin_messages(input) {
            ws.send.send(m).await.map_err(|_| gone())?;
        }
        ws.send.send(Message::Binary(vec![STDIN_EOF])).await.map_err(|_| gone())?;
        let mut out = Vec::new();
        while let Some(ev) = ws.recv.recv().await {
            match frame_of(ev?, false) {
                Frame::Output(ExecOutput::Stdout(b)) if out.len() < 1 << 20 => out.extend(b),
                Frame::Output(ExecOutput::Exit(code)) => return Ok((String::from_utf8_lossy(&out).into_owned(), code)),
                Frame::Output(ExecOutput::Error(e)) => return Err(Error::Network(e)),
                _ => {}
            }
        }
        Err(gone())
    };
    tokio::time::timeout(SHORT, run).await.map_err(|_| Error::Network("a helper command in the sprite did not finish in time".into()))?
}

/// Writes `content` to a new private file in the sprite and returns its path.
async fn stage(api: &Api, sprite: &str, content: &str) -> Result<String> {
    let path = temp_path("cmd");
    match short(api, sprite, &sh(STAGE, &[&path]), content.as_bytes()).await? {
        (_, 0) => Ok(path),
        (_, code) => Err(Error::Provider(format!("the sprite could not take the command (exit {code})"))),
    }
}

/// What a command left to end it by.
#[derive(Default)]
struct Ends {
    session: Option<String>,
    pgid: Option<u32>,
    pid_file: Option<String>,
}

impl Ends {
    /// Ends the command and what it left running, best effort.
    async fn run(&self, api: &Api, sprite: &str) {
        if let Some(cmd) = self.pgid.and_then(kill_group_command) {
            let _ = short(api, sprite, &sh(&cmd, &[]), &[]).await;
        }
        if let Some(f) = &self.pid_file {
            let _ = short(api, sprite, &sh(HANG_UP, &[f]), &[]).await;
        }
        if let Some(id) = &self.session {
            let _ = api.post_ignoring_body(&format!("/v1/sprites/{}/exec/{}/kill?signal=SIGKILL&timeout=2s", seg(sprite), seg(id))).await;
        }
    }
}

/// Runs `spec` in `sprite`, a checked name.
pub async fn run(api: Arc<Api>, sprite: String, spec: ExecSpec) -> Result<ExecHandle> {
    let tty = spec.tty;
    let pid_file = tty.then(|| temp_path("tty"));
    let argv = match &pid_file {
        Some(f) => sh(TERMINAL, &[f, &stage(&api, &sprite, &with_env(&spec)).await?]),
        None => sh(RUN_STAGED, &[&stage(&api, &sprite, &remote_command(&spec)).await?]),
    };
    let mut ws = open(&api, &sprite, &argv, tty.then_some((spec.cols, spec.rows))).await?;

    let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
    let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

    // The reader keeps its own sender: the socket stays open after the
    // input ends, until the command exits or the reader goes away.
    let input = ws.send.clone();
    tokio::spawn(async move {
        while let Some(msg) = in_rx.recv().await {
            let (messages, last): (Vec<Message>, bool) = match msg {
                ExecInput::Data(d) if tty => (d.chunks(CHUNK).map(|c| Message::Binary(c.to_vec())).collect(), false),
                ExecInput::Data(d) => (stdin_messages(&d).collect(), false),
                ExecInput::Resize { cols, rows } if tty => {
                    (vec![Message::Text(json!({ "type": "resize", "cols": cols.max(20), "rows": rows.max(5) }).to_string())], false)
                }
                // A command has no window to resize.
                ExecInput::Resize { .. } => continue,
                ExecInput::Close => (vec![Message::Binary(vec![if tty { EOT } else { STDIN_EOF }])], true),
            };
            for m in messages {
                if input.send(m).await.is_err() {
                    return;
                }
            }
            if last {
                return;
            }
        }
    });

    tokio::spawn(async move {
        let _keep_open = ws.send;
        let mut marker = Marker::new(tty);
        let mut ends = Ends { pid_file, ..Default::default() };
        loop {
            let ev = tokio::select! {
                _ = out_tx.closed() => {
                    ends.run(&api, &sprite).await;
                    return;
                }
                ev = ws.recv.recv() => ev,
            };
            let out = match ev {
                Some(Ok(ev)) => match frame_of(ev, tty) {
                    Frame::Output(ExecOutput::Stderr(data)) => {
                        let (found, rest) = marker.feed(&data);
                        ends.pgid = ends.pgid.or(found);
                        match rest.filter(|r| !r.is_empty()) {
                            Some(r) => ExecOutput::Stderr(r),
                            None => continue,
                        }
                    }
                    Frame::Output(o) => o,
                    Frame::Session(id) => {
                        ends.session = Some(id);
                        continue;
                    }
                    Frame::Nothing => continue,
                },
                Some(Err(e)) => ExecOutput::Error(e.to_string()),
                None => ExecOutput::Error("Sprites closed the command's socket".into()),
            };
            let ended = matches!(out, ExecOutput::Exit(_));
            // A broken socket leaves the command running with nothing to
            // read it, so it is ended as if the reader had gone.
            let broke = matches!(out, ExecOutput::Error(_));
            let delivered = out_tx.send(out).await.is_ok();
            if broke || (!ended && !delivered) {
                ends.run(&api, &sprite).await;
            }
            if ended || broke || !delivered {
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
    fn only_fixed_wrappers_and_paths_go_in_the_query() {
        let argv = sh(RUN_STAGED, &["/tmp/.stowbox-cmd-1"]);
        let path = exec_path("my-box", &argv, None);
        assert!(path.starts_with("/v1/sprites/my-box/exec?path=sh&cmd=sh&cmd=-c&cmd=c%3D%24%28cat"), "{path}");
        assert!(path.ends_with("&cmd=stowbox&cmd=%2Ftmp%2F.stowbox-cmd-1&stdin=true"), "{path}");
        assert!(exec_path("b", &argv, Some((0, 0))).ends_with("&stdin=true&tty=true&cols=20&rows=5"));
    }

    #[test]
    fn input_goes_in_chunks_with_its_stream_byte() {
        let data = vec![7u8; CHUNK * 2 + 1];
        let sizes: Vec<usize> = stdin_messages(&data)
            .map(|m| match m {
                Message::Binary(b) => {
                    assert_eq!(b.first(), Some(&STDIN));
                    b.len()
                }
                Message::Text(_) => 0,
            })
            .collect();
        assert_eq!(sizes, [CHUNK + 1, CHUNK + 1, 2]);
    }

    /// Runs one of the fixed wrappers under the local `sh`.
    fn local(script: &str, args: &[&str], stdin: &str) -> (String, i32) {
        use std::io::Write;
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg("stowbox")
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
    }

    #[test]
    fn the_wrappers_run_the_staged_command_and_clean_up() {
        let dir = crate::testing::temp_dir("sprites-wrappers");
        let staged = dir.join("my cmd");
        let staged = staged.to_str().unwrap();
        let spec = ExecSpec { script: "echo \"hi $A\"; exit 3".into(), tty: false, cols: 0, rows: 0, env: vec![("A".into(), "it's".into())] };
        assert_eq!(local(STAGE, &[staged], &remote_command(&spec)).1, 0);
        let (out, code) = local(RUN_STAGED, &[staged], "");
        assert_eq!((out.as_str(), code), ("hi it's\n", 3));
        assert!(!std::path::Path::new(staged).exists());

        let pid = dir.join("p id");
        let pid = pid.to_str().unwrap();
        local(STAGE, &[staged], &with_env(&spec));
        let (out, code) = local(TERMINAL, &[pid, staged], "");
        assert_eq!((out.as_str(), code), ("hi it's\n", 3));
        assert!(!std::path::Path::new(staged).exists() && !std::path::Path::new(pid).exists());

        // An empty or init pid file signals nothing.
        for content in ["", "1", "x"] {
            std::fs::write(pid, content).unwrap();
            assert_eq!(local(HANG_UP, &[pid], "").1, 0, "{content:?}");
            assert!(!std::path::Path::new(pid).exists());
        }
    }

    #[test]
    fn messages_become_output_by_mode() {
        let out = |b: &[u8]| frame_of(Event::Binary(b.to_vec()), false);
        assert!(matches!(out(&[1, b'h']), Frame::Output(ExecOutput::Stdout(b)) if b == b"h"));
        assert!(matches!(out(&[2]), Frame::Output(ExecOutput::Stderr(b)) if b.is_empty()));
        assert!(matches!(out(&[3, 7]), Frame::Output(ExecOutput::Exit(7))));
        assert!(matches!(out(&[3]), Frame::Output(ExecOutput::Exit(-1))));
        assert!(matches!(out(&[9, 0]), Frame::Nothing));
        assert!(matches!(out(&[]), Frame::Nothing));
        // With a TTY, a leading 3 is terminal output, not an exit.
        assert!(matches!(frame_of(Event::Binary(vec![3, 7]), true), Frame::Output(ExecOutput::Stdout(b)) if b == [3, 7]));
        let text = |t: &str| frame_of(Event::Text(t.into()), true);
        assert!(matches!(text(r#"{"type":"exit","exit_code":3}"#), Frame::Output(ExecOutput::Exit(3))));
        assert!(matches!(text(r#"{"type":"exit","exit_code":1e12}"#), Frame::Output(ExecOutput::Exit(-1))));
        assert!(matches!(text(r#"{"type":"session_info","session_id":"abc-1","tty":true}"#), Frame::Session(s) if s == "abc-1"));
        assert!(matches!(text(r#"{"type":"session_info","session_id":42}"#), Frame::Session(s) if s == "42"));
        assert!(matches!(text(r#"{"type":"session_info","session_id":"../x"}"#), Frame::Nothing));
        assert!(matches!(text(r#"{"type":"port_opened","port":3000}"#), Frame::Nothing));
        assert!(matches!(text("not json"), Frame::Nothing));
        assert!(matches!(text(r#"{"type":"error","error":"no such sprite"}"#), Frame::Output(ExecOutput::Error(e)) if e.contains("no such sprite")));
        assert!(
            matches!(frame_of(Event::Closed { code: Some(1011), reason: String::new() }, false), Frame::Output(ExecOutput::Error(e)) if e.contains("1011"))
        );
    }
}
