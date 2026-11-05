//! SSH access to a box from this computer, so VS Code or Cursor can open it
//! over Remote-SSH.
//!
//! boxd terminates every machine's SSH at a shared proxy: each machine has
//! its own port there (`ssh_port`), the user is `boxd`, and only a key
//! linked to the account (`LinkSshKey`) gets in. Editors resolve the host
//! through `~/.ssh/config`, so Stowbox keeps one `Host stowbox-<name>` per
//! box there, inside a block between `# BEGIN stowbox` and `# END stowbox`.
//! Nothing outside the markers is changed, byte for byte, and the previous
//! file is kept as `config.stowbox-backup` whenever the block changes.
//!
//! The key is Stowbox's own, `~/.ssh/stowbox/id_ed25519`, made with
//! `ssh-keygen` the first time and never replaced; the person's other keys
//! are not read or offered (`IdentitiesOnly`). The proxy's host key goes in
//! `~/.ssh/stowbox/known_hosts`, which only Stowbox writes, so the first
//! connection does not stop at a fingerprint prompt.
//!
//! Nothing here runs on its own: the shell calls [`setup`] when the person
//! asks to open a box in an editor, and [`plan`] only reads.

use crate::provider::Provider;
use crate::types::{Machine, MachineStatus};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "# BEGIN stowbox";
pub const END: &str = "# END stowbox";
/// The login boxd machines take SSH as.
pub const USER: &str = "boxd";
/// That user's home in the box, where a relative directory is resolved.
pub const REMOTE_HOME: &str = "/home/boxd";

const BLOCK_NOTE: &str = "# Managed by Stowbox: one Host per box opened in an editor. Edits between\n# these markers are replaced; everything outside them is left alone.";

/// The `Host` alias Stowbox writes for a box.
pub fn alias(machine_name: &str) -> String {
    let clean: String = machine_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    format!("stowbox-{clean}")
}

/// Where a box answers SSH.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub alias: String,
    pub host: String,
    pub port: u32,
    pub user: String,
}

impl Target {
    /// The box's address and port, or why it has none yet.
    pub fn of(m: &Machine) -> std::result::Result<Target, String> {
        if m.ssh_port == 0 {
            return Err(format!("boxd has not given {} an SSH port yet. Try again once the box is running.", m.name));
        }
        let domain = if m.access_domain.is_empty() { "boxd.sh" } else { m.access_domain.as_str() };
        Ok(Target { alias: alias(&m.name), host: format!("{}.{domain}", m.name), port: m.ssh_port, user: USER.into() })
    }

    /// A command that works without Stowbox's config, with any linked key.
    pub fn direct_command(&self) -> String {
        format!("ssh -p {} {}@{}", self.port, self.user, self.host)
    }

    pub fn alias_command(&self) -> String {
        format!("ssh {}", self.alias)
    }
}

/// The files Stowbox reads and writes. `dir` is the real `~/.ssh`; `shown`
/// is how paths appear in the config and in the UI (`~/.ssh`).
#[derive(Debug, Clone)]
pub struct Files {
    pub dir: PathBuf,
    pub shown: String,
}

impl Files {
    pub fn new(dir: impl Into<PathBuf>, shown: impl Into<String>) -> Self {
        Self { dir: dir.into(), shown: shown.into() }
    }
    pub fn config(&self) -> PathBuf {
        self.dir.join("config")
    }
    pub fn backup(&self) -> PathBuf {
        self.dir.join("config.stowbox-backup")
    }
    pub fn key(&self) -> PathBuf {
        self.dir.join("stowbox").join("id_ed25519")
    }
    pub fn pubkey(&self) -> PathBuf {
        self.dir.join("stowbox").join("id_ed25519.pub")
    }
    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join("stowbox").join("known_hosts")
    }
    pub fn show(&self, rel: &str) -> String {
        format!("{}/{rel}", self.shown.trim_end_matches(['/', '\\']))
    }
}

