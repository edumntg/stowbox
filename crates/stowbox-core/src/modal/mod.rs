//! Modal (modal.com) sandboxes behind the `Provider` trait. Beta: Modal's
//! client API is the one its SDKs use, not a published one, so the protos
//! are pinned to one SDK release (see `proto/modal/`).
//!
//! The lifecycle is Modal's gRPC control plane at `api.modal.com`. Every
//! sandbox belongs to one app, `stowbox`, and runs one image, built once
//! per workspace from `node:22-bookworm`. Commands run through the task
//! command router on the sandbox's worker (see [`exec`]), as root, with a
//! real PTY and stdin; no SSH is involved.
//!
//! A Modal sandbox cannot stop and start again: it runs until its timeout,
//! at most a day, or until it is terminated, and then it is gone, disk and
//! all. Stowbox shows that end as the box's auto-stop time.

pub mod api;
pub mod convert;
pub mod exec;

/// Modal's messages, generated from `proto/modal/`.
#[allow(clippy::all, clippy::pedantic)]
pub mod pb {
    pub mod client {
        tonic::include_proto!("modal.client");
    }
    pub mod router {
        tonic::include_proto!("modal.task_command_router");
    }
}

use crate::capabilities::{Capabilities, CredentialField, SizeOption};
use crate::cost::RateCard;
use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use async_trait::async_trait;
use pb::client::*;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

pub const ID: &str = "modal";
pub const DEFAULT_CONSOLE_URL: &str = "https://modal.com/apps";
/// The app every Stowbox sandbox belongs to.
const APP: &str = "stowbox";
/// Debian with Node, git and curl, so the agents install with npm.
const BASE_IMAGE: &str = "FROM node:22-bookworm";
pub const USER: &str = "root";
const HOME: &str = "/root";
pub const TOKEN_ID: &str = "token_id";
pub const TOKEN_SECRET: &str = "token_secret";
/// Modal's list prices: $0.0000197 a vCPU-second (Modal sells physical
/// cores at twice that, and a core is two vCPUs) and $0.00000667 a
/// GiB-second of memory while running.
const RATES: Rates = Rates { vcpu_hour_micro: 70_956, ram_gib_hour_micro: 24_012, disk_gib_hour_micro: 0 };
/// The longest Modal lets a sandbox live, and so the default.
const MAX_LIFETIME_SECS: u32 = 24 * 60 * 60;
/// The disk a sandbox gets when none is asked for, as `df /` reports it
/// inside one (2026-11-20). Modal does not charge for it.
pub const DISK_GIB: u64 = 512;
const MIN_LIFETIME_SECS: u32 = 60;
/// How long the first image build may take.
const BUILD_WAIT: Duration = Duration::from_secs(15 * 60);
/// One round of waiting on a build, shorter than a call's timeout.
const BUILD_ROUND_SECS: f32 = 55.0;
/// Pages of the sandbox list read at most, newest first.
const MAX_PAGES: usize = 20;
const KEY_HINT: &str = "Make a token in Modal's settings under API tokens. The id starts with ak-, the secret with as-.";

pub struct Size {
    pub milli_cpu: u32,
    pub memory_mib: u32,
}

impl Size {
    pub fn id(&self) -> String {
        format!("{}-vcpu", convert::vcpus(self.milli_cpu))
    }
}

/// Two GiB of memory a vCPU, as most providers sell it.
pub const SIZES: &[Size] = &[
    Size { milli_cpu: 500, memory_mib: 2048 },
    Size { milli_cpu: 1000, memory_mib: 4096 },
    Size { milli_cpu: 2000, memory_mib: 8192 },
    Size { milli_cpu: 4000, memory_mib: 16384 },
];
const DEFAULT_SIZE: usize = 1;

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind { id: ID, capabilities, connect: |credentials, _| connect(&credentials) };

