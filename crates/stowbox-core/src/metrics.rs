//! Live CPU, memory, disk and uptime of a running box.
//!
//! boxd has no telemetry RPC: `GetVm` carries the allocation and `GetVmUsage`
//! is billing-metered with up to 25 minutes of lag. So the numbers in the
//! fleet view are read inside the box, from `/proc` and `df`, over the
//! provider's exec stream. The sample is never taken on a box that is not
//! `running`: exec wakes standby and hibernated machines, which would cost
//! money and defeat auto-suspend.

use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineMetrics {
    /// Wall-clock unix milliseconds on this machine when the sample came back.
    pub sampled_at_ms: i64,
    /// Busy share of all vCPUs over the sample window, 0 to 100.
    pub cpu_percent: f64,
    /// Online processors as the guest sees them.
    pub vcpu: u32,
    /// One-minute load average.
    pub load1: f64,
    pub mem_total_bytes: u64,
    /// `MemTotal - MemAvailable`: what the kernel could not hand out right now.
    pub mem_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_used_bytes: u64,
    /// Bytes an unprivileged process can still write to `/`.
    pub disk_avail_bytes: u64,
    pub uptime_secs: u64,
}

/// What runs inside the box. Two `/proc/stat` reads bracket a half-second
/// sleep so the CPU figure is a measured share, not a load-average guess.
/// Every line is prefixed with a tag so the parser never depends on order.
pub const SCRIPT: &str = "\
printf 'stat '; head -n1 /proc/stat
sleep 0.5
printf 'stat '; head -n1 /proc/stat
printf 'ncpu '; nproc 2>/dev/null || getconf _NPROCESSORS_ONLN
printf 'load '; cat /proc/loadavg
printf 'uptime '; cat /proc/uptime
printf 'mem '; grep -E '^(MemTotal|MemAvailable):' /proc/meminfo | tr '\\n' ' '; echo
printf 'disk '; df -kP / | tail -n1
";

/// How long one sample may take end to end, including the exec round trip.
pub const DEADLINE: Duration = Duration::from_secs(12);

/// Samples `id` if it is running. `Ok(None)` means the box was in another
/// state and was deliberately left alone.
pub async fn sample<P: Provider + ?Sized>(p: &P, id: &str) -> Result<Option<MachineMetrics>> {
    let m = p.get_machine(id).await?;
    if m.status != MachineStatus::Running {
        return Ok(None);
    }
    let c = crate::git::capture(p, &m.id, format!("export LC_ALL=C\n{SCRIPT}"), None, DEADLINE, "the metrics sample").await?;
    if c.code != 0 {
        let text = String::from_utf8_lossy(&c.stderr);
        return Err(Error::Provider(format!("metrics script exited with code {}: {}", c.code, text.trim())));
    }
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    parse(&String::from_utf8_lossy(&c.stdout), now_ms).map(Some)
}

/// Totals from one `cpu` line of `/proc/stat`: (busy, all) jiffies.
fn cpu_line(rest: &str) -> Option<(u64, u64)> {
    let mut it = rest.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    let v: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let all: u64 = v.iter().sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((all - idle, all))
}

fn kib(s: &str) -> Option<u64> {
    s.parse::<u64>().ok().map(|k| k * 1024)
}

