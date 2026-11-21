//! Any machine Stowbox can reach over SSH, behind the `Provider` trait.
//!
//! There is no API: the box is the host the person connected, shown as one
//! box for as long as it is connected. Stowbox cannot make, power or delete
//! it. Commands run over the shared SSH exec ([`crate::sshexec`]) with
//! Stowbox's own key, or a key file the person names, and the host's key is
//! remembered the first time and held to after.

use crate::capabilities::{Capabilities, CredentialField};
use crate::cost::RateCard;
use crate::provider::Provider;
use crate::providers::{ConnectContext, Credentials};
use crate::sshexec::{self, Authorize, Dial, Endpoint, HostCheck, Login};
use crate::types::*;
use crate::{Error, Result};
use async_trait::async_trait;
use russh::keys::PrivateKey;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const ID: &str = "sshhost";
const HOST: &str = "host";
const USER: &str = "user";
const PORT: &str = "port";
const KEY_PATH: &str = "key_path";
/// How long a listing waits for the host's SSH port to answer.
const PROBE: Duration = Duration::from_secs(3);
/// How long one probe's answer stands.
const PROBE_EVERY: Duration = Duration::from_secs(60);

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind { id: ID, capabilities, connect };

fn connect(credentials: Credentials, context: &ConnectContext) -> Result<Arc<dyn Provider>> {
    let key_path = credentials.get(KEY_PATH);
    // The field takes a path. A pasted key would be echoed back in "could
    // not read the key file …", so it is refused without repeating it.
    if key_path.is_some_and(|p| p.contains('\n') || p.contains("PRIVATE KEY")) {
        return Err(Error::Auth("the key file field takes the path to a key, not the key itself".into()));
    }
    Ok(Arc::new(SshHost::new(Target::from_credentials(&credentials)?, key_path.map(PathBuf::from), context)?))
}

pub fn capabilities() -> Capabilities {
    Capabilities {
        create: false,
        new_box_blurb: "A machine you already have, reached over SSH. Stowbox does not make, stop or delete it.".into(),
        free_sizes: Vec::new(),
        default_size: String::new(),
        default_disk_gib: 0,
        ssh_hint: String::new(),
        credential_fields: vec![
            CredentialField::plain(HOST, "address", "A host name or address Stowbox can reach.", "server.example.com"),
            CredentialField::plain(USER, "user", "The account Stowbox logs in as.", "ubuntu"),
            CredentialField::plain(PORT, "port", "Leave empty for 22.", "22").optional(),
            CredentialField::plain(
                KEY_PATH,
                "key file",
                "Leave empty and Stowbox makes a key of its own; the first connection says which line to add to the host's authorized_keys. Otherwise the path to an unencrypted private key.",
                "~/.ssh/id_ed25519",
            )
            .optional(),
        ],
        icon: "SH".into(),
        label: "SSH host".into(),
        console_url: String::new(),
        key_prefixes: Vec::new(),
        // No price, so nothing is estimated.
        currency: String::new(),
        power: Vec::new(),
        fork: false,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: false,
        pauses_when_idle: false,
        custom_size: false,
        sizes: Vec::new(),
        https_links: false,
        https_named: false,
        https_list: false,
        https_remove: false,
        https_default_route: false,
        port_forwards: false,
        editor_ssh: false,
        max_upload_bytes: crate::upload::MAX_BYTES,
        remote_user: String::new(),
        remote_home: String::new(),
        metering_note: "An SSH host has no price Stowbox can read.".into(),
        metered_usage: false,
    }
}

/// Where the host is and who logs in, checked before anything connects.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl Target {
    pub fn from_credentials(c: &Credentials) -> Result<Self> {
        let host = c.get(HOST).unwrap_or("");
        let user = c.get(USER).unwrap_or("");
        let port = match c.get(PORT) {
            None => 22,
            Some(p) => p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| Error::Auth(format!("{p} is not a port; use 1 to 65535")))?,
        };
        if !valid_host(host) {
            return Err(Error::Auth(format!("{host:?} is not a host name or address")));
        }
        if !valid_user(user) {
            return Err(Error::Auth(format!("{user:?} is not a user name: letters, digits, '.', '_' and '-', not starting with '-'")));
        }
        Ok(Self { host: host.into(), port, user: user.into() })
    }
}

