//! Modal's client API over gRPC at `api.modal.com:443`. Every call carries
//! the token id and secret as metadata, as Modal's own SDKs send them.
//!
//! Exec goes to a second service, the task command router on the worker
//! that runs the sandbox, reached at a URL and with a short-lived JWT that
//! the control plane hands out per task (see [`Router`]).

use super::pb::client::modal_client_client::ModalClientClient;
use super::pb::router::task_command_router_client::TaskCommandRouterClient;
use crate::{Error, Result};
use std::time::Duration;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status};

pub const DEFAULT_ENDPOINT: &str = "https://api.modal.com:443";
/// The longest any control-plane call may take. Image builds are waited on
/// in rounds shorter than this.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest message accepted from Modal. Lists of sandboxes and output
/// chunks are far smaller; the cap stops a hostile length from allocating.
pub const MAX_MESSAGE: usize = 4 << 20;
/// Modal's `ClientType` for its Go SDK, whose calls Stowbox makes.
const CLIENT_TYPE: &str = "9";
/// The Python SDK version whose behaviour the calls follow, as the Go SDK
/// declares it.
const CLIENT_VERSION: &str = "1.0.0";
/// The Go SDK release whose protocol the vendored protos come from. Modal
/// gates what a client may do on this version, and refuses to make apps
/// for one it cannot read.
const LIBMODAL_VERSION: &str = "modal-go/0.11.2";

#[derive(Clone)]
pub struct Auth {
    token_id: MetadataValue<Ascii>,
    token_secret: MetadataValue<Ascii>,
}

impl Auth {
    pub fn new(token_id: &str, token_secret: &str) -> Result<Self> {
        let value = |v: &str, what: &str| {
            let mut m: MetadataValue<Ascii> = v.parse().map_err(|_| Error::Auth(format!("the Modal {what} is not valid header text")))?;
            m.set_sensitive(true);
            Ok::<_, Error>(m)
        };
        Ok(Self { token_id: value(token_id, "token id")?, token_secret: value(token_secret, "token secret")? })
    }
}

impl tonic::service::Interceptor for Auth {
    fn call(&mut self, mut req: Request<()>) -> std::result::Result<Request<()>, Status> {
        let m = req.metadata_mut();
        m.insert("x-modal-client-type", MetadataValue::from_static(CLIENT_TYPE));
        m.insert("x-modal-client-version", MetadataValue::from_static(CLIENT_VERSION));
        m.insert("x-modal-libmodal-version", MetadataValue::from_static(LIBMODAL_VERSION));
        m.insert("x-modal-host", MetadataValue::from_static("api.modal.com"));
        m.insert("x-modal-token-id", self.token_id.clone());
        m.insert("x-modal-token-secret", self.token_secret.clone());
        Ok(req)
    }
}

pub type Client = ModalClientClient<InterceptedService<Channel, Auth>>;

/// The control-plane client. Must be made inside a Tokio runtime: the
/// channel connects lazily and binds to the runtime it is made in.
pub fn client(endpoint: &str, auth: Auth) -> Result<Client> {
    let channel = Endpoint::from_shared(endpoint.to_string())
        .map_err(|e| Error::Network(e.to_string()))?
        .tls_config(ClientTlsConfig::new().with_webpki_roots())?
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(CALL_TIMEOUT)
        .connect_lazy();
    Ok(ModalClientClient::with_interceptor(channel, auth).max_decoding_message_size(MAX_MESSAGE))
}

#[derive(Clone)]
pub struct Bearer(MetadataValue<Ascii>);

impl tonic::service::Interceptor for Bearer {
    fn call(&mut self, mut req: Request<()>) -> std::result::Result<Request<()>, Status> {
        req.metadata_mut().insert("authorization", self.0.clone());
        Ok(req)
    }
}

pub type Router = TaskCommandRouterClient<InterceptedService<Channel, Bearer>>;

/// A client for one task's command router. The URL comes from Modal's
/// answer, so it must be https on a host name before anything is sent to
/// it. Streams last as long as a command, so there is no overall timeout:
/// HTTP/2 keepalives notice a peer that went away, and unary calls bound
/// their own wait.
pub async fn router(url: &str, jwt: &str) -> Result<Router> {
    let host = router_host(url).ok_or_else(|| Error::Provider("Modal named a command router that is not an https host of its own".into()))?;
    let mut bearer: MetadataValue<Ascii> =
        format!("Bearer {jwt}").parse().map_err(|_| Error::Provider("Modal's router token is not valid header text".into()))?;
    bearer.set_sensitive(true);
    let channel = Endpoint::from_shared(format!("https://{host}"))
        .map_err(|e| Error::Network(e.to_string()))?
        .tls_config(ClientTlsConfig::new().with_webpki_roots())?
        .connect_timeout(CONNECT_TIMEOUT)
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(10))
        .keep_alive_while_idle(true)
        .connect()
        .await?;
    Ok(TaskCommandRouterClient::with_interceptor(channel, Bearer(bearer)).max_decoding_message_size(MAX_MESSAGE))
}

/// Where Modal's workers serve their routers (`task-….w.modal.host`).
const ROUTER_DOMAIN: &str = ".modal.host";

/// `host` or `host:port` from an https URL on Modal's router domain with
/// nothing after the authority, or `None` for anything else: the router
/// token must not go anywhere else.
pub fn router_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?.trim_end_matches('/');
    let (name, port) = match rest.split_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (rest, None),
    };
    let name_ok = name.len() <= 253
        && name.strip_suffix(ROUTER_DOMAIN).is_some_and(|sub| !sub.is_empty())
        && !name.starts_with(['.', '-'])
        && !name.contains("..")
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    (name_ok && port_ok).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_https_host_is_a_router() {
        assert_eq!(router_host("https://w-abc.us-east-1.modal.host"), Some("w-abc.us-east-1.modal.host"));
        assert_eq!(router_host("https://w-abc.modal.host:8443/"), Some("w-abc.modal.host:8443"));
        for bad in [
            "http://w.modal.host",
            "https://",
            "https://localhost",
            "https://w.modal.host/x",
            "https://w.modal.host:x",
            "https://W.MODAL.HOST",
            "https://a@b.com",
            "https://w..modal.host",
            "https://modal.host",
            "https://evil.example.com",
            "https://w.modal.host.evil.com",
            "https://wmodal.host",
        ] {
            assert_eq!(router_host(bad), None, "{bad}");
        }
    }

    #[test]
    fn secrets_are_sensitive_metadata() {
        let a = Auth::new("ak-123", "as-456").unwrap();
        assert!(a.token_id.is_sensitive() && a.token_secret.is_sensitive());
        assert!(!format!("{:?}", a.token_secret).contains("as-456"));
        assert!(Auth::new("ak\n1", "x").is_err());
    }
}
