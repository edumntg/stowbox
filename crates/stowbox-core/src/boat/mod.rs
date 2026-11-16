//! boat.dev over its REST API (`https://boat.dev/api/v1`), with exec over
//! SSH because the command API has no stdin or TTY. See `docs/boat.md`.

mod api;
pub(crate) mod convert;
pub mod ssh;

use crate::capabilities::{Capabilities, SizeOption};
use crate::cost::{RateCard, SizeRate};
use crate::ports::HttpsRoute;
use crate::provider::Provider;
use crate::sshexec;
use crate::types::*;
use crate::{Error, Result};
use api::{seg, Api};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub const DEFAULT_API: &str = "https://boat.dev/api/v1";
pub const DEFAULT_CONSOLE_URL: &str = "https://boat.dev/dashboard";

/// Largest file one upload may carry. Uploads go to a signed URL, which has
/// no documented cap; the file is held in memory on its way through the
/// app, a few copies at once, and this keeps that sane.
pub const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// One size on boat's menu, from its pricing page (2026-11).
pub struct Size {
    pub id: &'static str,
    pub vcpu: u32,
    pub memory_gib: u64,
    pub disk_gb: u64,
    pub hourly_micro_usd: i64,
    pub note: &'static str,
}

pub const SIZES: &[Size] = &[
    Size { id: "small", vcpu: 2, memory_gib: 4, disk_gb: 51, hourly_micro_usd: 18_000, note: "" },
    Size { id: "default", vcpu: 4, memory_gib: 8, disk_gb: 51, hourly_micro_usd: 36_000, note: "" },
    Size { id: "large", vcpu: 8, memory_gib: 16, disk_gb: 111, hourly_micro_usd: 72_000, note: "not on the free trial" },
    Size { id: "xlarge", vcpu: 16, memory_gib: 32, disk_gb: 251, hourly_micro_usd: 200_000, note: "needs the $100 plan or higher" },
];

pub fn capabilities() -> Capabilities {
    use PowerAction::*;
    Capabilities {
        label: "boat".into(),
        console_url: DEFAULT_CONSOLE_URL.into(),
        key_hint: "Make one with `boat api-key create stowbox` or on the boat dashboard. It needs the ssh action: Stowbox runs every command over SSH.".into(),
        key_prefixes: vec!["boat_".into(), "box_".into()],
        currency: "usd".into(),
        power: vec![Start, Stop, Destroy],
        fork: true,
        isolated: false,
        auto_suspend: false,
        auto_hibernate: false,
        idle_stop: false,
        auto_stop: true,
        custom_size: false,
        sizes: SIZES
            .iter()
            .map(|s| SizeOption {
                id: s.id.into(),
                vcpu: s.vcpu,
                memory_bytes: s.memory_gib * 1024 * 1024 * 1024,
                disk_bytes: s.disk_gb * 1_000_000_000,
                hourly_micro: s.hourly_micro_usd,
                note: s.note.into(),
            })
            .collect(),
        https_links: true,
        https_named: false,
        https_list: false,
        https_remove: false,
        https_default_route: false,
        port_forwards: false,
        editor_ssh: false,
        max_upload_bytes: MAX_UPLOAD_BYTES,
        remote_user: "user".into(),
        remote_home: "/home/user".into(),
        metering_note: "boat counts by the second at list price; time included in the plan is not taken off here.".into(),
        metered_usage: true,
    }
}

#[derive(Debug, Clone)]
pub struct BoatConfig {
    pub api: String,
    /// Where Stowbox keeps its own SSH key for boat sandboxes.
    pub ssh_key: PathBuf,
}

impl BoatConfig {
    pub fn new(ssh_key: PathBuf) -> Self {
        Self { api: DEFAULT_API.into(), ssh_key }
    }
}

pub struct Boat {
    api: Api,
    ssh: sshexec::Pool,
    /// Links made from this Stowbox, by sandbox: boat has no call that
    /// lists them.
    links: Mutex<HashMap<String, Vec<HttpsRoute>>>,
}

impl Boat {
    pub fn new(api_key: String, config: BoatConfig) -> Result<Self> {
        let key = sshexec::load_or_make_key(&config.ssh_key)?;
        Ok(Self { api: Api::new(&config.api, api_key)?, ssh: sshexec::Pool::new(key)?, links: Default::default() })
    }

