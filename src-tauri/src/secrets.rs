//! Where the provider API key lives. OS keychain first; a 0600 file in the
//! app data dir when no keychain is reachable (headless Linux, some CI), and
//! the UI says which one is in use.

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