/// A path as an ssh_config argument: quoted when it holds a space.
fn arg(path: &str) -> String {
    if path.contains(char::is_whitespace) {
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

/// One `Host` stanza for `t`. Without the proxy's host key, the first
/// connection records whatever the proxy presents (`accept-new`).
pub fn stanza(t: &Target, files: &Files, host_key_known: bool) -> String {
    let mut s = format!(
        "Host {alias}\n  HostName {host}\n  Port {port}\n  User {user}\n  IdentityFile {key}\n  IdentitiesOnly yes\n  HostKeyAlias {alias}\n  UserKnownHostsFile {kh}\n",
        alias = t.alias,
        host = t.host,
        port = t.port,
        user = t.user,
        key = arg(&files.show("stowbox/id_ed25519")),
        kh = arg(&files.show("stowbox/known_hosts")),
    );
    if !host_key_known {
        s.push_str("  StrictHostKeyChecking accept-new\n");
    }
    s
}

/// Byte range of the managed block (from `# BEGIN stowbox` through the line
/// ending of `# END stowbox`), if the file has one.
fn block_range(text: &str) -> Option<(usize, usize)> {
    let mut start = None;
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if start.is_none() && t == BEGIN {
            start = Some(pos);
        } else if start.is_some() && t == END {
            return Some((start.unwrap(), pos + line.len()));
        }
        pos += line.len();
    }
    None
}

/// The stanzas in the managed block, as `(alias, text)`, in file order.
fn stanzas(block: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in block.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut words = t.split_whitespace();
        if words.next().is_some_and(|w| w.eq_ignore_ascii_case("host")) {
            let name = words.next().unwrap_or("").to_string();
            if name == "*" {
                continue;
            }
            out.push((name, format!("{t}\n")));
        } else if let Some(last) = out.last_mut() {
            last.1.push_str(&format!("  {t}\n"));
        }
    }
    out
}

/// The aliases in the managed block of `config`.
pub fn hosts(config: &str) -> Vec<String> {
    block_range(config).map(|(a, b)| stanzas(&config[a..b]).into_iter().map(|(n, _)| n).collect()).unwrap_or_default()
}

/// `config` with `stanza` for `alias` in the managed block: replaced where it
/// was, added after the others otherwise. A file without the block gets it
/// at the top, so the first-match rule of ssh_config gives its values
/// precedence over a `Host *` further down; it ends with a bare `Host *`
/// so lines that were global at the top of the file stay global. Bytes
/// outside the block are never changed.
pub fn merge(config: &str, alias: &str, stanza: &str) -> String {
    let nl = if config.contains("\r\n") { "\r\n" } else { "\n" };
    let (before, existing, after) = match block_range(config) {
        Some((a, b)) => (&config[..a], stanzas(&config[a..b]), &config[b..]),
        None => ("", Vec::new(), config),
    };
    let mut list = existing;
    match list.iter_mut().find(|(n, _)| n == alias) {
        Some(entry) => entry.1 = stanza.to_string(),
        None => list.push((alias.to_string(), stanza.to_string())),
    }
    let mut block = format!("{BEGIN}\n{BLOCK_NOTE}\n");
    for (_, s) in &list {
        block.push_str(s);
    }
    block.push_str("Host *\n");
    block.push_str(END);
    block.push('\n');
    if nl != "\n" {
        block = block.replace('\n', nl);
    }
    let mut out = String::with_capacity(config.len() + block.len() + 2);
    out.push_str(before);
    out.push_str(&block);
    if before.is_empty() && !after.is_empty() && !after.starts_with(['\n', '\r']) && block_range(config).is_none() {
        // A blank line between the new block and what was there.
        out.push_str(nl);
    }
    out.push_str(after);
    out
}

/// Stowbox's known_hosts with `key` recorded for `alias` (other lines kept).
pub fn merge_known_hosts(existing: &str, alias: &str, key: &str) -> String {
    let mut out: String = existing
        .lines()
        .filter(|l| !l.trim().is_empty() && l.split_whitespace().next() != Some(alias))
        .map(|l| format!("{l}\n"))
        .collect();
    out.push_str(&format!("{alias} {}\n", key.trim()));
    out
}

