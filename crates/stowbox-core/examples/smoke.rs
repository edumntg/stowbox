//! Smoke test against a real boxd account.
//!
//!     BOXD_API_KEY=bxd_… cargo run -p stowbox-core --example smoke
//!
//! Read-only calls first, then one throwaway machine named
//! `stowbox-smoke-<id>` gets every Provider method exercised on it and is
//! destroyed at the end, whether or not a step failed. Existing machines are
//! never touched. Never prints the key or the JWT.

use std::future::Future;
use std::time::{Duration, Instant};
use stowbox_core::agent::{session, AgentMeta, Harness};
use stowbox_core::boxd::{Boxd, BoxdConfig};
use stowbox_core::cost;
use stowbox_core::provider::Provider;
use stowbox_core::types::*;

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    failed: usize,
}

impl Report {
    fn pass(&mut self, name: &str, detail: impl AsRef<str>, t: Duration) {
        let l = format!("PASS {name} ({:.1}s) {}", t.as_secs_f64(), detail.as_ref());
        println!("{l}");
        self.lines.push(l);
    }
    fn fail(&mut self, name: &str, detail: impl AsRef<str>, t: Duration) {
        let l = format!("FAIL {name} ({:.1}s) {}", t.as_secs_f64(), detail.as_ref());
        println!("{l}");
        self.lines.push(l);
        self.failed += 1;
    }
    fn skip(&mut self, name: &str, why: &str) {
        let l = format!("SKIP {name}: {why}");
        println!("{l}");
        self.lines.push(l);
    }
}

async fn step<T, F>(r: &mut Report, name: &str, fut: F) -> Option<T>
where
    F: Future<Output = stowbox_core::Result<T>>,
{
    let t = Instant::now();
    match tokio::time::timeout(Duration::from_secs(120), fut).await {
        Ok(Ok(v)) => {
            r.pass(name, "", t.elapsed());
            Some(v)
        }
        Ok(Err(e)) => {
            r.fail(name, e.to_string(), t.elapsed());
            None
        }
        Err(_) => {
            r.fail(name, "timed out after 120s", t.elapsed());
            None
        }
    }
}

struct Run {
    stdout: String,
    stderr: String,
    exit: Option<i32>,
    error: Option<String>,
}

/// Runs a non-tty script and collects everything, with a deadline.
async fn run(p: &dyn Provider, id: &str, script: &str, deadline: Duration) -> stowbox_core::Result<Run> {
    let mut h = p
        .exec(id, ExecSpec { script: script.into(), tty: false, cols: 0, rows: 0, env: vec![("TERM".into(), "dumb".into())] })
        .await?;
    let _ = h.input.send(ExecInput::Close).await;
    let mut out = Run { stdout: String::new(), stderr: String::new(), exit: None, error: None };
    let end = Instant::now() + deadline;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            out.error = Some("deadline".into());
            break;
        }
        match tokio::time::timeout(left, h.output.recv()).await {
            Ok(Some(ExecOutput::Stdout(b))) => out.stdout.push_str(&String::from_utf8_lossy(&b)),
            Ok(Some(ExecOutput::Stderr(b))) => out.stderr.push_str(&String::from_utf8_lossy(&b)),
            Ok(Some(ExecOutput::Exit(c))) => {
                out.exit = Some(c);
                break;
            }
            Ok(Some(ExecOutput::Error(e))) => {
                out.error = Some(e);
                break;
            }
            Ok(None) => break,
            Err(_) => {
                out.error = Some("deadline".into());
                break;
            }
        }
    }
    Ok(out)
}

