//! Stowbox core: everything that does not depend on the desktop shell.
//!
//! - `provider`: the trait the UI talks to (list boxes, power actions, exec).
//! - `boxd`: boxd.sh, over its public gRPC API.
//! - `boat`: boat.dev, over its REST API, with exec over SSH.
//! - `capabilities`: what each provider supports, for the UI.
//! - `registry`: the connected providers, and which one owns each box.
//! - `agent`: a coding agent's interactive session inside a box, its state,
//!   and the Claude Code login the box uses.
//! - `cost`: live €/h estimates from the provider's rate card.
//! - `git`: the repository review (status, diff) and commit, push and pull
//!   request, run inside the box.
//! - `guard`: idle auto-standby decisions and the daily spend check.
//! - `metrics`: CPU, memory, disk and uptime sampled inside a running box.
//! - `notify`: which agent and setup changes deserve a desktop notification.
//! - `ports`: HTTPS links and raw port forwards into a box, and the ports
//!   something listens on inside it.
//! - `provision`: setup presets (commands and secrets) run in a new box.
//! - `ssh`: the key, `~/.ssh/config` block and editor links that open a box
//!   in VS Code or Cursor over Remote-SSH.
//! - `sshexec`: exec in a box over SSH, for providers whose command API
//!   lacks stdin or a TTY (boat, Daytona).
//! - `store`: on-disk persistence of agents, presets and setup runs.
//! - `upload`: files dropped on an agent's terminal, copied into the box.
//! - `worktree`: one git worktree and branch per agent, next to the repository.

pub mod agent;
pub mod boat;
pub mod boxd;
pub mod capabilities;
pub mod cost;
pub mod daytona;
pub mod error;
pub mod git;
pub mod guard;
pub mod metrics;
pub mod notify;
pub mod ports;
pub mod provider;
pub mod provision;
pub mod registry;
pub mod ssh;
pub mod sshexec;
pub mod store;
#[cfg(test)]
pub(crate) mod testing;
pub mod types;
pub mod upload;
pub mod worktree;

pub use error::{Error, Result};