    async fn sandbox(&self, id: &str) -> Result<Value> {
        let v = self.api.get(&format!("/sandboxes/{}", seg(id))).await?;
        v.get("sandbox").cloned().ok_or_else(|| Error::Provider("boat answered without the sandbox".into()))
    }

    /// The machine in an answer that carries one, else read it back.
    async fn machine_from(&self, answer: &Value, fallback_id: &str) -> Result<Machine> {
        if let Some(s) = answer.get("sandbox").filter(|s| s.get("id").is_some()) {
            return Ok(convert::machine(s));
        }
        let id = answer.get("id").and_then(Value::as_str).unwrap_or(fallback_id);
        if id.is_empty() {
            return Err(Error::Provider("boat answered without a sandbox id".into()));
        }
        Ok(convert::machine(&self.sandbox(id).await?))
    }
}

/// Writes `data` to `path` in the sandbox through a signed upload URL: no
/// size cap of the JSON file call (5 MiB) and no base64 over SSH. boat makes
/// missing directories and swaps the file in only once it is complete.
async fn upload_signed(api: &Api, id: &str, path: &str, data: Vec<u8>) -> Result<u64> {
    let size = data.len() as u64;
    let r = api.post(&format!("/sandboxes/{}/files/upload-url", seg(id)), json!({ "path": path })).await?;
    let url = r.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).ok_or_else(|| Error::Provider("boat answered without an upload URL".into()))?;
    api.put_signed(&api.absolute(url), data).await?;
    Ok(size)
}

/// `ttlSeconds` for a create or a resume: None leaves boat's default (one
/// hour), 0 is never.
fn ttl(secs: Option<u32>) -> Option<Value> {
    secs.map(|s| if s == 0 { Value::Null } else { json!(s) })
}

#[async_trait]
impl Provider for Boat {
    fn id(&self) -> &'static str {
        "boat"
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn whoami(&self) -> Result<Account> {
        let me = self.api.get("/me").await?;
        // The wallet is on the limits; an account without it still signs in.
        let limits = self.api.get("/limits").await.unwrap_or(Value::Null);
        Ok(convert::account(&me, &limits))
    }

    async fn list_machines(&self) -> Result<Vec<Machine>> {
        let all = self.api.list_all("/sandboxes", "sandboxes").await?;
        // A deleted sandbox can linger in the list while its data goes; its
        // record says so with `dataExpiredAt`, and it cannot come back.
        Ok(all.iter().filter(|s| s.get("dataExpiredAt").map_or(true, Value::is_null)).map(convert::machine).collect())
    }

    async fn get_machine(&self, id_or_name: &str) -> Result<Machine> {
        Ok(convert::machine(&self.sandbox(id_or_name).await?))
    }

