//! Cloudflare Containers behind the `Provider` trait, through a bridge.
//!
//! Cloudflare has no hosted sandbox API: a container is started by a
//! Durable Object in the person's own account. So the person deploys
//! `stowbox-bridge` (`bridge/cloudflare` in this repository) with wrangler,
//! and Stowbox talks only to it, with a token the person made. Stowbox never
//! holds a Cloudflare API token.
//!
//! A box is a Durable Object and its container. The container stops after
//! so long with nothing connected (the idle stop), and a stopped container
//! keeps nothing: the next start is a fresh disk from the image. Start,
//! stop and delete are offered; exec starts a stopped box.

pub mod api;
pub mod convert;
pub mod exec;

use crate::capabilities::{Capabilities, CredentialField, SizeOption};
use crate::cost::{RateCard, SizeRate};
use crate::provider::Provider;
use crate::providers::Credentials;
use crate::types::*;
use crate::{Error, Result};
use api::Api;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub const ID: &str = "cloudflare";
pub const URL: &str = "url";
pub const DEFAULT_CONSOLE_URL: &str = "https://dash.cloudflare.com/?to=/:account/workers/containers";
/// The bridge protocol this module speaks; a bridge reports its own.
const BRIDGE_VERSION: i64 = 1;
const DEFAULT_SIZE: &str = "standard-2";
/// The idle stop a new box gets, and the range the bridge takes.
const DEFAULT_IDLE_SECS: u32 = 30 * 60;
const MIN_IDLE_SECS: u32 = 60;
const MAX_IDLE_SECS: u32 = 24 * 60 * 60;
const USER: &str = "root";
const HOME: &str = "/root";

/// Cloudflare's list prices on Workers Paid (2026-11), in micro-USD:
/// $0.000020 a vCPU-second, $0.0000025 a GiB-second of memory and
/// $0.00000007 a GB-second of disk, past the monthly allowance.
const VCPU_HOUR_MICRO: i64 = 72_000;
const MEMORY_GIB_HOUR_MICRO: i64 = 9_000;
const DISK_GB_HOUR_MICRO: i64 = 252;

const TOKEN_HINT: &str = "The STOWBOX_TOKEN secret you gave the bridge Worker: at least 32 characters, made with openssl rand -hex 32.";

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind { id: ID, capabilities, connect: |credentials, _| connect(&credentials) };

fn connect(credentials: &Credentials) -> Result<Arc<dyn Provider>> {
    let url = credentials.get(URL).ok_or_else(|| Error::Auth("Cloudflare needs the bridge URL".into()))?;
    Ok(Arc::new(Cloudflare::new(url, &credentials.api_key()?)?))
}

/// A size's price per running hour, its CPU counted as fully busy: CPU is
/// billed on use, memory and disk on what the size provisions.
fn hourly_micro(s: &convert::Size) -> i64 {
    i64::from(s.vcpu) * VCPU_HOUR_MICRO + s.memory_gib as i64 * MEMORY_GIB_HOUR_MICRO + s.disk_gb as i64 * DISK_GB_HOUR_MICRO
}

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        create: true,
        new_box_blurb: "A Cloudflare container behind your stowbox-bridge Worker. It stops after the idle time with nothing connected, and a stopped box keeps no files: the next start is a fresh disk.".into(),
        free_sizes: Vec::new(),
        default_size: DEFAULT_SIZE.into(),
        default_disk_gib: 11,
        ssh_hint: String::new(),
        credential_fields: vec![
            CredentialField::plain(URL, "bridge URL", "The URL wrangler printed when you deployed bridge/cloudflare.", "https://stowbox-bridge.you.workers.dev"),
            CredentialField { label: "bridge token".into(), ..CredentialField::api_key(TOKEN_HINT) },
        ],
        icon: "CF".into(),
        label: "Cloudflare".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: Vec::new(),
        currency: "usd".into(),
        power: vec![Start, Stop, Destroy],
        fork: false,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: true,
        custom_size: false,
        sizes: convert::SIZES
            .iter()
            .map(|s| SizeOption {
                id: s.id.into(),
                vcpu: s.vcpu,
                memory_bytes: s.memory_gib << 30,
                disk_bytes: s.disk_gb * 1_000_000_000,
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
        metering_note: "The bridge cannot read Cloudflare's billing. This is the list price while running with the CPU fully busy; Cloudflare bills CPU on use, and a stopped box costs nothing.".into(),
        metered_usage: false,
    }
}

pub struct Cloudflare {
    api: Api,
}

impl Cloudflare {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        Ok(Self { api: Api::new(url, token)? })
    }

    fn path(id: &str, rest: &str) -> Result<String> {
        if convert::valid_id(id) {
            Ok(format!("/v1/boxes/{id}{rest}"))
        } else {
            Err(Error::NotFound(format!("{id:?} is not a box id from the bridge")))
        }
    }

    fn idle(secs: u32) -> Result<u32> {
        if (MIN_IDLE_SECS..=MAX_IDLE_SECS).contains(&secs) {
            Ok(secs)
        } else {
            Err(Error::Provider(format!(
                "a Cloudflare box stops after {} min to {} h with nothing connected; it cannot run on without a connection",
                MIN_IDLE_SECS / 60,
                MAX_IDLE_SECS / 3600
            )))
        }
    }
}

