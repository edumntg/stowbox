//! Stowbox core: everything that does not depend on the desktop shell.
//!
//! - `provider`: the trait the UI talks to (list boxes, power actions, exec).
//! - `boxd`: boxd.sh, over its public gRPC API.
//! - `boat`: boat.dev, over its REST API, with exec over SSH.
//! - `daytona`: Daytona, over its REST API, with exec over its SSH gateway.
//! - `agent37`: Agent37, over its REST API, with exec over SSH through a
//!   WebSocket at its edge.
//! - `e2b`: E2B, over its REST API, with exec over envd's Connect API.
//! - `vercel`: Vercel Sandbox, over its REST API, with commands streamed as
//!   NDJSON and terminals over a WebSocket.
//! - `blaxel`: Blaxel, over its REST API, with commands through each
//!   sandbox's process API and terminals over its WebSocket.
//! - `modal`: Modal, over its gRPC client API, with exec through each
//!   sandbox's task command router.
//! - `cloudflare`: Cloudflare Containers, through the `stowbox-bridge`
//!   Worker the person deploys, with exec over its WebSocket.
//! - `sshhost`: any machine reachable over SSH, with no lifecycle.
//! - `audit`: a local record of key, secret, setup and power actions.
//! - `capabilities`: what each provider supports, for the UI.
//! - `providers`: the list of providers, their credentials and how to connect.
//! - `registry`: the connected providers, and which one owns each box.
//! - `agent`: a coding agent's interactive session inside a box, its state,
//!   and the Claude Code login the box uses.
//! - `connectrpc`: a Connect protocol client for protobuf services (envd).
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
//!   lacks stdin or a TTY (boat, Daytona, Agent37).
//! - `store`: on-disk persistence of agents, presets and setup runs.
//! - `upload`: files dropped on an agent's terminal, copied into the box.
//! - `worktree`: one git worktree and branch per agent, next to the repository.
//! - `ndjson`: newline-delimited JSON off a streaming HTTP answer.
//! - `wstunnel`: a byte stream over a WebSocket, for SSH through a provider's edge.
//! - `websocket`: text and binary WebSocket messages, for providers whose terminal is one.

pub mod agent;
pub mod agent37;
pub mod audit;
pub mod blaxel;
pub mod boat;
pub mod boxd;
pub mod capabilities;
pub mod cloudflare;
pub mod connectrpc;
pub mod cost;
pub mod daytona;
pub mod e2b;
pub mod error;
pub mod git;
pub mod guard;
pub mod http;
pub mod metrics;
pub mod modal;
pub mod ndjson;
pub mod notify;
pub mod ports;
pub mod provider;
pub mod providers;
pub mod provision;
pub mod registry;
pub mod ssh;
pub mod sshexec;
pub mod sshhost;
pub mod store;
#[cfg(test)]
pub(crate) mod testing;
pub mod types;
pub mod upload;
pub mod vercel;
pub mod websocket;
pub mod worktree;
pub mod wstunnel;

pub use error::{Error, Result};
