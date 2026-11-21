//! Exec in a provider's box over SSH, for providers whose own command API
//! cannot do what Stowbox needs (stdin, a TTY, or both).
//!
//! A provider says how to reach a box through [`Authorize`]: the address,
//! the user, how to log in (Stowbox's own key, or a token that is the user
//! name) and how to check the server (a host key the provider announced, or
//! the first one seen, remembered in a file). One connection per box is
//! kept and reused; a connection that fails is dropped and the provider is
//! asked again, since a resume or fork can move the box to another machine.
//!
//! Dropping an exec kills what it started, as with boxd: a non-TTY command
//! runs under `setsid -w` and first writes its process group id to stderr
//! behind a marker; when the reader goes away before the exit, a second
//! channel sends that group SIGTERM, then SIGKILL. A TTY session ends with
//! its PTY, which hangs up its processes.

use crate::boxd::shell_quote;
use crate::types::{ExecHandle, ExecInput, ExecOutput, ExecSpec};
use crate::{Error, Result};
use russh::client;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::ChannelMsg;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// Starts the line a non-TTY exec writes first on stderr.
const MARKER: &[u8] = b"\x1estowbox-pgid:";

/// How Stowbox logs in once connected.
#[derive(Debug, Clone, PartialEq)]
pub enum Login {
    /// The pool's own key, which the provider authorized on the box.
    Key,
    /// The user name is an access token and nothing else is asked
    /// (Daytona's gateway); tried as "none", then as a password, then
    /// keyboard-interactive with empty answers.
    Token,
}

/// How the server is checked.
#[derive(Debug, Clone, PartialEq)]
pub enum HostCheck {
    /// Exactly the key the provider announced for this box.
    Pinned(PublicKey),
    /// The provider announces none: the first key seen for this host and
    /// port is kept in the file, and any other key later is refused.
    Remembered(PathBuf),
    /// Any key. Only for a [`Dial::WebSocket`] tunnel, where TLS and the
    /// API key at the provider's edge already say who answers, and the box
    /// makes a new host key whenever it moves (Agent37).
    Edge,
}

/// How the SSH byte stream gets to the box.
#[derive(Debug, Clone, PartialEq)]
pub enum Dial {
    /// Straight TCP to host and port.
    Tcp,
    /// A WebSocket to `wss://host:port/` with these headers on the upgrade
    /// (see [`crate::wstunnel`]).
    WebSocket(Vec<(String, String)>),
}

/// Where to reach one box over SSH.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub login: Login,
    pub check: HostCheck,
    pub dial: Dial,
    /// The provider's name, for messages.
    pub provider: &'static str,
    /// What to tell the person when the box refuses Stowbox's key.
    pub key_refused: String,
}

/// A host key as an OpenSSH line, with or without a host name in front
/// (`bx_1 ssh-ed25519 AAAA…`, as known_hosts writes it).
pub fn parse_host_key(line: &str) -> Result<PublicKey> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| w.starts_with("ssh-") || w.starts_with("ecdsa-") || w.starts_with("sk-"));
    let Some(at) = at.filter(|i| i + 1 < words.len()) else {
        return Err(Error::Provider("no SSH host key was sent for the box, so the connection cannot be checked".into()));
    };
    PublicKey::from_openssh(&format!("{} {}", words[at], words[at + 1])).map_err(|e| Error::Provider(format!("the box's SSH host key does not parse: {e}")))
}

/// Stowbox's own key for a provider, made the first time and kept in `path`
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

/// Checks the server per the endpoint's [`HostCheck`].
struct Checker {
    check: HostCheck,
    /// `host:port`, the remembered file's key.
    at: String,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        // No provider announces a certificate authority, so nothing vouches
        // for a certificate; it must not be pinned or remembered as a key.
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(self.check == HostCheck::Edge);
        };
        Ok(match &self.check {
            HostCheck::Pinned(expected) => key.key_data() == expected.key_data(),
            HostCheck::Remembered(path) => remember_host_key(path, &self.at, key),
            HostCheck::Edge => true,
        })
    }
}

