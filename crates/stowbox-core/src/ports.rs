//! Servers in a box, reached from the browser.
//!
//! boxd has two primitives, and they are not interchangeable:
//!
//! - HTTPS routes ("proxies"): boxd terminates TLS and forwards HTTP to a
//!   port in the box. Every box has a default route at `<box>.<domain>`
//!   (the machine's `url`) that follows the first listener on 80, 443, 8080,
//!   8000, 3000, 5000 or 5173, else 8000, unless a port is pinned. Named
//!   routes add `<name>.<box>.<domain>`, each pinned to a port. This is what
//!   a dev server wants: a real `https://` link that opens in a browser.
//! - Raw forwards (`ExposePort`): a TCP and/or UDP port from 40000 to 60000
//!   on the box's public address, passed through untouched, no TLS, at most
//!   three per box. For databases and other non-HTTP servers.
//!
//! Both are reachable from the internet by anyone who has the address.
//! Which ports something listens on is read inside the box with one
//! read-only command (`ss`, or `/proc/net/tcp*` where `ss` is missing) and
//! parsed here.

use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// One HTTPS route into a box, as boxd lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpsRoute {
    /// Empty for the default route on some servers; the default is marked by `is_default`.
    pub name: String,
    /// The host boxd returned, verbatim.
    pub domain: String,
    /// `https://<domain>`, or `None` when boxd returned no domain.
    pub url: Option<String>,
    /// The port traffic lands on: the pinned one, else the detected one. 0 = boxd did not say.
    pub port: u32,
    /// "locked" (pinned), "auto" (detected) or "" when boxd did not say.
    pub port_mode: String,
    pub is_default: bool,
}

/// One raw TCP/UDP forward, as boxd lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortForward {
    /// The host boxd returned, verbatim.
    pub dns: String,
    pub public_port: u32,
    pub vm_port: u32,
    /// "tcp", "udp" or "both".
    pub protocol: String,
    /// `<dns>:<public_port>`, or `None` when boxd returned no host or port.
    pub address: Option<String>,
}

/// A TCP port something listens on inside the box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    pub port: u16,
    /// Local addresses it is bound to, as `ss` prints them ("0.0.0.0", "::", "127.0.0.1").
    pub addresses: Vec<String>,
    /// The program, when the listing names it (`ss -p` only names the box user's own processes).
    pub process: Option<String>,
    /// Bound to loopback addresses only: boxd's proxy, which connects from
    /// outside the box, cannot reach it.
    pub local_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listening {
    pub ports: Vec<ListeningPort>,
    /// "ss" or "proc": where the list came from.
    pub source: String,
    pub sampled_at_ms: i64,
}

/// The link for a route. boxd's `domain` is a bare host; one that already
/// carries a scheme is kept as it is. Nothing is made up when it is empty.
pub fn https_url(domain: &str) -> Option<String> {
    let d = domain.trim();
    if d.is_empty() {
        return None;
    }
    if d.contains("://") {
        return Some(d.to_string());
    }
    Some(format!("https://{d}"))
}

/// `<dns>:<public_port>` for a raw forward, when boxd gave both.
pub fn forward_address(dns: &str, public_port: u32) -> Option<String> {
    let d = dns.trim();
    if d.is_empty() || public_port == 0 {
        return None;
    }
    Some(format!("{d}:{public_port}"))
}

/// A route name becomes a DNS label (`<name>.<box>.<domain>`): 1 to 63
/// lowercase letters, digits and hyphens, not starting or ending with a
/// hyphen. boxd has the last word; this only catches what cannot work.
pub fn validate_route_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(Error::Provider(format!(
            "\"{name}\" cannot be a link name: use lowercase letters, digits and hyphens, up to 63, not starting or ending with a hyphen"
        )))
    }
}

/// A free route name for `port`: `p<port>`, then `p<port>-2` and on.
pub fn suggest_route_name(port: u16, taken: &[String]) -> String {
    let base = format!("p{port}");
    if !taken.iter().any(|t| t == &base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken.iter().any(|t| t == c)).unwrap()
}

pub fn validate_port(port: u32) -> Result<u16> {
    match u16::try_from(port) {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(Error::Provider(format!("{port} is not a port: use 1 to 65535"))),
    }
}

pub fn validate_protocol(protocol: &str) -> Result<()> {
    match protocol {
        "tcp" | "udp" | "both" => Ok(()),
        _ => Err(Error::Provider(format!("\"{protocol}\" is not a protocol: use tcp, udp or both"))),
    }
}

/// Lists TCP listeners. Read-only: `ss` where it exists, else the kernel's
/// own tables. A marker line says which one answered.
pub const SCRIPT: &str = "\
if command -v ss >/dev/null 2>&1; then echo '@ss'; ss -ltnp; \
else echo '@proc'; cat /proc/net/tcp /proc/net/tcp6 2>/dev/null; fi
";