fn connect(credentials: &crate::providers::Credentials) -> Result<Arc<dyn Provider>> {
    let id = credentials.get(TOKEN_ID).ok_or_else(|| Error::Auth("Modal needs the token id".into()))?;
    let secret = credentials.get(TOKEN_SECRET).ok_or_else(|| Error::Auth("Modal needs the token secret".into()))?;
    Ok(Arc::new(Modal::new(api::DEFAULT_ENDPOINT, id, secret)?))
}

fn hourly_micro(s: &Size) -> i64 {
    i64::from(convert::vcpus(s.milli_cpu)) * RATES.vcpu_hour_micro + i64::from(s.memory_mib) * RATES.ram_gib_hour_micro / 1024
}

pub fn capabilities() -> Capabilities {
    let default = SIZES.get(DEFAULT_SIZE).map(Size::id).unwrap_or_default();
    Capabilities {
        create: true,
        new_box_blurb: "A Modal sandbox, a gVisor container on Modal's workers, running as root. It lives a day at most and then is gone, disk and all; Beta, as Modal's sandbox API is not a published one.".into(),
        free_sizes: Vec::new(),
        default_size: default,
        default_disk_gib: DISK_GIB as u32,
        ssh_hint: String::new(),
        credential_fields: vec![
            CredentialField::plain(TOKEN_ID, "token id", KEY_HINT, "ak-…"),
            CredentialField { name: TOKEN_SECRET.into(), label: "token secret".into(), secret: true, hint: KEY_HINT.into(), optional: false, placeholder: "as-…".into() },
        ],
        icon: "MO".into(),
        label: "Modal".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: Vec::new(),
        currency: "usd".into(),
        power: vec![PowerAction::Destroy],
        fork: false,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: false,
        custom_size: false,
        sizes: SIZES
            .iter()
            .map(|s| SizeOption {
                id: s.id(),
                vcpu: convert::vcpus(s.milli_cpu),
                memory_bytes: u64::from(s.memory_mib) << 20,
                disk_bytes: DISK_GIB << 30,
                hourly_micro: hourly_micro(s),
                note: String::new(),
            })
            .collect(),
        https_links: false,
        https_named: false,
        https_list: false,
        https_remove: false,
        https_default_route: false,
        port_forwards: false,
        editor_ssh: false,
        max_upload_bytes: crate::upload::MAX_BYTES,
        remote_user: USER.into(),
        remote_home: HOME.into(),
        metering_note: "Modal's API does not report what a sandbox has cost; its usage page does. This is the list price while it runs.".into(),
        metered_usage: false,
    }
}

pub struct Modal {
    client: api::Client,
    routers: Arc<exec::Routers>,
    app_id: OnceCell<String>,
    image_id: OnceCell<String>,
}

impl Modal {
    pub fn new(endpoint: &str, token_id: &str, token_secret: &str) -> Result<Self> {
        let client = api::client(endpoint, api::Auth::new(token_id, token_secret)?)?;
        Ok(Self { routers: Arc::new(exec::Routers::new(client.clone())), client, app_id: OnceCell::new(), image_id: OnceCell::new() })
    }

    fn checked(id: &str) -> Result<&str> {
        if convert::valid_id(id) {
            Ok(id)
        } else {
            Err(Error::NotFound(format!("{id:?} is not a Modal sandbox id")))
        }
    }

    async fn app(&self, create: bool) -> Result<Option<String>> {
        if let Some(id) = self.app_id.get() {
            return Ok(Some(id.clone()));
        }
        let kind = if create { ObjectCreationType::CreateIfMissing } else { ObjectCreationType::Unspecified };
        let req = AppGetOrCreateRequest { app_name: APP.into(), environment_name: String::new(), object_creation_type: kind as i32 };
        match self.client.clone().app_get_or_create(req).await {
            Ok(r) => {
                let id = r.into_inner().app_id;
                Ok(Some(self.app_id.get_or_init(|| async { id }).await.clone()))
            }
            Err(s) if !create && s.code() == tonic::Code::NotFound => Ok(None),
            Err(s) => Err(s.into()),
        }
    }

