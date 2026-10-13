//! Stowbox core: everything that does not depend on the desktop shell.
//!
//! - `provider`: the trait the UI talks to (list boxes, power actions, exec).
//! - `boxd`: the first provider, boxd.sh, over its public gRPC API.
//! - `agent`: how a coding agent is launched inside a box and how its
//!   JSONL output becomes chat events.
//! - `cost`: live €/h estimates from the provider's rate card.
//! - `store`: on-disk persistence of agent sessions and transcripts.

pub mod agent;
pub mod boxd;
pub mod cost;
pub mod error;
pub mod provider;
pub mod store;
pub mod types;

pub use error::{Error, Result};
