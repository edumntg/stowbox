//! Exec in a boat sandbox over SSH.
//!
//! boat's command API takes no stdin and has no TTY, and Stowbox needs both
//! (agents read their prompt on stdin, the terminal is a TTY). So every exec
//! is an SSH channel: `POST /sandboxes/{id}/sshkey` authorizes Stowbox's
//! own ed25519 key on the sandbox and answers with the address, the user
//! and the host key; the client checks the server against that key and
//! nothing else. One connection per sandbox is kept and reused; a resume
//! or fork moves the sandbox to another machine with another host key, so a
//! connection that fails is dropped and the sandbox is asked again.
//!
//! Dropping an exec kills what it started, as with boxd: a non-TTY command
//! runs under `setsid -w` and first writes its process group id to stderr
//! behind a marker; when the reader goes away before the exit, a second
//! channel sends that group SIGTERM, then SIGKILL. A TTY session ends with
//! its PTY, which hangs up its processes.

use super::api::{seg, Api};
use crate::boxd::shell_quote;
use crate::types::{ExecHandle, ExecInput, ExecOutput, ExecSpec};
use crate::{Error, Result};
use russh::client;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::ChannelMsg;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// Starts the line a non-TTY exec writes first on stderr.
const MARKER: &[u8] = b"\x1estowbox-pgid:";

/// Where to reach one sandbox over SSH, from boat's `sshkey` answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub host_key: PublicKey,
}

impl Endpoint {
    /// `sshEndpoint` ("host:port") wins when set: the sandbox has no public
    /// IPv4 of its own. Otherwise `machineIp`, port 22.
    pub fn from_answer(v: &Value) -> Result<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let (host, port) = match s("sshEndpoint") {
            e if !e.is_empty() => {
                let (h, p) = e.rsplit_once(':').ok_or_else(|| Error::Provider(format!("boat gave an SSH endpoint without a port: {e}")))?;
                let p = p.parse::<u16>().map_err(|_| Error::Provider(format!("boat gave an SSH endpoint with a bad port: {e}")))?;
                (h.trim_start_matches('[').trim_end_matches(']').to_string(), p)
            }
            _ => (s("machineIp"), 22),
        };
        if host.is_empty() {
            return Err(Error::Provider("boat gave no address to reach the sandbox over SSH".into()));
        }
        let host_key = parse_host_key(&s("hostKey"))?;
        let user = Some(s("sshUser")).filter(|u| !u.is_empty()).unwrap_or_else(|| "user".into());
        Ok(Self { host, port, user, host_key })
    }
}

/// A host key as an OpenSSH line, with or without a host name in front
/// (`bx_1 ssh-ed25519 AAAA…`, as boat's docs write it to known_hosts).
pub fn parse_host_key(line: &str) -> Result<PublicKey> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| w.starts_with("ssh-") || w.starts_with("ecdsa-") || w.starts_with("sk-"));
    let Some(at) = at.filter(|i| i + 1 < words.len()) else {
        return Err(Error::Provider("boat did not send the sandbox's SSH host key, so the connection cannot be checked".into()));
    };
    PublicKey::from_openssh(&format!("{} {}", words[at], words[at + 1])).map_err(|e| Error::Provider(format!("boat sent an SSH host key that does not parse: {e}")))
}

/// Stowbox's own key for boat, made the first time and kept in `path`
/// (0600). It is never one of the person's keys.
pub fn load_or_make_key(path: &Path) -> Result<PrivateKey> {
    if let Ok(text) = std::fs::read_to_string(path) {
        return PrivateKey::from_openssh(text.as_bytes()).map_err(|e| Error::Provider(format!("{} is not an SSH private key: {e}", path.display())));
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| Error::Provider(format!("no randomness for a new SSH key: {e}")))?;
    let pair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let mut key = PrivateKey::from(pair);
    key.set_comment("stowbox");
    let pem = key.to_openssh(russh::keys::ssh_key::LineEnding::LF).map_err(|e| Error::Provider(format!("could not encode the SSH key: {e}")))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    match opts.open(path) {
        Ok(mut f) => f.write_all(pem.as_bytes())?,
        // Another exec made it first: use that one.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return load_or_make_key(path),
        Err(e) => return Err(e.into()),
    }
    Ok(key)
}