/// A DNS name or an IPv4 or IPv6 address. Nothing that `ssh` could read as
/// an option, and nothing a shell or a URL would treat specially.
fn valid_host(h: &str) -> bool {
    (1..=253).contains(&h.len())
        && !h.starts_with(['-', '.'])
        && (h.parse::<std::net::IpAddr>().is_ok() || h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'))
}

/// A POSIX-portable user name.
fn valid_user(u: &str) -> bool {
    (1..=32).contains(&u.len()) && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// An OpenSSH private key is a few KiB even for RSA 16384; more is not a key.
const MAX_KEY_BYTES: u64 = 64 * 1024;

/// A key file the person named. Stowbox reads it and keeps nothing.
fn read_key(path: &Path) -> Result<PrivateKey> {
    use std::io::Read;
    let path = expand_home(path);
    let unreadable = |e: std::io::Error| Error::Auth(format!("could not read the key file {}: {e}", path.display()));
    let file = std::fs::File::open(&path).map_err(unreadable)?;
    // A device or a pipe could never end; only a regular file of key size is read.
    let meta = file.metadata().map_err(unreadable)?;
    if !meta.is_file() || meta.len() > MAX_KEY_BYTES {
        return Err(Error::Auth(format!("{} is not a private key file", path.display())));
    }
    let mut text = String::new();
    file.take(MAX_KEY_BYTES).read_to_string(&mut text).map_err(unreadable)?;
    let key = PrivateKey::from_openssh(text.as_bytes()).map_err(|e| Error::Auth(format!("{} is not an OpenSSH private key: {e}", path.display())))?;
    if key.is_encrypted() {
        return Err(Error::Auth(format!(
            "{} has a passphrase, which Stowbox cannot ask for; leave the key file empty to use Stowbox's own key",
            path.display()
        )));
    }
    Ok(key)
}

fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

pub struct SshHost {
    target: Target,
    pool: sshexec::Pool,
    known_hosts: PathBuf,
    /// What to tell the person when the host refuses the key.
    key_refused: String,
    /// The last reachability probe and when it ran.
    probed: tokio::sync::Mutex<Option<(Instant, MachineStatus)>>,
}

impl SshHost {
    pub fn new(target: Target, key_path: Option<PathBuf>, context: &ConnectContext) -> Result<Self> {
        let (key, own) = match &key_path {
            Some(p) => (read_key(p)?, false),
            None => (sshexec::load_or_make_key(&context.ssh_file("sshhost_ed25519"))?, true),
        };
        let pool = sshexec::Pool::new(key)?;
        let key_refused = if own {
            format!("add this line to ~/.ssh/authorized_keys for {} on {}: {}", target.user, target.host, pool.public_line())
        } else {
            format!("{} does not accept that key for {}", target.host, target.user)
        };
        Ok(Self { target, pool, known_hosts: context.ssh_file("sshhost_known_hosts"), key_refused, probed: Default::default() })
    }

    fn machine(&self, status: MachineStatus) -> Machine {
        Machine {
            id: self.target.host.clone(),
            provider: ID.into(),
            size: String::new(),
            name: self.target.host.clone(),
            status,
            vcpu: 0,
            memory_bytes: 0,
            disk_bytes: 0,
            public_ip: String::new(),
            access_domain: String::new(),
            url: String::new(),
            desktop_url: String::new(),
            ssh_port: u32::from(self.target.port),
            org: String::new(),
            billing_org: String::new(),
            source: String::new(),
            isolated: false,
            created_at: 0,
            hibernated_at_ms: 0,
            boot_time_ms: 0,
            last_connected_ms: 0,
            auto_suspend_secs: 0,
            auto_hibernate_secs: 0,
            auto_destroy_secs: 0,
            auto_stop_at: 0,
            idle_stop_secs: 0,
        }
    }

    /// Running while Stowbox holds a live connection, or when the SSH port
    /// answered a probe; Stowbox cannot tell an unreachable host from a
    /// stopped one, so otherwise unknown. Every probe is a connection sshd
    /// logs as dropped before auth, and sshguard or fail2ban may count those
    /// against the person's address, so one probe serves a minute of listing.
    async fn status(&self) -> MachineStatus {
        if self.pool.is_connected(&self.target.host) {
            return MachineStatus::Running;
        }
        if let Some((at, status)) = *self.probed.lock().await {
            if at.elapsed() < PROBE_EVERY {
                return status;
            }
        }
        let addr = (self.target.host.as_str(), self.target.port);
        let status = match tokio::time::timeout(PROBE, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => MachineStatus::Running,
            _ => MachineStatus::Unknown,
        };
        *self.probed.lock().await = Some((Instant::now(), status));
        status
    }

    fn check_id(&self, id: &str) -> Result<()> {
        if id == self.target.host {
            Ok(())
        } else {
            Err(Error::NotFound(format!("this SSH host is {}, not {id}", self.target.host)))
        }
    }

    fn cannot(&self, what: &str) -> Error {
        Error::Provider(format!("Stowbox cannot {what} an SSH host"))
    }
}

#[async_trait]
impl Authorize for SshHost {
    async fn authorize(&self, _id: &str, _public_line: &str) -> Result<Endpoint> {
        Ok(Endpoint {
            host: self.target.host.clone(),
            port: self.target.port,
            user: self.target.user.clone(),
            login: Login::Key,
            check: HostCheck::Remembered(self.known_hosts.clone()),
            dial: Dial::Tcp,
            provider: "the SSH host",
            key_refused: self.key_refused.clone(),
        })
    }
}

#[async_trait]
impl Provider for SshHost {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    /// Logs in, so a wrong host, user or key shows up when connecting.
    async fn whoami(&self) -> Result<Account> {
        let out = crate::git::capture(self, &self.target.host, "id -un".into(), None, Duration::from_secs(30), "checking the login").await?;
        if out.code != 0 {
            return Err(Error::Provider(format!("id -un failed on {}: {}", self.target.host, String::from_utf8_lossy(&out.stderr).trim())));
        }
        let name = format!("{}@{}", String::from_utf8_lossy(&out.stdout).trim(), self.target.host);
        Ok(Account {
            user_id: name.clone(),
            display_name: name,
            billing: Billing {
                mode: String::new(),
                currency: String::new(),
                balance_micro: 0,
                accruing_micro: None,
                max_vms: 1,
                default_vcpu: 0,
                default_memory_bytes: 0,
                ceiling_vcpu: 0,
                ceiling_memory_bytes: 0,
                subscription_status: String::new(),
            },
        })
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        Ok(vec![self.machine(self.status().await)])
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        self.check_id(id_or_name)?;
        Ok(self.machine(self.status().await))
    }

    async fn create_machine(&self, _spec: CreateSpec) -> Result<Machine> {
        Err(self.cannot("make"))
    }

    async fn fork_machine(&self, _source: &str, _name: Option<String>) -> Result<Machine> {
        Err(self.cannot("fork"))
    }

    async fn power(&self, _id: &str, action: PowerAction) -> Result<()> {
        Err(self.cannot(&format!("{action:?}").to_lowercase()))
    }

    async fn set_auto_hibernate(&self, _id: &str, _secs: u32) -> Result<()> {
        Err(self.cannot("hibernate"))
    }

    async fn set_auto_suspend(&self, _id: &str, _secs: u32) -> Result<()> {
        Err(self.cannot("suspend"))
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        self.check_id(id)?;
        self.pool.exec(self, id, spec).await
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("an SSH host has no usage Stowbox can read".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("an SSH host has no billing".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(pairs: &[(&str, &str)]) -> Credentials {
        Credentials::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    #[test]
    fn a_target_takes_a_host_a_user_and_maybe_a_port() {
        let t = Target::from_credentials(&creds(&[(HOST, "box.example.com"), (USER, "ubuntu")])).unwrap();
        assert_eq!(t, Target { host: "box.example.com".into(), port: 22, user: "ubuntu".into() });
        let t = Target::from_credentials(&creds(&[(HOST, "2001:db8::1"), (USER, "me"), (PORT, "2222")])).unwrap();
        assert_eq!((t.host.as_str(), t.port), ("2001:db8::1", 2222));
        assert!(Target::from_credentials(&creds(&[(HOST, "10.0.0.2"), (USER, "me"), (PORT, "0")])).is_err());
        assert!(Target::from_credentials(&creds(&[(HOST, "10.0.0.2"), (USER, "me"), (PORT, "70000")])).is_err());
    }

    #[test]
    fn hostile_hosts_and_users_are_refused_before_ssh() {
        for host in ["", "-oProxyCommand=sh", ".hidden", "a b", "evil.com;rm", "host/path", "a@b", &"a".repeat(254)] {
            assert!(Target::from_credentials(&creds(&[(HOST, host), (USER, "me")])).is_err(), "{host}");
        }
        for user in ["", "-l", "root;id", "me@host", "a b", &"u".repeat(33)] {
            assert!(Target::from_credentials(&creds(&[(HOST, "h"), (USER, user)])).is_err(), "{user}");
        }
    }

    #[test]
    fn the_descriptor_takes_the_fields_the_target_reads() {
        let caps = capabilities();
        let c = creds(&[(HOST, "h.example"), (USER, "me")]);
        c.validate(&caps).unwrap();
        assert!(creds(&[(HOST, "h.example")]).validate(&caps).is_err(), "the user is required");
        assert!(creds(&[(HOST, "h"), (USER, "me"), ("api_key", "x")]).validate(&caps).is_err(), "no stray fields");
    }

    #[test]
    fn a_key_file_with_a_passphrase_is_refused() {
        let dir = crate::testing::temp_dir("stowbox-sshhost-key");
        let plain = dir.join("plain");
        sshexec::load_or_make_key(&plain).unwrap();
        assert!(read_key(&plain).is_ok());
        assert!(read_key(&dir.join("missing")).unwrap_err().to_string().contains("could not read"));
        std::fs::write(dir.join("junk"), "not a key").unwrap();
        assert!(read_key(&dir.join("junk")).is_err());
        std::fs::write(dir.join("huge"), vec![b'a'; 70 * 1024]).unwrap();
        assert!(read_key(&dir.join("huge")).unwrap_err().to_string().contains("not a private key file"));
        assert!(read_key(&dir).is_err(), "a directory is not a key");
        #[cfg(unix)]
        assert!(read_key(Path::new("/dev/zero")).unwrap_err().to_string().contains("not a private key file"));
        let locked = dir.join("locked");
        if std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "a passphrase", "-f"]).arg(&locked).status().is_ok_and(|s| s.success()) {
            assert!(read_key(&locked).unwrap_err().to_string().contains("has a passphrase"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pasted_key_is_refused_without_being_echoed() {
        let dir = crate::testing::temp_dir("stowbox-sshhost-paste");
        // Shaped like a pasted key (several lines) without being one.
        for pasted in ["first line of a key\nsecret-body-line", "an OPENSSH PRIVATE KEY secret-body-line"] {
            let err = connect(creds(&[(HOST, "h.example"), (USER, "me"), (KEY_PATH, pasted)]), &ConnectContext::new(&dir)).err().unwrap().to_string();
            assert!(err.contains("not the key itself") && !err.contains("secret-body-line"), "{err}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn its_own_key_says_which_line_to_authorize() {
        let dir = crate::testing::temp_dir("stowbox-sshhost-own");
        let h = SshHost::new(Target { host: "h.example".into(), port: 22, user: "me".into() }, None, &ConnectContext::new(&dir)).unwrap();
        assert!(h.key_refused.contains("authorized_keys for me on h.example: ssh-ed25519 "), "{}", h.key_refused);
        assert!(dir.join("ssh").join("sshhost_ed25519").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Against a throwaway local sshd: connect, exec, upload, and the
    /// lifecycle calls it does not have.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "starts a local sshd; run with --ignored"]
    async fn local_sshd_keeps_its_claims() {
        let dir = crate::testing::temp_dir("stowbox-sshhost-sshd");
        let context = ConnectContext::new(&dir);
        let own = sshexec::load_or_make_key(&context.ssh_file("sshhost_ed25519")).unwrap();
        let sshd = crate::testing::LocalSshd::start(&own.public_key().to_openssh().unwrap()).await;
        let target = Target { host: "127.0.0.1".into(), port: sshd.port, user: sshd.user.clone() };
        let p = SshHost::new(target, None, &context).unwrap();

        let outcome = crate::testing::conformance::run(&p).await;
        assert!(outcome.passed.contains(&"list"), "{outcome:?}");
        assert_eq!(p.whoami().await.unwrap().display_name, format!("{}@127.0.0.1", sshd.user));
        assert!(std::fs::read_to_string(context.ssh_file("sshhost_known_hosts")).unwrap().contains("ssh-ed25519"));

        let data: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let path = dir.join("uploaded.bin");
        assert_eq!(p.upload_file("127.0.0.1", path.to_str().unwrap(), data.clone()).await.unwrap(), data.len() as u64);
        assert_eq!(std::fs::read(&path).unwrap(), data);

        assert!(p.exec("other-host", ExecSpec::default()).await.is_err());
        // Connected: running without a probe. A fresh provider has no
        // connection, so it probes, and a stopped sshd reads as unknown.
        assert_eq!(p.get_machine("127.0.0.1").await.unwrap().status, MachineStatus::Running);
        let port = sshd.port;
        drop(sshd);
        let fresh = SshHost::new(Target { host: "127.0.0.1".into(), port, user: "me".into() }, None, &context).unwrap();
        assert_eq!(fresh.get_machine("127.0.0.1").await.unwrap().status, MachineStatus::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
