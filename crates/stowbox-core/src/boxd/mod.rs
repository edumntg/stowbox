//! boxd.sh over its public gRPC API (`boxd.sh:9443`, service `boxd.api.v1.BoxdApi`).
//!
//! Auth is a two-step: the user's API key (`bxd_…`) is exchanged for a JWT at
//! `app.boxd.sh`, and the JWT rides on every gRPC call as
//! `authorization: Bearer …`. See `auth.rs`.

mod auth;
mod convert;

pub mod pb {
    tonic::include_proto!("boxd.api.v1");
}

use crate::ports::{HttpsRoute, PortForward};
use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use async_trait::async_trait;
use pb::boxd_api_client::BoxdApiClient;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig};
use tonic::{Request, Status};

pub use auth::TokenExchange;

const KEY_HINT: &str = "Make one at app.boxd.sh or with `boxd auth keys create stowbox`. Keys are fenced to one organization.";

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind {
    id: "boxd",
    capabilities,
    connect: |credentials, _| Ok(std::sync::Arc::new(Boxd::new(credentials.api_key()?, BoxdConfig::default())?)),
};

/// What boxd offers, for the UI. Everything Stowbox was first built on.
pub fn capabilities() -> crate::capabilities::Capabilities {
    use PowerAction::*;
    crate::capabilities::Capabilities {
        create: true,
        new_box_blurb: "A persistent Linux machine on boxd. It boots in well under a second and hibernates on its own after four idle hours.".into(),
        free_sizes: [(1, 4), (2, 8), (4, 16)].into_iter().map(|(vcpu, memory_gib)| crate::capabilities::FreeSize { vcpu, memory_gib }).collect(),
        default_size: String::new(),
        default_disk_gib: 100,
        ssh_hint: "ssh -p {sshPort} boxd@{name}.{accessDomain|boxd.sh}".into(),
        credential_fields: vec![crate::capabilities::CredentialField::api_key(KEY_HINT)],
        icon: "BX".into(),
        label: "boxd".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: vec!["bxd_".into()],
        currency: "eur".into(),
        power: vec![Start, Stop, Reboot, Pause, Resume, Hibernate, Wake, Destroy],
        fork: true,
        isolated: true,
        auto_suspend: true,
        auto_hibernate: true,
        idle_stop: false,
        auto_stop: false,
        custom_size: true,
        sizes: Vec::new(),
        https_links: true,
        https_named: true,
        https_list: true,
        https_remove: true,
        https_default_route: true,
        port_forwards: true,
        editor_ssh: true,
        max_upload_bytes: crate::upload::MAX_BYTES,
        remote_user: crate::ssh::USER.into(),
        remote_home: crate::ssh::REMOTE_HOME.into(),
        metering_note: "boxd meters with a lag of up to about 25 minutes, so the last half hour may be missing here.".into(),
        metered_usage: true,
    }
}

pub const DEFAULT_ENDPOINT: &str = "https://boxd.sh:9443";
pub const DEFAULT_AUTH_URL: &str = "https://app.boxd.sh/api/v1/auth/token";
pub const DEFAULT_CONSOLE_URL: &str = "https://app.boxd.sh";

#[derive(Debug, Clone)]
pub struct BoxdConfig {
    pub endpoint: String,
    pub auth_url: String,
    pub console_url: String,
}

impl Default for BoxdConfig {
    fn default() -> Self {
        Self { endpoint: DEFAULT_ENDPOINT.into(), auth_url: DEFAULT_AUTH_URL.into(), console_url: DEFAULT_CONSOLE_URL.into() }
    }
}

#[derive(Clone)]
struct AuthInterceptor(MetadataValue<Ascii>);

impl tonic::service::Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: Request<()>) -> std::result::Result<Request<()>, Status> {
        req.metadata_mut().insert("authorization", self.0.clone());
        Ok(req)
    }
}

type Api = BoxdApiClient<InterceptedService<Channel, AuthInterceptor>>;