    /// The image every sandbox runs, built on first use. Modal keeps a
    /// built image, so this is quick after the first time.
    async fn image(&self, app_id: &str) -> Result<String> {
        self.image_id.get_or_try_init(|| self.build_image(app_id)).await.cloned()
    }

    async fn build_image(&self, app_id: &str) -> Result<String> {
        let mut client = self.client.clone();
        let env = client.environment_get_or_create(EnvironmentGetOrCreateRequest::default()).await?.into_inner();
        let builder_version = env.metadata.and_then(|m| m.settings).map(|s| s.image_builder_version).unwrap_or_default();
        let req = ImageGetOrCreateRequest { image: Some(Image { dockerfile_commands: vec![BASE_IMAGE.into()] }), app_id: app_id.into(), builder_version };
        let made = client.image_get_or_create(req).await?.into_inner();
        let mut result = made.result.filter(|r| r.status != 0);
        let mut last_entry_id = String::new();
        let start = Instant::now();
        while result.is_none() {
            if start.elapsed() > BUILD_WAIT {
                return Err(Error::Network(format!("Modal did not finish building the image in {} minutes", BUILD_WAIT.as_secs() / 60)));
            }
            let req = ImageJoinStreamingRequest { image_id: made.image_id.clone(), timeout: BUILD_ROUND_SECS, last_entry_id: last_entry_id.clone() };
            let mut stream = client.image_join_streaming(req).await?.into_inner();
            while let Some(m) = stream.message().await? {
                if !m.entry_id.is_empty() {
                    last_entry_id = m.entry_id;
                }
                if let Some(r) = m.result.filter(|r| r.status != 0) {
                    result = Some(r);
                    break;
                }
                if m.eof {
                    break;
                }
            }
        }
        match result {
            Some(r) if r.status == generic_result::GenericStatus::Success as i32 => Ok(made.image_id),
            Some(r) => Err(Error::Provider(format!("Modal could not build the image: {}", r.exception.chars().take(300).collect::<String>()))),
            None => Err(Error::Provider("Modal did not say how the image build ended".into())),
        }
    }

    async fn sandboxes(&self) -> Result<Vec<SandboxInfo>> {
        let Some(app_id) = self.app(false).await? else { return Ok(Vec::new()) };
        let mut client = self.client.clone();
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        let mut before = 0.0;
        for _ in 0..MAX_PAGES {
            let req = SandboxListRequest {
                app_id: app_id.clone(),
                before_timestamp: before,
                environment_name: String::new(),
                include_finished: false,
                tags: Vec::new(),
            };
            let page = client.sandbox_list(req).await?.into_inner().sandboxes;
            let oldest = page.iter().map(|s| s.created_at).filter(|t| t.is_finite() && *t > 0.0).fold(f64::INFINITY, f64::min);
            let fresh: Vec<_> = page.into_iter().filter(|s| seen.insert(s.id.clone())).collect();
            if fresh.is_empty() {
                break;
            }
            all.extend(fresh);
            // Without a creation time there is no next page to ask for.
            if !oldest.is_finite() {
                break;
            }
            before = oldest;
        }
        Ok(all)
    }
}

