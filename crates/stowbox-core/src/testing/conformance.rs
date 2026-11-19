//! Checks that a provider does what its `Capabilities` claim, against a fake
//! of its API. Each fake answers in the shapes the provider's own `convert`
//! tests pin; none of them is recorded from the real service.

use crate::agent37::{Agent37, Agent37Config};
use crate::boat::{Boat, BoatConfig};
use crate::daytona::{Daytona, DaytonaConfig};
use crate::provider::Provider;
use crate::types::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What one run covered.
#[derive(Debug, Default)]
pub struct Outcome {
    pub passed: Vec<&'static str>,
}

/// Each power action that takes a box down, the one that brings it back,
/// and the step's name. Run in order, each pair leaves the box running.
pub(super) const POWER_PAIRS: &[(PowerAction, Option<PowerAction>, &str)] = &[
    (PowerAction::Reboot, None, "reboot"),
    (PowerAction::Pause, Some(PowerAction::Resume), "pause and resume"),
    (PowerAction::Hibernate, Some(PowerAction::Wake), "hibernate and wake"),
    (PowerAction::Stop, Some(PowerAction::Start), "stop and start"),
];

/// The status a box must reach after `action`.
pub(super) fn after(action: PowerAction) -> Option<MachineStatus> {
    match action {
        PowerAction::Start | PowerAction::Resume | PowerAction::Wake | PowerAction::Reboot => Some(MachineStatus::Running),
        PowerAction::Stop => Some(MachineStatus::Stopped),
        PowerAction::Pause => Some(MachineStatus::Suspended),
        PowerAction::Hibernate => Some(MachineStatus::Hibernated),
        PowerAction::Destroy => None,
    }
}

/// Runs every lifecycle claim. Exec and upload need a transport the fakes do
/// not have; `testing::live` covers them against the real service.
pub async fn run(p: &dyn Provider) -> Outcome {
    let caps = p.capabilities();
    let mut out = Outcome::default();
    assert!(caps.supports(PowerAction::Destroy), "{} cannot delete a box", p.id());

    p.whoami().await.unwrap();
    out.passed.push("whoami");

    let made = p.create_machine(CreateSpec { name: Some("conformance".into()), ..Default::default() }).await.unwrap();
    assert_eq!(made.provider, p.id());
    assert_eq!(made.name, "conformance");
    out.passed.push("create");

    assert_eq!(p.get_machine(&made.id).await.unwrap().id, made.id);
    out.passed.push("get");
    assert!(p.list_machines().await.unwrap().iter().any(|m| m.id == made.id));
    out.passed.push("list");

    for &(down, up, _) in POWER_PAIRS {
        for action in [Some(down), up].into_iter().flatten().filter(|a| caps.supports(*a)) {
            p.power(&made.id, action).await.unwrap();
            assert_eq!(Some(p.get_machine(&made.id).await.unwrap().status), after(action), "{action:?}");
        }
    }
    out.passed.push("power");

    let fork = p.fork_machine(&made.id, Some("conformance-fork".into())).await;
    if caps.fork {
        let fork = fork.unwrap();
        assert_ne!(fork.id, made.id);
        p.power(&fork.id, PowerAction::Destroy).await.unwrap();
        out.passed.push("fork");
    } else {
        assert!(fork.is_err(), "{} forked without claiming to", p.id());
    }

    if caps.https_links {
        let route = p.add_https_route(&made.id, "web", 3000).await.unwrap();
        assert_eq!(route.port, 3000);
        assert!(p.https_routes(&made.id).await.unwrap().iter().any(|r| r.port == 3000));
        out.passed.push("https routes");
    }

    p.power(&made.id, PowerAction::Destroy).await.unwrap();
    assert!(!p.list_machines().await.unwrap().iter().any(|m| m.id == made.id));
    out.passed.push("delete");
    out
}

