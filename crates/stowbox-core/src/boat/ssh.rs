//! How a boat sandbox is reached over SSH; the exec itself is
//! [`crate::sshexec`].
//!
//! boat's command API takes no stdin and has no TTY, and Stowbox needs both
//! (agents read their prompt on stdin, the terminal is a TTY). So every exec
//! is an SSH channel: `POST /sandboxes/{id}/sshkey` authorizes Stowbox's
//! own ed25519 key on the sandbox and answers with the address, the user
//! and the host key; the client checks the server against that key and
//! nothing else.

use super::api::{seg, Api};
use crate::sshexec::{parse_host_key, Authorize, Endpoint, HostCheck, Login};
use crate::{Error, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// `sshEndpoint` ("host:port") wins when set: the sandbox has no public
/// IPv4 of its own. Otherwise `machineIp`, port 22.
pub fn endpoint(v: &Value) -> Result<Endpoint> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let (host, port) = match s("sshEndpoint") {
        e if !e.is_empty() => {
            let (h, p) = e.rsplit_once(':').ok_or_else(|| Error::Provider(format!("boat gave an SSH endpoint without a port: {e}")))?;
            let p = p.parse::<u16>().map_err(|_| Error::Provider(format!("boat gave an SSH endpoint with a bad port: {e}")))?;
            (h.trim_start_matches('[').trim_end_matches(']').to_string(), p)
        }
        _ => (s("machineIp"), 22),
    };
    if host.is_empty() {
        return Err(Error::Provider("boat gave no address to reach the sandbox over SSH".into()));
    }
    let host_key = parse_host_key(&s("hostKey")).map_err(|_| Error::Provider("boat did not send the sandbox's SSH host key, so the connection cannot be checked".into()))?;
    let user = Some(s("sshUser")).filter(|u| !u.is_empty()).unwrap_or_else(|| "user".into());
    Ok(Endpoint { host, port, user, login: Login::Key, check: HostCheck::Pinned(host_key), provider: "boat" })
}

/// boat's `POST /sandboxes/{id}/sshkey`. The first seconds of a start
/// answer `sandbox_restoring` or `sandbox_securing`; those are waited out.
pub struct ApiAuth<'a>(pub &'a Api);

#[async_trait::async_trait]
impl Authorize for ApiAuth<'_> {
    async fn authorize(&self, id: &str, public_line: &str) -> Result<Endpoint> {
        let mut wait = Duration::from_millis(750);
        let mut tries = 0;
        loop {
            let r = self.0.post(&format!("/sandboxes/{}/sshkey", seg(id)), json!({ "key": public_line })).await;
            match r {
                Ok(v) => return endpoint(&v),
                Err(Error::Provider(m)) if m.contains("machine_not_running") => {
                    return Err(Error::Provider("the sandbox is not running; start it first".into()));
                }
                Err(Error::Provider(m)) if tries < 8 && ["sandbox_restoring", "sandbox_securing", "sandbox_starting", "ssh_host_key_unavailable", "sandbox_resuming"].iter().any(|c| m.contains(c)) => {
                    tries += 1;
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(4));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

    #[test]
    fn endpoint_prefers_the_relay_and_checks_the_host_key() {
        let ep = endpoint(&json!({ "machineIp": "203.0.113.10", "sshUser": "user", "hostKey": ED, "sshEndpoint": null })).unwrap();
        assert_eq!((ep.host.as_str(), ep.port, ep.user.as_str()), ("203.0.113.10", 22, "user"));
        assert!(matches!(ep.check, HostCheck::Pinned(_)) && ep.login == Login::Key);
        let ep = endpoint(&json!({ "machineIp": "10.0.0.1", "sshEndpoint": "relay.boat.dev:40022", "hostKey": format!("bx_1 {ED}") })).unwrap();
        assert_eq!((ep.host.as_str(), ep.port, ep.user.as_str()), ("relay.boat.dev", 40022, "user"));
        // No host key, no connection.
        assert!(endpoint(&json!({ "machineIp": "203.0.113.10" })).is_err());
        assert!(endpoint(&json!({ "hostKey": ED })).is_err());
        assert!(endpoint(&json!({ "sshEndpoint": "relay:notaport", "hostKey": ED })).is_err());
    }
}