/// Lines of `host:port keytype base64`. The first key seen for a host is
/// written down and accepted; later, only that key is.
fn remember_host_key(path: &Path, at: &str, key: &PublicKey) -> bool {
    let Ok(line) = key.to_openssh() else { return false };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    for l in text.lines() {
        if let Some((host, known)) = l.split_once(' ') {
            if host == at {
                return parse_host_key(known).map(|k| k.key_data() == key.key_data()).unwrap_or(false);
            }
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    use std::io::Write;
    let written = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| writeln!(f, "{at} {line}"));
    written.is_ok()
}

type Conn = Arc<client::Handle<Checker>>;

/// Where the pool gets a box's endpoint from: the provider's API, or a
/// test's own sshd.
#[async_trait::async_trait]
pub trait Authorize: Send + Sync {
    /// Makes the box reachable (authorizes `public_line` on it, or mints a
    /// token) and says how to reach it. `public_line` is empty for a pool
    /// without a key.
    async fn authorize(&self, id: &str, public_line: &str) -> Result<Endpoint>;
}

pub struct Pool {
    key: Option<Arc<PrivateKey>>,
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
        Ok(Self { key: Some(Arc::new(key)), public_line, slots: Default::default() })
    }

    /// A pool for endpoints that log in with a token, not a key.
    pub fn without_key() -> Self {
        Self { key: None, public_line: String::new(), slots: Default::default() }
    }

    pub fn public_line(&self) -> &str {
        &self.public_line
    }

    fn slot(&self, id: &str) -> Arc<Mutex<Option<Conn>>> {
        self.slots.lock().unwrap().entry(id.to_string()).or_default().clone()
    }

    /// Whether a live connection to the box is held right now. Never waits:
    /// a slot busy connecting counts as not connected.
    pub fn is_connected(&self, id: &str) -> bool {
        let slot = self.slots.lock().ok().and_then(|s| s.get(id).cloned());
        slot.is_some_and(|s| s.try_lock().is_ok_and(|c| c.as_ref().is_some_and(|c| !c.is_closed())))
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
        let at = format!("{}:{}", ep.host, ep.port);
        let checker = Checker { check: ep.check.clone(), at: at.clone() };
        // The tunnel first, so its refusals (a revoked API key) keep their own error.
        let tunnel = match &ep.dial {
            Dial::Tcp => None,
            Dial::WebSocket(headers) => Some(
                tokio::time::timeout(Duration::from_secs(20), crate::wstunnel::connect(&ep.host, ep.port, headers))
                    .await
                    .map_err(|_| Error::Network(format!("no tunnel to {} within 20 s", ep.host)))??,
            ),
        };
        let connect = async {
            match tunnel {
                None => client::connect(config, (ep.host.as_str(), ep.port), checker).await,
                Some(stream) => client::connect_stream(config, stream, checker).await,
            }
        };
        let mut handle = tokio::time::timeout(Duration::from_secs(20), connect)
            .await
            .map_err(|_| Error::Network(format!("ssh: no answer from {}:{} within 20 s", ep.host, ep.port)))?
            .map_err(|e| match e {
                russh::Error::UnknownKey => Error::Provider(match &ep.check {
                    HostCheck::Pinned(_) => format!("the box presented another SSH host key than {} announced; not connecting", ep.provider),
                    HostCheck::Remembered(path) => format!(
                        "{} presented another SSH host key than the first time; not connecting. If {} changed it, remove the line starting {at} from {}",
                        ep.host,
                        ep.provider,
                        path.display()
                    ),
                    HostCheck::Edge => format!("{} refused the SSH host key", ep.provider),
                }),
                e => ssh_err(e),
            })?;
        let ok = match ep.login {
            Login::Key => {
                let key = self.key.clone().ok_or_else(|| Error::Provider("no SSH key to log in with".into()))?;
                let hash = handle.best_supported_rsa_hash().await.map_err(ssh_err)?.flatten();
                handle.authenticate_publickey(ep.user.clone(), PrivateKeyWithHashAlg::new(key, hash)).await.map_err(ssh_err)?.success()
            }
            Login::Token => token_login(&mut handle, &ep.user).await?,
        };
        if !ok {
            return Err(Error::Auth(match ep.login {
                Login::Key => format!("the box refused Stowbox's SSH key; {}", ep.key_refused),
                Login::Token => format!("{}'s SSH gateway refused the access token", ep.provider),
            }));
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
            let mut marker = Marker::new(tty);
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
                        let _ = out_tx.send(ExecOutput::Error("the box refused to run the command".into())).await;
                        return;
                    }
                    // Whatever the marker still holds back is a prefix of the
                    // marker line itself, never the command's own stderr.
                    Some(ChannelMsg::Close) | None => {
                        let last = match code {
                            Some(c) => ExecOutput::Exit(c),
                            None => ExecOutput::Error("the SSH connection to the box closed".into()),
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

/// A gateway that takes the token as the user name: "none" first, then the
/// token as a password, then keyboard-interactive answered with the token.
async fn token_login(handle: &mut client::Handle<Checker>, user: &str) -> Result<bool> {
    if handle.authenticate_none(user).await.map_err(ssh_err)?.success() {
        return Ok(true);
    }
    if handle.authenticate_password(user, user).await.map_err(ssh_err)?.success() {
        return Ok(true);
    }
    let mut r = handle.authenticate_keyboard_interactive_start(user, None).await.map_err(ssh_err)?;
    for _ in 0..3 {
        match r {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(true),
            client::KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| user.to_string()).collect();
                r = handle.authenticate_keyboard_interactive_respond(answers).await.map_err(ssh_err)?;
            }
        }
    }
    Ok(false)
}

/// The script with the spec's environment as `export`s in front, for
/// `bash -lc`. Names that are not shell names are left out.
pub fn with_env(spec: &ExecSpec) -> String {
    let mut script = String::new();
    for (k, v) in &spec.env {
        let shell_name = k.bytes().next().is_some_and(|b| !b.is_ascii_digit()) && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if shell_name {
            script.push_str(&format!("export {}={} && ", k, shell_quote(v)));
        }
    }
    script.push_str(&spec.script);
    script
}

/// What runs on the far side. Environment goes in as `export`s, since
/// sshd drops `env` requests it was not told to accept.
pub fn remote_command(spec: &ExecSpec) -> String {
    let script = with_env(spec);
    if spec.tty {
        format!("bash -lc {}", shell_quote(&script))
    } else {
        let inner = "printf '\\036stowbox-pgid:%s\\n' \"$$\" >&2; exec bash -lc \"$1\"";
        // setsid -w is util-linux. Elsewhere (macOS, BSD, BusyBox) the script
        // runs without a group of its own: it works, but dropping the exec
        // cannot kill what it left running. `sh -c` keeps this independent
        // of the account's login shell.
        let pick = "if setsid -w true 2>/dev/null; then exec setsid -w bash -c \"$1\" stowbox \"$2\"; else exec bash -c \"$1\" stowbox \"$2\"; fi";
        format!("sh -c {} stowbox {} {}", shell_quote(pick), shell_quote(&inner), shell_quote(&script))
    }
}

/// Reads the process group id off the front of stderr, once. Any exec that
/// runs [`remote_command`] uses it, over SSH or not.
pub(crate) enum Marker {
    Reading(Vec<u8>),
    Done,
}

impl Marker {
    /// Waiting for the marker, which only a command without a TTY prints.
    pub(crate) fn new(tty: bool) -> Self {
        if tty {
            Marker::Done
        } else {
            Marker::Reading(Vec::new())
        }
    }

    /// Returns the group id when this chunk completed the marker line, and
    /// the stderr bytes to pass on.
    pub(crate) fn feed(&mut self, data: &[u8]) -> (Option<u32>, Option<Vec<u8>>) {
        let Marker::Reading(buf) = self else { return (None, Some(data.to_vec())) };
        buf.extend_from_slice(data);
        let n = buf.len().min(MARKER.len());
        if buf.get(..n) != MARKER.get(..n) {
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
        let id = buf.get(MARKER.len()..nl).and_then(|b| std::str::from_utf8(b).ok()).and_then(|s| s.trim().parse::<u32>().ok());
        let rest = buf.get(nl + 1..).unwrap_or_default().to_vec();
        *self = Marker::Done;
        (id, Some(rest))
    }
}

/// The shell command that ends process group `pgid`: TERM, a second for it
/// to land, then KILL. `None` for a group that must never be signalled.
/// No `--` before the group: dash's `kill` (Debian's `sh`) refuses it, and
/// `kill -SIG -N` is POSIX.
pub(crate) fn kill_group_command(pgid: u32) -> Option<String> {
    (pgid > 1).then(|| format!("kill -TERM -{pgid} 2>/dev/null; sleep 1; kill -KILL -{pgid} 2>/dev/null; true"))
}

/// Best effort; see [`kill_group_command`].
async fn kill_group(conn: &Conn, pgid: u32) {
    let Some(cmd) = kill_group_command(pgid) else { return };
    let Ok(ch) = conn.channel_open_session().await else { return };
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
    use russh::client::Handler;

    const HOST_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIBJTU59j3HRwRnqvcgkRZcLUm4HubVI5GWQ6LxuS8O8";
    /// `HOST_KEY` signed by a throwaway certificate authority.
    const HOST_CERT: &str = "ssh-ed25519-cert-v01@openssh.com AAAAIHNzaC1lZDI1NTE5LWNlcnQtdjAxQG9wZW5zc2guY29tAAAAIF1sYftjGlln+VgcRvZsKMUiaTDJYKZ2DlhWwYvLIcWjAAAAIIBJTU59j3HRwRnqvcgkRZcLUm4HubVI5GWQ6LxuS8O8AAAAAAAAAAAAAAACAAAAB2ZpeHR1cmUAAAAPAAAAC2JveC5leGFtcGxlAAAAAAAAAAD//////////wAAAAAAAAAAAAAAAAAAADMAAAALc3NoLWVkMjU1MTkAAAAgPqqyDemW/z0Qek3OWIw3yl4n35t1bx79cIN4gPoHOecAAABTAAAAC3NzaC1lZDI1NTE5AAAAQIFTld30CS7LMwA+MTEDlGtPrr0c6vw3a3HGZn2lqyw0mcrI3Nsfj4p+Y9k082lI9OFRycPjn/GIDt4hRQVnSAk= host";

    async fn accepts(check: HostCheck, key: PublicKeyOrCertificate) -> bool {
        Checker { check, at: "box.example:22".into() }.check_server_key(&key).await.unwrap()
    }

    #[tokio::test]
    async fn a_certificate_is_never_taken_for_the_key_inside_it() {
        let key = parse_host_key(HOST_KEY).unwrap();
        let cert = || PublicKeyOrCertificate::from(russh::keys::Certificate::from_openssh(HOST_CERT).unwrap());
        assert!(accepts(HostCheck::Pinned(key.clone()), key.clone().into()).await);
        assert!(!accepts(HostCheck::Pinned(key), cert()).await);

        let dir = crate::testing::temp_dir("stowbox-hostcert");
        let known = dir.join("known_hosts");
        assert!(!accepts(HostCheck::Remembered(known.clone()), cert()).await);
        assert!(!known.exists(), "a refused certificate must not be remembered");
        assert!(accepts(HostCheck::Edge, cert()).await);
        let _ = std::fs::remove_dir_all(dir);
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
        let spec = ExecSpec {
            script: "echo 'hi' | wc -c".into(),
            tty: false,
            cols: 0,
            rows: 0,
            env: vec![("A".into(), "x y".into()), ("bad key".into(), "z".into())],
        };
        let c = remote_command(&spec);
        assert!(c.starts_with("sh -c ") && c.contains("setsid -w bash -c"));
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
        let spec =
            ExecSpec { script: "read x; echo \"got $x $A\"; echo oops >&2; exit 3".into(), env: vec![("A".into(), "it's x y".into())], ..Default::default() };
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

    /// A host without util-linux (macOS, BSD) still runs the script.
    #[cfg(unix)]
    #[test]
    fn the_wrapper_runs_without_setsid() {
        if !crate::testing::has_bash() {
            return;
        }
        let which = |b: &str| String::from_utf8(std::process::Command::new("sh").args(["-c", &format!("command -v {b}")]).output().unwrap().stdout).unwrap();
        let dir = crate::testing::temp_dir("stowbox-no-setsid");
        for b in ["bash", "sh"] {
            std::os::unix::fs::symlink(which(b).trim(), dir.join(b)).unwrap();
        }
        let spec = ExecSpec { script: "echo ran; exit 4".into(), ..Default::default() };
        let out = std::process::Command::new(dir.join("sh")).arg("-c").arg(remote_command(&spec)).env("PATH", &dir).output().unwrap();
        assert_eq!(out.status.code(), Some(4));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_key_is_kept_and_read_back() {
        let dir = crate::testing::temp_dir("stowbox-sshexec-key");
        let path = dir.join("keys").join("provider_ed25519");
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
        let dir = crate::testing::temp_dir("stowbox-sshexec-sshd");
        let pool = Pool::new(load_or_make_key(&dir.join("client")).unwrap()).unwrap();
        let sshd = crate::testing::LocalSshd::start(pool.public_line()).await;
        let (port, user) = (sshd.port, sshd.user.clone());

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
        let host_key = parse_host_key(&sshd.host_key).unwrap();
        let ep = Endpoint {
            host: "127.0.0.1".into(),
            port,
            user: user.clone(),
            login: Login::Key,
            check: HostCheck::Pinned(host_key),
            dial: Dial::Tcp,
            provider: "test",
            key_refused: String::new(),
        };
        let auth = Local { ep, calls: AtomicUsize::new(0) };

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
        let spec = ExecSpec {
            script: "read x; echo \"got $x $GREETING\"; echo oops >&2; exit 7".into(),
            env: vec![("GREETING".into(), "it's me".into())],
            ..Default::default()
        };
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
        let other_key = parse_host_key(&std::fs::read_to_string(other.with_extension("pub")).unwrap()).unwrap();
        let wrong = Local { ep: Endpoint { check: HostCheck::Pinned(other_key.clone()), ..auth.ep.clone() }, calls: AtomicUsize::new(0) };
        let e = pool.exec(&wrong, "bx_2", ExecSpec { script: "true".into(), ..Default::default() }).await.err().expect("refused");
        assert!(e.to_string().contains("host key"), "{e}");

        // No key announced: the first one is remembered, then held to.
        let known = dir.join("known_hosts");
        let tofu = Local { ep: Endpoint { check: HostCheck::Remembered(known.clone()), ..auth.ep.clone() }, calls: AtomicUsize::new(0) };
        let (out, _, code) = collect(pool.exec(&tofu, "bx_3", ExecSpec { script: "echo first".into(), ..Default::default() }).await.unwrap()).await;
        assert_eq!((out.as_str(), code), ("first\n", Some(0)));
        assert!(std::fs::read_to_string(&known).unwrap().starts_with(&format!("127.0.0.1:{port} ssh-ed25519 ")));
        let at = format!("127.0.0.1:{port}");
        assert!(!remember_host_key(&known, &at, &other_key), "a changed key is refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
