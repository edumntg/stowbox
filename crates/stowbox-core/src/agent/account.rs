//! Which Claude Code login a box uses.
//!
//! boxd hands every non-isolated box in an org the same coding-agent
//! credentials, so two boxes sign in as the same Anthropic account. A box
//! can opt out: once `~/.stowbox/claude` exists in it, every Claude Code that
//! Stowbox starts there (chat turns and the agent's terminal) runs with
//! `CLAUDE_CONFIG_DIR` pointed at that directory and without the token
//! variables, so `/login` stores a login that belongs to that box alone. The
//! choice lives in the box and goes away with it.
//!
//! Sessions stay where they were: `projects/` in the box's own directory is a
//! link to `~/.claude/projects`, so an agent keeps resuming its conversation
//! after a switch in either direction.

use crate::boxd::shell_quote;
use crate::git::capture;
use crate::provider::Provider;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// The box's own Claude Code config directory, under `$HOME`.
pub const OWN_DIR: &str = ".stowbox/claude";

/// Variables that sign Claude Code in ahead of any stored login.
pub const TOKEN_VARS: [&str; 3] = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"];

pub const DEADLINE: Duration = Duration::from_secs(20);

/// Shell that goes in front of every Claude Code command Stowbox runs in a
/// box: switches to the box's own login when it has one.
pub fn prelude() -> String {
    format!(
        "if [ -d \"$HOME/{OWN_DIR}\" ]; then export CLAUDE_CONFIG_DIR=\"$HOME/{OWN_DIR}\"; unset {}; fi;",
        TOKEN_VARS.join(" ")
    )
}

/// Claude Code's own sign-in, for a terminal in the box.
pub fn login_command() -> String {
    format!("{} exec claude /login", prelude())
}

/// The login a box's Claude Code would use, as far as the box shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAccount {
    /// The box has its own login directory (see the module docs).
    pub own: bool,
    /// What signs Claude Code in: a token variable's name, or "stored login"
    /// for a `/login` kept on disk. `None` when nothing was found, which for
    /// the org login can also mean boxd provides it in a way the box does
    /// not show.
    pub source: Option<String>,
    /// The signed-in account's email and organization, when Claude Code
    /// recorded them.
    pub email: Option<String>,
    pub organization: Option<String>,
}

fn marker(nonce: &str) -> String {
    format!("::stowbox-{nonce}::")
}

pub fn status_script(nonce: &str) -> String {
    let m = marker(nonce);
    let mut s = format!("own=\"$HOME/{OWN_DIR}\"\n");
    s.push_str("if [ -d \"$own\" ]; then\n");
    s.push_str(&format!("  printf '{m} own\\n'; dir=\"$own\"; json=\"$own/.claude.json\"\n"));
    s.push_str("else\n");
    s.push_str(&format!("  printf '{m} org\\n'\n"));
    for v in TOKEN_VARS {
        s.push_str(&format!("  [ -n \"${{{v}:-}}\" ] && printf '{m} env {v}\\n'\n"));
    }
    s.push_str("  if [ -n \"${CLAUDE_CONFIG_DIR:-}\" ]; then dir=\"$CLAUDE_CONFIG_DIR\"; json=\"$dir/.claude.json\"; else dir=\"$HOME/.claude\"; json=\"$HOME/.claude.json\"; fi\n");
    s.push_str("fi\n");
    s.push_str(&format!("[ -s \"$dir/.credentials.json\" ] && printf '{m} file\\n'\n"));
    s.push_str(&format!("printf '{m} json\\n'; cat \"$json\" 2>/dev/null\n"));
    s
}

/// Makes the box's own login directory (`own`) or removes it, which also
/// removes the login stored in it.
pub fn set_script(own: bool) -> String {
    let dir = format!("\"$HOME\"/{}", shell_quote(OWN_DIR));
    if !own {
        return format!("rm -rf -- {dir}\n");
    }
    let mut s = String::from("set -e\n");
    s.push_str("mkdir -p \"$HOME/.claude/projects\"\n");
    s.push_str(&format!("mkdir -p {dir} && chmod 700 {dir}\n"));
    s.push_str(&format!("[ -e {dir}/projects ] || ln -s \"$HOME/.claude/projects\" {dir}/projects\n"));
    // Settings and permissions carry over; the login does not.
    s.push_str(&format!("[ -e {dir}/settings.json ] || [ ! -f \"$HOME/.claude/settings.json\" ] || cp \"$HOME/.claude/settings.json\" {dir}/\n"));
    s
}

