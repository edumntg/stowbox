//! Stowbox core: everything that does not depend on the desktop shell.
//!
//! - `provider`: the trait the UI talks to (list boxes, power actions, exec).
//! - `boxd`: the first provider, boxd.sh, over its public gRPC API.
//! - `agent`: how a coding agent is launched inside a box and how its
//!   JSONL output becomes chat events.
//! - `cost`: live €/h estimates from the provider's rate card.
//! - `git`: the repository review (status, diff) and commit, push and pull
//!   request, run inside the box.
//! - `metrics`: CPU, memory, disk and uptime sampled inside a running box.
//! - `notify`: which agent and setup outcomes deserve a desktop notification.
//! - `provision`: setup presets (commands and secrets) run in a new box.
//! - `ssh`: the key, `~/.ssh/config` block and editor links that open a box
//!   in VS Code or Cursor over Remote-SSH.
//! - `store`: on-disk persistence of agent sessions and transcripts.
//! - `worktree`: one git worktree and branch per agent, next to the repository.

pub mod agent;
pub mod boxd;
pub mod cost;
pub mod error;
pub mod git;
pub mod metrics;
pub mod notify;
pub mod provider;
pub mod provision;
pub mod ssh;
pub mod store;
#[cfg(test)]
pub(crate) mod testing;
pub mod types;
pub mod worktree;

pub use error::{Error, Result};