struct Request {
    method: String,
    /// Without the query string.
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

type Boxes = Arc<Mutex<BTreeMap<String, Value>>>;

/// Serves `route` on a local port and returns its address.
async fn serve(route: impl Fn(Request, &Boxes) -> (u16, Value) + Send + Sync + 'static) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let route = Arc::new(route);
    let boxes: Boxes = Default::default();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (route, boxes) = (route.clone(), boxes.clone());
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else { return };
                let (status, body) = route(request, &boxes);
                let body = body.to_string();
                let head = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = stream.write_all((head + &body).as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_string(), first.next()?);
    let path = target.split('?').next()?.to_string();
    let headers: HashMap<String, String> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())).collect();
    let want: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = raw[end + 4..].to_vec();
    while body.len() < want {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Request { method, path, headers, body: serde_json::from_slice(&body).unwrap_or(Value::Null) })
}

fn next_id(boxes: &BTreeMap<String, Value>, prefix: &str) -> String {
    (1..).map(|n| format!("{prefix}{n}")).find(|id| !boxes.contains_key(id)).unwrap()
}

fn missing() -> (u16, Value) {
    (404, json!({ "code": "not_found", "message": "no such box" }))
}

fn boat_api(r: Request, boxes: &Boxes) -> (u16, Value) {
    let mut boxes = boxes.lock().unwrap();
    let parts: Vec<&str> = r.path.trim_matches('/').split('/').collect();
    let answer = |s: &Value| (200, json!({ "ok": true, "sandbox": s }));
    match (r.method.as_str(), parts.as_slice()) {
        ("GET", ["me"]) => (200, json!({ "ok": true, "user": { "id": "u_1", "login": "ana" } })),
        ("GET", ["limits"]) => (200, json!({ "ok": true })),
        ("GET", ["sandboxes"]) => (200, json!({ "ok": true, "sandboxes": boxes.values().collect::<Vec<_>>(), "pageInfo": { "hasMore": false } })),
        ("POST", ["sandboxes"]) => {
            let id = next_id(&boxes, "sbx_");
            let s = json!({ "id": id, "name": r.body["name"], "type": r.body["type"], "state": "running" });
            boxes.insert(id, s.clone());
            answer(&s)
        }
        ("POST", ["sandboxes", id, "fork"]) if boxes.contains_key(*id) => {
            let new = next_id(&boxes, "sbx_");
            let s = json!({ "id": new, "name": "", "type": "default", "state": "running" });
            boxes.insert(new, s.clone());
            answer(&s)
        }
        ("POST", ["sandboxes", id, "host"]) if boxes.contains_key(*id) => (200, json!({ "ok": true, "url": format!("https://3000-{id}.on.boat.dev") })),
        ("POST", ["sandboxes", id, action @ ("stop" | "resume")]) => match boxes.get_mut(*id) {
            Some(s) => {
                s["state"] = json!(if *action == "stop" { "archived" } else { "running" });
                (200, json!({ "ok": true }))
            }
            None => missing(),
        },
        ("GET", ["sandboxes", id]) => boxes.get(*id).map_or_else(missing, answer),
        ("PATCH", ["sandboxes", id]) => match boxes.get_mut(*id) {
            Some(s) => {
                s["name"] = r.body["name"].clone();
                answer(s)
            }
            None => missing(),
        },
        ("DELETE", ["sandboxes", id]) => {
            assert_eq!(r.headers.get("x-ascii-confirm-delete").map(String::as_str), Some(*id), "boat asks for the id in a header");
            boxes.remove(*id).map_or_else(missing, |_| (200, json!({ "ok": true })))
        }
        _ => missing(),
    }
}

fn daytona_api(r: Request, boxes: &Boxes) -> (u16, Value) {
    let mut boxes = boxes.lock().unwrap();
    let parts: Vec<&str> = r.path.trim_matches('/').split('/').collect();
    match (r.method.as_str(), parts.as_slice()) {
        ("GET", ["organizations"]) => (200, json!([])),
        ("GET", ["sandbox"]) => (200, json!(boxes.values().collect::<Vec<_>>())),
        ("POST", ["sandbox"]) => {
            let id = next_id(&boxes, "dt-");
            let s = json!({ "id": id, "name": r.body["name"], "state": "started", "cpu": 1, "memory": 1, "disk": 3 });
            boxes.insert(id, s.clone());
            (200, s)
        }
        ("GET", ["sandbox", id]) => boxes.get(*id).map_or_else(missing, |s| (200, s.clone())),
        ("GET", ["sandbox", id, "ports", port, "signed-preview-url"]) => (200, json!({ "url": format!("https://{port}-{id}.proxy.daytona.works") })),
        ("POST", ["sandbox", id, action @ ("start" | "stop")]) => match boxes.get_mut(*id) {
            Some(s) => {
                s["state"] = json!(if *action == "start" { "started" } else { "stopped" });
                (200, json!({}))
            }
            None => missing(),
        },
        ("DELETE", ["sandbox", id]) => boxes.remove(*id).map_or_else(missing, |_| (200, json!({}))),
        _ => missing(),
    }
}