/// Accepts exactly the host key boat named for this sandbox.
struct Checker {
    expected: PublicKey,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> std::result::Result<bool, Self::Error> {
        Ok(key.key_data() == self.expected.key_data())
    }
}

type Conn = Arc<client::Handle<Checker>>;

/// Where the pool gets a sandbox's endpoint from: boat's `sshkey` call, or
/// a test's own sshd.
#[async_trait::async_trait]
pub trait Authorize: Send + Sync {
    /// Authorizes `public_line` on the sandbox and says how to reach it.
    async fn authorize(&self, id: &str, public_line: &str) -> Result<Endpoint>;
}

/// boat's `POST /sandboxes/{id}/sshkey`. The first seconds of a start
/// answer `sandbox_restoring` or `sandbox_securing`; those are waited out.
pub struct ApiAuth<'a>(pub &'a Api);

#[async_trait::async_trait]
impl Authorize for ApiAuth<'_> {
    async fn authorize(&self, id: &str, public_line: &str) -> Result<Endpoint> {
        let mut wait = Duration::from_millis(750);
        let mut tries = 0;
        loop {
            let r = self.0.post(&format!("/sandboxes/{}/sshkey", seg(id)), json!({ "key": public_line })).await;
            match r {
                Ok(v) => return Endpoint::from_answer(&v),
                Err(Error::Provider(m)) if m.contains("machine_not_running") => {
                    return Err(Error::Provider("the sandbox is not running; start it first".into()));
                }
                Err(Error::Provider(m)) if tries < 8 && ["sandbox_restoring", "sandbox_securing", "sandbox_starting", "ssh_host_key_unavailable", "sandbox_resuming"].iter().any(|c| m.contains(c)) => {
                    tries += 1;
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(4));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

pub struct Pool {
    key: Arc<PrivateKey>,
    public_line: String,
    /// One slot per sandbox, so two sandboxes connect at the same time but
    /// one sandbox is never connected to twice at once.
    slots: std::sync::Mutex<HashMap<String, Arc<Mutex<Option<Conn>>>>>,
}

fn ssh_err(e: impl std::fmt::Display) -> Error {
    Error::Network(format!("ssh: {e}"))
}

impl Pool {
    pub fn new(key: PrivateKey) -> Result<Self> {
        let public_line = key.public_key().to_openssh().map_err(|e| Error::Provider(format!("could not encode the SSH public key: {e}")))?;
        Ok(Self { key: Arc::new(key), public_line, slots: Default::default() })
    }

    pub fn public_line(&self) -> &str {
        &self.public_line
    }

    fn slot(&self, id: &str) -> Arc<Mutex<Option<Conn>>> {
        self.slots.lock().unwrap().entry(id.to_string()).or_default().clone()
    }

    /// Forgets the connection to a sandbox (after a stop, resume or delete).
    pub fn drop_conn(&self, id: &str) {
        self.slots.lock().unwrap().remove(id);
    }

    async fn connect(&self, auth: &dyn Authorize, id: &str) -> Result<Conn> {
        let ep = auth.authorize(id, &self.public_line).await?;
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(3600)),
            keepalive_interval: Some(Duration::from_secs(20)),
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        });
        let connect = client::connect(config, (ep.host.as_str(), ep.port), Checker { expected: ep.host_key.clone() });
        let mut handle = tokio::time::timeout(Duration::from_secs(20), connect)
            .await
            .map_err(|_| Error::Network(format!("ssh: no answer from {}:{} within 20 s", ep.host, ep.port)))?
            .map_err(|e| match e {
                russh::Error::UnknownKey => Error::Provider("the sandbox presented another SSH host key than boat announced; not connecting".into()),
                e => ssh_err(e),
            })?;
        let hash = handle.best_supported_rsa_hash().await.map_err(ssh_err)?.flatten();
        let auth = handle.authenticate_publickey(ep.user.clone(), PrivateKeyWithHashAlg::new(self.key.clone(), hash)).await.map_err(ssh_err)?;
        if !auth.success() {
            return Err(Error::Auth("the sandbox refused Stowbox's SSH key; the API key may lack the ssh action".into()));
        }
        Ok(Arc::new(handle))
    }

    /// A fresh channel on the sandbox's connection, connecting (again) when
    /// there is none or the one there stopped working.
    async fn channel(&self, auth: &dyn Authorize, id: &str) -> Result<(Conn, russh::Channel<client::Msg>)> {
        let slot = self.slot(id);
        let mut guard = slot.lock().await;
        if let Some(conn) = guard.as_ref().filter(|c| !c.is_closed()).cloned() {
            if let Ok(ch) = conn.channel_open_session().await {
                return Ok((conn, ch));
            }
        }
        *guard = None;
        let conn = self.connect(auth, id).await?;
        let ch = conn.channel_open_session().await.map_err(ssh_err)?;
        *guard = Some(conn.clone());
        Ok((conn, ch))
    }

    pub async fn exec(&self, auth: &dyn Authorize, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        let (conn, ch) = self.channel(auth, id).await?;
        if spec.tty {
            ch.request_pty(true, "xterm-256color", spec.cols.max(20) as u32, spec.rows.max(5) as u32, 0, 0, &[]).await.map_err(ssh_err)?;
        }
        ch.exec(true, remote_command(&spec)).await.map_err(ssh_err)?;
        let (mut rd, wr) = ch.split();

        let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
        let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

        tokio::spawn(async move {
            while let Some(msg) = in_rx.recv().await {
                let ok = match msg {
                    ExecInput::Data(data) => wr.data(&data[..]).await.is_ok(),
                    ExecInput::Resize { cols, rows } => wr.window_change(cols as u32, rows as u32, 0, 0).await.is_ok(),
                    ExecInput::Close => {
                        let _ = wr.eof().await;
                        break;
                    }
                };
                if !ok {
                    break;
                }
            }
            // Dropping the sender closes stdin the same way.
            let _ = wr.eof().await;
        });

        let tty = spec.tty;
        tokio::spawn(async move {
            let mut marker = if tty { Marker::Done } else { Marker::Reading(Vec::new()) };
            let mut pgid: Option<u32> = None;
            let mut code: Option<i32> = None;
            loop {
                let next = tokio::select! {
                    _ = out_tx.closed() => {
                        if code.is_none() {
                            if let Some(g) = pgid {
                                kill_group(&conn, g).await;
                            }
                        }
                        return;
                    }
                    m = rd.wait() => m,
                };
                let out = match next {
                    Some(ChannelMsg::Data { data }) => Some(ExecOutput::Stdout(data.to_vec())),
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let (found, rest) = marker.feed(&data);
                        if found.is_some() {
                            pgid = found;
                        }
                        rest.filter(|r| !r.is_empty()).map(ExecOutput::Stderr)
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        code = Some(exit_status as i32);
                        None
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        // A shell reports death by signal as 128 + n; do the same.
                        code = Some(128 + signal_number(&signal_name));
                        None
                    }
                    Some(ChannelMsg::Failure) => {
                        let _ = out_tx.send(ExecOutput::Error("the sandbox refused to run the command".into())).await;
                        return;
                    }
                    // Whatever the marker still holds back is a prefix of the
                    // marker line itself, never the command's own stderr.
                    Some(ChannelMsg::Close) | None => {
                        let last = match code {
                            Some(c) => ExecOutput::Exit(c),
                            None => ExecOutput::Error("the SSH connection to the sandbox closed".into()),
                        };
                        let _ = out_tx.send(last).await;
                        return;
                    }
                    Some(_) => None,
                };
                if let Some(o) = out {
                    if out_tx.send(o).await.is_err() {
                        if code.is_none() {
                            if let Some(g) = pgid {
                                kill_group(&conn, g).await;
                            }
                        }
                        return;
                    }
                }
            }
        });

        Ok(ExecHandle { input: in_tx, output: out_rx })
    }
}

