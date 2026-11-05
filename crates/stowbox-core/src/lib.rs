//! Stowbox core: everything that does not depend on the desktop shell.
//!
//! - `provider`: the trait the UI talks to (list boxes, power actions, exec).
//! - `boxd`: the first provider, boxd.sh, over its public gRPC API.
//! - `agent`: how a coding agent is launched inside a box and how its
//!   JSONL output becomes chat events.
//! - `cost`: live €/h estimates from the provider's rate card.
//! - `metrics`: CPU, memory, disk and uptime sampled inside a running box.
//! - `notify`: which agent and setup outcomes deserve a desktop notification.
//! - `provision`: setup presets (commands and secrets) run in a new box.
//! - `store`: on-disk persistence of agent sessions and transcripts.

pub mod agent;
pub mod boxd;
pub mod cost;
pub mod error;
pub mod metrics;
pub mod notify;
pub mod provider;
pub mod provision;
pub mod store;
pub mod types;

pub use error::{Error, Result};