fn agent37_api(r: Request, boxes: &Boxes) -> (u16, Value) {
    let mut boxes = boxes.lock().unwrap();
    let parts: Vec<&str> = r.path.trim_matches('/').split('/').collect();
    let instance = |id: &str, name: &Value| json!({ "id": id, "name": name, "status": "running", "resources": { "cpu": 2, "memory": 4, "disk": 4 } });
    match (r.method.as_str(), parts.as_slice()) {
        ("GET", ["billing"]) => (200, json!({ "balance_usd": 5.0, "instance_limit": 5 })),
        ("GET", ["instances"]) => (200, json!({ "data": boxes.values().collect::<Vec<_>>() })),
        ("POST", ["instances"]) => {
            let id = next_id(&boxes, "abc");
            let i = instance(&id, &r.body["name"]);
            boxes.insert(id, i.clone());
            (200, i)
        }
        // The bootstrap's install, then its exit code.
        ("POST", ["instances", _, "exec"]) => (200, json!({ "stdout": "0\n" })),
        ("POST", ["instances", id, "fork"]) if boxes.contains_key(*id) => {
            let new = next_id(&boxes, "abc");
            let i = instance(&new, &r.body["name"]);
            boxes.insert(new, i.clone());
            (200, i)
        }
        ("POST", ["instances", id, "signed-url"]) => (200, json!({ "url": format!("https://{id}-3000.agent37.app/?token=t") })),
        ("POST", ["instances", id, action @ ("start" | "stop" | "restart" | "sleep")]) => match boxes.get_mut(*id) {
            Some(i) => {
                i["status"] = json!(match *action {
                    "stop" => "stopped",
                    "sleep" => "sleeping",
                    _ => "running",
                });
                (200, json!({}))
            }
            None => missing(),
        },
        ("GET", ["instances", id]) => boxes.get(*id).map_or_else(missing, |i| (200, i.clone())),
        ("DELETE", ["instances", id]) => boxes.remove(*id).map_or_else(missing, |_| (200, json!({}))),
        _ => missing(),
    }
}

fn ssh_file(name: &str) -> std::path::PathBuf {
    crate::testing::temp_dir("stowbox-conformance").join(name)
}

#[tokio::test]
async fn boat_keeps_its_claims() {
    let mut config = BoatConfig::new(ssh_file("boat_ed25519"));
    config.api = serve(boat_api).await;
    let outcome = run(&Boat::new("boat_test".into(), config).unwrap()).await;
    assert!(outcome.passed.contains(&"fork"), "{outcome:?}");
}

#[tokio::test]
async fn daytona_keeps_its_claims() {
    let mut config = DaytonaConfig::new(ssh_file("daytona_known_hosts"));
    config.api = serve(daytona_api).await;
    let outcome = run(&Daytona::new(config, "dtn_test".into()).unwrap()).await;
    assert!(outcome.passed.contains(&"https routes") && !outcome.passed.contains(&"fork"), "{outcome:?}");
}

#[tokio::test]
async fn agent37_keeps_its_claims() {
    let mut config = Agent37Config::new(ssh_file("agent37_ed25519"));
    let base = serve(agent37_api).await;
    config.api = base.clone();
    config.sandbox_api = base;
    let outcome = run(&Agent37::new("sk_live_test".into(), config).unwrap()).await;
    assert!(outcome.passed.contains(&"fork"), "{outcome:?}");
}