/// What runs on the far side. Environment goes in as `export`s, since
/// sshd drops `env` requests it was not told to accept.
pub fn remote_command(spec: &ExecSpec) -> String {
    let mut script = String::new();
    for (k, v) in &spec.env {
        if k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && !k.is_empty() {
            script.push_str(&format!("export {}={} && ", k, shell_quote(v)));
        }
    }
    script.push_str(&spec.script);
    if spec.tty {
        format!("bash -lc {}", shell_quote(&script))
    } else {
        let inner = format!("printf '\\036stowbox-pgid:%s\\n' \"$$\" >&2; exec bash -lc \"$1\"");
        format!("setsid -w bash -c {} stowbox {}", shell_quote(&inner), shell_quote(&script))
    }
}

/// Reads the process group id off the front of stderr, once.
enum Marker {
    Reading(Vec<u8>),
    Done,
}

impl Marker {
    /// Returns the group id when this chunk completed the marker line, and
    /// the stderr bytes to pass on.
    fn feed(&mut self, data: &[u8]) -> (Option<u32>, Option<Vec<u8>>) {
        let Marker::Reading(buf) = self else { return (None, Some(data.to_vec())) };
        buf.extend_from_slice(data);
        let n = buf.len().min(MARKER.len());
        if buf[..n] != MARKER[..n] {
            let all = std::mem::take(buf);
            *self = Marker::Done;
            return (None, Some(all));
        }
        let Some(nl) = buf.iter().position(|b| *b == b'\n') else {
            // Still inside the marker line; a runaway one is not a marker.
            if buf.len() > 64 {
                let all = std::mem::take(buf);
                *self = Marker::Done;
                return (None, Some(all));
            }
            return (None, None);
        };
        let id = std::str::from_utf8(&buf[MARKER.len()..nl]).ok().and_then(|s| s.trim().parse::<u32>().ok());
        let rest = buf[nl + 1..].to_vec();
        *self = Marker::Done;
        (id, Some(rest))
    }

}