pub const DEADLINE: Duration = Duration::from_secs(10);

/// Reads what listens in the box. `Ok(None)` when the box is not running
/// and `wake` is false: exec would wake it.
pub async fn listening<P: Provider + ?Sized>(p: &P, id: &str, wake: bool) -> Result<Option<Listening>> {
    let m = p.get_machine(id).await?;
    let asleep = matches!(m.status, MachineStatus::Suspended | MachineStatus::Hibernated);
    if m.status != MachineStatus::Running && !(wake && asleep) {
        return Ok(None);
    }
    let c = crate::git::capture(p, &m.id, SCRIPT.into(), None, DEADLINE, "listing the listening ports").await?;
    let out = String::from_utf8_lossy(&c.stdout);
    match parse_listing(&out) {
        Some((source, ports)) => Ok(Some(Listening { ports, source: source.into(), sampled_at_ms: now_ms() })),
        None => Err(crate::git::script_failed(&c, "the port listing")),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Parses the script's output. `None` when no marker came back.
pub fn parse_listing(out: &str) -> Option<(&'static str, Vec<ListeningPort>)> {
    let (marker, rest) = out.split_once('\n').unwrap_or((out, ""));
    match marker.trim() {
        "@ss" => Some(("ss", parse_ss(rest))),
        "@proc" => Some(("proc", parse_proc_net(rest))),
        _ => None,
    }
}

/// `ss -ltn[p]` lines, with or without the header:
/// `LISTEN 0 511 0.0.0.0:3000 0.0.0.0:* users:(("node",pid=12,fd=20))`.
pub fn parse_ss(text: &str) -> Vec<ListeningPort> {
    let mut found = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 || cols[0] != "LISTEN" {
            continue;
        }
        let Some((addr, port)) = split_host_port(cols[3]) else { continue };
        let process = cols.get(5..).and_then(|rest| process_name(&rest.join(" ")));
        found.push((port, addr, process));
    }
    group(found)
}

/// `ss` prints `0.0.0.0:22`, `[::]:22`, `*:8080`, `127.0.0.53%lo:53`, `[::1]:5173`.
fn split_host_port(s: &str) -> Option<(String, u16)> {
    let (host, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = host.split('%').next().unwrap_or(host);
    Some((host.to_string(), port))
}

/// The first program in `users:(("node",pid=12,fd=20),…)`.
fn process_name(s: &str) -> Option<String> {
    let start = s.find("((\"")? + 3;
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string()).filter(|n| !n.is_empty())
}

/// `/proc/net/tcp` and `tcp6` rows in state `0A` (LISTEN). Addresses are
/// hex in the kernel's byte order.
pub fn parse_proc_net(text: &str) -> Vec<ListeningPort> {
    let mut found = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 || cols[3] != "0A" || !cols[0].ends_with(':') {
            continue;
        }
        let Some((addr, port)) = cols[1].split_once(':') else { continue };
        let Ok(port) = u16::from_str_radix(port, 16) else { continue };
        let Some(addr) = proc_addr(addr) else { continue };
        found.push((port, addr, None));
    }
    group(found)
}

fn proc_addr(hex: &str) -> Option<String> {
    match hex.len() {
        8 => {
            let n = u32::from_str_radix(hex, 16).ok()?;
            Some(std::net::Ipv4Addr::from(n.to_le_bytes()).to_string())
        }
        32 => {
            // Four 32-bit words, each in host (little-endian) order. `get`
            // returns None when a non-ASCII character straddles a word.
            let mut bytes = Vec::with_capacity(16);
            for w in 0..4 {
                let word = hex.get(w * 8..w * 8 + 8)?;
                bytes.extend_from_slice(&u32::from_str_radix(word, 16).ok()?.to_le_bytes());
            }
            let bytes: [u8; 16] = bytes.try_into().ok()?;
            let v6 = std::net::Ipv6Addr::from(bytes);
            Some(match v6.to_ipv4_mapped() {
                Some(v4) => v4.to_string(),
                None => v6.to_string(),
            })
        }
        _ => None,
    }
}

fn is_loopback(addr: &str) -> bool {
    addr == "localhost" || addr == "::1" || addr.starts_with("127.")
}