/// An agent's or a box's directory as an absolute path in the box: `~` and
/// relative paths are under `/home/boxd`.
pub fn remote_path(dir: Option<&str>) -> String {
    let d = dir.map(str::trim).unwrap_or("");
    let p = if d.is_empty() || d == "~" {
        REMOTE_HOME.to_string()
    } else if let Some(rest) = d.strip_prefix("~/") {
        format!("{REMOTE_HOME}/{rest}")
    } else if d.starts_with('/') {
        d.to_string()
    } else {
        format!("{REMOTE_HOME}/{d}")
    };
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() { "/".into() } else { trimmed.to_string() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Editor {
    Vscode,
    Cursor,
}

impl Editor {
    pub fn label(self) -> &'static str {
        match self {
            Editor::Vscode => "VS Code",
            Editor::Cursor => "Cursor",
        }
    }
    fn scheme(self) -> &'static str {
        match self {
            Editor::Vscode => "vscode",
            Editor::Cursor => "cursor",
        }
    }
}

/// The Remote-SSH deep link that opens `path` on `alias` in a new window.
pub fn editor_url(editor: Editor, alias: &str, path: &str) -> String {
    let mut enc = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{}://vscode-remote/ssh-remote+{alias}{enc}?windowId=_blank", editor.scheme())
}

/// What opening a box in an editor would do on this computer, read only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub alias: String,
    /// `ssh <alias>` once the config is current, else the direct command.
    pub ssh_command: String,
    pub direct_command: String,
    pub key_path: String,
    pub key_exists: bool,
    pub config_path: String,
    pub backup_path: String,
    /// "add", "update" or "current".
    pub config: String,
    /// Why the box cannot be opened over SSH right now.
    pub blocker: Option<String>,
}

fn read(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(Error::Provider(format!("could not read {}: {e}", path.display()))),
    }
}

/// Whether Stowbox's known_hosts holds a key for `alias` (written by
/// [`setup`], or by ssh itself on a first `accept-new` connection).
fn host_key_recorded(files: &Files, alias: &str) -> bool {
    read(&files.known_hosts()).map(|t| t.lines().any(|l| l.split_whitespace().next() == Some(alias))).unwrap_or(false)
}

pub fn plan(files: &Files, m: &Machine) -> Result<Plan> {
    let target = Target::of(m);
    let alias = alias(&m.name);
    let mut blocker = target.as_ref().err().cloned();
    if blocker.is_none() && matches!(m.status, MachineStatus::Stopped | MachineStatus::Failed) {
        blocker = Some(format!("{} is {}. Start it first.", m.name, if m.status == MachineStatus::Failed { "failed" } else { "stopped" }));
    }
    let config_text = read(&files.config())?;
    let config = match &target {
        Ok(t) => {
            let block = block_range(&config_text).map(|(a, b)| stanzas(&config_text[a..b])).unwrap_or_default();
            let want = stanza(t, files, host_key_recorded(files, &alias));
            match block.iter().find(|(n, _)| *n == alias) {
                None => "add",
                Some((_, s)) if *s == want => "current",
                Some(_) => "update",
            }
        }
        Err(_) => if hosts(&config_text).contains(&alias) { "update" } else { "add" },
    };
    let direct = target.as_ref().map(Target::direct_command).unwrap_or_default();
    Ok(Plan {
        ssh_command: if config == "current" { format!("ssh {alias}") } else { direct.clone() },
        direct_command: direct,
        alias,
        key_path: files.show("stowbox/id_ed25519"),
        key_exists: files.key().exists(),
        config_path: files.show("config"),
        backup_path: files.show("config.stowbox-backup"),
        config: config.into(),
        blocker,
    })
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result<()> {
    Ok(())
}

fn ensure_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        set_mode(dir, 0o700)?;
    }
    Ok(())
}

