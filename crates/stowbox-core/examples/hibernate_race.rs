//! Reproduces: hibernate → wake → pause soon after → the next resume fails.
//! Measures how long after `wake` a `pause` is safe, what the machine
//! reports after the failure, and whether in-memory state survives.
//! Creates a throwaway box and destroys it at the end.

use std::time::{Duration, Instant};
use stowbox_core::boxd::{Boxd, BoxdConfig};
use stowbox_core::provider::Provider;
use stowbox_core::types::*;

async fn run(p: &dyn Provider, id: &str, script: &str) -> Result<(String, Option<i32>), String> {
    let mut h = p
        .exec(id, ExecSpec { script: script.into(), tty: false, cols: 0, rows: 0, env: vec![] })
        .await
        .map_err(|e| e.to_string())?;
    let _ = h.input.send(ExecInput::Close).await;
    let mut out = String::new();
    let mut exit = None;
    while let Ok(Some(o)) = tokio::time::timeout(Duration::from_secs(30), h.output.recv()).await {
        match o {
            ExecOutput::Stdout(b) | ExecOutput::Stderr(b) => out.push_str(&String::from_utf8_lossy(&b)),
            ExecOutput::Exit(c) => {
                exit = Some(c);
                break;
            }
            ExecOutput::Error(e) => return Err(e),
        }
    }
    Ok((out, exit))
}

async fn wait_status(p: &dyn Provider, id: &str, want: MachineStatus, max: Duration) -> Result<Duration, String> {
    let t = Instant::now();
    loop {
        let m = p.get_machine(id).await.map_err(|e| e.to_string())?;
        if m.status == want {
            return Ok(t.elapsed());
        }
        if t.elapsed() > max {
            return Err(format!("still {:?} after {:?}", m.status, max));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn status(p: &dyn Provider, id: &str) -> String {
    p.get_machine(id).await.map(|m| format!("{:?}", m.status).to_lowercase()).unwrap_or_else(|e| format!("error: {e}"))
}

#[tokio::main]
async fn main() {
    let key = std::env::var("BOXD_API_KEY").expect("BOXD_API_KEY");
    let boxd = Boxd::new(key, BoxdConfig::default()).unwrap();
    let p: &dyn Provider = &boxd;
    let m = p.create_machine(CreateSpec { name: Some("stowbox-smoke-race".into()), ..Default::default() }).await.unwrap();
    let id = m.id.clone();
    println!("box {} ({}) {:?}", m.name, m.id, m.status);

    // Something to lose: a background process that only survives if memory
    // state is kept across the pause/resume.
    let _ = run(p, &id, "nohup sleep 3600 >/dev/null 2>&1 & echo marker-started").await;

    println!("\n| delay after wake | pause | status after pause | exec (resume) | status after | retry 2s later | sleep survived |");
    println!("|---|---|---|---|---|---|---|");
    for delay_ms in [0u64, 250, 500, 1000, 2000, 3000, 5000, 8000] {
        // Fresh cycle: hibernate, wake.
        if let Err(e) = p.power(&id, PowerAction::Hibernate).await {
            println!("| {delay_ms} ms | hibernate failed: {e} |");
            continue;
        }
        let hib = wait_status(p, &id, MachineStatus::Hibernated, Duration::from_secs(60)).await;
        if let Err(e) = hib {
            println!("| {delay_ms} ms | never hibernated: {e} |");
            continue;
        }
        let t_wake = Instant::now();
        if let Err(e) = p.power(&id, PowerAction::Wake).await {
            println!("| {delay_ms} ms | wake failed: {e} |");
            continue;
        }
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(60)).await;
        let running_after = t_wake.elapsed();
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        let pause = match p.power(&id, PowerAction::Pause).await {
            Ok(()) => "ok".to_string(),
            Err(e) => format!("error: {e}"),
        };
        let st_pause = status(p, &id).await;
        let exec1 = match run(p, &id, "echo resumed").await {
            Ok((o, c)) => format!("ok ({}, exit {:?})", o.trim(), c),
            Err(e) => format!("ERROR: {}", e.replace('|', "¦")),
        };
        let st_after = status(p, &id).await;
        let retry = if exec1.starts_with("ERROR") {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match run(p, &id, "echo resumed").await {
                Ok((o, c)) => format!("ok ({}, exit {:?})", o.trim(), c),
                Err(e) => format!("ERROR: {}", e.replace('|', "¦")),
            }
        } else {
            "n/a".to_string()
        };
        let survived = match run(p, &id, "pgrep -fc 'sleep 360[0]' || true").await {
            Ok((o, _)) => if o.trim() == "1" { "yes".to_string() } else { format!("no ({})", o.trim()) },
            Err(e) => format!("? {e}"),
        };
        println!(
            "| {delay_ms} ms (running reported {:.0} ms after wake) | {pause} | {st_pause} | {exec1} | {st_after} | {retry} | {survived} |",
            running_after.as_millis()
        );
        // Make sure the marker exists for the next cycle.
        let _ = run(p, &id, "pgrep -f 'sleep 360[0]' >/dev/null || (nohup sleep 3600 >/dev/null 2>&1 &)").await;
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(30)).await;
    }

    // Control: the same pause → exec cycle without a preceding hibernate.
    let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(30)).await;
    let pause = p.power(&id, PowerAction::Pause).await.map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string());
    let st = status(p, &id).await;
    let exec = run(p, &id, "echo resumed").await.map(|(o, _)| o.trim().to_string()).unwrap_or_else(|e| format!("ERROR {e}"));
    println!("\ncontrol (pause with no recent wake): pause {pause}, status {st}, exec {exec}");

    // Explicit Resume RPC right after wake, instead of an exec.
    let _ = p.power(&id, PowerAction::Hibernate).await;
    let _ = wait_status(p, &id, MachineStatus::Hibernated, Duration::from_secs(60)).await;
    let _ = p.power(&id, PowerAction::Wake).await;
    let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(60)).await;
    let pause = p.power(&id, PowerAction::Pause).await.map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string());
    let st = status(p, &id).await;
    let resume = p.power(&id, PowerAction::Resume).await.map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string());
    let st2 = status(p, &id).await;
    println!("explicit ResumeVm right after wake+pause: pause {pause}, status {st}, resume {resume}, status after {st2}");

    p.power(&id, PowerAction::Destroy).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = p.list_machines().await.unwrap().iter().filter(|x| x.name.starts_with("stowbox-smoke")).count();
    println!("\ndestroyed; smoke machines left: {left}");
}