#[async_trait]
impl Provider for Cloudflare {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        let info = self.api.get("/v1/info").await?;
        if info.get("bridge").and_then(Value::as_str) != Some("stowbox-bridge") {
            return Err(Error::Provider("that URL answers, but not as a stowbox-bridge Worker".into()));
        }
        let version = info.get("version").and_then(Value::as_i64).unwrap_or(0);
        if version != BRIDGE_VERSION {
            return Err(Error::Provider(format!(
                "the bridge speaks version {version}; this Stowbox needs {BRIDGE_VERSION}. Deploy bridge/cloudflare from this release."
            )));
        }
        let size = convert::size(DEFAULT_SIZE);
        Ok(Account {
            user_id: String::new(),
            display_name: "Cloudflare bridge".into(),
            billing: Billing {
                mode: String::new(),
                currency: "usd".into(),
                balance_micro: 0,
                accruing_micro: None,
                max_vms: 0,
                default_vcpu: size.map(|s| s.vcpu).unwrap_or(0),
                default_memory_bytes: size.map(|s| s.memory_gib << 30).unwrap_or(0),
                ceiling_vcpu: 0,
                ceiling_memory_bytes: 0,
                subscription_status: String::new(),
            },
        })
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let page = self.api.get("/v1/boxes").await?;
        let boxes = page.get("boxes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        Ok(boxes.iter().map(convert::machine).filter(|m| convert::valid_id(&m.id)).collect())
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        if convert::valid_id(id_or_name) {
            return Ok(convert::machine(&self.api.get(&Self::path(id_or_name, "")?).await?));
        }
        self.list_machines().await?.into_iter().find(|m| m.name == id_or_name).ok_or_else(|| Error::NotFound(format!("the bridge has no box {id_or_name}")))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let size = spec.size.as_deref().filter(|s| !s.is_empty()).unwrap_or(DEFAULT_SIZE);
        if convert::size(size).is_none() {
            return Err(Error::Provider(format!("the bridge has no size {size}")));
        }
        let idle = Self::idle(spec.auto_stop_secs.unwrap_or(DEFAULT_IDLE_SECS))?;
        let name = Some(convert::name_from(spec.name.as_deref().unwrap_or(""))).filter(|n| !n.is_empty()).unwrap_or_else(|| "box".into());
        let env: serde_json::Map<String, Value> = spec.env.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
        let created = self.api.post("/v1/boxes", &json!({ "name": name, "size": size, "idle_secs": idle, "env": env })).await?;
        Ok(convert::machine(&created))
    }