pub struct Boxd {
    channel: Channel,
    auth: Arc<TokenExchange>,
    config: BoxdConfig,
}

impl Boxd {
    /// Must be called inside a Tokio runtime: the channel connects lazily
    /// and binds to the runtime it is created in.
    pub fn new(api_key: String, config: BoxdConfig) -> Result<Self> {
        let tls = ClientTlsConfig::new().with_webpki_roots();
        let channel = Channel::from_shared(config.endpoint.clone()).map_err(|e| Error::Network(e.to_string()))?.tls_config(tls)?.connect_lazy();
        let auth = Arc::new(TokenExchange::new(api_key, config.auth_url.clone()));
        Ok(Self { channel, auth, config })
    }

    pub fn config(&self) -> &BoxdConfig {
        &self.config
    }

    async fn api(&self) -> Result<Api> {
        let token = self.auth.token().await?;
        let value: MetadataValue<Ascii> = format!("Bearer {token}").parse().map_err(|_| Error::Auth("token is not valid header text".into()))?;
        Ok(BoxdApiClient::with_interceptor(self.channel.clone(), AuthInterceptor(value)))
    }

    fn desktop_url(&self, name: &str) -> String {
        format!("{}/desktop/{}", self.config.console_url, name)
    }
}

#[async_trait]
impl Provider for Boxd {
    fn id(&self) -> &'static str {
        "boxd"
    }

    fn capabilities(&self) -> crate::capabilities::Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        let r = self.api().await?.whoami(pb::WhoamiRequest {}).await?.into_inner();
        Ok(convert::account(r))
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let r = self.api().await?.list_vms(pb::ListVmsRequest { org: String::new(), all_contexts: true }).await?.into_inner();
        Ok(r.vms.into_iter().map(|vm| convert::machine(vm, &self.desktop_url_fn())).collect())
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        let r = self.api().await?.get_vm(pb::GetVmRequest { vm_id: id_or_name.into() }).await?.into_inner();
        Ok(convert::machine(r, &self.desktop_url_fn()))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let mut api = self.api().await?;
        let name = match spec.name.filter(|n| !n.trim().is_empty()) {
            Some(n) => n,
            None => api.suggest_vm_name(pb::SuggestVmNameRequest {}).await?.into_inner().name,
        };
        let config = if spec.vcpu.is_some() || spec.memory_bytes.is_some() {
            Some(pb::VmConfig { vcpu: spec.vcpu.unwrap_or(0), memory_bytes: spec.memory_bytes.unwrap_or(0), ..Default::default() })
        } else {
            None
        };
        let req = pb::CreateVmRequest {
            name: name.clone(),
            env: spec.env.into_iter().map(|(key, value)| pb::EnvVar { key, value }).collect(),
            config,
            org: spec.org.unwrap_or_default(),
            isolated: spec.isolated,
            ..Default::default()
        };
        let created = api.create_vm(req).await?.into_inner();
        // The create response is thin; read the machine back for the full shape.
        self.get_machine(&created.vm_id).await
    }

    async fn fork_machine(&self, source: &str, name: Option<String>) -> Result<Machine> {
        let mut api = self.api().await?;
        let name = match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => n,
            None => api.suggest_vm_name(pb::SuggestVmNameRequest {}).await?.into_inner().name,
        };
        let forked = api.fork_vm(pb::ForkVmRequest { source_vm_id: source.into(), name, ..Default::default() }).await?.into_inner();
        self.get_machine(&forked.vm_id).await
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        let mut api = self.api().await?;
        let vm_id = id.to_string();
        match action {
            PowerAction::Start => {
                api.start_vm(pb::StartVmRequest { vm_id }).await?;
            }
            PowerAction::Stop => {
                api.stop_vm(pb::StopVmRequest { vm_id }).await?;
            }
            PowerAction::Reboot => {
                api.reboot_vm(pb::RebootVmRequest { vm_id }).await?;
            }
            PowerAction::Pause => {
                api.suspend_vm(pb::SuspendVmRequest { vm_id }).await?;
            }
            PowerAction::Resume => {
                api.resume_vm(pb::ResumeVmRequest { vm_id }).await?;
            }
            PowerAction::Hibernate => {
                api.hibernate_vm(pb::HibernateVmRequest { vm_id }).await?;
            }
            PowerAction::Wake => {
                api.wake_vm(pb::WakeVmRequest { vm_id }).await?;
            }
            PowerAction::Destroy => {
                api.destroy_vm(pb::DestroyVmRequest { vm_id }).await?;
            }
        }
        Ok(())
    }

    async fn set_auto_hibernate(&self, id: &str, secs: u32) -> Result<()> {
        self.api().await?.set_auto_hibernate_timeout(pb::SetAutoHibernateTimeoutRequest { vm_id: id.into(), timeout_secs: secs }).await?;
        Ok(())
    }

    async fn set_auto_suspend(&self, id: &str, secs: u32) -> Result<()> {
        self.api().await?.set_auto_suspend_timeout(pb::SetAutoSuspendTimeoutRequest { vm_id: id.into(), timeout_secs: secs }).await?;
        Ok(())
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        let mut api = self.api().await?;

        // Wrap in a login shell so the command string is interpreted the same
        // way whether the server execs it directly or hands it to `sh -c`.
        let mut script = String::new();
        for (k, v) in &spec.env {
            script.push_str(&format!("export {}={} && ", k, shell_quote(v)));
        }
        script.push_str(&spec.script);
        let command = format!("bash -lc {}", shell_quote(&script));

        let (wire_tx, wire_rx) = mpsc::channel::<pb::ExecChunk>(64);
        wire_tx
            .send(pb::ExecChunk {
                vm_id: id.into(),
                command,
                tty: spec.tty,
                cols: if spec.tty { spec.cols as u32 } else { 0 },
                rows: if spec.tty { spec.rows as u32 } else { 0 },
                ..Default::default()
            })
            .await
            .map_err(|_| Error::Provider("exec channel closed before start".into()))?;

        let mut remote = api.exec(ReceiverStream::new(wire_rx)).await?.into_inner();

        let (in_tx, mut in_rx) = mpsc::channel::<ExecInput>(64);
        let (out_tx, out_rx) = mpsc::channel::<ExecOutput>(256);

        // Local input → wire. Dropping `in_tx` (or sending Close) ends this
        // task, which drops `wire_tx`: the server sees stdin EOF.
        tokio::spawn(async move {
            while let Some(msg) = in_rx.recv().await {
                let chunk = match msg {
                    ExecInput::Data(data) => pb::ExecChunk { data, stdin: true, ..Default::default() },
                    ExecInput::Resize { cols, rows } => pb::ExecChunk { window_change: true, cols: cols as u32, rows: rows as u32, ..Default::default() },
                    ExecInput::Close => break,
                };
                if wire_tx.send(chunk).await.is_err() {
                    break;
                }
            }
        });

        // Wire → local output. If the receiver goes away we stop reading,
        // which drops the call and kills the remote process group.
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    // Receiver gone: stop reading now, not at the next chunk.
                    // Dropping `remote` cancels the call and the server kills
                    // the remote process group.
                    _ = out_tx.closed() => return,
                    msg = remote.message() => msg,
                };
                match next {
                    Ok(Some(chunk)) => {
                        let pb::ExecChunk { data, is_stderr, exit_code, .. } = chunk;
                        if !data.is_empty() {
                            let out = if is_stderr { ExecOutput::Stderr(data) } else { ExecOutput::Stdout(data) };
                            if out_tx.send(out).await.is_err() {
                                return;
                            }
                        }
                        // A zero exit is indistinguishable from "no exit yet"
                        // on the wire, so it is detected by the stream closing
                        // (`Ok(None)` below); a non-zero one is explicit here.
                        if exit_code != 0 {
                            let _ = out_tx.send(ExecOutput::Exit(exit_code)).await;
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = out_tx.send(ExecOutput::Exit(0)).await;
                        return;
                    }
                    Err(status) => {
                        let _ = out_tx.send(ExecOutput::Error(status.message().to_string())).await;
                        return;
                    }
                }
            }
        });

        Ok(ExecHandle { input: in_tx, output: out_rx })
    }

    async fn machine_usage(&self, id: &str) -> Result<Usage> {
        let r = self.api().await?.get_vm_usage(pb::GetVmUsageRequest { vm_id: id.into(), ..Default::default() }).await?.into_inner();
        Ok(convert::usage(r))
    }

    async fn org_usage(&self, org: &str, since: i64, until: i64) -> Result<OrgUsage> {
        let r = self.api().await?.get_org_usage(pb::GetOrgUsageRequest { org: org.into(), since, until }).await?.into_inner();
        Ok(convert::org_usage(r))
    }

    async fn org_billing(&self, org: &str) -> Result<OrgBilling> {
        let r = self.api().await?.get_org_billing(pb::GetOrgBillingRequest { org: org.into() }).await?.into_inner();
        Ok(convert::org_billing(r))
    }

    async fn link_ssh_key(&self, pubkey: &str, device_id: &str, label: &str) -> Result<()> {
        self.api().await?.link_ssh_key(pb::LinkSshKeyRequest { pubkey: pubkey.into(), device_id: device_id.into(), label: label.into() }).await?;
        Ok(())
    }

    async fn upload_file(&self, id: &str, path: &str, data: Vec<u8>) -> Result<u64> {
        let expected = data.len() as u64;
        let mut api = self.api().await?;
        let written = if data.len() <= UNARY_UPLOAD_MAX {
            api.upload_file(pb::UploadFileRequest { vm_id: id.into(), path: path.into(), data }).await?.into_inner().bytes_written
        } else {
            api.upload_file_stream(tokio_stream::iter(upload_chunks(id, path, data, UPLOAD_CHUNK))).await?.into_inner().bytes_written
        };
        crate::upload::confirm_written(written, expected, path)
    }

    async fn ssh_host_key(&self) -> Result<Option<String>> {
        // Every machine's SSH ends at the proxy, which presents one key; it
        // only travels with a machine listing.
        let r = self.api().await?.list_vms(pb::ListVmsRequest { org: String::new(), all_contexts: false }).await?.into_inner();
        Ok(Some(r.ssh_host_key).filter(|k| !k.trim().is_empty()))
    }

    async fn https_routes(&self, id: &str) -> Result<Vec<HttpsRoute>> {
        let r = self.api().await?.list_proxies(pb::ListProxiesRequest { vm: id.into(), ..Default::default() }).await?.into_inner();
        Ok(r.proxies.into_iter().map(convert::route).collect())
    }

    async fn add_https_route(&self, id: &str, name: &str, port: u16) -> Result<HttpsRoute> {
        crate::ports::validate_route_name(name)?;
        let r = self
            .api()
            .await?
            .create_proxy(pb::CreateProxyRequest { name: name.into(), port: port as u32, vm: id.into(), ..Default::default() })
            .await?
            .into_inner();
        Ok(convert::created_route(r))
    }

    async fn remove_https_route(&self, id: &str, name: &str) -> Result<()> {
        self.api().await?.delete_proxy(pb::DeleteProxyRequest { name: name.into(), vm: id.into(), ..Default::default() }).await?;
        Ok(())
    }

    async fn set_default_route_port(&self, id: &str, port: Option<u16>) -> Result<()> {
        // An empty name addresses the default route; "auto" is accepted only there.
        let port = port.map(|p| p.to_string()).unwrap_or_else(|| "auto".into());
        self.api().await?.set_proxy_port(pb::SetProxyPortRequest { name: String::new(), port, vm: id.into(), ..Default::default() }).await?;
        Ok(())
    }

    async fn port_forwards(&self, id: &str) -> Result<Vec<PortForward>> {
        let r = self.api().await?.list_exposed_ports(pb::ListExposedPortsRequest { vm: id.into() }).await?.into_inner();
        Ok(r.forwards.into_iter().map(convert::forward).collect())
    }

    async fn expose_port(&self, id: &str, port: u16, protocol: &str) -> Result<PortForward> {
        crate::ports::validate_protocol(protocol)?;
        let r = self
            .api()
            .await?
            .expose_port(pb::ExposePortRequest { vm_port: port as u32, protocol: protocol.into(), vm: id.into(), ..Default::default() })
            .await?
            .into_inner();
        Ok(convert::forward(r))
    }

    async fn unexpose_port(&self, id: &str, port: u16) -> Result<()> {
        self.api().await?.unexpose_port(pb::UnexposePortRequest { vm_port: port as u32, vm: id.into(), ..Default::default() }).await?;
        Ok(())
    }

    async fn rates(&self) -> Result<crate::cost::RateCard> {
        // The rate card only travels with a usage response. Ask for a window
        // that is cheap to compute: the last hour of the default org.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let r = self.api().await?.get_org_usage(pb::GetOrgUsageRequest { org: String::new(), since: now - 3600, until: 0 }).await?.into_inner();
        Ok(crate::cost::RateCard::resources(&r.currency, convert::rates(r.rates)))
    }
}

