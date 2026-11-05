//! On-disk state: agent sessions (`agents.json`), one JSONL transcript per
//! agent (`transcripts/<id>.jsonl`), setup presets (`presets.json`, names
//! of secrets only) and the last setup run per box (`setup-runs.json`).
//! Writes are whole-file and small; the transcript appends are what grows.

use crate::agent::{AgentEvent, AgentMeta};
use crate::provision::{Preset, ProvisionRun};
use crate::search::{SearchLimits, SearchResults};
use crate::Result;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn agents_file(&self) -> PathBuf {
        self.dir.join("agents.json")
    }

    fn transcript_file(&self, id: &str) -> PathBuf {
        self.dir.join("transcripts").join(format!("{id}.jsonl"))
    }

    pub async fn load_agents(&self) -> Result<Vec<AgentMeta>> {
        match tokio::fs::read(self.agents_file()).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn save_agents(&self, agents: &[AgentMeta]) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let tmp = self.dir.join("agents.json.tmp");
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(agents)?).await?;
        tokio::fs::rename(&tmp, self.agents_file()).await?;
        Ok(())
    }

    async fn load_json<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> Result<T> {
        match tokio::fs::read(self.dir.join(name)).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(e.into()),
        }
    }

    async fn save_json<T: serde::Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<()> {
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

    pub async fn append_event(&self, id: &str, ev: &AgentEvent) -> Result<()> {
        let path = self.transcript_file(id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut f = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await?;
        let mut line = serde_json::to_vec(ev)?;
        line.push(b'\n');
        f.write_all(&line).await?;
        Ok(())
    }

    pub async fn read_events(&self, id: &str) -> Result<Vec<AgentEvent>> {
        let path = self.transcript_file(id);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }

    /// Full-text search over the transcripts of `agent_ids`, in that order.
    /// Runs on a blocking thread: it reads files line by line.
    pub async fn search_transcripts(&self, agent_ids: Vec<String>, query: String, limits: SearchLimits) -> Result<SearchResults> {
        let dir = self.dir.join("transcripts");
        tokio::task::spawn_blocking(move || crate::search::search_dir(&dir, &agent_ids, &query, limits))
            .await
            .map_err(|e| crate::Error::Provider(format!("search stopped: {e}")))?
    }

    pub async fn delete_transcript(&self, id: &str) -> Result<()> {
        match tokio::fs::remove_file(self.transcript_file(id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
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

    #[tokio::test]
    async fn search_reads_appended_transcripts() {
        let dir = std::env::temp_dir().join(format!("stowbox-store-search-{}", uuid::Uuid::new_v4().simple()));
        let store = Store::new(&dir);
        store.append_event("a1", &AgentEvent::UserMessage { text: "Rotate the API keys".into(), attachments: vec![] }).await.unwrap();
        store.append_event("a1", &AgentEvent::Stderr { text: "warning: key expired".into() }).await.unwrap();
        let r = store.search_transcripts(vec!["a1".into(), "a2".into()], "KEY".into(), SearchLimits::default()).await.unwrap();
        assert_eq!(r.scanned, 1);
        assert_eq!(r.hits.iter().map(|h| h.event).collect::<Vec<_>>(), vec![1, 0]);
        assert_eq!(store.read_events("a1").await.unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
