//! Second pass on the hibernate → wake → pause race: a finer delay sweep and
//! the raw gRPC status of an explicit ResumeVm. Throwaway box, destroyed.

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

    // Raw client for exact status codes.
    let auth = TokenExchange::new(key, DEFAULT_AUTH_URL.into());
    let jwt = auth.token().await.unwrap();
    let channel = Channel::from_static(DEFAULT_ENDPOINT).tls_config(ClientTlsConfig::new().with_webpki_roots()).unwrap().connect_lazy();
    let bearer: tonic::metadata::MetadataValue<tonic::metadata::Ascii> = format!("Bearer {jwt}").parse().unwrap();
    let mut raw = pb::boxd_api_client::BoxdApiClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", bearer.clone());
        Ok(req)
    });

    let m = p.create_machine(CreateSpec { name: Some("stowbox-smoke-race2".into()), ..Default::default() }).await.unwrap();
    let id = m.id.clone();
    println!("box {} ({}) {:?} · {} vCPU {} GiB", m.name, m.id, m.status, m.vcpu, m.memory_bytes >> 30);

    println!("\n| delay between `running` and pause | first resume via ResumeVm | status | retry ResumeVm after 2 s | status |");
    println!("|---|---|---|---|---|");
    for delay_ms in [0u64, 25, 50, 100, 150, 200, 300] {
        let _ = p.power(&id, PowerAction::Hibernate).await;
        if let Err(e) = wait_status(p, &id, MachineStatus::Hibernated, Duration::from_secs(60)).await {
            println!("| {delay_ms} ms | never hibernated: {e} |");
            continue;
        }
        let _ = p.power(&id, PowerAction::Wake).await;
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(60)).await;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let _ = raw.suspend_vm(pb::SuspendVmRequest { vm_id: id.clone() }).await;
        let first = match raw.resume_vm(pb::ResumeVmRequest { vm_id: id.clone() }).await {
            Ok(r) => format!("ok (resume_us {})", r.into_inner().resume_us),
            Err(s) => format!("{:?}: {}", s.code(), s.message()),
        };
        let st1 = p.get_machine(&id).await.map(|m| format!("{:?}", m.status).to_lowercase()).unwrap_or_default();
        let (retry, st2) = if first.starts_with("ok") {
            ("n/a".to_string(), st1.clone())
        } else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let r = match raw.resume_vm(pb::ResumeVmRequest { vm_id: id.clone() }).await {
                Ok(r) => format!("ok (resume_us {})", r.into_inner().resume_us),
                Err(s) => format!("{:?}: {}", s.code(), s.message()),
            };
            (r, p.get_machine(&id).await.map(|m| format!("{:?}", m.status).to_lowercase()).unwrap_or_default())
        };
        println!("| {delay_ms} ms | {first} | {st1} | {retry} | {st2} |");
        let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(30)).await;
    }

    // Does the failing path report anything in the machine's own view?
    let _ = p.power(&id, PowerAction::Hibernate).await;
    let _ = wait_status(p, &id, MachineStatus::Hibernated, Duration::from_secs(60)).await;
    let _ = p.power(&id, PowerAction::Wake).await;
    let _ = wait_status(p, &id, MachineStatus::Running, Duration::from_secs(60)).await;
    let _ = raw.suspend_vm(pb::SuspendVmRequest { vm_id: id.clone() }).await;
    let vm = raw.get_vm(pb::GetVmRequest { vm_id: id.clone() }).await.map(|r| r.into_inner());
    if let Ok(vm) = vm {
        println!("\nGetVm right after the racy pause: status={} hibernated_at_ms={} boot_time_ms={}", vm.status, vm.hibernated_at_ms, vm.boot_time_ms);
    }
    let first = match raw.resume_vm(pb::ResumeVmRequest { vm_id: id.clone() }).await {
        Ok(_) => "ok".to_string(),
        Err(s) => format!("{:?}: {}", s.code(), s.message()),
    };
    println!("ResumeVm: {first}");
    // How long until a resume works again? Poll every 250 ms.
    let t = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        match raw.resume_vm(pb::ResumeVmRequest { vm_id: id.clone() }).await {
            Ok(_) => {
                println!("resume succeeded {} ms after the failed one", t.elapsed().as_millis());
                break;
            }
            Err(s) if t.elapsed() < Duration::from_secs(20) => {
                if t.elapsed().as_millis() % 1000 < 260 {
                    println!("  still failing at {} ms: {:?}: {}", t.elapsed().as_millis(), s.code(), s.message());
                }
            }
            Err(s) => {
                println!("still failing after 20 s: {:?}: {}", s.code(), s.message());
                break;
            }
        }
    }

    p.power(&id, PowerAction::Destroy).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = p.list_machines().await.unwrap().iter().filter(|x| x.name.starts_with("stowbox-smoke")).count();
    println!("\ndestroyed; smoke machines left: {left}");
}