/// Turns the script's output into metrics. Tolerates a missing CPU window
/// (falls back to load average over processors) but not missing memory or
/// disk, since those would have to be invented.
pub fn parse(text: &str, sampled_at_ms: i64) -> Result<MachineMetrics> {
    let mut stats: Vec<(u64, u64)> = Vec::new();
    let mut vcpu: u32 = 0;
    let mut load1: f64 = 0.0;
    let mut uptime: f64 = 0.0;
    let mut mem: Option<(u64, u64)> = None;
    let mut disk: Option<(u64, u64, u64)> = None;

    for line in text.lines() {
        let Some((tag, rest)) = line.split_once(' ') else { continue };
        let rest = rest.trim();
        match tag {
            "stat" => {
                if let Some(s) = cpu_line(rest) {
                    stats.push(s);
                }
            }
            "ncpu" => vcpu = rest.parse().unwrap_or(0),
            "load" => load1 = rest.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0.0),
            "uptime" => uptime = rest.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0.0),
            "mem" => {
                let mut total = None;
                let mut avail = None;
                let f: Vec<&str> = rest.split_whitespace().collect();
                for w in f.windows(2) {
                    match w[0] {
                        "MemTotal:" => total = kib(w[1]),
                        "MemAvailable:" => avail = kib(w[1]),
                        _ => {}
                    }
                }
                if let (Some(t), Some(a)) = (total, avail) {
                    mem = Some((t, t.saturating_sub(a)));
                }
            }
            "disk" => {
                // df -kP: filesystem, 1024-blocks, used, available, capacity, mount
                let f: Vec<&str> = rest.split_whitespace().collect();
                if f.len() >= 4 {
                    if let (Some(t), Some(u), Some(a)) = (kib(f[1]), kib(f[2]), kib(f[3])) {
                        disk = Some((t, u, a));
                    }
                }
            }
            _ => {}
        }
    }

    let (mem_total_bytes, mem_used_bytes) = mem.ok_or_else(|| Error::Provider("metrics: no memory line in the sample".into()))?;
    let (disk_total_bytes, disk_used_bytes, disk_avail_bytes) = disk.ok_or_else(|| Error::Provider("metrics: no disk line in the sample".into()))?;

    let cpu_percent = match (stats.first(), stats.get(1)) {
        (Some(&(b1, a1)), Some(&(b2, a2))) if a2 > a1 => (b2.saturating_sub(b1)) as f64 * 100.0 / (a2 - a1) as f64,
        _ if vcpu > 0 => (load1 / vcpu as f64 * 100.0).min(100.0),
        _ => 0.0,
    };

    Ok(MachineMetrics {
        sampled_at_ms,
        cpu_percent: (cpu_percent * 10.0).round() / 10.0,
        vcpu,
        load1,
        mem_total_bytes,
        mem_used_bytes,
        disk_total_bytes,
        disk_used_bytes,
        disk_avail_bytes,
        uptime_secs: uptime.max(0.0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
stat cpu  1000 0 500 8000 100 0 20 0 0 0
stat cpu  1100 0 540 8300 110 0 30 0 0 0
ncpu 2
load 0.42 0.30 0.10 1/180 4242
uptime 98765.43 180000.00
mem MemTotal: 8000000 kB MemAvailable: 6000000 kB
disk /dev/vda1 104857600 20971520 78643200 22% /
";

    #[test]
    fn parses_a_full_sample() {
        let m = parse(SAMPLE, 1234).unwrap();
        // busy went 1520 -> 1670 (+150), all went 9620 -> 10080 (+460)
        assert!((m.cpu_percent - 32.6).abs() < 0.05, "cpu {}", m.cpu_percent);
        assert_eq!(m.vcpu, 2);
        assert!((m.load1 - 0.42).abs() < 1e-9);
        assert_eq!(m.uptime_secs, 98765);
        assert_eq!(m.mem_total_bytes, 8_000_000 * 1024);
        assert_eq!(m.mem_used_bytes, 2_000_000 * 1024);
        assert_eq!(m.disk_total_bytes, 104_857_600 * 1024);
        assert_eq!(m.disk_used_bytes, 20_971_520 * 1024);
        assert_eq!(m.disk_avail_bytes, 78_643_200 * 1024);
        assert_eq!(m.sampled_at_ms, 1234);
    }

    #[test]
    fn falls_back_to_load_average_without_a_cpu_window() {
        let text = SAMPLE.lines().skip(1).collect::<Vec<_>>().join("\n");
        let m = parse(&text, 0).unwrap();
        assert!((m.cpu_percent - 21.0).abs() < 1e-9, "cpu {}", m.cpu_percent);
    }

    #[test]
    fn load_fallback_is_capped_at_full() {
        let text = "ncpu 1\nload 7.5 7.0 6.9 3/100 1\nmem MemTotal: 10 kB MemAvailable: 5 kB\ndisk / 10 5 5 50% /\n";
        let m = parse(text, 0).unwrap();
        assert_eq!(m.cpu_percent, 100.0);
    }

    #[test]
    fn identical_stat_lines_do_not_divide_by_zero() {
        let text = "stat cpu 1 2 3 4\nstat cpu 1 2 3 4\nncpu 2\nload 1.0 0 0 1/1 1\nmem MemTotal: 10 kB MemAvailable: 5 kB\ndisk / 10 5 5 50% /\n";
        let m = parse(text, 0).unwrap();
        assert_eq!(m.cpu_percent, 50.0);
    }

    #[test]
    fn refuses_to_invent_memory_or_disk() {
        assert!(parse("stat cpu 1 2 3 4\nncpu 1\n", 0).is_err());
        assert!(parse("mem MemTotal: 10 kB MemAvailable: 5 kB\n", 0).is_err());
    }

    #[test]
    fn ignores_noise_lines() {
        let text = format!("bash: warning: setlocale\n{SAMPLE}\nstat garbage\n");
        assert!(parse(&text, 0).is_ok());
    }

    #[test]
    fn serializes_camel_case_for_the_ui() {
        let m = parse(SAMPLE, 1).unwrap();
        let j = serde_json::to_string(&m).unwrap();
        assert!(j.contains("\"cpuPercent\""));
        assert!(j.contains("\"diskAvailBytes\""));
    }
}