pub fn parse_status(out: &[u8], nonce: &str) -> Option<ClaudeAccount> {
    let text = String::from_utf8_lossy(out);
    let m = marker(nonce);
    let mut a = ClaudeAccount::default();
    let mut seen = false;
    let mut json = String::new();
    let mut in_json = false;
    for line in text.lines() {
        if in_json {
            json.push_str(line);
            json.push('\n');
            continue;
        }
        let Some(rest) = line.strip_prefix(&m) else { continue };
        match rest.trim() {
            "own" => {
                a.own = true;
                seen = true;
            }
            "org" => seen = true,
            "file" => {
                a.source.get_or_insert_with(|| "stored login".into());
            }
            "json" => in_json = true,
            other => {
                if let Some(var) = other.strip_prefix("env ") {
                    a.source.get_or_insert_with(|| var.to_string());
                }
            }
        }
    }
    if !seen {
        return None;
    }
    // An email left in the config after a logout says nothing.
    if a.source.as_deref() == Some("stored login") {
        if let Ok(v) = serde_json::from_str::<Value>(&json) {
            let field = |k: &str| v.pointer(&format!("/oauthAccount/{k}")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            a.email = field("emailAddress");
            a.organization = field("organizationName");
        }
    }
    Some(a)
}

pub async fn status<P: Provider + ?Sized>(p: &P, machine_id: &str) -> Result<ClaudeAccount> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let c = capture(p, machine_id, status_script(&nonce), None, DEADLINE, "reading the Claude Code login").await?;
    parse_status(&c.stdout, &nonce).ok_or_else(|| crate::git::script_failed(&c, "reading the Claude Code login"))
}

pub async fn set<P: Provider + ?Sized>(p: &P, machine_id: &str, own: bool) -> Result<ClaudeAccount> {
    let c = capture(p, machine_id, set_script(own), None, DEADLINE, "switching the Claude Code login").await?;
    if c.code != 0 {
        return Err(crate::git::script_failed(&c, "switching the Claude Code login"));
    }
    status(p, machine_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{has_bash, temp_dir, LocalBash};

    #[test]
    fn prelude_switches_only_when_the_box_has_its_own_login() {
        let p = prelude();
        assert!(p.starts_with("if [ -d \"$HOME/.stowbox/claude\" ]; then export CLAUDE_CONFIG_DIR="));
        assert!(p.contains("unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN CLAUDE_CODE_OAUTH_TOKEN"));
    }

    #[test]
    fn status_ignores_noise_and_reads_the_account() {
        let m = marker("n");
        let out = format!("motd\n{m} own\n{m} file\n{m} json\n{{\"oauthAccount\":{{\"emailAddress\":\"me@work.dev\",\"organizationName\":\"Work\"}}}}\n");
        let a = parse_status(out.as_bytes(), "n").unwrap();
        assert_eq!(a, ClaudeAccount { own: true, source: Some("stored login".into()), email: Some("me@work.dev".into()), organization: Some("Work".into()) });
        assert_eq!(parse_status(b"nothing", "n"), None);
        // A token wins over a file, and no email is claimed for it.
        let out = format!("{m} org\n{m} env CLAUDE_CODE_OAUTH_TOKEN\n{m} file\n{m} json\n{{\"oauthAccount\":{{\"emailAddress\":\"x@y\"}}}}\n");
        let a = parse_status(out.as_bytes(), "n").unwrap();
        assert_eq!(a.source.as_deref(), Some("CLAUDE_CODE_OAUTH_TOKEN"));
        assert_eq!(a.email, None);
    }

    #[tokio::test]
    #[cfg_attr(not(unix), ignore = "box-side script, needs a Linux box's bash; see testing.rs")]
    async fn switching_keeps_sessions_and_hides_the_org_login() {
        if !has_bash() {
            return;
        }
        let home = temp_dir("stowbox-account");
        std::fs::create_dir_all(home.join(".claude/projects/-work")).unwrap();
        std::fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
        std::fs::write(home.join(".claude/projects/-work/s1.jsonl"), "{}\n").unwrap();
        std::fs::write(home.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"org@corp.dev"}}"#).unwrap();
        let p = LocalBash { home: home.clone(), env: vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), String::new())] };

        let a = status(&p, "vm").await.unwrap();
        assert_eq!(a, ClaudeAccount { own: false, source: Some("stored login".into()), email: Some("org@corp.dev".into()), organization: None });

        let a = set(&p, "vm", true).await.unwrap();
        assert_eq!(a, ClaudeAccount { own: true, ..Default::default() });
        assert!(home.join(".stowbox/claude/projects/-work/s1.jsonl").exists());
        // What a turn would see.
        let c = capture(&p, "vm", format!("CLAUDE_CODE_OAUTH_TOKEN=t; export CLAUDE_CODE_OAUTH_TOKEN; {} echo \"$CLAUDE_CONFIG_DIR|${{CLAUDE_CODE_OAUTH_TOKEN:-}}\"", prelude()), None, DEADLINE, "t")
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&c.stdout).trim(), format!("{}/.stowbox/claude|", home.display()));

        std::fs::write(home.join(".stowbox/claude/.credentials.json"), "{}").unwrap();
        assert_eq!(status(&p, "vm").await.unwrap().source.as_deref(), Some("stored login"));

        let a = set(&p, "vm", false).await.unwrap();
        assert!(!a.own);
        assert!(!home.join(".stowbox/claude").exists());
        assert!(home.join(".claude/projects/-work/s1.jsonl").exists(), "the sessions survive going back");
    }
}
