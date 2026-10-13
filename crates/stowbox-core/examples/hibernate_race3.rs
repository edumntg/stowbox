//! Control for the resume failure: is it "resume right after pause" in
//! general, or only after a recent wake from hibernation? Throwaway box.

use std::time::{Duration, Instant};
use stowbox_core::boxd::{pb, Boxd, BoxdConfig, TokenExchange, DEFAULT_AUTH_URL, DEFAULT_ENDPOINT};
use stowbox_core::provider::Provider;
use stowbox_core::types::*;
use tonic::transport::{Channel, ClientTlsConfig};

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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::main]
async fn main() {
    let key = std::env::var("BOXD_API_KEY").expect("BOXD_API_KEY");
    let boxd = Boxd::new(key.clone(), BoxdConfig::default()).unwrap();
    let p: &dyn Provider = &boxd;
    let auth = TokenExchange::new(key, DEFAULT_AUTH_URL.into());
    let jwt = auth.token().await.unwrap();
    let channel = Channel::from_static(DEFAULT_ENDPOINT).tls_config(ClientTlsConfig::new().with_webpki_roots()).unwrap().connect_lazy();
    let bearer: tonic::metadata::MetadataValue<tonic::metadata::Ascii> = format!("Bearer {jwt}").parse().unwrap();
    let mut raw = pb::boxd_api_client::BoxdApiClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", bearer.clone());
        Ok(req)
    });

    let m = p.create_machine(CreateSpec { name: Some("stowbox-smoke-race3".into()), ..Default::default() }).await.unwrap();
    let id = m.id.clone();
    println!("box {} ({})", m.name, m.id);

    macro_rules! resume {
        ($raw:expr, $id:expr) => {
            match $raw.resume_vm(pb::ResumeVmRequest { vm_id: $id }).await {
                Ok(r) => format!("ok ({} µs)", r.into_inner().resume_us),
                Err(s) => format!("{:?}: {}", s.code(), s.message()),
            }
        };
    }

    println!("\nA. pause → immediate ResumeVm, machine running for 5 s beforehand, no hibernate in this cycle (x5)");
    for i in 1..=5 {
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(30)).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let t = Instant::now();
        let _ = raw.suspend_vm(pb::SuspendVmRequest { vm_id: id.clone() }).await;
        let r = resume!(raw, id.clone());
        println!("  {i}: pause→resume gap {} ms → {r}", t.elapsed().as_millis());
    }

    println!("\nB. pause → wait 1 s → ResumeVm, no hibernate (x3)");
    for i in 1..=3 {
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(30)).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = raw.suspend_vm(pb::SuspendVmRequest { vm_id: id.clone() }).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let r = resume!(raw, id.clone());
        println!("  {i}: → {r}");
    }

    println!("\nC. hibernate → wake → wait 5 s → pause → immediate ResumeVm (x3)");
    for i in 1..=3 {
        let _ = p.power(&id, PowerAction::Hibernate).await;
        let _ = wait_status(p, &id, MachineStatus::Hibernated, Duration::from_secs(60)).await;
        let _ = p.power(&id, PowerAction::Wake).await;
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(60)).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = raw.suspend_vm(pb::SuspendVmRequest { vm_id: id.clone() }).await;
        let r = resume!(raw, id.clone());
        println!("  {i}: → {r}");
    }

    println!("\nD. hibernate → wake → immediate pause → wait 1 s → ResumeVm (x3)");
    for i in 1..=3 {
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(30)).await;
        let _ = p.power(&id, PowerAction::Hibernate).await;
        let _ = wait_status(p, &id, MachineStatus::Hibernated, Duration::from_secs(60)).await;
        let _ = p.power(&id, PowerAction::Wake).await;
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(60)).await;
        let _ = raw.suspend_vm(pb::SuspendVmRequest { vm_id: id.clone() }).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let r = resume!(raw, id.clone());
        println!("  {i}: → {r}");
    }

    println!("\nE. hibernate → wake → immediate pause → immediate ResumeVm, then ResumeVm again at once (x3)");
    for i in 1..=3 {
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(30)).await;
        let _ = p.power(&id, PowerAction::Hibernate).await;
        let _ = wait_status(p, &id, MachineStatus::Hibernated, Duration::from_secs(60)).await;
        let _ = p.power(&id, PowerAction::Wake).await;
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(60)).await;
        let _ = raw.suspend_vm(pb::SuspendVmRequest { vm_id: id.clone() }).await;
        let r1 = resume!(raw, id.clone());
        let r2 = resume!(raw, id.clone());
        println!("  {i}: first {r1} · second {r2}");
    }

    p.power(&id, PowerAction::Destroy).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = p.list_machines().await.unwrap().iter().filter(|x| x.name.starts_with("stowbox-smoke")).count();
    println!("\ndestroyed; smoke machines left: {left}");
}
