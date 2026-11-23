//! Sprites (sprites.dev, by Fly.io) behind the `Provider` trait.
//!
//! The lifecycle is the REST API at `api.sprites.dev/v1`, with an
//! organization's access token. A sprite is named, and its name is its id.
//! Commands and terminals go over a WebSocket per exec (see [`exec`]).
//!
//! A sprite pauses by itself about 30 seconds after its last use: `warm`
//! keeps memory and wakes in under a second, `cold` keeps only the disk.
//! The next exec wakes it, and the disk always persists. So there is no
//! idle stop to set, and no power action but delete. Every sprite has 8
//! vCPUs and 100 GB of disk; memory is managed by Sprites and not reported.

pub mod api;
pub mod convert;
pub mod exec;

use crate::capabilities::{Capabilities, CredentialField, SizeOption};
use crate::cost::{RateCard, SizeRate};
use crate::daytona::api::seg;
use crate::provider::Provider;
use crate::providers::Credentials;
use crate::types::*;
use crate::{Error, Result};
use api::Api;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub const ID: &str = "sprites";
pub const DEFAULT_CONSOLE_URL: &str = "https://sprites.dev";
/// The one shape a sprite has.
pub const SIZE: &str = "sprite";
pub const VCPUS: u32 = 8;
pub const DISK_GB: u64 = 100;
const USER: &str = "sprite";
const HOME: &str = "/home/sprite";
/// The largest file Sprites' `fs/write` takes.
const MAX_UPLOAD: u64 = 16 << 20;
/// Pages of 100 read before the list stops.
const MAX_PAGES: usize = 20;

/// Sprites' list price (2026-11), in micro-USD: $0.0385 a CPU-hour of CPU
/// time used. Memory ($0.021875 a GB-hour) and storage are billed on use,
/// which the API does not report, so the estimate is the CPU at full use.
const CPU_HOUR_MICRO: i64 = 38_500;
const HOURLY_MICRO: i64 = VCPUS as i64 * CPU_HOUR_MICRO;

const TOKEN_HINT: &str = "An access token for your Sprites organization, from sprites.dev. It looks like org/id/secret.";

pub const KIND: crate::providers::ProviderKind = crate::providers::ProviderKind { id: ID, capabilities, connect: |credentials, _| connect(&credentials) };

fn connect(credentials: &Credentials) -> Result<Arc<dyn Provider>> {
    Ok(Arc::new(Sprites::new(api::DEFAULT_API, &credentials.api_key()?)?))
}

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        create: true,
        new_box_blurb: "A Sprites microVM with 8 vCPUs and a 100 GB disk that persists. It pauses by itself seconds after its last use and wakes on the next command.".into(),
        free_sizes: Vec::new(),
        default_size: SIZE.into(),
        default_disk_gib: 93,
        ssh_hint: String::new(),
        credential_fields: vec![CredentialField::api_key(TOKEN_HINT)],
        icon: "SP".into(),
        label: "Sprites".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_prefixes: Vec::new(),
        currency: "usd".into(),
        power: vec![Destroy],
        fork: false,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        auto_stop: false,
        idle_stop: false,
        pauses_when_idle: true,
        custom_size: false,
        sizes: vec![SizeOption {
            id: SIZE.into(),
            vcpu: VCPUS,
            memory_bytes: 0,
            disk_bytes: DISK_GB * 1_000_000_000,
            hourly_micro: HOURLY_MICRO,
            note: String::new(),
        }],
        https_links: false,
        https_named: false,
        https_list: false,
        https_remove: false,
        https_default_route: false,
        port_forwards: false,
        editor_ssh: false,
        max_upload_bytes: MAX_UPLOAD,
        remote_user: USER.into(),
        remote_home: HOME.into(),
        metering_note: "Sprites bills CPU time, memory and storage on use, and its API does not report them. This is the CPU at full use while awake; a paused sprite costs only its storage.".into(),
        metered_usage: false,
    }
}

pub struct Sprites {
    api: Arc<Api>,
}

impl Sprites {
    pub fn new(base: &str, token: &str) -> Result<Self> {
        Ok(Self { api: Arc::new(Api::new(base, token)?) })
    }

    fn checked(name: &str) -> Result<&str> {
        if convert::valid_name(name) {
            Ok(name)
        } else {
            Err(Error::NotFound(format!("{name:?} is not a sprite name")))
        }
    }

    fn path(name: &str) -> Result<String> {
        Ok(format!("/v1/sprites/{}", seg(Self::checked(name)?)))
    }
}

