//! Conformance against the real services. Each test makes and deletes real
//! boxes, so it is ignored by default and does nothing unless
//! `STOWBOX_LIVE_<ID>_KEY` is set. Never run it in CI:
//! `cargo test -p stowbox-core testing::live -- --ignored --nocapture`.
//! The box is the provider's smallest size unless `STOWBOX_LIVE_<ID>_SIZE`
//! names another from its menu.
//!
//! Unlike the fakes, a step that fails does not stop the run: every box made
//! is deleted before the test reports, so a failure costs nothing more.

use crate::provider::Provider;
use crate::providers::{ConnectContext, Credentials, API_KEY};
use crate::types::*;
use std::future::Future;
use std::time::{Duration, Instant};

/// Long enough for a cold create or a restore from disk.
const SETTLE: Duration = Duration::from_secs(600);

#[derive(Debug, Default)]
struct Report {
    passed: Vec<&'static str>,
    failed: Vec<(&'static str, String)>,
}

impl Report {
    fn check(&mut self, step: &'static str, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => {
                eprintln!("  {step}: ok");
                self.passed.push(step);
                true
            }
            Err(e) => {
                eprintln!("  {step}: {e}");
                self.failed.push((step, e));
                false
            }
        }
    }
}

async fn timed<T>(what: &str, f: impl Future<Output = crate::Result<T>>) -> Result<T, String> {
    match tokio::time::timeout(SETTLE, f).await {
        Ok(r) => r.map_err(|e| format!("{what}: {e}")),
        Err(_) => Err(format!("{what}: no answer in {}s", SETTLE.as_secs())),
    }
}

