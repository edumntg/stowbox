//! A local, append-only record of what Stowbox did with credentials and
//! boxes: API keys saved and cleared, preset secrets changed, setup runs
//! (which secret names went into which box) and power actions. Names only:
//! a secret value or an API key never reaches this file.
//!
//! One JSON object per line in `audit.jsonl`, created 0600. Past
//! `MAX_BYTES` the file moves to `audit.jsonl.1` (replacing the older one)
//! and a new one starts, so the record stays bounded on disk.

use crate::provision::RunStatus;
use crate::types::PowerAction;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE: &str = "audit.jsonl";
pub const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Event {
    KeySaved { provider: String, backend: String },
    KeyCleared { provider: String },
    /// `changed` holds the secrets that got a new value in this save.
    PresetSaved { preset_id: String, preset_name: String, secret_names: Vec<String>, changed: Vec<String> },
    PresetDeleted { preset_id: String, preset_name: String },
    SetupStarted { run_id: String, machine_id: String, machine_name: String, preset_id: String, preset_name: String, secret_names: Vec<String> },
    SetupEnded { run_id: String, machine_id: String, status: RunStatus, secrets_written: bool },
    BoxCreated { provider: String, machine_id: String, machine_name: String },
    BoxForked { source: String, machine_id: String, machine_name: String },
    Power { machine_id: String, power: PowerAction },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Record<'a> {
    at_ms: i64,
    #[serde(flatten)]
    event: &'a Event,
}

pub struct AuditLog {
    path: PathBuf,
    max_bytes: u64,
    /// One append at a time, so lines never interleave and a rotation
    /// never races a write.
    writing: std::sync::Mutex<()>,
}

impl AuditLog {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self::with_limit(dir, MAX_BYTES)
    }

    pub fn with_limit(dir: impl AsRef<Path>, max_bytes: u64) -> Self {
        Self { path: dir.as_ref().join(FILE), max_bytes, writing: std::sync::Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn record(&self, event: &Event, now_ms: i64) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(&Record { at_ms: now_ms, event }).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let _one_at_a_time = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() + line.len() as u64 > self.max_bytes) {
            std::fs::rename(&self.path, self.path.with_extension("jsonl.1"))?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(&self.path)?.write_all(&line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        std::env::temp_dir().join(format!("stowbox-audit-{}", uuid::Uuid::new_v4().simple()))
    }

    #[test]
    fn appends_one_json_line_per_event() {
        let d = dir();
        let log = AuditLog::new(&d);
        log.record(&Event::KeySaved { provider: "boxd".into(), backend: "keyring".into() }, 1).unwrap();
        log.record(&Event::Power { machine_id: "vm1".into(), power: PowerAction::Destroy }, 2).unwrap();
        let text = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0], serde_json::json!({ "atMs": 1, "action": "key-saved", "provider": "boxd", "backend": "keyring" }));
        assert_eq!(lines[1], serde_json::json!({ "atMs": 2, "action": "power", "machineId": "vm1", "power": "destroy" }));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(log.path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rotates_past_the_limit_keeping_one_older_file() {
        let d = dir();
        let log = AuditLog::with_limit(&d, 200);
        let ev = Event::KeyCleared { provider: "daytona".into() };
        for i in 0..10 {
            log.record(&ev, i).unwrap();
        }
        let current = std::fs::read_to_string(log.path()).unwrap();
        let older = std::fs::read_to_string(d.join("audit.jsonl.1")).unwrap();
        assert!(current.len() <= 200 && older.len() <= 200);
        // The newest event is in the current file, the ones just before it in the older one.
        assert!(current.contains("\"atMs\":9"));
        assert!(!older.is_empty() && !older.contains("\"atMs\":9"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