#[async_trait]
impl Provider for Sprites {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        let page = self.api.get("/v1/sprites?max_results=1").await?;
        // The page names the organization; a sprite on it does too.
        let first_sprite = page.get("sprites").and_then(Value::as_array).and_then(|s| s.first());
        let org: String = [page.get("name"), first_sprite.and_then(|s| s.get("organization"))]
            .into_iter()
            .find_map(|v| v.and_then(Value::as_str).filter(|s| !s.is_empty()))
            .unwrap_or("")
            .chars()
            .take(100)
            .collect();
        Ok(Account {
            user_id: org.clone(),
            // The UI puts the provider's label in front.
            display_name: if org.is_empty() { "organization".into() } else { org.clone() },
            billing: Billing {
                mode: String::new(),
                currency: "usd".into(),
                balance_micro: 0,
                accruing_micro: None,
                max_vms: page.get("running_limit").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
                default_vcpu: VCPUS,
                default_memory_bytes: 0,
                ceiling_vcpu: 0,
                ceiling_memory_bytes: 0,
                subscription_status: String::new(),
            },
        })
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let path = match &token {
                Some(t) => format!("/v1/sprites?max_results=100&continuation_token={}", seg(t)),
                None => "/v1/sprites?max_results=100".into(),
            };
            let page = self.api.get(&path).await?;
            let sprites = page.get("sprites").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            out.extend(sprites.iter().map(convert::machine).filter(|m| convert::valid_name(&m.id)));
            token = page.get("next_continuation_token").and_then(Value::as_str).filter(|t| !t.is_empty() && t.len() < 1024).map(String::from);
            if page.get("has_more").and_then(Value::as_bool) != Some(true) || token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        let m = convert::machine(&self.api.get(&Self::path(id_or_name)?).await?);
        if convert::valid_name(&m.id) {
            Ok(m)
        } else {
            Err(Error::Provider("Sprites answered with a sprite that has no usable name".into()))
        }
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let typed = convert::name_from(spec.name.as_deref().unwrap_or(""));
        let name = if typed.is_empty() { format!("box-{}", uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>()) } else { typed };
        let mut body = serde_json::Map::new();
        body.insert("name".into(), Value::String(name.clone()));
        if !spec.env.is_empty() {
            body.insert("environment".into(), Value::Object(spec.env.into_iter().map(|(k, v)| (k, Value::String(v))).collect()));
        }
        let created = self.api.post("/v1/sprites", &Value::Object(body)).await?;
        let m = convert::machine(&created);
        // Some answers carry only the id; the name asked for is the sprite's.
        if convert::valid_name(&m.id) {
            Ok(m)
        } else {
            self.get_machine(&name).await
        }
    }

    async fn fork_machine(&self, _source: &str, _name: Option<String>) -> Result<Machine> {
        Err(Error::Provider("a sprite cannot be forked from Stowbox".into()))
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        match action {
            PowerAction::Destroy => self.api.delete(&Self::path(id)?).await.map(drop),
            _ => Err(Error::Provider("a sprite pauses and wakes by itself; it can only be deleted".into())),
        }
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a sprite pauses by itself seconds after its last use".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a sprite pauses by itself seconds after its last use".into()))
    }

    async fn set_auto_stop(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("a sprite pauses by itself seconds after its last use".into()))
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        exec::run(self.api.clone(), Self::checked(id)?.to_string(), spec).await
    }

    /// Sprites' file API, which takes the bytes as they are: no base64 over
    /// an exec, and no shell between the path and the file.
    async fn upload_file(&self, id: &str, path: &str, data: Vec<u8>) -> Result<u64> {
        if !path.starts_with('/') {
            return Err(Error::Provider(format!("{path:?} is not an absolute path")));
        }
        let expected = data.len() as u64;
        if expected > MAX_UPLOAD {
            return Err(Error::Provider(format!("Sprites takes files of up to 16 MB; this one is {expected} bytes")));
        }
        let written = self.api.put_bytes(&format!("{}/fs/write?path={}&workingDir=%2F", Self::path(id)?, seg(path)), data).await?;
        crate::upload::confirm_written(written.get("size").and_then(Value::as_u64).unwrap_or(0), expected, path)
    }

    async fn machine_usage(&self, _id: &str) -> Result<Usage> {
        Err(Error::Provider("Sprites' API does not report what a sprite has cost; sprites.dev shows it".into()))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Err(Error::Provider("Sprites' API does not report billing".into()))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard { currency: "usd".into(), rates: Rates::default(), sizes: vec![SizeRate { size: SIZE.into(), hourly_micro: HOURLY_MICRO }] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn capabilities_match_what_sprites_does() {
        let c = capabilities();
        assert!(c.create && !c.fork && !c.idle_stop && !c.auto_stop && !c.metered_usage);
        assert!(c.supports(PowerAction::Destroy) && !c.supports(PowerAction::Stop) && !c.supports(PowerAction::Start));
        assert_eq!(c.sizes.iter().map(|s| (s.id.as_str(), s.vcpu, s.hourly_micro)).collect::<Vec<_>>(), [("sprite", 8, 308_000)]);
        assert!(c.credential_fields.iter().all(|f| f.secret));
    }

    #[test]
    fn a_paused_sprite_costs_nothing_for_cpu() {
        let card = RateCard { currency: "usd".into(), rates: Rates::default(), sizes: vec![SizeRate { size: SIZE.into(), hourly_micro: HOURLY_MICRO }] };
        let mut m = convert::machine(&serde_json::json!({ "name": "a", "status": "running" }));
        assert_eq!(card.hourly_micro(&m), 308_000);
        m.status = MachineStatus::Suspended;
        assert_eq!(card.hourly_micro(&m), 0);
    }

    #[test]
    fn names_that_could_change_a_path_are_refused() {
        assert_eq!(Sprites::path("my-box").unwrap(), "/v1/sprites/my-box");
        for bad in ["../x", "a/b", "a?b", ""] {
            assert!(matches!(Sprites::path(bad), Err(Error::NotFound(_))), "{bad}");
        }
    }

    /// Live: a terminal resizes, dropping a terminal or a script ends what it
    /// started, and the disk persists while the sprite sleeps. Needs
    /// `STOWBOX_LIVE_SPRITES_KEY`.
    #[tokio::test]
    #[ignore = "makes and deletes a real sprite"]
    async fn a_terminal_resizes_and_dropping_an_exec_ends_its_session() {
        let Ok(token) = std::env::var("STOWBOX_LIVE_SPRITES_KEY") else { return };
        let p = Sprites::new(api::DEFAULT_API, &token).unwrap();
        let m = p.create_machine(CreateSpec { name: Some("stowbox-live-tty".into()), ..Default::default() }).await.unwrap();
        let run = |script: &'static str| {
            let p = &p;
            let id = m.id.clone();
            async move {
                let mut h = p.exec(&id, ExecSpec { script: script.into(), ..Default::default() }).await?;
                let mut out = Vec::new();
                while let Some(o) = h.output.recv().await {
                    match o {
                        ExecOutput::Stdout(b) => out.extend(b),
                        ExecOutput::Error(e) => return Err(Error::Network(e)),
                        _ => {}
                    }
                }
                Ok::<_, Error>(String::from_utf8_lossy(&out).trim().to_string())
            }
        };
        let check = async {
            let mut h = p.exec(&m.id, ExecSpec { script: "sleep 1; stty size; exit 3".into(), tty: true, cols: 100, rows: 30, env: vec![] }).await?;
            h.input.send(ExecInput::Resize { cols: 132, rows: 40 }).await.map_err(|_| Error::Network("closed".into()))?;
            let (mut size, mut code) = (Vec::new(), None);
            while let Some(o) = h.output.recv().await {
                match o {
                    ExecOutput::Stdout(b) => size.extend(b),
                    ExecOutput::Exit(c) => code = Some(c),
                    _ => {}
                }
            }
            let tty = p.exec(&m.id, ExecSpec { script: "sleep 777".into(), tty: true, cols: 80, rows: 24, env: vec![] }).await?;
            let script = p.exec(&m.id, ExecSpec { script: "sleep 778 & sleep 779".into(), ..Default::default() }).await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop((tty, script));
            tokio::time::sleep(Duration::from_secs(5)).await;
            let left = run("pgrep -c -f '^sleep 77[789]' || true").await?;
            let who = run("id -un; echo $HOME; touch ~/marker").await?;
            // Long enough for the sprite to pause.
            tokio::time::sleep(Duration::from_secs(90)).await;
            let paused = p.get_machine(&m.id).await?.status;
            let kept = run("test -e ~/marker && echo kept || echo gone").await?;
            Ok::<_, Error>((String::from_utf8_lossy(&size).trim().to_string(), code, left, who, paused, kept))
        };
        let result = check.await;
        p.power(&m.id, PowerAction::Destroy).await.unwrap();
        let (size, code, left, who, paused, kept) = result.unwrap();
        assert!(size.ends_with("40 132"), "{size:?}");
        assert_eq!(code, Some(3));
        assert_eq!(left, "0");
        assert_eq!(who, format!("{USER}\n{HOME}"));
        assert_ne!(paused, MachineStatus::Running);
        assert_eq!(kept, "kept");
    }
}
