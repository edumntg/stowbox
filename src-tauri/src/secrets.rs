//! Where the provider API key lives. OS keychain first; a 0600 file in the
//! app data dir when no keychain is reachable (headless Linux, some CI), and
//! the UI says which one is in use.

use std::collections::HashMap;
use std::path::PathBuf;

const SERVICE: &str = "sh.stowbox.desktop";
const ACCOUNT: &str = "boxd-api-key";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Keyring,
    File,
}

pub struct Secrets {
    file: PathBuf,
}

impl Secrets {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { file: data_dir.join("boxd-api-key") }
    }

    pub fn load(&self) -> Option<(String, Backend)> {
        if let Ok(entry) = keyring::Entry::new(SERVICE, ACCOUNT) {
            if let Ok(v) = entry.get_password() {
                if !v.is_empty() {
                    return Some((v, Backend::Keyring));
                }
            }
        }
        std::fs::read_to_string(&self.file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| (s, Backend::File))
    }

    pub fn save(&self, value: &str) -> std::io::Result<Backend> {
        if let Ok(entry) = keyring::Entry::new(SERVICE, ACCOUNT) {
            if entry.set_password(value).is_ok() {
                let _ = std::fs::remove_file(&self.file);
                return Ok(Backend::Keyring);
            }
        }
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.file, value)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.file, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(Backend::File)
    }

    pub fn clear(&self) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, ACCOUNT) {
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
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            // Created 0600, so the values are never readable by others.
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        use std::io::Write;
        opts.open(self.file(id))?.write_all(json.as_bytes())?;
        Ok(Backend::File)
    }

    pub fn delete(&self, id: &str) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &Self::account(id)) {
            let _ = entry.delete_credential();
        }
        let _ = std::fs::remove_file(self.file(id));
    }
}
