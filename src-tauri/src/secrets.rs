//! Where the provider API keys live, one entry per provider. OS keychain first; a 0600 file in the
//! app data dir when no keychain is reachable (headless Linux, some CI), and
//! the UI says which one is in use.
//!
//! On Windows the keychain is the Credential Manager, which holds at most
//! 2560 bytes per entry (1280 characters, stored as UTF-16): enough for the
//! API key, not always for a preset's secrets (a private key as a value), so
//! those can land in the file. Windows has no mode bits; the file is in the
//! per-user app data folder (`%APPDATA%\sh.stowbox.desktop`), whose
//! inherited ACL gives access to the user, SYSTEM and administrators only.

use std::collections::HashMap;
use std::path::PathBuf;

const SERVICE: &str = "sh.stowbox.desktop";

/// Writes `bytes` to `path` readable by the owner only. A new file is
/// created 0600, so the value is never readable by others, not even for a
/// moment; an older file with wider bits is narrowed before the write.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode(0o600);
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    use std::io::Write;
    opts.open(path)?.write_all(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Keyring,
    File,
}

pub struct Secrets {
    account: String,
    file: PathBuf,
}

impl Secrets {
    /// The key of `provider`: keychain account `<provider>-api-key`, file
    /// of the same name. boxd's entry keeps the name it always had.
    pub fn new(data_dir: PathBuf, provider: &str) -> Self {
        let account = format!("{provider}-api-key");
        Self { file: data_dir.join(&account), account }
    }

    pub fn load(&self) -> Option<(String, Backend)> {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &self.account) {
            if let Ok(v) = entry.get_password() {
                if !v.is_empty() {
                    return Some((v, Backend::Keyring));
                }
            }
        }
        std::fs::read_to_string(&self.file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(|s| (s, Backend::File))
    }

    pub fn save(&self, value: &str) -> std::io::Result<Backend> {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &self.account) {
            if entry.set_password(value).is_ok() {
                let _ = std::fs::remove_file(&self.file);
                return Ok(Backend::Keyring);
            }
            let _ = entry.delete_credential();
        }
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_private(&self.file, value.as_bytes())?;
        Ok(Backend::File)
    }

    pub fn clear(&self) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &self.account) {
            let _ = entry.delete_credential();
        }
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Secret values of setup presets, one keychain entry per preset holding a
/// JSON object `{ "NAME": "value" }`. Without a keychain, one 0600 file per
/// preset under `preset-secrets/`. The UI never reads values back.
pub struct PresetSecrets {
    dir: PathBuf,
}

impl PresetSecrets {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { dir: data_dir.join("preset-secrets") }
    }

    fn account(id: &str) -> String {
        format!("preset:{id}")
    }

    fn file(&self, id: &str) -> PathBuf {
        // Preset ids are UUIDs minted here; keep only safe characters anyway.
        let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        self.dir.join(format!("{safe}.json"))
    }

    pub fn load(&self, id: &str) -> HashMap<String, String> {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &Self::account(id)) {
            if let Ok(v) = entry.get_password() {
                if let Ok(map) = serde_json::from_str(&v) {
                    return map;
                }
            }
        }
        std::fs::read_to_string(self.file(id)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, id: &str, values: &HashMap<String, String>) -> std::io::Result<Backend> {
        let json = serde_json::to_string(values).map_err(std::io::Error::other)?;
        if let Ok(entry) = keyring::Entry::new(SERVICE, &Self::account(id)) {
            if entry.set_password(&json).is_ok() {
                let _ = std::fs::remove_file(self.file(id));
                return Ok(Backend::Keyring);
            }
            // Refused (too long for Windows' Credential Manager, for one):
            // drop the older entry, or `load` would keep returning it.
            let _ = entry.delete_credential();
        }
        std::fs::create_dir_all(&self.dir)?;
        write_private(&self.file(id), json.as_bytes())?;
        Ok(Backend::File)
    }

    pub fn delete(&self, id: &str) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &Self::account(id)) {
            let _ = entry.delete_credential();
        }
        let _ = std::fs::remove_file(self.file(id));
    }
}