    async fn create_machine(&self, spec: CreateSpec) -> Result<Machine> {
        let mut body = Map::new();
        body.insert("type".into(), json!(spec.size.filter(|s| !s.is_empty()).unwrap_or_else(|| "default".into())));
        if let Some(n) = spec.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            body.insert("name".into(), json!(n));
        }
        if let Some(t) = ttl(spec.auto_stop_secs) {
            body.insert("ttlSeconds".into(), t);
        }
        if !spec.env.is_empty() {
            body.insert("env".into(), Value::Object(spec.env.into_iter().map(|(k, v)| (k, json!(v))).collect()));
        }
        if let Some(org) = spec.org.filter(|o| !o.is_empty()) {
            body.insert("org".into(), json!(org));
        }
        let r = self.api.post_once("/sandboxes", Value::Object(body)).await?;
        self.machine_from(&r, "").await
    }

    async fn fork_machine(&self, source: &str, name: Option<String>) -> Result<Machine> {
        // The fork keeps the source's auto-stop; boat's default would stop it in an hour.
        let src = self.sandbox(source).await?;
        let mut body = Map::new();
        if let Some(t) = src.get("ttlSeconds") {
            body.insert("ttlSeconds".into(), t.clone());
        }
        let r = self.api.post_once(&format!("/sandboxes/{}/fork", seg(source)), Value::Object(body)).await?;
        let m = self.machine_from(&r, "").await?;
        match name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            // A fork takes no name; it is set right after.
            Some(n) => {
                let r = self.api.patch(&format!("/sandboxes/{}", seg(&m.id)), json!({ "name": n })).await?;
                self.machine_from(&r, &m.id).await
            }
            None => Ok(m),
        }
    }

    async fn power(&self, id: &str, action: PowerAction) -> Result<()> {
        let path = format!("/sandboxes/{}", seg(id));
        match action {
            PowerAction::Start | PowerAction::Resume | PowerAction::Wake => {
                // A resume without ttlSeconds falls back to boat's one hour;
                // keep the sandbox's own setting.
                let s = self.sandbox(id).await?;
                let body = match s.get("ttlSeconds") {
                    Some(t) => json!({ "ttlSeconds": t }),
                    None => json!({}),
                };
                self.api.post(&format!("{path}/resume"), body).await?;
            }
            PowerAction::Stop => {
                self.api.post(&format!("{path}/stop"), json!({})).await?;
            }
            PowerAction::Destroy => {
                self.api.delete(&path, &[("X-Ascii-Confirm-Delete", id)]).await?;
                self.links.lock().unwrap().remove(id);
            }
            PowerAction::Pause | PowerAction::Hibernate | PowerAction::Reboot => {
                return Err(Error::Provider(format!("boat sandboxes cannot {}; stop it instead", format!("{action:?}").to_lowercase())));
            }
        }
        // The sandbox moves to another machine on the next start.
        self.ssh.drop_conn(id);
        Ok(())
    }

    async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("boat has no auto hibernate; use auto-stop".into()))
    }

    async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
        Err(Error::Provider("boat has no auto standby; use auto-stop".into()))
    }

    async fn set_auto_stop(&self, id: &str, secs: u32) -> Result<()> {
        let t = ttl(Some(secs)).unwrap_or(Value::Null);
        self.api.patch(&format!("/sandboxes/{}", seg(id)), json!({ "ttlSeconds": t })).await?;
        Ok(())
    }

    async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
        self.ssh.exec(&ssh::ApiAuth(&self.api), id, spec).await
    }

    async fn upload_file(&self, id: &str, path: &str, data: Vec<u8>) -> Result<u64> {
        upload_signed(&self.api, id, path, data).await
    }

    async fn machine_usage(&self, id: &str) -> Result<Usage> {
        let u = self.api.get(&format!("/sandboxes/{}/usage", seg(id))).await?;
        Ok(convert::usage(&u))
    }

    async fn org_billing(&self, _org: &str) -> Result<OrgBilling> {
        Ok(convert::org_billing(&self.api.get("/limits").await?))
    }

    async fn rates(&self) -> Result<RateCard> {
        Ok(RateCard {
            currency: "usd".into(),
            rates: Rates::default(),
            sizes: SIZES.iter().map(|s| SizeRate { size: s.id.into(), hourly_micro: s.hourly_micro_usd }).collect(),
        })
    }

    async fn https_routes(&self, id: &str) -> Result<Vec<HttpsRoute>> {
        Ok(self.links.lock().unwrap().get(id).cloned().unwrap_or_default())
    }

    async fn add_https_route(&self, id: &str, _name: &str, port: u16) -> Result<HttpsRoute> {
        let r = self.api.post(&format!("/sandboxes/{}/host", seg(id)), json!({ "port": port })).await?;
        let url = r.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let domain = url.trim_start_matches("https://").split(['/', '?']).next().unwrap_or("").to_string();
        let route = HttpsRoute {
            name: port.to_string(),
            domain,
            url: Some(url).filter(|u| !u.is_empty()),
            port: port as u32,
            port_mode: "locked".into(),
            is_default: false,
        };
        let mut links = self.links.lock().unwrap();
        let list = links.entry(id.to_string()).or_default();
        list.retain(|l| l.port != route.port);
        list.push(route.clone());
        list.sort_by_key(|l| l.port);
        Ok(route)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_and_the_rate_card_agree() {
        let caps = capabilities();
        assert_eq!(caps.sizes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["small", "default", "large", "xlarge"]);
        assert!(!caps.supports(PowerAction::Pause) && !caps.supports(PowerAction::Hibernate) && caps.supports(PowerAction::Stop));
        // A default sandbox for a 730-hour month: boat's "$26".
        let default = SIZES.iter().find(|s| s.id == "default").unwrap();
        assert_eq!(crate::cost::money(default.hourly_micro_usd * 730, "usd"), "$26.28");
    }

    /// A one-thread HTTP/1.1 server that answers `replies` in order and
    /// hands back each request as (head, body).
    fn serve(replies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<(String, Vec<u8>)>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut replies = replies.into_iter();
            while let Some(reply) = replies.next() {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                let mut reply = Some(reply);
                // Keep-alive: several requests can come on one connection.
                while let Some(r) = reply.take() {
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                            break;
                        }
                        head.push_str(&line);
                    }
                    let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    let mut body = vec![0; len];
                    reader.read_exact(&mut body).unwrap();
                    seen.push((head, body));
                    stream.write_all(r.as_bytes()).unwrap();
                    stream.flush().unwrap();
                    reply = replies.next();
                    if reply.is_none() {
                        break;
                    }
                }
            }
            seen
        });
        (base, handle)
    }

    fn json_reply(status: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
    }

    #[tokio::test]
    async fn uploads_go_to_a_signed_url_without_the_key() {
        let data: Vec<u8> = (0..=255u8).cycle().take(6 * 1024 * 1024).collect();
        // The URL comes back as a path; it resolves against the API's origin.
        let (base, server) = serve(vec![
            json_reply("200 OK", r#"{"ok":true,"url":"/upload/abc?sig=s3cr3t","expiresAt":"2026-11-16T12:00:00Z"}"#),
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".into(),
        ]);
        let api = Api::new(&format!("{base}/api/v1"), "boat_key".into()).unwrap();
        let n = upload_signed(&api, "bx_1", "/home/user/.stowbox/uploads/a/t-big.bin", data.clone()).await.unwrap();
        assert_eq!(n, data.len() as u64);
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2);
        let (ask, ask_body) = &seen[0];
        assert!(ask.starts_with("POST /api/v1/sandboxes/bx_1/files/upload-url "), "{ask}");
        assert!(ask.to_ascii_lowercase().contains("authorization: bearer boat_key"), "{ask}");
        assert_eq!(serde_json::from_slice::<Value>(ask_body).unwrap(), json!({ "path": "/home/user/.stowbox/uploads/a/t-big.bin" }));
        let (put, put_body) = &seen[1];
        assert!(put.starts_with("PUT /upload/abc?sig=s3cr3t "), "{put}");
        assert!(!put.to_ascii_lowercase().contains("authorization"), "the key went to the signed URL: {put}");
        assert_eq!(put_body, &data);
    }

    #[tokio::test]
    async fn a_refused_upload_says_why_without_the_url() {
        let (base, server) = serve(vec![
            json_reply("200 OK", &format!(r#"{{"url":"http://{}/upload/abc?sig=s3cr3t"}}"#, "127.0.0.1:1")),
        ]);
        let api = Api::new(&base, "boat_key".into()).unwrap();
        // Port 1 refuses the connection: a network error, which must not name the URL.
        let e = upload_signed(&api, "bx_1", "/tmp/f", b"x".to_vec()).await.unwrap_err().to_string();
        assert!(!e.contains("s3cr3t"), "{e}");
        server.join().unwrap();

        let (base, server) = serve(vec![
            json_reply("200 OK", r#"{"url":"/upload/abc?sig=s3cr3t"}"#),
            json_reply("403 Forbidden", r#"{"code":"url_expired","message":"This upload URL has expired."}"#),
        ]);
        let api = Api::new(&base, "boat_key".into()).unwrap();
        let e = upload_signed(&api, "bx_1", "/tmp/f", b"x".to_vec()).await.unwrap_err().to_string();
        assert_eq!(e, "boat refused the upload (403): This upload URL has expired.");
        server.join().unwrap();

        let (base, server) = serve(vec![json_reply("400 Bad Request", r#"{"code":"invalid_path","message":"Path must be under /home/user or /tmp."}"#)]);
        let api = Api::new(&base, "boat_key".into()).unwrap();
        let e = upload_signed(&api, "bx_1", "/etc/f", b"x".to_vec()).await.unwrap_err().to_string();
        assert!(e.contains("invalid_path"), "{e}");
        server.join().unwrap();
    }

    #[test]
    fn ttl_maps_never_to_null() {
        assert_eq!(ttl(None), None);
        assert_eq!(ttl(Some(0)), Some(Value::Null));
        assert_eq!(ttl(Some(3600)), Some(json!(3600)));
    }
}