#[async_trait]
impl Provider for Modal {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        let who = self.client.clone().workspace_name_lookup(Empty {}).await?.into_inner();
        let size = SIZES.get(DEFAULT_SIZE);
        Ok(Account {
            user_id: String::new(),
            display_name: format!("Modal {}", who.username.chars().take(64).collect::<String>()),
            billing: Billing {
                mode: String::new(),
                currency: "usd".into(),
                balance_micro: 0,
                accruing_micro: None,
                max_vms: 0,
                default_vcpu: size.map(|s| convert::vcpus(s.milli_cpu)).unwrap_or(0),
                default_memory_bytes: size.map(|s| u64::from(s.memory_mib) << 20).unwrap_or(0),
                ceiling_vcpu: 0,
                ceiling_memory_bytes: 0,
                subscription_status: String::new(),
            },
        })
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        Ok(self.sandboxes().await?.iter().map(convert::machine).collect())
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        self.sandboxes()
            .await?
            .iter()
            .find(|s| s.id == id_or_name || s.name == id_or_name)
            .map(convert::machine)
            .ok_or_else(|| Error::NotFound(format!("no running Modal sandbox is called {id_or_name}")))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let size = match spec.size.as_deref().filter(|s| !s.is_empty()) {
            Some(id) => SIZES.iter().find(|s| s.id() == id).ok_or_else(|| Error::Provider(format!("Modal has no size {id} here")))?,
            None => SIZES.get(DEFAULT_SIZE).ok_or_else(|| Error::Provider("Modal has no default size".into()))?,
        };
        let lifetime = match spec.auto_stop_secs {
            None | Some(0) => MAX_LIFETIME_SECS,
            Some(s) => s.clamp(MIN_LIFETIME_SECS, MAX_LIFETIME_SECS),
        };
        let typed = spec.name.as_deref().map(convert::name_from).filter(|n| convert::valid_name(n));
        let name = typed.unwrap_or_else(|| format!("box-{}", uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect::<String>()));
        let env = spec.env.into_iter().filter(|(k, _)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')).collect();
        let app_id = self.app(true).await?.ok_or_else(|| Error::Provider("Modal did not make the stowbox app".into()))?;
        let image_id = self.image(&app_id).await?;
        let definition = Sandbox {
            entrypoint_args: vec!["sleep".into(), "infinity".into()],
            image_id,
            resources: Some(Resources { memory_mb: size.memory_mib, milli_cpu: size.milli_cpu }),
            timeout_secs: lifetime,
            workdir: Some(HOME.into()),
            name: Some(name.clone()),
            idle_timeout_secs: None,
            environment_variables: Some(StringMap { contents: env }),
        };
        let tags = vec![SandboxTag { tag_name: "created-by".into(), tag_value: "stowbox".into() }];
        let id = self.client.clone().sandbox_create(SandboxCreateRequest { app_id, definition: Some(definition), tags }).await?.into_inner().sandbox_id;
        let id = Self::checked(&id)?.to_string();
        // The first command waits for the sandbox anyway; waiting here
        // shows it running, and sets up its router.
        let _ = self.routers.task(&id).await;
        match self.get_machine(&id).await {
            Ok(m) => Ok(m),
            Err(Error::NotFound(_)) => Err(Error::Provider(format!("Modal made {name} but does not list it; it may have failed to start"))),
            Err(e) => Err(e),
        }
    }