/// Writes `text` to `path` through a temporary file and a rename, keeping
/// the old file's permissions (0600 for a new one). A symlinked file, as
/// dotfile managers leave `~/.ssh/config`, is written through to its target.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let real = match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let dir = real.parent().ok_or_else(|| Error::Provider(format!("{} has no parent directory", real.display())))?;
    ensure_dir(dir)?;
    let tmp = dir.join(format!(".{}.stowbox-tmp", real.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
    std::fs::write(&tmp, text)?;
    match std::fs::metadata(&real) {
        Ok(md) => std::fs::set_permissions(&tmp, md.permissions())?,
        Err(_) => set_mode(&tmp, 0o600)?,
    }
    std::fs::rename(&tmp, &real).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Stowbox's public key, made with `keygen` (normally `ssh-keygen`) when
/// there is none. An existing private key is never replaced; a missing
/// `.pub` next to it is derived from it. Returns the key line and whether
/// a new key was made.
pub fn ensure_key(files: &Files, keygen: &str, comment: &str) -> Result<(String, bool)> {
    let key = files.key();
    let pubkey = files.pubkey();
    let missing = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::Provider(format!("{keygen} was not found. Install the OpenSSH client (on Windows: Settings > System > Optional features > OpenSSH Client), then try again."))
        } else {
            Error::Provider(format!("could not run {keygen}: {e}"))
        }
    };
    if key.exists() {
        if let Ok(line) = std::fs::read_to_string(&pubkey) {
            if !line.trim().is_empty() {
                return Ok((line.trim().to_string(), false));
            }
        }
        let out = std::process::Command::new(keygen).arg("-y").arg("-f").arg(&key).output().map_err(missing)?;
        if !out.status.success() {
            return Err(Error::Provider(format!("{} exists but its public key could not be read: {}", files.show("stowbox/id_ed25519"), String::from_utf8_lossy(&out.stderr).trim())));
        }
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        write_atomic(&pubkey, &format!("{line}\n"))?;
        return Ok((line, false));
    }
    ensure_dir(&files.dir)?;
    ensure_dir(key.parent().unwrap_or(&files.dir))?;
    if pubkey.exists() {
        // A public key without its private key is useless and would make
        // ssh-keygen stop at an overwrite prompt. Set it aside, not delete it.
        std::fs::rename(&pubkey, files.dir.join("stowbox").join("id_ed25519.pub.orphan"))?;
    }
    let out = std::process::Command::new(keygen)
        .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
        .arg(&key)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(missing)?;
    if !out.status.success() {
        return Err(Error::Provider(format!("{keygen} failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let line = std::fs::read_to_string(&pubkey)?.trim().to_string();
    Ok((line, true))
}

/// What [`setup`] changed, in the words the UI shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub alias: String,
    pub changes: Vec<String>,
}

/// Who this computer is to the provider, so re-linking replaces the key
/// Stowbox linked before instead of adding another.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub label: String,
}

/// Makes `m` reachable as `ssh <alias>`: the key (made if missing), linked
/// to the account, the Host stanza in the managed block and the proxy's
/// host key. Idempotent; files are only written when their text changes.
pub async fn setup(provider: &dyn Provider, files: &Files, m: &Machine, device: &Device, keygen: &str) -> Result<Applied> {
    let target = Target::of(m).map_err(Error::Provider)?;
    let mut changes = Vec::new();

    let (pubkey, created) = ensure_key(files, keygen, &format!("stowbox@{}", device.label))?;
    if created {
        changes.push(format!("Created an SSH key for Stowbox at {} (your other keys are untouched)", files.show("stowbox/id_ed25519")));
    }
    provider.link_ssh_key(&pubkey, &device.id, &format!("Stowbox on {}", device.label)).await?;
    if created {
        changes.push(format!("Linked it to your {} account", provider.id()));
    }

    let host_key = provider.ssh_host_key().await.ok().flatten().filter(|k| !k.trim().is_empty());
    if let Some(key) = &host_key {
        let path = files.known_hosts();
        let old = read(&path)?;
        let new = merge_known_hosts(&old, &target.alias, key);
        if new != old {
            write_atomic(&path, &new)?;
        }
    }

    let path = files.config();
    let old = read(&path)?;
    let known = host_key.is_some() || host_key_recorded(files, &target.alias);
    let new = merge(&old, &target.alias, &stanza(&target, files, known));
    if new != old {
        if path.exists() {
            let real = std::fs::canonicalize(&path)?;
            std::fs::copy(&real, files.backup())?;
            set_mode(&files.backup(), 0o600)?;
        }
        write_atomic(&path, &new)?;
        let verb = if hosts(&old).contains(&target.alias) { "Updated" } else { "Added" };
        let backup = if old.is_empty() { String::new() } else { format!("; the previous file is at {}", files.show("config.stowbox-backup")) };
        changes.push(format!("{verb} Host {} in {}{backup}", target.alias, files.show("config")));
    }
    Ok(Applied { alias: target.alias, changes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{machine, temp_dir};
    use std::sync::Mutex;

    fn box_named(name: &str, port: u32) -> Machine {
        let mut m = machine("vm_1", MachineStatus::Running);
        m.name = name.into();
        m.access_domain = "boxd.sh".into();
        m.ssh_port = port;
        m
    }

    fn files(dir: &Path) -> Files {
        Files::new(dir, "~/.ssh")
    }

    #[test]
    fn target_and_commands() {
        let t = Target::of(&box_named("api", 21843)).unwrap();
        assert_eq!(t.alias, "stowbox-api");
        assert_eq!(t.host, "api.boxd.sh");
        assert_eq!(t.direct_command(), "ssh -p 21843 boxd@api.boxd.sh");
        assert_eq!(t.alias_command(), "ssh stowbox-api");
        let mut custom = box_named("web", 1);
        custom.access_domain = "preview.example.com".into();
        assert_eq!(Target::of(&custom).unwrap().host, "web.preview.example.com");
        assert!(Target::of(&box_named("api", 0)).unwrap_err().contains("SSH port"));
        assert_eq!(alias("My Box"), "stowbox-my-box");
    }

    #[test]
    fn stanza_shape() {
        let t = Target::of(&box_named("api", 21843)).unwrap();
        let s = stanza(&t, &files(Path::new("/x")), true);
        assert_eq!(
            s,
            "Host stowbox-api\n  HostName api.boxd.sh\n  Port 21843\n  User boxd\n  IdentityFile ~/.ssh/stowbox/id_ed25519\n  IdentitiesOnly yes\n  HostKeyAlias stowbox-api\n  UserKnownHostsFile ~/.ssh/stowbox/known_hosts\n"
        );
        assert!(stanza(&t, &files(Path::new("/x")), false).ends_with("StrictHostKeyChecking accept-new\n"));
        let spaced = stanza(&t, &Files::new("/x", "C:/Users/Ann Lee/.ssh"), true);
        assert!(spaced.contains("IdentityFile \"C:/Users/Ann Lee/.ssh/stowbox/id_ed25519\"\n"));
    }

    const A: &str = "Host stowbox-a\n  HostName a.boxd.sh\n  Port 1\n";
    const B: &str = "Host stowbox-b\n  HostName b.boxd.sh\n  Port 2\n";

    #[test]
    fn merge_into_an_empty_file() {
        let out = merge("", "stowbox-a", A);
        assert!(out.starts_with("# BEGIN stowbox\n"));
        assert!(out.ends_with("Host *\n# END stowbox\n"));
        assert_eq!(hosts(&out), vec!["stowbox-a"]);
    }

    #[test]
    fn merge_puts_the_block_first_and_keeps_the_rest_byte_for_byte() {
        let user = "ServerAliveInterval 60\n\nHost github.com\n    User git\n    IdentityFile ~/.ssh/gh\n# trailing comment, no newline";
        let out = merge(user, "stowbox-a", A);
        let (_, end) = block_range(&out).unwrap();
        // One blank line, then the person's file unchanged.
        assert_eq!(&out[end..], format!("\n{user}"));
        // The global line now sits after `Host *`, so it is still global.
        let block = &out[..end];
        assert!(block.trim_end().ends_with("Host *\n# END stowbox"));
    }

    #[test]
    fn merge_replaces_in_place_and_is_idempotent() {
        let user_top = "Include ~/.colima/ssh_config\n\n";
        let user_bottom = "\nHost *\n  AddKeysToAgent yes\n";
        let first = merge("", "stowbox-a", A);
        let file = format!("{user_top}{first}{user_bottom}");
        let two = merge(&file, "stowbox-b", B);
        assert_eq!(hosts(&two), vec!["stowbox-a", "stowbox-b"]);
        assert!(two.starts_with(user_top) && two.ends_with(user_bottom));
        // Same stanza again: nothing changes.
        assert_eq!(merge(&two, "stowbox-b", B), two);
        assert_eq!(merge(&two, "stowbox-a", A), two);
        // A new port for a: replaced where it was, b kept, order kept.
        let a2 = "Host stowbox-a\n  HostName a.boxd.sh\n  Port 9\n";
        let three = merge(&two, "stowbox-a", a2);
        assert_eq!(hosts(&three), vec!["stowbox-a", "stowbox-b"]);
        assert!(three.contains("Port 9\n") && !three.contains("Port 1\n") && three.contains("Port 2\n"));
        assert!(three.starts_with(user_top) && three.ends_with(user_bottom));
        assert_eq!(three.matches(BEGIN).count(), 1);
    }

    #[test]
    fn merge_keeps_crlf_files_crlf() {
        let user = "Host x\r\n  User y\r\n";
        let out = merge(user, "stowbox-a", A);
        assert!(out.ends_with(user));
        assert!(!out.replace("\r\n", "").contains('\n'));
        assert_eq!(merge(&out, "stowbox-a", A), out);
    }

    #[test]
    fn known_hosts_upsert() {
        let one = merge_known_hosts("", "stowbox-a", "ssh-ed25519 AAA1\n");
        assert_eq!(one, "stowbox-a ssh-ed25519 AAA1\n");
        let two = merge_known_hosts(&one, "stowbox-b", "ssh-ed25519 BBB");
        let again = merge_known_hosts(&two, "stowbox-a", "ssh-ed25519 AAA2");
        assert_eq!(again, "stowbox-b ssh-ed25519 BBB\nstowbox-a ssh-ed25519 AAA2\n");
    }

    #[test]
    fn remote_paths_and_urls() {
        assert_eq!(remote_path(None), "/home/boxd");
        assert_eq!(remote_path(Some("~")), "/home/boxd");
        assert_eq!(remote_path(Some("~/app/")), "/home/boxd/app");
        assert_eq!(remote_path(Some("app")), "/home/boxd/app");
        assert_eq!(remote_path(Some("/srv/api-wt/fix-login")), "/srv/api-wt/fix-login");
        assert_eq!(remote_path(Some("/")), "/");
        assert_eq!(editor_url(Editor::Vscode, "stowbox-api", "/home/boxd/my app"), "vscode://vscode-remote/ssh-remote+stowbox-api/home/boxd/my%20app?windowId=_blank");
        assert_eq!(editor_url(Editor::Cursor, "stowbox-api", "/home/boxd"), "cursor://vscode-remote/ssh-remote+stowbox-api/home/boxd?windowId=_blank");
    }

    /// Records what a provider was asked to link.
    #[derive(Default)]
    struct Fake {
        linked: Mutex<Vec<(String, String)>>,
        host_key: Option<String>,
    }

    #[async_trait::async_trait]
    impl Provider for Fake {
        fn id(&self) -> &'static str {
            "boxd"
        }
        async fn whoami(&self) -> Result<crate::types::Account> {
            unimplemented!()
        }
        async fn list_machines(&self) -> Result<Vec<Machine>> {
            unimplemented!()
        }
        async fn get_machine(&self, _: &str) -> Result<Machine> {
            unimplemented!()
        }
        async fn create_machine(&self, _: crate::types::CreateSpec) -> Result<Machine> {
            unimplemented!()
        }
        async fn fork_machine(&self, _: &str, _: Option<String>) -> Result<Machine> {
            unimplemented!()
        }
        async fn power(&self, _: &str, _: crate::types::PowerAction) -> Result<()> {
            unimplemented!()
        }
        async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
            unimplemented!()
        }
        async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
            unimplemented!()
        }
        async fn exec(&self, _: &str, _: crate::types::ExecSpec) -> Result<crate::types::ExecHandle> {
            unimplemented!()
        }
        async fn machine_usage(&self, _: &str) -> Result<crate::types::Usage> {
            unimplemented!()
        }
        async fn org_billing(&self, _: &str) -> Result<crate::types::OrgBilling> {
            unimplemented!()
        }
        async fn rates(&self) -> Result<(String, crate::types::Rates)> {
            unimplemented!()
        }
        async fn link_ssh_key(&self, pubkey: &str, device_id: &str, _: &str) -> Result<()> {
            self.linked.lock().unwrap().push((pubkey.into(), device_id.into()));
            Ok(())
        }
        async fn ssh_host_key(&self) -> Result<Option<String>> {
            Ok(self.host_key.clone())
        }
    }

    fn has_keygen() -> bool {
        std::process::Command::new("ssh-keygen").arg("-?").output().is_ok()
    }

    #[test]
    fn missing_keygen_names_the_step() {
        let dir = temp_dir("stowbox-ssh");
        let e = ensure_key(&files(&dir), "stowbox-no-such-keygen", "c").unwrap_err().to_string();
        assert!(e.contains("was not found") && e.contains("OpenSSH"), "{e}");
        assert!(!files(&dir).key().exists());
    }

    #[tokio::test]
    async fn setup_end_to_end_in_a_temp_dir() {
        if !has_keygen() {
            eprintln!("skipped: no ssh-keygen");
            return;
        }
        // Never the real ~/.ssh: a fresh directory per run.
        let dir = temp_dir("stowbox-ssh").join(".ssh");
        let f = files(&dir);
        let p = Fake { host_key: Some("ssh-ed25519 AAAAproxy".into()), ..Default::default() };
        let device = Device { id: "dev-1".into(), label: "laptop".into() };
        let m = box_named("api", 21843);

        let plan0 = plan(&f, &m).unwrap();
        assert_eq!((plan0.config.as_str(), plan0.key_exists, plan0.blocker.clone()), ("add", false, None));
        assert_eq!(plan0.ssh_command, "ssh -p 21843 boxd@api.boxd.sh");

        let first = setup(&p, &f, &m, &device, "ssh-keygen").await.unwrap();
        assert_eq!(first.alias, "stowbox-api");
        assert_eq!(first.changes.len(), 3, "{:?}", first.changes);
        assert!(first.changes[0].starts_with("Created an SSH key"));
        let pubkey = std::fs::read_to_string(f.pubkey()).unwrap();
        assert!(pubkey.starts_with("ssh-ed25519 ") && pubkey.trim_end().ends_with("stowbox@laptop"));
        assert_eq!(p.linked.lock().unwrap().clone(), vec![(pubkey.trim().to_string(), "dev-1".to_string())]);
        let config = std::fs::read_to_string(f.config()).unwrap();
        assert_eq!(hosts(&config), vec!["stowbox-api"]);
        assert!(!config.contains("StrictHostKeyChecking"));
        assert_eq!(std::fs::read_to_string(f.known_hosts()).unwrap(), "stowbox-api ssh-ed25519 AAAAproxy\n");
        assert!(!f.backup().exists(), "no backup of a file that did not exist");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(f.config()).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(f.key()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(plan(&f, &m).unwrap().config, "current");
        assert_eq!(plan(&f, &m).unwrap().ssh_command, "ssh stowbox-api");

        // The person edits their config outside the block; a second run
        // changes nothing, keeps the key and still links (idempotent there).
        let key_before = std::fs::read(f.key()).unwrap();
        let edited = format!("{config}\nHost github.com\n  User git\n");
        std::fs::write(f.config(), &edited).unwrap();
        let second = setup(&p, &f, &m, &device, "ssh-keygen").await.unwrap();
        assert!(second.changes.is_empty(), "{:?}", second.changes);
        assert_eq!(std::fs::read_to_string(f.config()).unwrap(), edited);
        assert_eq!(std::fs::read(f.key()).unwrap(), key_before);
        assert_eq!(p.linked.lock().unwrap().len(), 2);

        // The box moves to another port: the stanza is updated in place and
        // the file before that change is the backup.
        let moved = box_named("api", 22000);
        assert_eq!(plan(&f, &moved).unwrap().config, "update");
        let third = setup(&p, &f, &moved, &device, "ssh-keygen").await.unwrap();
        assert_eq!(third.changes.len(), 1);
        assert!(third.changes[0].starts_with("Updated Host stowbox-api in ~/.ssh/config; the previous file is at ~/.ssh/config.stowbox-backup"));
        assert_eq!(std::fs::read_to_string(f.backup()).unwrap(), edited);
        let now = std::fs::read_to_string(f.config()).unwrap();
        assert!(now.contains("Port 22000") && now.ends_with("\nHost github.com\n  User git\n"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn setup_writes_through_a_symlinked_config_and_keeps_its_mode() {
        if !has_keygen() {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("stowbox-ssh");
        let dir = root.join(".ssh");
        std::fs::create_dir_all(&dir).unwrap();
        let dotfiles = root.join("dotfiles-ssh-config");
        std::fs::write(&dotfiles, "Host old\n  User me\n").unwrap();
        std::fs::set_permissions(&dotfiles, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&dotfiles, dir.join("config")).unwrap();
        let f = files(&dir);
        let p = Fake::default();
        let device = Device { id: "d".into(), label: "l".into() };
        setup(&p, &f, &box_named("api", 1), &device, "ssh-keygen").await.unwrap();
        assert!(std::fs::symlink_metadata(f.config()).unwrap().file_type().is_symlink());
        let text = std::fs::read_to_string(&dotfiles).unwrap();
        assert!(text.contains("Host stowbox-api") && text.ends_with("Host old\n  User me\n"));
        // No host key known: the first connection records it.
        assert!(text.contains("StrictHostKeyChecking accept-new"));
        assert_eq!(std::fs::metadata(&dotfiles).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(std::fs::read_to_string(f.backup()).unwrap(), "Host old\n  User me\n");
    }

    #[test]
    fn existing_key_is_never_replaced() {
        if !has_keygen() {
            return;
        }
        let dir = temp_dir("stowbox-ssh");
        let f = files(&dir);
        let (line, created) = ensure_key(&f, "ssh-keygen", "first").unwrap();
        assert!(created);
        let private = std::fs::read(f.key()).unwrap();
        // The .pub is lost: it is derived from the private key, not remade.
        std::fs::remove_file(f.pubkey()).unwrap();
        let (again, created) = ensure_key(&f, "ssh-keygen", "second").unwrap();
        assert!(!created);
        assert_eq!(std::fs::read(f.key()).unwrap(), private);
        assert_eq!(again.split_whitespace().nth(1), line.split_whitespace().nth(1));
    }

    #[test]
    fn plan_blocks_a_stopped_box_or_one_without_a_port() {
        let dir = temp_dir("stowbox-ssh");
        let mut m = box_named("api", 5);
        m.status = MachineStatus::Stopped;
        assert!(plan(&files(&dir), &m).unwrap().blocker.unwrap().contains("Start it first"));
        assert!(plan(&files(&dir), &box_named("api", 0)).unwrap().blocker.unwrap().contains("SSH port"));
    }
}