/// Best effort: TERM, a second for it to land, then KILL.
async fn kill_group(conn: &Conn, pgid: u32) {
    if pgid <= 1 {
        return;
    }
    let Ok(ch) = conn.channel_open_session().await else { return };
    let cmd = format!("kill -TERM -- -{pgid} 2>/dev/null; sleep 1; kill -KILL -- -{pgid} 2>/dev/null; true");
    let _ = ch.exec(false, cmd).await;
    let _ = ch.eof().await;
}

fn signal_number(s: &russh::Sig) -> i32 {
    use russh::Sig::*;
    match s {
        HUP => 1,
        INT => 2,
        QUIT => 3,
        ILL => 4,
        ABRT => 6,
        FPE => 8,
        KILL => 9,
        SEGV => 11,
        PIPE => 13,
        ALRM => 14,
        TERM => 15,
        USR1 => 10,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

    #[test]
    fn endpoint_prefers_the_relay_and_checks_the_host_key() {
        let ep = Endpoint::from_answer(&serde_json::json!({ "machineIp": "203.0.113.10", "sshUser": "user", "hostKey": ED, "sshEndpoint": null })).unwrap();
        assert_eq!((ep.host.as_str(), ep.port, ep.user.as_str()), ("203.0.113.10", 22, "user"));
        let ep = Endpoint::from_answer(&serde_json::json!({ "machineIp": "10.0.0.1", "sshEndpoint": "relay.boat.dev:40022", "hostKey": format!("bx_1 {ED}") })).unwrap();
        assert_eq!((ep.host.as_str(), ep.port, ep.user.as_str()), ("relay.boat.dev", 40022, "user"));
        // No host key, no connection.
        assert!(Endpoint::from_answer(&serde_json::json!({ "machineIp": "203.0.113.10" })).is_err());
        assert!(Endpoint::from_answer(&serde_json::json!({ "hostKey": ED })).is_err());
        assert!(Endpoint::from_answer(&serde_json::json!({ "sshEndpoint": "relay:notaport", "hostKey": ED })).is_err());
    }

    #[test]
    fn the_marker_yields_the_group_and_hides_itself() {
        let mut m = Marker::Reading(Vec::new());
        assert_eq!(m.feed(b"\x1estowbox-pg"), (None, None));
        assert_eq!(m.feed(b"id:4242\nwarning: x\n"), (Some(4242), Some(b"warning: x\n".to_vec())));
        assert_eq!(m.feed(b"more"), (None, Some(b"more".to_vec())));

        // A command whose stderr starts otherwise passes through untouched.
        let mut m = Marker::Reading(Vec::new());
        assert_eq!(m.feed(b"fatal: not a repo\n"), (None, Some(b"fatal: not a repo\n".to_vec())));
        // Held-back bytes that turn out not to be the marker are not lost.
        let mut m = Marker::Reading(Vec::new());
        assert_eq!(m.feed(b"\x1e"), (None, None));
        assert_eq!(m.feed(b"x"), (None, Some(b"\x1ex".to_vec())));
    }

    #[test]
    fn commands_quote_everything_and_only_detach_without_a_tty() {
        let spec = ExecSpec { script: "echo 'hi' | wc -c".into(), tty: false, cols: 0, rows: 0, env: vec![("A".into(), "x y".into()), ("bad key".into(), "z".into())] };
        let c = remote_command(&spec);
        assert!(c.starts_with("setsid -w bash -c "));
        assert!(c.contains("export A="));
        assert!(!c.contains("bad key"));
        let tty = remote_command(&ExecSpec { tty: true, ..spec });
        assert!(tty.starts_with("bash -lc "));
        assert!(!tty.contains("setsid"));
    }

    #[cfg(unix)]
    #[test]
    fn the_wrapper_reports_its_group_and_keeps_stdin_and_the_exit_code() {
        if !crate::testing::has_bash() || std::process::Command::new("setsid").arg("--version").output().is_err() {
            return;
        }
        let spec = ExecSpec { script: "read x; echo \"got $x $A\"; echo oops >&2; exit 3".into(), env: vec![("A".into(), "it's x y".into())], ..Default::default() };
        let mut child = std::process::Command::new("bash")
            .arg("-c")
            .arg(remote_command(&spec))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"ping\n").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "got ping it's x y\n");
        let mut m = Marker::Reading(Vec::new());
        let (pgid, rest) = m.feed(&out.stderr);
        assert!(pgid.is_some_and(|g| g > 1));
        assert_eq!(rest.unwrap(), b"oops\n");
    }

    #[test]
    fn a_new_key_is_kept_and_read_back() {
        let dir = crate::testing::temp_dir("stowbox-boat-key");
        let path = dir.join("keys").join("boat_ed25519");
        let a = load_or_make_key(&path).unwrap();
        let b = load_or_make_key(&path).unwrap();
        assert_eq!(a.public_key().key_data(), b.public_key().key_data());
        let line = Pool::new(b).unwrap().public_line().to_string();
        assert!(line.starts_with("ssh-ed25519 "));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    /// The real path, against a throwaway sshd on 127.0.0.1 with its own
    /// config, host key and authorized_keys. Needs OpenSSH's sshd; run with
    /// `cargo test -p stowbox-core -- --ignored local_sshd`.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "starts a local sshd; run with --ignored"]
    async fn local_sshd_exec_end_to_end() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"].into_iter().find(|p| Path::new(p).exists()).expect("sshd");
        let dir = crate::testing::temp_dir("stowbox-boat-sshd");
        let host = dir.join("host");
        assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&host).status().unwrap().success());
        let pool = Pool::new(load_or_make_key(&dir.join("client")).unwrap()).unwrap();
        std::fs::write(dir.join("authorized_keys"), format!("{}\n", pool.public_line())).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let user = String::from_utf8(std::process::Command::new("id").arg("-un").output().unwrap().stdout).unwrap().trim().to_string();
        let cfg = dir.join("sshd_config");
        std::fs::write(
            &cfg,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nPermitRootLogin yes\nPidFile {}\n",
                host.display(),
                dir.join("authorized_keys").display(),
                dir.join("pid").display()
            ),
        )
        .unwrap();
        let _ = std::fs::create_dir_all("/run/sshd");
        let mut daemon = std::process::Command::new(sshd).arg("-D").arg("-e").arg("-f").arg(&cfg).stderr(std::process::Stdio::null()).spawn().unwrap();
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        struct Local {
            ep: Endpoint,
            calls: AtomicUsize,
        }
        #[async_trait::async_trait]
        impl Authorize for Local {
            async fn authorize(&self, _: &str, _: &str) -> Result<Endpoint> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.ep.clone())
            }
        }
        let host_key = parse_host_key(&std::fs::read_to_string(host.with_extension("pub")).unwrap()).unwrap();
        let auth = Local { ep: Endpoint { host: "127.0.0.1".into(), port, user: user.clone(), host_key }, calls: AtomicUsize::new(0) };

        async fn collect(mut h: ExecHandle) -> (String, String, Option<i32>) {
            let (mut out, mut err, mut code) = (Vec::new(), Vec::new(), None);
            while let Some(o) = h.output.recv().await {
                match o {
                    ExecOutput::Stdout(b) => out.extend(b),
                    ExecOutput::Stderr(b) => err.extend(b),
                    ExecOutput::Exit(c) => code = Some(c),
                    ExecOutput::Error(e) => panic!("exec error: {e}"),
                }
            }
            (String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned(), code)
        }

        // stdin in, stdout and stderr apart, the exit code back, env applied.
        let spec = ExecSpec { script: "read x; echo \"got $x $GREETING\"; echo oops >&2; exit 7".into(), env: vec![("GREETING".into(), "it's me".into())], ..Default::default() };
        let h = pool.exec(&auth, "bx_1", spec).await.unwrap();
        h.input.send(ExecInput::Data(b"ping\n".to_vec())).await.unwrap();
        h.input.send(ExecInput::Close).await.unwrap();
        assert_eq!(collect(h).await, ("got ping it's me\n".into(), "oops\n".into(), Some(7)));

        // A TTY, on the same connection.
        let tty = ExecSpec { script: "tty; exit 0".into(), tty: true, cols: 100, rows: 30, ..Default::default() };
        let (out, _, code) = collect(pool.exec(&auth, "bx_1", tty).await.unwrap()).await;
        assert!(out.contains("/dev/pts/"), "{out}");
        assert_eq!(code, Some(0));
        assert_eq!(auth.calls.load(Ordering::SeqCst), 1, "one connection serves both execs");

        // Dropping a running exec ends what it started, children included.
        let long = ExecSpec { script: "sleep 300 & echo $!; wait".into(), ..Default::default() };
        let mut h = pool.exec(&auth, "bx_1", long).await.unwrap();
        let pid = loop {
            match h.output.recv().await {
                Some(ExecOutput::Stdout(b)) => break String::from_utf8_lossy(&b).trim().to_string(),
                Some(_) => continue,
                None => panic!("no pid"),
            }
        };
        let alive = |pid: &str| std::process::Command::new("kill").arg("-0").arg(pid).status().map(|s| s.success()).unwrap_or(false);
        assert!(alive(&pid));
        drop(h);
        let mut gone = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if !alive(&pid) {
                gone = true;
                break;
            }
        }
        assert!(gone, "sleep {pid} outlived its exec");

        // Another host key than the one announced: no connection.
        let other = dir.join("other");
        assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&other).status().unwrap().success());
        let wrong = Local { ep: Endpoint { host_key: parse_host_key(&std::fs::read_to_string(other.with_extension("pub")).unwrap()).unwrap(), ..auth.ep.clone() }, calls: AtomicUsize::new(0) };
        let e = pool.exec(&wrong, "bx_2", ExecSpec { script: "true".into(), ..Default::default() }).await.err().expect("refused");
        assert!(e.to_string().contains("host key"), "{e}");

        let _ = daemon.kill();
        let _ = daemon.wait();
    }
}
