//! Test doubles shared by the modules that drive a box over exec.

use crate::provider::Provider;
use crate::types::*;
use crate::Result;

pub fn machine(id: &str, status: MachineStatus) -> Machine {
    Machine {
        id: id.into(),
        provider: "boxd".into(),
        size: String::new(),
        auto_stop_at: 0,
        idle_stop_secs: 0,
        name: "box".into(),
        status,
        vcpu: 2,
        memory_bytes: 0,
        disk_bytes: 0,
        public_ip: String::new(),
        access_domain: String::new(),
        url: String::new(),
        desktop_url: String::new(),
        ssh_port: 0,
        org: String::new(),
        billing_org: String::new(),
        source: String::new(),
        isolated: false,
        created_at: 0,
        hibernated_at_ms: 0,
        boot_time_ms: 0,
        last_connected_ms: 0,
        auto_suspend_secs: 0,
        auto_hibernate_secs: 0,
        auto_destroy_secs: 0,
    }
}

// Box-side scripts (git, setup presets, uploads, ports, worktrees, agent
// turns) always run in a Linux box, so their end-to-end tests run them with
// local bash on Linux and macOS only. On Windows they are marked ignored:
// `bash` there can be WSL's launcher in System32 (which `Command` finds before
// `PATH`) or Git Bash, neither is the box's environment, and a pass or a
// failure there would say nothing about a box. What runs on this computer
// (ssh config, keys, paths, the store, search) is tested on every OS.
pub fn has_bash() -> bool {
    std::process::Command::new("bash").arg("-c").arg("true").status().is_ok()
}

/// A fresh empty directory under the system temp dir.
pub fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Runs `cmd` with local bash in `dir`, with `home` as `$HOME`; panics on failure.
pub fn sh(dir: &std::path::Path, home: &std::path::Path, cmd: &str) -> String {
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{cmd}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn has_git() -> bool {
    has_bash() && std::process::Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// A home with a git identity, a bare remote and a clone of it with one
/// pushed commit on `main`.
pub fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
    let home = temp_dir("stowbox-git");
    std::fs::write(home.join(".gitconfig"), "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n").unwrap();
    sh(&home, &home, "git init -q --bare remote.git && git clone -q remote.git work 2>/dev/null");
    let work = home.join("work");
    sh(&work, &home, "printf 'one\\ntwo\\n' > a.txt && git add a.txt && git commit -qm init && git push -q -u origin main 2>/dev/null");
    (home, work)
}

/// Runs exec scripts with local bash in `home`, so code that talks to a box
/// over exec is tested end to end. stdout and stderr stay separate, like on
/// the wire. Tokens and `GIT_SSH_COMMAND` from the test runner's own
/// environment are removed so a developer's setup cannot change the outcome; `env` adds variables
/// (for example a `PATH` with stub binaries in front).
pub struct LocalBash {
    pub home: std::path::PathBuf,
    pub env: Vec<(String, String)>,
}

#[async_trait::async_trait]
impl Provider for LocalBash {
    fn id(&self) -> &'static str {
        "local"
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
        Ok(machine(id, MachineStatus::Running))
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
    async fn machine_usage(&self, _: &str) -> Result<Usage> {
        unimplemented!()
    }
    async fn org_billing(&self, _: &str) -> Result<OrgBilling> {
        unimplemented!()
    }
    async fn rates(&self) -> Result<crate::cost::RateCard> {
        unimplemented!()
    }
    async fn exec(&self, _: &str, spec: ExecSpec) -> Result<ExecHandle> {
        use std::io::{Read, Write};
        let (in_tx, mut in_rx) = tokio::sync::mpsc::channel::<ExecInput>(8);
        let (out_tx, out_rx) = tokio::sync::mpsc::channel::<ExecOutput>(64);
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c")
            .arg(&spec.script)
            .env("HOME", &self.home)
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GIT_SSH_COMMAND")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&self.home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (k, v) in self.env.iter().chain(spec.env.iter()) {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        tokio::spawn(async move {
            while let Some(ExecInput::Data(d)) = in_rx.recv().await {
                let _ = stdin.write_all(&d);
            }
            // Close or a dropped sender: stdin EOF.
        });
        let err_tx = out_tx.clone();
        let err_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = err_tx.blocking_send(ExecOutput::Stderr(buf[..n].to_vec()));
            }
        });
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = out_tx.blocking_send(ExecOutput::Stdout(buf[..n].to_vec()));
            }
            let _ = err_thread.join();
            let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            let _ = out_tx.blocking_send(ExecOutput::Exit(code));
        });
        Ok(ExecHandle { input: in_tx, output: out_rx })
    }
}