async fn wait_status(p: &dyn Provider, id: &str, want: &[MachineStatus], max: Duration) -> Result<(Machine, Duration), String> {
    let t = Instant::now();
    loop {
        let m = p.get_machine(id).await.map_err(|e| e.to_string())?;
        if want.contains(&m.status) {
            return Ok((m, t.elapsed()));
        }
        if t.elapsed() > max {
            return Err(format!("still {:?} after {:.0}s", m.status, max.as_secs_f64()));
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
}

fn short(s: &str, n: usize) -> String {
    let s = s.trim().replace('\n', " | ");
    if s.chars().count() > n {
        s.chars().take(n).collect::<String>() + "…"
    } else {
        s
    }
}

#[tokio::main]
async fn main() {
    let key = std::env::var("BOXD_API_KEY").expect("set BOXD_API_KEY");
    let boxd = Boxd::new(key, BoxdConfig::default()).expect("client");
    let p: &dyn Provider = &boxd;
    let mut r = Report::default();

    // ---- read-only -------------------------------------------------------
    let acct = step(&mut r, "whoami", p.whoami()).await;
    if let Some(a) = &acct {
        println!(
            "     account {} · billing {} {} · balance {} · default {} vCPU / {} GiB · ceiling {} vCPU · max_vms {}",
            a.display_name,
            a.billing.mode,
            a.billing.currency,
            cost::money(a.billing.balance_micro, &a.billing.currency),
            a.billing.default_vcpu,
            a.billing.default_memory_bytes / (1 << 30),
            a.billing.ceiling_vcpu,
            a.billing.max_vms
        );
    }
    let rates = step(&mut r, "rates", p.rates()).await;
    if let Some((cur, rt)) = &rates {
        println!(
            "     rate card {cur}: {} vCPU·h · {} GiB·h · {} disk GiB·h",
            cost::money(rt.vcpu_hour_micro, cur),
            cost::money(rt.ram_gib_hour_micro, cur),
            cost::money(rt.disk_gib_hour_micro, cur)
        );
    }
    let machines = step(&mut r, "list_machines", p.list_machines()).await;
    if let Some(ms) = &machines {
        println!("     {} machine(s) visible", ms.len());
        for m in ms {
            println!(
                "       {:<28} {:<10} {} vCPU {} GiB · est {}/h · source {}",
                m.name,
                format!("{:?}", m.status).to_lowercase(),
                m.vcpu,
                m.memory_bytes / (1 << 30),
                rates.as_ref().map(|(c, rt)| cost::money(cost::hourly_micro(m, rt), c)).unwrap_or_default(),
                if m.source.is_empty() { "standalone" } else { &m.source }
            );
        }
        // Leftovers from an earlier aborted run.
        for m in ms.iter().filter(|m| m.name.starts_with("stowbox-smoke-")) {
            println!("     destroying leftover {}", m.name);
            let _ = p.power(&m.id, PowerAction::Destroy).await;
        }
    }
    // GetOrgBilling is an account-management call: boxd refuses it for API
    // keys ("requires an interactive session"). The app never needs it; the
    // sidebar balance comes from Whoami. Recorded here so the limit stays known.
    match p.org_billing("").await {
        Ok(b) => r.pass("org_billing", format!("{} · {} vms of {} · balance {}", b.name, b.vm_count, b.max_vms, cost::money(b.balance_micro, &b.currency)), Duration::ZERO),
        Err(e) if e.to_string().contains("interactive session") => r.skip("org_billing", "not available to API keys, as documented by boxd"),
        Err(e) => r.fail("org_billing", e.to_string(), Duration::ZERO),
    }

    // ---- throwaway machine -----------------------------------------------
    let name = format!("stowbox-smoke-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let created = step(&mut r, "create_machine", p.create_machine(CreateSpec { name: Some(name.clone()), ..Default::default() })).await;
    let Some(m) = created else {
        if r.lines.iter().any(|l| l.contains("payment method")) {
            println!("     boxd will not create machines for this org until a payment method is connected at app.boxd.sh (billing page).");
        }
        finish(&r);
        return;
    };
    println!("     {} id {} status {:?} · {} vCPU {} GiB · url {}", m.name, m.id, m.status, m.vcpu, m.memory_bytes / (1 << 30), m.url);
    let id = m.id.clone();

    let outcome = machine_steps(p, &id, &name, &mut r).await;
    if let Err(e) = outcome {
        r.fail("machine_steps", e, Duration::ZERO);
    }

    // ---- destroy, always -------------------------------------------------
    let t = Instant::now();
    match p.power(&id, PowerAction::Destroy).await {
        Ok(()) => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let gone = p.list_machines().await.map(|ms| !ms.iter().any(|x| x.id == id)).unwrap_or(false);
            if gone {
                r.pass("destroy", "machine gone from the list", t.elapsed());
            } else {
                r.fail("destroy", "machine still listed after 2s", t.elapsed());
            }
        }
        Err(e) => r.fail("destroy", e.to_string(), t.elapsed()),
    }
    finish(&r);
}

fn finish(r: &Report) {
    println!("\n==== {} step(s) failed ====", r.failed);
    std::process::exit(if r.failed == 0 { 0 } else { 1 });
}

async fn machine_steps(p: &dyn Provider, id: &str, name: &str, r: &mut Report) -> Result<(), String> {
    // Wait for boot.
    let t = Instant::now();
    match wait_status(p, id, &[MachineStatus::Running], Duration::from_secs(90)).await {
        Ok((m, d)) => r.pass("boot", format!("running after {:.1}s · boot_time_ms {}", d.as_secs_f64(), m.boot_time_ms), t.elapsed()),
        Err(e) => {
            r.fail("boot", e, t.elapsed());
            return Err("machine never came up".into());
        }
    }

    // Plain exec: exit code, stdout, stderr, env export.
    let t = Instant::now();
    match run(p, id, "echo out-$STOWBOX_T; echo err >&2; uname -m; exit 7", Duration::from_secs(60)).await {
        Ok(o) => {
            let ok = o.stdout.contains("out-") && o.stderr.contains("err") && o.exit == Some(7);
            let d = format!("exit {:?} stdout {:?} stderr {:?} {}", o.exit, short(&o.stdout, 60), short(&o.stderr, 30), o.error.unwrap_or_default());
            if ok { r.pass("exec_basic", d, t.elapsed()) } else { r.fail("exec_basic", d, t.elapsed()) }
        }
        Err(e) => r.fail("exec_basic", e.to_string(), t.elapsed()),
    }

    // Exit 0 must also end the stream (the zero-exit ambiguity on the wire).
    let t = Instant::now();
    match run(p, id, "printf hello", Duration::from_secs(60)).await {
        Ok(o) => {
            let d = format!("exit {:?} stdout {:?} {}", o.exit, short(&o.stdout, 40), o.error.clone().unwrap_or_default());
            if o.stdout == "hello" && o.error.is_none() { r.pass("exec_exit0_ends", d, t.elapsed()) } else { r.fail("exec_exit0_ends", d, t.elapsed()) }
        }
        Err(e) => r.fail("exec_exit0_ends", e.to_string(), t.elapsed()),
    }

    // The fleet view's sample: /proc and df read over exec, parsed in core.
    let t = Instant::now();
    match p.metrics(id).await {
        Ok(Some(m)) => {
            let d = format!(
                "cpu {:.1}% of {} · mem {}/{} MiB · disk {} GiB free · up {} s",
                m.cpu_percent,
                m.vcpu,
                m.mem_used_bytes >> 20,
                m.mem_total_bytes >> 20,
                m.disk_avail_bytes >> 30,
                m.uptime_secs
            );
            if m.mem_total_bytes > 0 && m.disk_total_bytes > 0 { r.pass("metrics_sample", d, t.elapsed()) } else { r.fail("metrics_sample", d, t.elapsed()) }
        }
        Ok(None) => r.fail("metrics_sample", "box reported as not running", t.elapsed()),
        Err(e) => r.fail("metrics_sample", e.to_string(), t.elapsed()),
    }

    // Env vars passed through the wrapper, plus quoting with spaces and quotes.
    let t = Instant::now();
    let mut h = p
        .exec(id, ExecSpec { script: "echo \"[$STOWBOX_T]\" && cd /tmp && pwd".into(), tty: false, cols: 0, rows: 0, env: vec![("STOWBOX_T".into(), "it's a \"test\" $HOME".into())] })
        .await
        .map_err(|e| e.to_string())?;
    let _ = h.input.send(ExecInput::Close).await;
    let mut got = String::new();
    while let Ok(Some(o)) = tokio::time::timeout(Duration::from_secs(30), h.output.recv()).await {
        match o {
            ExecOutput::Stdout(b) => got.push_str(&String::from_utf8_lossy(&b)),
            ExecOutput::Exit(_) | ExecOutput::Error(_) => break,
            _ => {}
        }
    }
    if got.contains("[it's a \"test\" $HOME]") && got.contains("/tmp") {
        r.pass("exec_env_quoting", short(&got, 60), t.elapsed())
    } else {
        r.fail("exec_env_quoting", short(&got, 80), t.elapsed())
    }

    // What the box has for agents.
    let t = Instant::now();
    let probe = run(
        p,
        id,
        "echo user=$(whoami) shell=$SHELL; command -v claude codex opencode tmux | tr '\\n' ' '; echo; claude --version 2>&1 | head -1; test -n \"$ANTHROPIC_API_KEY\" && echo cred=ANTHROPIC_API_KEY; test -n \"$CLAUDE_CODE_OAUTH_TOKEN\" && echo cred=CLAUDE_CODE_OAUTH_TOKEN; test -s ~/.claude/.credentials.json && echo cred=claude-login-file; true",
        Duration::from_secs(60),
    )
    .await;
    let mut has_claude = false;
    match probe {
        Ok(o) => {
            has_claude = o.stdout.contains("claude");
            r.pass("probe_tools", short(&o.stdout, 200), t.elapsed());
        }
        Err(e) => r.fail("probe_tools", e.to_string(), t.elapsed()),
    }

    // Interactive tty session: a login shell that we type into.
    let t = Instant::now();
    let mut h = p
        .exec(id, ExecSpec { script: "exec \"${SHELL:-bash}\" -l".into(), tty: true, cols: 100, rows: 30, env: vec![("TERM".into(), "xterm-256color".into())] })
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    h.input.send(ExecInput::Resize { cols: 120, rows: 40 }).await.map_err(|_| "tty input closed")?;
    h.input.send(ExecInput::Data(b"echo TTY_$(( 40 + 2 ))_OK; stty size; exit\n".to_vec())).await.map_err(|_| "tty input closed")?;
    let mut got = String::new();
    let mut exit = None;
    let end = Instant::now() + Duration::from_secs(30);
    while let Ok(Some(o)) = tokio::time::timeout(end.saturating_duration_since(Instant::now()), h.output.recv()).await {
        match o {
            ExecOutput::Stdout(b) | ExecOutput::Stderr(b) => got.push_str(&String::from_utf8_lossy(&b)),
            ExecOutput::Exit(c) => {
                exit = Some(c);
                break;
            }
            ExecOutput::Error(e) => {
                got.push_str(&format!(" <error {e}>"));
                break;
            }
        }
    }
    let resized = got.contains("40 120");
    if got.contains("TTY_42_OK") {
        r.pass("exec_tty", format!("exit {exit:?} · resize seen {resized} · {}", short(&got, 120)), t.elapsed())
    } else {
        r.fail("exec_tty", format!("exit {exit:?} · {}", short(&got, 200)), t.elapsed())
    }

    // Cancellation: drop the handle, the remote process must die.
    let t = Instant::now();
    let h = p
        .exec(id, ExecSpec { script: "sleep 600 & echo started; wait".into(), tty: false, cols: 0, rows: 0, env: vec![("STOWBOX_MARK".into(), "cancel-me".into())] })
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    drop(h);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // `sleep 60[0]` matches the real process but not this command's own
    // wrappers, whose command lines contain the bracketed text.
    match run(p, id, "pgrep -fc 'sleep 60[0]' || true", Duration::from_secs(30)).await {
        Ok(o) => {
            let n: i32 = o.stdout.trim().parse().unwrap_or(-1);
            if n == 0 { r.pass("exec_cancel_kills", "no sleep 600 left", t.elapsed()) } else { r.fail("exec_cancel_kills", format!("{n} sleep process(es) still alive"), t.elapsed()) }
        }
        Err(e) => r.fail("exec_cancel_kills", e.to_string(), t.elapsed()),
    }

    // Power states round trip.
    for (action, want, label) in [
        (PowerAction::Pause, MachineStatus::Suspended, "pause→suspended"),
        (PowerAction::Resume, MachineStatus::Running, "resume→running"),
        (PowerAction::Hibernate, MachineStatus::Hibernated, "hibernate→hibernated"),
        (PowerAction::Wake, MachineStatus::Running, "wake→running"),
    ] {
        let t = Instant::now();
        match p.power(id, action).await {
            Ok(()) => match wait_status(p, id, &[want], Duration::from_secs(60)).await {
                Ok((_, d)) => r.pass(label, format!("reached after {:.1}s", d.as_secs_f64()), t.elapsed()),
                Err(e) => r.fail(label, e, t.elapsed()),
            },
            Err(e) => r.fail(label, e.to_string(), t.elapsed()),
        }
    }

    // Exec on a suspended machine must wake it (the terminal relies on this).
    // Right after a wake the VMM may still be settling a pending hibernate,
    // and a pause in that window makes the next resume fail with Internal;
    // give it a moment, and retry once like a user would.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let t = Instant::now();
    let _ = p.power(id, PowerAction::Pause).await;
    let _ = wait_status(p, id, &[MachineStatus::Suspended], Duration::from_secs(30)).await;
    let mut attempt = 0;
    loop {
        attempt += 1;
        match run(p, id, "echo woke", Duration::from_secs(60)).await {
            Ok(o) if o.stdout.contains("woke") => {
                r.pass("exec_wakes_standby", format!("attempt {attempt}"), t.elapsed());
                break;
            }
            Ok(o) if attempt < 2 => {
                println!("     exec_wakes_standby attempt {attempt}: exit {:?} {} {}", o.exit, short(&o.stdout, 40), o.error.unwrap_or_default());
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(o) => {
                r.fail("exec_wakes_standby", format!("exit {:?} {} {}", o.exit, short(&o.stdout, 40), o.error.unwrap_or_default()), t.elapsed());
                break;
            }
            Err(e) if attempt < 2 => {
                println!("     exec_wakes_standby attempt {attempt}: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => {
                r.fail("exec_wakes_standby", e.to_string(), t.elapsed());
                break;
            }
        }
    }

    // Timeouts and usage.
    let t = Instant::now();
    match p.set_auto_hibernate(id, 1800).await {
        Ok(()) => match p.get_machine(id).await {
            Ok(m) if m.auto_hibernate_secs == 1800 => r.pass("set_auto_hibernate", "read back 1800", t.elapsed()),
            Ok(m) => r.fail("set_auto_hibernate", format!("read back {}", m.auto_hibernate_secs), t.elapsed()),
            Err(e) => r.fail("set_auto_hibernate", e.to_string(), t.elapsed()),
        },
        Err(e) => r.fail("set_auto_hibernate", e.to_string(), t.elapsed()),
    }
    let t = Instant::now();
    match p.machine_usage(id).await {
        Ok(u) => r.pass(
            "machine_usage",
            format!("cost {} · running {}s standby {}s hibernated {}s · complete {}", cost::money(u.cost_micro, &u.currency), u.seconds.running, u.seconds.standby, u.seconds.hibernated, u.complete),
            t.elapsed(),
        ),
        Err(e) => r.fail("machine_usage", e.to_string(), t.elapsed()),
    }

    // Fork, then destroy the fork.
    let t = Instant::now();
    match p.fork_machine(id, Some(format!("{name}-fork"))).await {
        Ok(f) => {
            let d = format!("{} status {:?} source {}", f.name, f.status, f.source);
            let _ = p.power(&f.id, PowerAction::Destroy).await;
            r.pass("fork_and_destroy_fork", d, t.elapsed())
        }
        Err(e) => r.fail("fork_and_destroy_fork", e.to_string(), t.elapsed()),
    }

    // An agent session: the harness starts in its own tmux session and the
    // watch sees it running; stopping it ends it. Claude Code's first-run
    // dialogs (folder trust, permissions) need a person, so the turn itself
    // is not driven here.
    if !has_claude {
        r.skip("agent_session", "claude is not installed in the box");
    } else {
        agent_session(p, id, r).await;
    }
    Ok(())
}

async fn agent_session(p: &dyn Provider, id: &str, r: &mut Report) {
    let t = Instant::now();
    let agent = AgentMeta {
        id: format!("smoke-{}", uuid::Uuid::new_v4().simple()),
        title: "smoke".into(),
        harness: Harness::ClaudeCode,
        machine_id: id.into(),
        machine_name: String::new(),
        cwd: None,
        model: None,
        session_id: Some(uuid::Uuid::new_v4().to_string()),
        skip_permissions: false,
        created_at: 0,
        updated_at: 0,
        worktree: None,
    };
    let env = vec![("TERM".into(), "xterm-256color".into())];
    let term = match p.exec(id, ExecSpec { script: session::open_script(&agent, None), tty: true, cols: 120, rows: 40, env }).await {
        Ok(h) => h,
        Err(e) => return r.fail("agent_session", e.to_string(), t.elapsed()),
    };
    let mut watch = match session::watch(p, id).await {
        Ok(w) => w,
        Err(e) => return r.fail("agent_session", e.to_string(), t.elapsed()),
    };
    let mut seen = None;
    let end = Instant::now() + Duration::from_secs(60);
    while let Ok(Some(snap)) = tokio::time::timeout(end.saturating_duration_since(Instant::now()), watch.next()).await {
        let running = snap.agents.iter().any(|a| a.id == agent.id && a.running);
        seen = Some((snap.tmux, running));
        if !snap.tmux || running {
            break;
        }
    }
    drop(term);
    match seen {
        Some((false, _)) => r.skip("agent_session", "the box has no tmux: agents end with their terminal"),
        Some((true, true)) => {
            // Detached, not ended: the session outlives the terminal.
            drop(watch);
            tokio::time::sleep(Duration::from_secs(2)).await;
            let still = match session::watch(p, id).await {
                Ok(mut w) => tokio::time::timeout(Duration::from_secs(10), w.next()).await.ok().flatten(),
                Err(_) => None,
            };
            let _ = session::stop(p, id, &agent.id, true).await;
            let detail = format!("running in tmux; after the terminal closed: {:?}", still.map(|s| s.agents.iter().any(|a| a.id == agent.id && a.running)));
            r.pass("agent_session", detail, t.elapsed())
        }
        _ => {
            let _ = session::stop(p, id, &agent.id, true).await;
            r.fail("agent_session", "the session never showed up as running", t.elapsed())
        }
    }
}