/// One entry per port, sorted, addresses deduplicated in the order seen.
fn group(found: Vec<(u16, String, Option<String>)>) -> Vec<ListeningPort> {
    let mut by_port: BTreeMap<u16, ListeningPort> = BTreeMap::new();
    for (port, addr, process) in found {
        let e = by_port.entry(port).or_insert_with(|| ListeningPort { port, addresses: Vec::new(), process: None, local_only: true });
        if !e.addresses.contains(&addr) {
            e.addresses.push(addr);
        }
        if e.process.is_none() {
            e.process = process;
        }
    }
    by_port
        .into_values()
        .map(|mut e| {
            e.local_only = e.addresses.iter().all(|a| is_loopback(a));
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SS: &str = "\
State  Recv-Q Send-Q Local Address:Port  Peer Address:PortProcess
LISTEN 0      4096   127.0.0.53%lo:53         0.0.0.0:*
LISTEN 0      511          0.0.0.0:3000       0.0.0.0:*    users:((\"node\",pid=1234,fd=20))
LISTEN 0      511        127.0.0.1:5173       0.0.0.0:*    users:((\"node\",pid=88,fd=31),(\"npm\",pid=80,fd=3))
LISTEN 0      511            [::1]:5173          [::]:*    users:((\"node\",pid=88,fd=32))
LISTEN 0      128             [::]:22            [::]:*
LISTEN 0      128          0.0.0.0:22         0.0.0.0:*
LISTEN 0      4096               *:8080             *:*
ESTAB  0      0          10.0.0.5:22      10.0.0.1:51234
";

    #[test]
    fn ss_lines_group_by_port() {
        let ports = parse_ss(SS);
        let summary: Vec<(u16, Vec<&str>, Option<&str>, bool)> =
            ports.iter().map(|p| (p.port, p.addresses.iter().map(String::as_str).collect(), p.process.as_deref(), p.local_only)).collect();
        assert_eq!(
            summary,
            vec![
                (22, vec!["::", "0.0.0.0"], None, false),
                (53, vec!["127.0.0.53"], None, true),
                (3000, vec!["0.0.0.0"], Some("node"), false),
                (5173, vec!["127.0.0.1", "::1"], Some("node"), true),
                (8080, vec!["*"], None, false),
            ]
        );
    }

    #[test]
    fn proc_addresses_decode_and_reject_non_ascii() {
        assert_eq!(proc_addr("0100007F").as_deref(), Some("127.0.0.1"));
        assert_eq!(proc_addr("00000000000000000000000001000000").as_deref(), Some("::1"));
        // U+FFFD is three bytes, so this is 32 bytes with a split word.
        assert_eq!(proc_addr("0000000\u{FFFD}0000000000000000000001"), None);
        assert_eq!(proc_addr("\u{FFFD}\u{FFFD}"), None);
    }

    #[test]
    fn ss_without_header_or_process_column() {
        let ports = parse_ss("LISTEN 0 128 0.0.0.0:8000 0.0.0.0:*\n");
        assert_eq!(ports, vec![ListeningPort { port: 8000, addresses: vec!["0.0.0.0".into()], process: None, local_only: false }]);
        assert!(parse_ss("").is_empty());
        assert!(parse_ss("LISTEN 0 128 nonsense 0.0.0.0:*\n").is_empty());
    }

    #[test]
    fn proc_net_rows_decode_addresses_and_ports() {
        let text = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:9AED 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 6833704 1
   1: 00000000:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 7281 1
   2: 0500000A:0016 0100000A:C822 01 00000000:00000000 00:00000000 00000000     0        0 0 1
  sl  local_address                         remote_address                        st
   0: 00000000000000000000000000000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000
   1: 00000000000000000000000001000000:1435 00000000000000000000000000000000:0000 0A 00000000:00000000
   2: 0000000000000000FFFF00000100007F:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000
";
        let ports = parse_proc_net(text);
        let summary: Vec<(u16, Vec<&str>, bool)> = ports.iter().map(|p| (p.port, p.addresses.iter().map(String::as_str).collect(), p.local_only)).collect();
        assert_eq!(
            summary,
            vec![(3000, vec!["0.0.0.0", "::"], false), (5173, vec!["::1"], true), (8080, vec!["127.0.0.1"], true), (39661, vec!["127.0.0.1"], true)]
        );
        assert!(ports.iter().all(|p| p.process.is_none()));
    }

    #[test]
    fn listing_needs_its_marker() {
        let (src, ports) = parse_listing("@ss\nLISTEN 0 128 0.0.0.0:3000 0.0.0.0:*\n").unwrap();
        assert_eq!((src, ports.len()), ("ss", 1));
        let (src, ports) = parse_listing("@proc\n   1: 00000000:0BB8 00000000:0000 0A 0\n").unwrap();
        assert_eq!((src, ports[0].port), ("proc", 3000));
        assert_eq!(parse_listing(""), None);
        assert_eq!(parse_listing("bash: ss: command not found\n"), None);
    }

    #[test]
    fn urls_come_from_what_boxd_returned() {
        assert_eq!(https_url("api.mock-api.boxd.sh").as_deref(), Some("https://api.mock-api.boxd.sh"));
        assert_eq!(https_url(" https://x.example.com ").as_deref(), Some("https://x.example.com"));
        assert_eq!(https_url(""), None);
        assert_eq!(forward_address("mock-api.boxd.sh", 48211).as_deref(), Some("mock-api.boxd.sh:48211"));
        assert_eq!(forward_address("", 48211), None);
        assert_eq!(forward_address("mock-api.boxd.sh", 0), None);
    }

    #[test]
    fn names_ports_and_protocols_are_checked() {
        for ok in ["api", "p3000", "web-2", "a"] {
            assert!(validate_route_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "API", "-api", "api-", "a.b", "my_app", &"a".repeat(64)] {
            assert!(validate_route_name(bad).is_err(), "{bad}");
        }
        assert_eq!(validate_port(3000).unwrap(), 3000);
        assert!(validate_port(0).is_err());
        assert!(validate_port(70000).is_err());
        assert!(validate_protocol("both").is_ok());
        assert!(validate_protocol("http").is_err());
    }

    #[test]
    fn suggested_names_avoid_taken_ones() {
        assert_eq!(suggest_route_name(5173, &[]), "p5173");
        assert_eq!(suggest_route_name(5173, &["p5173".into(), "p5173-2".into()]), "p5173-3");
    }

    #[cfg(unix)] // box-side script with unix-only test plumbing; see testing.rs
    #[tokio::test]
    async fn the_script_runs_and_parses_on_this_machine() {
        if !crate::testing::has_bash() {
            return;
        }
        let home = crate::testing::temp_dir("stowbox-ports");
        let p = crate::testing::LocalBash { home, env: vec![] };
        let l = listening(&p, "vm_1", false).await.unwrap().expect("a running box is read");
        assert!(l.source == "ss" || l.source == "proc");
        assert!(l.ports.windows(2).all(|w| w[0].port < w[1].port));

        // Without ss on PATH the kernel tables answer instead.
        let bin = crate::testing::temp_dir("stowbox-ports-bin");
        for tool in ["cat", "bash"] {
            if let Ok(path) = which(tool) {
                std::os::unix::fs::symlink(path, bin.join(tool)).unwrap();
            }
        }
        let p = crate::testing::LocalBash { home: bin.clone(), env: vec![("PATH".into(), bin.display().to_string())] };
        let l = listening(&p, "vm_1", false).await.unwrap().unwrap();
        assert_eq!(l.source, "proc");
    }

    #[cfg(unix)]
    fn which(tool: &str) -> std::io::Result<std::path::PathBuf> {
        let out = std::process::Command::new("bash").arg("-c").arg(format!("command -v {tool}")).output()?;
        Ok(std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
    }

    #[tokio::test]
    #[cfg_attr(not(unix), ignore = "box-side script, needs a Linux box's bash; see testing.rs")]
    async fn a_sleeping_box_is_left_alone_unless_asked() {
        struct Asleep(crate::testing::LocalBash);
        #[async_trait::async_trait]
        impl Provider for Asleep {
            fn id(&self) -> &'static str {
                "asleep"
            }
            fn capabilities(&self) -> crate::capabilities::Capabilities {
                crate::boxd::capabilities()
            }
            async fn whoami(&self) -> Result<Account> {
                unimplemented!()
            }
            async fn list_machines(&self) -> Result<Vec<Machine>> {
                unimplemented!()
            }
            async fn get_machine(&self, id: &str) -> Result<Machine> {
                Ok(crate::testing::machine(id, MachineStatus::Suspended))
            }
            async fn create_machine(&self, _: CreateSpec) -> Result<Machine> {
                unimplemented!()
            }
            async fn fork_machine(&self, _: &str, _: Option<String>) -> Result<Machine> {
                unimplemented!()
            }
            async fn power(&self, _: &str, _: PowerAction) -> Result<()> {
                unimplemented!()
            }
            async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
                unimplemented!()
            }
            async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
                unimplemented!()
            }
            async fn exec(&self, id: &str, spec: ExecSpec) -> Result<ExecHandle> {
                self.0.exec(id, spec).await
            }
            async fn machine_usage(&self, _: &str) -> Result<Usage> {
                unimplemented!()
            }
            async fn org_billing(&self, _: &str) -> Result<OrgBilling> {
                unimplemented!()
            }
            async fn rates(&self) -> Result<crate::cost::RateCard> {
                unimplemented!()
            }
        }
        if !crate::testing::has_bash() {
            return;
        }
        let p = Asleep(crate::testing::LocalBash { home: crate::testing::temp_dir("stowbox-ports-asleep"), env: vec![] });
        assert_eq!(listening(&p, "vm_1", false).await.unwrap(), None);
        assert!(listening(&p, "vm_1", true).await.unwrap().is_some());
    }
}