impl Boxd {
    fn desktop_url_fn(&self) -> impl Fn(&str) -> String + '_ {
        move |name| self.desktop_url(name)
    }
}

/// Files up to this size go in one `UploadFile` message; gRPC caps a
/// message at 4 MiB, so larger ones stream.
const UNARY_UPLOAD_MAX: usize = 3 * 1024 * 1024;

/// Bytes per `UploadFileStream` chunk.
const UPLOAD_CHUNK: usize = 1024 * 1024;

/// The `UploadFileStream` messages for one file: the first carries the
/// machine, the path and the total size with the first slice; the rest
/// carry bytes only. An empty file is one chunk with no bytes.
fn upload_chunks(vm_id: &str, path: &str, data: Vec<u8>, chunk: usize) -> Vec<pb::UploadFileChunk> {
    let total_size = data.len() as u64;
    let mut slices = data.chunks(chunk.max(1)).map(<[u8]>::to_vec);
    let first = pb::UploadFileChunk { vm_id: vm_id.into(), path: path.into(), total_size, data: slices.next().unwrap_or_default() };
    std::iter::once(first).chain(slices.map(|data| pb::UploadFileChunk { data, ..Default::default() })).collect()
}

/// POSIX single-quote escaping: safe for any byte sequence except NUL.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:=@%+".contains(&b)) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::{shell_quote, upload_chunks};

    #[test]
    fn upload_chunks_carry_the_header_once_and_every_byte() {
        let data: Vec<u8> = (0..10u8).collect();
        let chunks = upload_chunks("vm_1", "/home/boxd/a.png", data.clone(), 4);
        assert_eq!(chunks.len(), 3);
        assert_eq!((chunks[0].vm_id.as_str(), chunks[0].path.as_str(), chunks[0].total_size), ("vm_1", "/home/boxd/a.png", 10));
        assert!(chunks[1..].iter().all(|c| c.vm_id.is_empty() && c.path.is_empty() && c.total_size == 0));
        assert_eq!(chunks.iter().flat_map(|c| c.data.clone()).collect::<Vec<u8>>(), data);
        let empty = upload_chunks("vm_1", "/p", Vec::new(), 4);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].total_size, 0);
        assert!(empty[0].data.is_empty());
        assert_eq!(upload_chunks("vm_1", "/p", vec![1; 8], 4).len(), 2);
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(shell_quote("plain-word_1"), "plain-word_1");
        assert_eq!(shell_quote("has space"), "'has space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
