//! What does boxd kill when the client drops an Exec stream? Creates a
//! throwaway box, runs a few shapes of long-lived commands, drops them, and
//! lists what survived. Destroys the box at the end.

use std::time::Duration;
use stowbox_core::boxd::{Boxd, BoxdConfig};
use stowbox_core::provider::Provider;
use stowbox_core::types::*;

async fn run(p: &dyn Provider, id: &str, script: &str) -> String {
    let mut h = p.exec(id, ExecSpec { script: script.into(), tty: false, cols: 0, rows: 0, env: vec![] }).await.unwrap();
    let _ = h.input.send(ExecInput::Close).await;
    let mut out = String::new();
    while let Ok(Some(o)) = tokio::time::timeout(Duration::from_secs(20), h.output.recv()).await {
        match o {
            ExecOutput::Stdout(b) | ExecOutput::Stderr(b) => out.push_str(&String::from_utf8_lossy(&b)),
            _ => break,
        }
    }
    out
}

#[tokio::main]
async fn main() {
    let key = std::env::var("BOXD_API_KEY").expect("BOXD_API_KEY");
    let boxd = Boxd::new(key, BoxdConfig::default()).unwrap();
    let p: &dyn Provider = &boxd;
    let m = p.create_machine(CreateSpec { name: Some("stowbox-smoke-cancel".into()), ..Default::default() }).await.unwrap();
    println!("box {} {:?}", m.name, m.status);

    let cases = [
        ("A plain wrapper, background child", "sleep 600 & echo started; wait", "sleep 600"),
        ("B exec replaces the shell", "exec sleep 601", "sleep 601"),
        ("C foreground child of the shell", "sleep 602; echo done", "sleep 602"),
        ("D exec -a marker", "exec -a stowbox-agent-probe sleep 603", "stowbox-agent-probe"),
    ];
    for (label, script, needle) in cases {
        let h = p.exec(&m.id, ExecSpec { script: script.into(), tty: false, cols: 0, rows: 0, env: vec![] }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        drop(h);
        for wait_ms in [500u64, 2500, 6000] {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            let ps = run(p, &m.id, &format!("ps -eo pid,ppid,pgid,sid,stat,cmd | grep -F '{needle}' | grep -v -F 'grep -F' | grep -v -F 'ps -eo'")).await;
            let alive = ps.lines().filter(|l| !l.trim().is_empty()).count();
            println!("{label}: {}ms after drop → {alive} alive", wait_ms);
            for l in ps.lines().filter(|l| !l.trim().is_empty()) {
                println!("    {}", l.trim());
            }
            if alive == 0 {
                break;
            }
        }
        // Clean up whatever survived so cases do not pollute each other.
        let _ = run(p, &m.id, &format!("pkill -KILL -f '{}' || true", needle.replacen(char::is_alphabetic, "[", 1).replacen('[', "[", 1))).await;
        let _ = run(p, &m.id, "pkill -KILL sleep || true").await;
    }

    // Marker pattern with the bracket trick must not match its own wrapper.
    let h = p.exec(&m.id, ExecSpec { script: "exec -a stowbox-agent-zz sleep 604".into(), tty: false, cols: 0, rows: 0, env: vec![] }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let out = run(p, &m.id, "pkill -TERM -f 'stowbox-agent-[z]z'; echo pkill=$?; sleep 0.5; ps -eo pid,cmd | grep -F 'sleep 604' | grep -v -F grep | wc -l").await;
    println!("E pkill by bracketed marker: {}", out.trim().replace('\n', " | "));
    drop(h);

    p.power(&m.id, PowerAction::Destroy).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = p.list_machines().await.unwrap().iter().filter(|x| x.name.starts_with("stowbox-smoke")).count();
    println!("destroyed; smoke machines left: {left}");
}
