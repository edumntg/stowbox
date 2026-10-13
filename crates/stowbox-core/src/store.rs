//! On-disk state: agent sessions (`agents.json`) and one JSONL transcript per
//! agent (`transcripts/<id>.jsonl`). Writes are whole-file and small; the
//! transcript appends are what grows.

use crate::agent::{AgentEvent, AgentMeta};
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

    pub async fn delete_transcript(&self, id: &str) -> Result<()> {
        match tokio::fs::remove_file(self.transcript_file(id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
