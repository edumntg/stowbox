//! On-disk state: agents (`agents.json`), setup presets (`presets.json`,
//! names of secrets only) and the last setup run per box
//! (`setup-runs.json`). Writes are whole-file and small. What an agent did
//! lives in its harness's own session in the box, not here.

use crate::agent::AgentMeta;
use crate::provision::{Preset, ProvisionRun};
use crate::Result;
use std::path::{Path, PathBuf};

pub struct Store {
    dir: PathBuf,
    /// One write at a time: each goes through `<name>.tmp`, and two at once
    /// would rename the same temporary file twice (the second fails with
    /// "file not found").
    writing: tokio::sync::Mutex<()>,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), writing: tokio::sync::Mutex::new(()) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn agents_file(&self) -> PathBuf {
        self.dir.join("agents.json")
    }

    pub async fn load_agents(&self) -> Result<Vec<AgentMeta>> {
        match tokio::fs::read(self.agents_file()).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn save_agents(&self, agents: &[AgentMeta]) -> Result<()> {
        self.save_json("agents.json", agents).await
    }

    async fn load_json<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> Result<T> {
        match tokio::fs::read(self.dir.join(name)).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(e.into()),
        }
    }

    async fn save_json<T: serde::Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<()> {
        let _one_at_a_time = self.writing.lock().await;
        tokio::fs::create_dir_all(&self.dir).await?;
        let tmp = self.dir.join(format!("{name}.tmp"));
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(value)?).await?;
        tokio::fs::rename(&tmp, self.dir.join(name)).await?;
        Ok(())
    }

    pub async fn load_presets(&self) -> Result<Vec<Preset>> {
        self.load_json("presets.json").await
    }

    pub async fn save_presets(&self, presets: &[Preset]) -> Result<()> {
        self.save_json("presets.json", presets).await
    }

    pub async fn load_runs(&self) -> Result<Vec<ProvisionRun>> {
        self.load_json("setup-runs.json").await
    }

    pub async fn save_runs(&self, runs: &[ProvisionRun]) -> Result<()> {
        self.save_json("setup-runs.json", runs).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn presets_round_trip_and_default_to_empty() {
        let dir = std::env::temp_dir().join(format!("stowbox-store-{}", uuid::Uuid::new_v4().simple()));
        let store = Store::new(&dir);
        assert!(store.load_presets().await.unwrap().is_empty());
        let p = Preset {
            id: "p1".into(),
            name: "main commands".into(),
            commands: vec!["git clone https://example.com/r.git".into()],
            secret_names: vec!["GITHUB_TOKEN".into()],
            variables: vec![crate::provision::Variable { name: "branch".into(), default: "main".into() }],
            created_at: 1,
            updated_at: 2,
        };
        store.save_presets(std::slice::from_ref(&p)).await.unwrap();
        assert_eq!(store.load_presets().await.unwrap(), vec![p]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rename and a terminal opening at the same moment both save the
    /// agents; neither may fail, and the file stays whole.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_saves_do_not_trip_over_each_other() {
        let dir = std::env::temp_dir().join(format!("stowbox-store-{}", uuid::Uuid::new_v4().simple()));
        let store = std::sync::Arc::new(Store::new(&dir));
        let saves: Vec<_> = (0..32)
            .map(|_| {
                let store = store.clone();
                tokio::spawn(async move { store.save_agents(&[]).await })
            })
            .collect();
        for s in saves {
            s.await.unwrap().unwrap();
        }
        assert!(store.load_agents().await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