    async fn fork_machine(&self, _source: &str, _name: Option<String>) -> Result<Machine> {
        Err(Error::Provider("a Cloudflare box cannot be forked".into()))
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        match action {
            PowerAction::Start => self.api.post(&Self::path(id, "/start")?, &json!({})).await.map(drop),
            PowerAction::Stop => self.api.post(&Self::path(id, "/stop")?, &json!({})).await.map(drop),
            PowerAction::Destroy => self.api.delete(&Self::path(id, "")?).await.map(drop),
            _ => Err(Error::Provider("a Cloudflare box can be started, stopped or deleted".into())),
        }
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a Cloudflare box has no hibernate; it stops when idle".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a Cloudflare box has no standby; it stops when idle".into()))
    }

    async fn set_auto_stop(&self, id: &str, secs: u32) -> Result<()> {
        let path = Self::path(id, "/idle")?;
        self.api.put(&path, &json!({ "secs": Self::idle(secs)? })).await.map(drop)
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        exec::run(&self.api, &Self::path(id, "/exec")?, spec).await
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("the bridge cannot read Cloudflare's billing; the Cloudflare dashboard shows it".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("the bridge cannot read Cloudflare's billing".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        let sizes = convert::SIZES.iter().map(|s| SizeRate { size: s.id.into(), hourly_micro: hourly_micro(s) }).collect();
        Ok(RateCard { currency: "usd".into(), rates: Rates::default(), sizes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn capabilities_match_what_the_bridge_does() {
        let c = capabilities();
        assert!(c.create && !c.fork && c.idle_stop && !c.auto_stop && !c.metered_usage);
        assert!(c.supports(PowerAction::Stop) && c.supports(PowerAction::Start) && c.supports(PowerAction::Destroy) && !c.supports(PowerAction::Hibernate));
        assert_eq!(c.sizes.iter().map(|s| (s.id.as_str(), s.vcpu)).collect::<Vec<_>>(), [("standard-2", 1), ("standard-3", 2), ("standard-4", 4)]);
        assert!(c.credential_fields.iter().any(|f| f.name == URL && !f.secret));
    }

    #[test]
    fn sizes_cost_their_list_price_only_while_running() {
        // 1 vCPU × $0.072 + 6 GiB × $0.009 + 12 GB × $0.000252 an hour.
        assert_eq!(capabilities().sizes.iter().map(|s| s.hourly_micro).collect::<Vec<_>>(), [129_024, 220_032, 401_040]);
        let card = RateCard { currency: "usd".into(), rates: Rates::default(), sizes: vec![SizeRate { size: "standard-2".into(), hourly_micro: 129_024 }] };
        let mut m = crate::testing::machine("box-abcdefghij0123456789", MachineStatus::Running);
        m.size = "standard-2".into();
        assert_eq!(card.hourly_micro(&m), 129_024);
        m.status = MachineStatus::Stopped;
        assert_eq!(card.hourly_micro(&m), 0);
    }

    #[test]
    fn ids_that_could_change_a_path_and_bad_idle_times_are_refused() {
        assert_eq!(Cloudflare::path("box-abcdefghij0123456789", "/stop").unwrap(), "/v1/boxes/box-abcdefghij0123456789/stop");
        assert!(matches!(Cloudflare::path("../info", ""), Err(Error::NotFound(_))));
        assert!(Cloudflare::idle(0).is_err() && Cloudflare::idle(59).is_err() && Cloudflare::idle(86_401).is_err());
        assert_eq!(Cloudflare::idle(600).unwrap(), 600);
    }

    #[test]
    fn a_connection_needs_the_url_and_the_token() {
        let token = ("api_key".to_string(), "t".repeat(40));
        assert!(matches!(connect(&Credentials::new([token.clone()])), Err(Error::Auth(_))));
        assert!(matches!(connect(&Credentials::new([("url".into(), "https://b.example.com".into())])), Err(Error::Auth(_))));
        assert!(connect(&Credentials::new([token.clone(), ("url".into(), "http://b.example.com".into())])).is_err());
        assert!(connect(&Credentials::new([token, ("url".into(), "https://b.example.com".into())])).is_ok());
    }

    /// Live: a terminal resizes, dropping a terminal or a script ends what it
    /// started, and a stopped box comes back on a fresh disk. Needs
    /// `STOWBOX_LIVE_CLOUDFLARE_URL` and `_KEY` (the bridge token).
    #[tokio::test]
    #[ignore = "makes and deletes a real Cloudflare container"]
    async fn a_terminal_resizes_and_dropping_an_exec_ends_its_session() {
        let (Ok(url), Ok(token)) = (std::env::var("STOWBOX_LIVE_CLOUDFLARE_URL"), std::env::var("STOWBOX_LIVE_CLOUDFLARE_KEY")) else { return };
        let p = Cloudflare::new(&url, &token).unwrap();
        let m = p.create_machine(CreateSpec { name: Some("stowbox-live-tty".into()), ..Default::default() }).await.unwrap();
        let run = |script: &'static str| {
            let p = &p;
            let id = m.id.clone();
            async move {
                let mut h = p.exec(&id, ExecSpec { script: script.into(), ..Default::default() }).await?;
                let mut out = Vec::new();
                while let Some(o) = h.output.recv().await {
                    if let ExecOutput::Stdout(b) = o {
                        out.extend(b);
                    }
                }
                Ok::<_, Error>(String::from_utf8_lossy(&out).trim().to_string())
            }
        };
        let check = async {
            let mut h = p.exec(&m.id, ExecSpec { script: "sleep 1; stty size".into(), tty: true, cols: 100, rows: 30, env: vec![] }).await?;
            h.input.send(ExecInput::Resize { cols: 132, rows: 40 }).await.map_err(|_| Error::Network("closed".into()))?;
            let mut size = Vec::new();
            while let Some(o) = h.output.recv().await {
                if let ExecOutput::Stdout(b) = o {
                    size.extend(b);
                }
            }
            let tty = p.exec(&m.id, ExecSpec { script: "sleep 777".into(), tty: true, cols: 80, rows: 24, env: vec![] }).await?;
            let script = p.exec(&m.id, ExecSpec { script: "sleep 778 & sleep 779".into(), ..Default::default() }).await?;
            // Its leader exits at once; the child keeps the output open.
            let orphan = p.exec(&m.id, ExecSpec { script: "(sleep 776 &); echo started".into(), ..Default::default() }).await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop((tty, script, orphan));
            tokio::time::sleep(Duration::from_secs(4)).await;
            let left = run("pgrep -c -f '^sleep 77[6789]' || true").await?;
            let who = run("id -un; echo $HOME; touch /root/marker").await?;
            p.power(&m.id, PowerAction::Stop).await?;
            let fresh = run("test -e /root/marker && echo kept || echo fresh").await?;
            Ok::<_, Error>((String::from_utf8_lossy(&size).trim().to_string(), left, who, fresh))
        };
        let result = check.await;
        p.power(&m.id, PowerAction::Destroy).await.unwrap();
        let (size, left, who, fresh) = result.unwrap();
        assert_eq!(size, "40 132");
        assert_eq!(left, "0");
        assert_eq!(who, format!("{USER}\n{HOME}"));
        assert_eq!(fresh, "fresh");
    }
}