/// Asks `done` every few seconds until it says yes or `within` runs out.
async fn poll_until<F: Future<Output = Result<bool, String>>>(within: Duration, mut done: impl FnMut() -> F) -> Result<bool, String> {
    let start = Instant::now();
    while start.elapsed() < within {
        if done().await? {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Ok(false)
}

/// Polls until the box reports `want`, since a power call may return before
/// the box gets there.
async fn reach(p: &dyn Provider, id: &str, want: MachineStatus) -> Result<(), String> {
    let last = std::cell::Cell::new(MachineStatus::Unknown);
    let reached = poll_until(SETTLE, || async {
        last.set(timed("get", p.get_machine(id)).await?.status);
        Ok(last.get() == want)
    })
    .await?;
    if reached {
        Ok(())
    } else {
        Err(format!("still {:?} after {}s, wanted {want:?}", last.get(), SETTLE.as_secs()))
    }
}

/// Whether the provider lists `id`.
async fn listed(p: &dyn Provider, id: &str) -> Result<bool, String> {
    Ok(timed("list", p.list_machines()).await?.iter().any(|m| m.id == id))
}

/// Runs `spec`, sends `stdin`, and returns stdout and stderr together with
/// the exit code.
async fn run(p: &dyn Provider, id: &str, spec: ExecSpec, stdin: &[u8]) -> Result<(String, Option<i32>), String> {
    // A terminal never closes its input, and a closed one can hang up the TTY
    // before the command has written anything.
    let close = !spec.tty;
    let mut h = timed("exec", p.exec(id, spec)).await?;
    if !stdin.is_empty() {
        h.input.send(ExecInput::Data(stdin.to_vec())).await.map_err(|e| e.to_string())?;
    }
    if close {
        h.input.send(ExecInput::Close).await.map_err(|e| e.to_string())?;
    }
    let collect = async {
        let (mut out, mut code) = (Vec::new(), None);
        while let Some(o) = h.output.recv().await {
            match o {
                ExecOutput::Stdout(b) | ExecOutput::Stderr(b) => out.extend(b),
                ExecOutput::Exit(c) => code = Some(c),
                ExecOutput::Error(e) => return Err(e),
            }
        }
        Ok((String::from_utf8_lossy(&out).into_owned(), code))
    };
    tokio::time::timeout(Duration::from_secs(120), collect).await.map_err(|_| "exec did not finish in 120s".to_string())?
}

async fn exec_claims(p: &dyn Provider, id: &str, r: &mut Report) {
    let plain = run(p, id, ExecSpec { script: "printf 'in:'; cat; exit 3".into(), ..Default::default() }, b"through\n").await;
    r.check("exec with stdin and exit code", plain.and_then(|got| if got == ("in:through\n".into(), Some(3)) { Ok(()) } else { Err(format!("got {got:?}")) }));

    let tty = run(p, id, ExecSpec { script: "tty; stty size".into(), tty: true, cols: 100, rows: 30, ..Default::default() }, b"").await;
    r.check("exec with a TTY", tty.and_then(|(out, _)| if out.contains("/dev/pts/") && out.contains("30 100") { Ok(()) } else { Err(format!("got {out:?}")) }));

    // Larger than one WebSocket frame or gRPC message chunk.
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let upload = async {
        let n = timed("upload", p.upload_file(id, "/tmp/stowbox-live.bin", data.clone())).await?;
        if n != data.len() as u64 {
            return Err(format!("upload reported {n} bytes"));
        }
        let (out, _) = run(p, id, ExecSpec { script: "sha1sum < /tmp/stowbox-live.bin".into(), ..Default::default() }, b"").await?;
        use sha1::Digest;
        let want = format!("{:x}", sha1::Sha1::digest(&data));
        if out.starts_with(&want) {
            Ok(())
        } else {
            Err(format!("sha1 {out:?}, wanted {want}"))
        }
    };
    r.check("upload", upload.await);
}

async fn power_claims(p: &dyn Provider, id: &str, r: &mut Report) {
    let caps = p.capabilities();
    for &(down, up, step) in super::conformance::POWER_PAIRS {
        if !caps.supports(down) {
            continue;
        }
        let res = async {
            for action in [Some(down), up].into_iter().flatten() {
                timed(&format!("{action:?}"), p.power(id, action)).await?;
                if let Some(want) = super::conformance::after(action) {
                    reach(p, id, want).await.map_err(|e| format!("{action:?}: {e}"))?;
                }
            }
            Ok(())
        };
        if !r.check(step, res.await) {
            // Later steps need a running box; try once to get it back.
            let _ = reach(p, id, MachineStatus::Running).await;
        }
    }
}

/// Every claim the provider makes, on a box made for the run.
async fn conform(p: &dyn Provider) -> Report {
    let caps = p.capabilities();
    let mut r = Report::default();
    let name = format!("stowbox-live-{}", crate::upload::new_tag());
    let mut made = Vec::new();

    r.check("whoami", timed("whoami", p.whoami()).await.map(|_| ()));

    // The size New box starts on, or the one `STOWBOX_LIVE_<ID>_SIZE` names;
    // for free sizes, the smallest.
    let menu = std::env::var(format!("STOWBOX_LIVE_{}_SIZE", p.id().to_uppercase())).ok().or(Some(caps.default_size.clone())).filter(|s| !s.is_empty());
    let free = caps.free_sizes.first();
    let spec = CreateSpec {
        name: Some(name.clone()),
        size: menu,
        vcpu: free.map(|s| s.vcpu),
        memory_bytes: free.map(|s| u64::from(s.memory_gib) << 30),
        // A safety net in case the delete below never runs.
        auto_stop_secs: (caps.auto_stop || caps.idle_stop).then_some(1800),
        ..Default::default()
    };
    let created = timed("create", p.create_machine(spec)).await;
    let id = match created {
        Ok(m) => {
            made.push(m.id.clone());
            let named = if m.name == name { Ok(()) } else { Err(format!("named {:?}", m.name)) };
            r.check("create", named);
            m.id
        }
        Err(e) => {
            r.check("create", Err(e));
            return r;
        }
    };

    if r.check("running after create", reach(p, &id, MachineStatus::Running).await) {
        // Daytona's list lags a create by a few seconds.
        let shown = poll_until(Duration::from_secs(60), || listed(p, &id)).await;
        r.check("list", shown.and_then(|yes| if yes { Ok(()) } else { Err("not listed after 60s".into()) }));
        exec_claims(p, &id, &mut r).await;
        power_claims(p, &id, &mut r).await;

        if caps.fork {
            match timed("fork", p.fork_machine(&id, Some(format!("{name}-fork")))).await {
                Ok(f) => {
                    made.push(f.id.clone());
                    r.check("fork", if f.id == id { Err("fork kept the source's id".into()) } else { Ok(()) });
                }
                Err(e) => {
                    r.check("fork", Err(e));
                }
            }
        }
        if caps.https_links {
            let route = async {
                let route = timed("add route", p.add_https_route(&id, "web", 3000)).await?;
                let listed = timed("routes", p.https_routes(&id)).await?;
                if route.port == 3000 && listed.iter().any(|x| x.port == 3000) {
                    Ok(())
                } else {
                    Err(format!("route {route:?}, listed {listed:?}"))
                }
            };
            r.check("https routes", route.await);
        }
    }

    for id in &made {
        let gone = async {
            timed("delete", p.power(id, PowerAction::Destroy)).await?;
            if poll_until(SETTLE, || async { Ok(!listed(p, id).await?) }).await? {
                Ok(())
            } else {
                Err(format!("{id} still listed after delete"))
            }
        };
        r.check("delete", gone.await);
    }
    r
}

async fn live(id: &str) {
    // `STOWBOX_LIVE_<ID>_KEY` for the API key, `STOWBOX_LIVE_<ID>_<FIELD>`
    // for any other credential field.
    let kind = crate::providers::find(id).unwrap();
    let mut values = Vec::new();
    for field in (kind.capabilities)().credential_fields {
        let suffix = if field.name == API_KEY { "KEY".to_string() } else { field.name.to_uppercase() };
        let var = format!("STOWBOX_LIVE_{}_{suffix}", id.to_uppercase());
        let Ok(value) = std::env::var(&var) else {
            eprintln!("{var} is not set; skipping {id}");
            return;
        };
        values.push((field.name, value));
    }
    let dir = crate::testing::temp_dir(&format!("stowbox-live-{id}"));
    let p = (kind.connect)(Credentials::new(values), &ConnectContext::new(&dir)).unwrap();
    eprintln!("{id}:");
    let report = conform(p.as_ref()).await;
    assert!(report.failed.is_empty(), "{id}: {report:#?}");
}

#[tokio::test]
#[ignore = "makes and deletes real boxd boxes"]
async fn boxd_live() {
    live("boxd").await;
}

#[tokio::test]
#[ignore = "makes and deletes real boat sandboxes"]
async fn boat_live() {
    live("boat").await;
}

#[tokio::test]
#[ignore = "makes and deletes real Daytona sandboxes"]
async fn daytona_live() {
    live("daytona").await;
}

#[tokio::test]
#[ignore = "makes and deletes real Agent37 instances"]
async fn agent37_live() {
    live("agent37").await;
}

#[tokio::test]
#[ignore = "makes and deletes real E2B sandboxes"]
async fn e2b_live() {
    live("e2b").await;
}