    async fn fork_machine(&self, _source: &str, _name: Option<String>) -> Result<Machine> {
        Err(Error::Provider("Modal cannot fork a sandbox".into()))
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        let id = Self::checked(id)?;
        match action {
            PowerAction::Destroy => {
                self.client.clone().sandbox_terminate(SandboxTerminateRequest { sandbox_id: id.into() }).await?;
                self.routers.forget(id);
                Ok(())
            }
            _ => Err(Error::Provider("a Modal sandbox cannot stop and start again; it runs until it is deleted or its day is up".into())),
        }
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Modal sandboxes do not hibernate".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("Modal sandboxes do not suspend".into()))
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        let id = Self::checked(id)?.to_string();
        exec::exec(self.routers.clone(), id, spec).await
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("Modal's API does not report what a sandbox has cost; Modal's usage page does".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("Modal's API does not report a balance".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard::resources("usd", RATES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_what_modal_does() {
        let c = capabilities();
        assert!(c.create && !c.fork && !c.auto_stop && !c.idle_stop && !c.metered_usage);
        assert_eq!(c.power, vec![PowerAction::Destroy]);
        assert_eq!((c.default_size.as_str(), c.remote_home.as_str()), ("2-vcpu", "/root"));
        let names: Vec<_> = c.credential_fields.iter().map(|f| (f.name.as_str(), f.secret)).collect();
        assert_eq!(names, [(TOKEN_ID, false), (TOKEN_SECRET, true)]);
        assert_eq!(c.sizes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["1-vcpu", "2-vcpu", "4-vcpu", "8-vcpu"]);
    }

    #[test]
    fn a_size_costs_its_list_price() {
        // 2 × 0.070956 + 4 × 0.024012 = 0.237960 an hour.
        assert_eq!(hourly_micro(&SIZES[DEFAULT_SIZE]), 237_960);
        let card = RateCard::resources("usd", RATES);
        let mut m = crate::testing::machine("s1", MachineStatus::Running);
        m.vcpu = 2;
        m.memory_bytes = 4 << 30;
        m.disk_bytes = 0;
        assert_eq!(card.hourly_micro(&m), 237_960);
    }

    #[test]
    fn ids_are_checked_before_a_request() {
        assert!(Modal::checked("sb-dwXsvnH78BrCLTuZBFVKag").is_ok());
        assert!(matches!(Modal::checked("../x"), Err(Error::NotFound(_))));
    }

    /// Live: a terminal resizes, and dropping a terminal or a script ends
    /// what it left running. Needs the `STOWBOX_LIVE_MODAL_TOKEN_*`
    /// variables; makes and deletes a sandbox.
    #[tokio::test]
    #[ignore = "makes and deletes a real Modal sandbox"]
    async fn a_terminal_resizes_and_dropping_an_exec_kills_its_children() {
        let (Ok(id), Ok(secret)) = (std::env::var("STOWBOX_LIVE_MODAL_TOKEN_ID"), std::env::var("STOWBOX_LIVE_MODAL_TOKEN_SECRET")) else { return };
        let p = Modal::new(api::DEFAULT_ENDPOINT, &id, &secret).unwrap();
        let m = p
            .create_machine(CreateSpec { name: Some("stowbox-live-kill".into()), auto_stop_secs: Some(900), size: Some("1-vcpu".into()), ..Default::default() })
            .await
            .unwrap();
        let run = |script: &str| {
            let (p, id, script) = (&p, m.id.clone(), script.to_string());
            async move {
                let mut c = p.exec(&id, ExecSpec { script, ..Default::default() }).await?;
                let mut out = Vec::new();
                while let Some(o) = c.output.recv().await {
                    if let ExecOutput::Stdout(b) = o {
                        out.extend(b);
                    }
                }
                Ok::<_, Error>(String::from_utf8_lossy(&out).trim().to_string())
            }
        };
        let check = async {
            let h = p.exec(&m.id, ExecSpec { script: "sleep 777 & sleep 778; echo never".into(), ..Default::default() }).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            drop(h.output);
            drop(h.input);
            let mut t = p.exec(&m.id, ExecSpec { script: "sleep 779".into(), tty: true, cols: 80, rows: 24, env: vec![] }).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            t.input.send(ExecInput::Resize { cols: 132, rows: 40 }).await.map_err(|_| Error::Provider("input closed".into()))?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            let size = run("for f in /tmp/.stowbox-*.tty; do read p t < \"$f\"; stty -F \"$t\" size; done").await?;
            t.output.close();
            drop(t);
            tokio::time::sleep(Duration::from_secs(5)).await;
            let left = run("pgrep -c -f 'sleep 77[789]' || true").await?;
            Ok::<_, Error>((size, left))
        };
        let result = check.await;
        p.power(&m.id, PowerAction::Destroy).await.unwrap();
        let (size, left) = result.unwrap();
        assert_eq!(size, "40 132");
        assert_eq!(left, "0");
    }
}
