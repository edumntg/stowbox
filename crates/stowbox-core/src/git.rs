//! Review what an agent changed in its repository, then commit, push and
//! open a pull request, all inside the box.
//!
//! Every operation is one script over the provider's exec stream (a login
//! shell, so the secrets from `~/.config/stowbox/env.sh` such as `GH_TOKEN`
//! are exported). Nothing can wait for a person: prompts are switched off
//! (`GIT_TERMINAL_PROMPT=0`, ssh in batch mode, `GH_PROMPT_DISABLED=1`),
//! the editor is `true`, stdin is closed or carries the commit message, and
//! each call has a deadline after which the exec stream is dropped, which
//! kills the remote process group.
//!
//! The review script prints its sections behind a per-call marker, so any
//! text a login shell prints first (a motd, an rc file's echo) is skipped.
//! The status and the diff are cut inside the box (`head -c`) so a huge
//! diff never crosses the wire whole; the review says when it was cut.

use crate::boxd::shell_quote;
use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Bytes of `git status -z` output kept; the rest is reported as cut.
pub const STATUS_CAP: usize = 256 * 1024;
/// Bytes of patch text kept, tracked changes first, then untracked files.
pub const DIFF_CAP: usize = 512 * 1024;

pub const REVIEW_DEADLINE: Duration = Duration::from_secs(45);
pub const COMMIT_DEADLINE: Duration = Duration::from_secs(120);
pub const PUSH_DEADLINE: Duration = Duration::from_secs(120);
pub const PR_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepoState {
    Ok,
    /// The directory does not exist in the box.
    NoDir,
    NotRepo,
    /// `git` is not installed in the box.
    NoGit,
}

/// One entry of `git status --porcelain=v2`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    /// The old path of a rename or copy.
    pub orig_path: Option<String>,
    /// One letter for the list: M, A, D, R, C, T, U (conflict) or ? (untracked).
    pub code: String,
    /// Some of the change is already in the index.
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub old_path: Option<String>,
    /// "modified", "added", "deleted" or "renamed".
    pub change: String,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    /// The hunks, from the first `@@` line on. Empty for binary files and
    /// pure renames.
    pub patch: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    /// `None` when HEAD is detached.
    pub head: Option<String>,
    /// `None` before the first commit.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReview {
    pub state: RepoState,
    /// The directory that was asked for, as given.
    pub cwd: String,
    /// The repository's top level; status and diff cover all of it.
    pub root: String,
    pub branch: Branch,
    pub remote: Option<String>,
    /// Without any user or token part.
    pub remote_url: Option<String>,
    /// The remote's default branch ("main"), when the clone recorded it.
    pub base: Option<String>,
    /// `gh` is installed in the box.
    pub gh: bool,
    /// `GH_TOKEN` or `GITHUB_TOKEN` is set in a login shell.
    pub token: bool,
    pub files: Vec<FileChange>,
    pub files_truncated: bool,
    pub diff: Vec<DiffFile>,
    pub diff_truncated: bool,
    pub sampled_at_ms: i64,
}

impl GitReview {
    fn empty(state: RepoState, cwd: &str, now_ms: i64) -> Self {
        Self {
            state,
            cwd: cwd.into(),
            root: String::new(),
            branch: Branch::default(),
            remote: None,
            remote_url: None,
            base: None,
            gh: false,
            token: false,
            files: vec![],
            files_truncated: false,
            diff: vec![],
            diff_truncated: false,
            sampled_at_ms: now_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrVia {
    /// `gh pr create --fill` ran in the box.
    Gh,
    /// No usable `gh`: the host's compare page, to finish in the browser.
    Compare,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrOutcome {
    pub url: String,
    pub via: PrVia,
    /// `gh` found a pull request for this branch already open.
    pub existing: bool,
}

fn marker(nonce: &str) -> String {
    format!("::stowbox-{nonce}::")
}

/// The directory as a shell word. `~` and `~/…` are expanded in the box,
/// everything else is quoted literally. No directory means `$HOME`, which
/// is where an agent without a cwd runs.
pub fn cwd_word(cwd: Option<&str>) -> String {
    match cwd.map(str::trim).filter(|c| !c.is_empty()) {
        None | Some("~") => "\"$HOME\"".into(),
        Some(c) => match c.strip_prefix("~/") {
            Some(rest) if !rest.is_empty() => format!("\"$HOME\"/{}", shell_quote(rest)),
            Some(_) => "\"$HOME\"".into(),
            None => shell_quote(c),
        },
    }
}

/// Settings every script starts with: no prompts, no editor, no pager, and
/// the marker function. Then it enters the repository's top level or says
/// why it cannot.
pub(crate) fn prelude(cwd: Option<&str>, nonce: &str) -> String {
    let mark = marker(nonce);
    format!(
        concat!(
            "export GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never GIT_EDITOR=true GIT_SEQUENCE_EDITOR=true GIT_PAGER=cat PAGER=cat LC_ALL=C GIT_OPTIONAL_LOCKS=0\n",
            "stowbox_mark() {{ printf '\\n{mark} %s\\n' \"$1\"; }}\n",
            "command -v git >/dev/null 2>&1 || {{ stowbox_mark nogit; exit 0; }}\n",
            "cd -- {cwd} 2>/dev/null || {{ stowbox_mark nodir; exit 0; }}\n",
            "stowbox_top=$(git rev-parse --show-toplevel 2>&1) || {{ stowbox_mark norepo; printf '%s' \"$stowbox_top\"; exit 0; }}\n",
            // Where the directory sits inside the repository ("web/" or empty).
            "stowbox_prefix=$(git rev-parse --show-prefix 2>/dev/null)\n",
            "cd -- \"$stowbox_top\" || exit 1\n",
            // Batch-mode ssh by default, but never over the person's own
            // choice: an exported GIT_SSH_COMMAND or the repository's (or
            // global) core.sshCommand wins. Their ssh still cannot hang on a
            // question: stdin is closed (or carries only the commit message)
            // and GIT_TERMINAL_PROMPT=0, and the call has a deadline.
            "if [ -z \"${{GIT_SSH_COMMAND:-}}\" ] && [ -z \"$(git config --get core.sshCommand)\" ]; then export GIT_SSH_COMMAND='ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new'; fi\n",
            "stowbox_mark top; printf '%s' \"$stowbox_top\"\n",
            // The branch, the remote it tracks (or origin, or the only one)
            // and that remote's default branch.
            "stowbox_head=$(git symbolic-ref -q --short HEAD)\n",
            "stowbox_remote=\n",
            "[ -n \"$stowbox_head\" ] && stowbox_remote=$(git config \"branch.$stowbox_head.remote\")\n",
            "[ -n \"$stowbox_remote\" ] || {{ if git remote | grep -qx origin; then stowbox_remote=origin; else stowbox_remote=$(git remote | head -n1); fi; }}\n",
            "stowbox_mark head; printf '%s' \"$stowbox_head\"\n",
            "stowbox_mark remote; [ -n \"$stowbox_remote\" ] && printf '%s\\t%s' \"$stowbox_remote\" \"$(git remote get-url \"$stowbox_remote\" 2>/dev/null)\"\n",
            "stowbox_mark base; [ -n \"$stowbox_remote\" ] && git symbolic-ref -q --short \"refs/remotes/$stowbox_remote/HEAD\"\n",
        ),
        mark = mark,
        cwd = cwd_word(cwd),
    )
}

/// Status, branch and the full patch of the working tree against HEAD,
/// with untracked files shown as new files.
pub fn review_script(cwd: Option<&str>, nonce: &str) -> String {
    let mut s = prelude(cwd, nonce);
    s.push_str(&format!(
        concat!(
            "stowbox_mark tools; command -v gh >/dev/null 2>&1 && printf 'gh '; [ -n \"${{GH_TOKEN:-}}${{GITHUB_TOKEN:-}}\" ] && printf 'token'\n",
            "stowbox_mark status\n",
            "git status --porcelain=v2 -z --branch --untracked-files=all | head -c {status_cap}\n",
            "stowbox_mark diff\n",
            // Before the first commit, compare with the empty tree.
            "if git rev-parse -q --verify HEAD >/dev/null; then stowbox_base=HEAD; else stowbox_base=$(git hash-object -t tree /dev/null); fi\n",
            "{{ git diff --no-color --no-ext-diff --no-textconv -M \"$stowbox_base\" --\n",
            "  git ls-files -z --others --exclude-standard | while IFS= read -r -d '' f; do\n",
            "    git diff --no-color --no-ext-diff --no-index -- /dev/null \"$f\"\n",
            // 141 is SIGPIPE: head has what it needs, stop walking files.
            "    [ $? -ge 128 ] && break\n",
            "  done\n",
            "}} 2>/dev/null | head -c {diff_cap}\n",
            "stowbox_mark end\n",
        ),
        status_cap = STATUS_CAP + 1,
        diff_cap = DIFF_CAP + 1,
    ));
    s
}

/// Stages everything and commits with the message read from stdin, so the
/// message never has to survive shell quoting. Hooks run as usual.
pub fn commit_script(cwd: Option<&str>, nonce: &str) -> String {
    let mut s = prelude(cwd, nonce);
    s.push_str("stowbox_mark run\n");
    s.push_str("git add -A 2>&1 || exit $?\n");
    s.push_str("git commit -F - 2>&1\n");
    s
}

/// The `-c` options that let git use a GitHub token from the environment
/// for https://github.com remotes, so a preset's `GH_TOKEN` is enough to
/// push. They reset other helpers for that host only and only when a token
/// is set; the token is read inside the helper, never put on a command line.
const TOKEN_HELPER: &str = concat!(
    "stowbox_auth=()\n",
    "if [ -n \"${GH_TOKEN:-}${GITHUB_TOKEN:-}\" ]; then\n",
    "  stowbox_auth=(-c 'credential.https://github.com.helper=' -c 'credential.https://github.com.helper=!f() { test \"$1\" = get || return 0; echo username=x-access-token; echo \"password=${GH_TOKEN:-$GITHUB_TOKEN}\"; }; f')\n",
    "fi\n",
);

/// Pushes the current branch. Without an upstream it pushes to the remote
/// picked in the prelude and sets the upstream (`push -u <remote> HEAD`).
pub fn push_script(cwd: Option<&str>, nonce: &str) -> String {
    let mut s = prelude(cwd, nonce);
    s.push_str("stowbox_mark run\n");
    s.push_str(TOKEN_HELPER);
    s.push_str(concat!(
        "exec 2>&1 </dev/null\n",
        "[ -n \"$stowbox_head\" ] || { echo 'HEAD is detached: check out a branch to push it'; exit 2; }\n",
        "if git rev-parse -q --verify '@{upstream}' >/dev/null 2>&1; then\n",
        "  git \"${stowbox_auth[@]}\" push\n",
        "else\n",
        "  [ -n \"$stowbox_remote\" ] || { echo 'this repository has no remote: add one with git remote add'; exit 2; }\n",
        "  git \"${stowbox_auth[@]}\" push -u \"$stowbox_remote\" HEAD\n",
        "fi\n",
    ));
    s
}

/// Runs `gh pr create --fill` when `gh` is installed and signed in (a token
/// in the environment counts) and the remote is on GitHub; otherwise only
/// reports the remote so the caller can build the compare URL.
pub fn pr_script(cwd: Option<&str>, nonce: &str) -> String {
    let mut s = prelude(cwd, nonce);
    s.push_str(concat!(
        "export GH_PROMPT_DISABLED=1 GH_NO_UPDATE_NOTIFIER=1 GH_SPINNER_DISABLED=1 NO_COLOR=1\n",
        "stowbox_url=$(git remote get-url \"$stowbox_remote\" 2>/dev/null)\n",
        "case \"$stowbox_url\" in *github*) stowbox_gh_host=1 ;; *) stowbox_gh_host= ;; esac\n",
        "if [ -n \"$stowbox_gh_host\" ] && command -v gh >/dev/null 2>&1 && { [ -n \"${GH_TOKEN:-}${GITHUB_TOKEN:-}\" ] || timeout 20 gh auth status >/dev/null 2>&1; }; then\n",
        "  stowbox_mark gh\n",
        "  timeout 90 gh pr create --fill </dev/null 2>&1\n",
        "  stowbox_mark \"exit $?\"\n",
        "else\n",
        "  stowbox_mark nogh\n",
        "fi\n",
    ));
    s
}

/// Splits script output into `(name, bytes)` sections. Anything before the
/// first marker (login shell noise) is dropped.
pub fn sections(out: &[u8], nonce: &str) -> Vec<(String, Vec<u8>)> {
    let needle = format!("\n{} ", marker(nonce));
    let needle = needle.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0;
    while i + needle.len() <= out.len() {
        if &out[i..i + needle.len()] == needle {
            starts.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    let mut res = Vec::new();
    for (k, &at) in starts.iter().enumerate() {
        let name_from = at + needle.len();
        let Some(nl) = out[name_from..].iter().position(|&b| b == b'\n') else {
            // A marker cut off mid-line: the script was killed.
            let name = String::from_utf8_lossy(&out[name_from..]).trim().to_string();
            res.push((name, Vec::new()));
            break;
        };
        let name = String::from_utf8_lossy(&out[name_from..name_from + nl]).trim().to_string();
        let body_from = name_from + nl + 1;
        let body_to = starts.get(k + 1).copied().unwrap_or(out.len()).max(body_from);
        res.push((name, out[body_from..body_to].to_vec()));
    }
    res
}

pub(crate) fn section<'a>(secs: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a [u8]> {
    secs.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_slice())
}

pub(crate) fn text(b: Option<&[u8]>) -> String {
    b.map(|b| String::from_utf8_lossy(b).trim().to_string()).unwrap_or_default()
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Parses `git status --porcelain=v2 -z --branch`. `truncated` drops the
/// last entry, which may have been cut in half.
pub fn parse_status(raw: &[u8], truncated: bool) -> (Branch, Vec<FileChange>) {
    let mut branch = Branch::default();
    let mut files = Vec::new();
    let mut entries: Vec<String> = raw.split(|&b| b == 0).map(|e| String::from_utf8_lossy(e).into_owned()).collect();
    // `-z` ends every entry with NUL, so the last split is empty or cut.
    let last = entries.pop();
    if !truncated {
        if let Some(l) = last.filter(|l| !l.is_empty()) {
            entries.push(l);
        }
    }
    let mut it = entries.into_iter();
    while let Some(e) = it.next() {
        if let Some(h) = e.strip_prefix("# ") {
            let (key, val) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" => branch.oid = (val != "(initial)").then(|| val.to_string()),
                "branch.head" => branch.head = (val != "(detached)").then(|| val.to_string()),
                "branch.upstream" => branch.upstream = Some(val.to_string()),
                "branch.ab" => {
                    for part in val.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            branch.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            branch.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = e.chars().next().unwrap_or(' ');
        match kind {
            '1' | '2' => {
                let fields = if kind == '1' { 9 } else { 10 };
                let parts: Vec<&str> = e.splitn(fields, ' ').collect();
                if parts.len() < fields {
                    continue;
                }
                let xy: Vec<char> = parts[1].chars().collect();
                let (x, y) = (xy.first().copied().unwrap_or('.'), xy.get(1).copied().unwrap_or('.'));
                let orig_path = if kind == '2' { it.next() } else { None };
                let code = if kind == '2' {
                    x
                } else if y != '.' {
                    y
                } else {
                    x
                };
                files.push(FileChange { path: parts[fields - 1].to_string(), orig_path, code: code.to_string(), staged: x != '.' });
            }
            'u' => {
                let parts: Vec<&str> = e.splitn(11, ' ').collect();
                if let Some(p) = parts.get(10) {
                    files.push(FileChange { path: p.to_string(), orig_path: None, code: "U".into(), staged: false });
                }
            }
            '?' => {
                if let Some(p) = e.get(2..) {
                    files.push(FileChange { path: p.to_string(), orig_path: None, code: "?".into(), staged: false });
                }
            }
            _ => {}
        }
    }
    (branch, files)
}

/// `a/x b/x` from a `diff --git` header, when both sides are the same path
/// (the only case that is unambiguous with spaces in names).
fn header_path(rest: &str) -> Option<String> {
    let rest = rest.strip_prefix("a/")?;
    if rest.len() < 4 || (rest.len() - 3) % 2 != 0 {
        return None;
    }
    let half = (rest.len() - 3) / 2;
    let (a, b) = (rest.get(..half)?, rest.get(half..)?);
    (b.strip_prefix(" b/")? == a).then(|| a.to_string())
}

fn unquote(p: &str) -> String {
    let p = p.trim_end_matches('\t');
    p.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(p).to_string()
}

/// Splits a unified diff (as `git diff` prints it) into files with counts.
/// `truncated` drops a last line that may have been cut in half.
pub fn parse_diff(raw: &str, truncated: bool) -> Vec<DiffFile> {
    let mut lines: Vec<&str> = raw.split('\n').collect();
    if truncated || lines.last() == Some(&"") {
        lines.pop();
    }
    let mut files: Vec<DiffFile> = Vec::new();
    let mut in_hunk = false;
    for line in lines {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.push(DiffFile {
                path: header_path(rest).unwrap_or_else(|| rest.to_string()),
                old_path: None,
                change: "modified".into(),
                binary: false,
                additions: 0,
                deletions: 0,
                patch: String::new(),
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if line.starts_with("@@") {
            in_hunk = true;
        }
        if in_hunk {
            match line.as_bytes().first() {
                Some(b'+') => f.additions += 1,
                Some(b'-') => f.deletions += 1,
                _ => {}
            }
            f.patch.push_str(line);
            f.patch.push('\n');
            continue;
        }
        if line.starts_with("new file mode") {
            f.change = "added".into();
        } else if line.starts_with("deleted file mode") {
            f.change = "deleted".into();
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.change = "renamed".into();
            f.old_path = Some(unquote(p));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.path = unquote(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if p != "/dev/null" {
                let p = unquote(p);
                f.path = p.strip_prefix("b/").map(str::to_string).unwrap_or(p);
            }
        } else if let Some(p) = line.strip_prefix("--- ") {
            if p != "/dev/null" && f.change == "deleted" {
                let p = unquote(p);
                f.path = p.strip_prefix("a/").map(str::to_string).unwrap_or(p);
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
            if let Some(p) = line.strip_suffix(" differ").and_then(|l| l.rsplit_once(" and ")).map(|(_, b)| b) {
                if p != "/dev/null" {
                    let p = unquote(p);
                    f.path = p.strip_prefix("b/").map(str::to_string).unwrap_or(p);
                }
            }
        }
    }
    files
}

/// Drops the user and password from a remote URL before it is shown.
pub fn redact_remote(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/').map(|(a, p)| (a, Some(p))).unwrap_or((rest, None));
        if let Some((_, host)) = authority.rsplit_once('@') {
            if scheme.starts_with("http") {
                return match path {
                    Some(p) => format!("{scheme}://{host}/{p}"),
                    None => format!("{scheme}://{host}"),
                };
            }
        }
    }
    url.to_string()
}

/// `(host, owner/repo)` from an https, ssh, git or scp-style remote URL.
pub fn remote_web(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host);
        (host.to_string(), path.to_string())
    } else {
        // scp-like: [user@]host:owner/repo
        let (left, path) = url.split_once(':')?;
        if left.contains('/') {
            return None;
        }
        let host = left.rsplit_once('@').map(|(_, h)| h).unwrap_or(left);
        (host.to_string(), path.to_string())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    if host.is_empty() || path.is_empty() || !path.contains('/') {
        return None;
    }
    let host = if host == "ssh.github.com" { "github.com".to_string() } else { host.to_lowercase() };
    Some((host, path.to_string()))
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The page that starts a pull (or merge) request from `branch` into
/// `base` on GitHub, GitLab or Bitbucket. `None` for other hosts.
pub fn compare_url(remote_url: &str, base: Option<&str>, branch: &str) -> Option<String> {
    let (host, path) = remote_web(remote_url)?;
    let b = encode(branch);
    if host.contains("github") {
        return Some(match base {
            Some(base) => format!("https://{host}/{path}/compare/{}...{b}?expand=1", encode(base)),
            None => format!("https://{host}/{path}/compare/{b}?expand=1"),
        });
    }
    if host.contains("gitlab") {
        let mut u = format!("https://{host}/{path}/-/merge_requests/new?merge_request%5Bsource_branch%5D={b}");
        if let Some(base) = base {
            u.push_str(&format!("&merge_request%5Btarget_branch%5D={}", encode(base)));
        }
        return Some(u);
    }
    if host == "bitbucket.org" {
        let mut u = format!("https://{host}/{path}/pull-requests/new?source={b}");
        if let Some(base) = base {
            u.push_str(&format!("&dest={}", encode(base)));
        }
        return Some(u);
    }
    None
}

/// The last https URL `gh` printed (it prints the new pull request's URL,
/// or names the existing one in its error).
pub fn last_url(output: &str) -> Option<String> {
    output.split_whitespace().filter(|w| w.starts_with("https://")).last().map(|w| w.trim_end_matches(['.', ',', ')', '"', '\'']).to_string())
}

/// What one script run left behind.
#[derive(Debug, Default)]
pub struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
}

/// Runs `script` in the box, feeds `stdin` and closes it, and collects the
/// output until the process exits or `deadline` passes. On timeout the
/// stream is dropped, which kills the remote process group.
pub async fn capture<P: Provider + ?Sized>(
    p: &P,
    machine_id: &str,
    script: String,
    stdin: Option<Vec<u8>>,
    deadline: Duration,
    what: &str,
) -> Result<Captured> {
    let mut h = p.exec(machine_id, ExecSpec { script, tty: false, cols: 0, rows: 0, env: vec![] }).await?;
    if let Some(data) = stdin {
        let _ = h.input.send(ExecInput::Data(data)).await;
    }
    let _ = h.input.send(ExecInput::Close).await;
    let end = Instant::now() + deadline;
    let mut c = Captured::default();
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let timed_out = || Error::Provider(format!("{what} did not finish within {} s, so it was stopped", deadline.as_secs()));
        if left.is_zero() {
            return Err(timed_out());
        }
        match tokio::time::timeout(left, h.output.recv()).await {
            Ok(Some(ExecOutput::Stdout(b))) => c.stdout.extend_from_slice(&b),
            Ok(Some(ExecOutput::Stderr(b))) => c.stderr.extend_from_slice(&b),
            Ok(Some(ExecOutput::Exit(code))) => {
                c.code = code;
                return Ok(c);
            }
            Ok(None) => return Ok(c),
            Ok(Some(ExecOutput::Error(e))) => return Err(Error::Provider(e)),
            Err(_) => return Err(timed_out()),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Where the prelude stopped, if it did. `Err` carries git's own words when
/// the directory exists but git refuses it for another reason (for example
/// "detected dubious ownership").
pub(crate) fn early_state(secs: &[(String, Vec<u8>)]) -> Result<Option<RepoState>> {
    if section(secs, "nogit").is_some() {
        return Ok(Some(RepoState::NoGit));
    }
    if section(secs, "nodir").is_some() {
        return Ok(Some(RepoState::NoDir));
    }
    if let Some(msg) = section(secs, "norepo") {
        let msg = String::from_utf8_lossy(msg).trim().to_string();
        if msg.is_empty() || msg.contains("not a git repository") {
            return Ok(Some(RepoState::NotRepo));
        }
        return Err(Error::Provider(msg));
    }
    Ok(None)
}

pub(crate) fn script_failed(c: &Captured, what: &str) -> Error {
    let err = String::from_utf8_lossy(&c.stderr).trim().to_string();
    if err.is_empty() {
        Error::Provider(format!("{what} exited with code {} and printed nothing", c.code))
    } else {
        Error::Provider(err)
    }
}

/// Builds the review from the review script's output.
pub fn parse_review(out: &[u8], nonce: &str, cwd: &str, now_ms: i64) -> Result<Option<GitReview>> {
    let secs = sections(out, nonce);
    if let Some(state) = early_state(&secs)? {
        return Ok(Some(GitReview::empty(state, cwd, now_ms)));
    }
    if section(&secs, "end").is_none() {
        return Ok(None);
    }
    let mut r = GitReview::empty(RepoState::Ok, cwd, now_ms);
    r.root = text(section(&secs, "top"));
    let remote = text(section(&secs, "remote"));
    if let Some((name, url)) = remote.split_once('\t') {
        r.remote = non_empty(name.to_string());
        r.remote_url = non_empty(redact_remote(url));
    }
    let remote_name = r.remote.clone().unwrap_or_default();
    r.base = non_empty(text(section(&secs, "base"))).map(|b| b.strip_prefix(&format!("{remote_name}/")).map(str::to_string).unwrap_or(b));
    let tools = text(section(&secs, "tools"));
    r.gh = tools.split_whitespace().any(|t| t == "gh");
    r.token = tools.split_whitespace().any(|t| t == "token");

    let status = section(&secs, "status").unwrap_or_default();
    r.files_truncated = status.len() > STATUS_CAP;
    let (branch, files) = parse_status(&status[..status.len().min(STATUS_CAP)], r.files_truncated);
    r.branch = branch;
    r.files = files;

    let diff = section(&secs, "diff").unwrap_or_default();
    r.diff_truncated = diff.len() > DIFF_CAP;
    let diff = String::from_utf8_lossy(&diff[..diff.len().min(DIFF_CAP)]);
    r.diff = parse_diff(&diff, r.diff_truncated);
    Ok(Some(r))
}

/// Reads the repository that contains `cwd` in the box.
pub async fn review<P: Provider + ?Sized>(p: &P, machine_id: &str, cwd: Option<&str>) -> Result<GitReview> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let c = capture(p, machine_id, review_script(cwd, &nonce), None, REVIEW_DEADLINE, "reading the repository").await?;
    match parse_review(&c.stdout, &nonce, cwd.unwrap_or("~"), now_ms())? {
        Some(r) => Ok(r),
        None => Err(script_failed(&c, "the review script")),
    }
}

fn require_repo(secs: &[(String, Vec<u8>)]) -> Result<()> {
    match early_state(secs)? {
        None => Ok(()),
        Some(RepoState::NoGit) => Err(Error::Provider("git is not installed in the box".into())),
        Some(RepoState::NoDir) => Err(Error::Provider("the directory does not exist in the box".into())),
        Some(_) => Err(Error::Provider("the directory is not inside a git repository".into())),
    }
}

/// The text after the `run` marker, which the action scripts send to stdout
/// with stderr folded in.
fn run_output(secs: &[(String, Vec<u8>)]) -> String {
    text(section(secs, "run"))
}

/// Stages every change (`git add -A`) and commits it with `message`.
/// Returns git's summary; a refusal comes back as an error with git's words.
pub async fn commit<P: Provider + ?Sized>(p: &P, machine_id: &str, cwd: Option<&str>, message: &str) -> Result<String> {
    let message = message.trim();
    if message.is_empty() {
        return Err(Error::Provider("write a commit message".into()));
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut msg = message.to_string();
    msg.push('\n');
    let c = capture(p, machine_id, commit_script(cwd, &nonce), Some(msg.into_bytes()), COMMIT_DEADLINE, "git commit").await?;
    let secs = sections(&c.stdout, &nonce);
    require_repo(&secs)?;
    let out = run_output(&secs);
    if c.code != 0 {
        return Err(Error::Provider(if out.is_empty() { script_failed(&c, "git commit").to_string() } else { out }));
    }
    Ok(out)
}

/// Pushes the current branch, setting the upstream on the first push. A
/// rejected push comes back as an error carrying git's message verbatim.
pub async fn push<P: Provider + ?Sized>(p: &P, machine_id: &str, cwd: Option<&str>) -> Result<String> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let c = capture(p, machine_id, push_script(cwd, &nonce), None, PUSH_DEADLINE, "git push").await?;
    let secs = sections(&c.stdout, &nonce);
    require_repo(&secs)?;
    let out = run_output(&secs);
    if c.code != 0 {
        return Err(Error::Provider(if out.is_empty() { script_failed(&c, "git push").to_string() } else { out }));
    }
    Ok(out)
}

/// Opens a pull request for the current branch: with `gh pr create --fill`
/// in the box when it can, otherwise by handing back the host's compare
/// page. Errors carry `gh`'s message verbatim.
pub async fn open_pr<P: Provider + ?Sized>(p: &P, machine_id: &str, cwd: Option<&str>) -> Result<PrOutcome> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let c = capture(p, machine_id, pr_script(cwd, &nonce), None, PR_DEADLINE, "opening the pull request").await?;
    let secs = sections(&c.stdout, &nonce);
    require_repo(&secs)?;
    pr_outcome(&secs)
}

fn pr_outcome(secs: &[(String, Vec<u8>)]) -> Result<PrOutcome> {
    let head = text(section(secs, "head"));
    if head.is_empty() {
        return Err(Error::Provider("HEAD is detached: check out a branch first".into()));
    }
    if let Some(out) = section(secs, "gh") {
        let out = String::from_utf8_lossy(out).trim().to_string();
        let code: i32 = secs.iter().find_map(|(n, _)| n.strip_prefix("exit ").and_then(|c| c.trim().parse().ok())).unwrap_or(-1);
        if code == 0 {
            if let Some(url) = last_url(&out) {
                return Ok(PrOutcome { url, via: PrVia::Gh, existing: false });
            }
        }
        if out.contains("already exists") {
            if let Some(url) = last_url(&out) {
                return Ok(PrOutcome { url, via: PrVia::Gh, existing: true });
            }
        }
        return Err(Error::Provider(if out.is_empty() { format!("gh pr create exited with code {code}") } else { out }));
    }
    let remote = text(section(secs, "remote"));
    let url = remote.split_once('\t').map(|(_, u)| u.trim().to_string()).unwrap_or_default();
    if url.is_empty() {
        return Err(Error::Provider("this repository has no remote to open a pull request on".into()));
    }
    let remote_name = remote.split_once('\t').map(|(n, _)| n).unwrap_or("");
    let base = non_empty(text(section(secs, "base"))).map(|b| b.strip_prefix(&format!("{remote_name}/")).map(str::to_string).unwrap_or(b));
    match compare_url(&url, base.as_deref(), &head) {
        Some(url) => Ok(PrOutcome { url, via: PrVia::Compare, existing: false }),
        None => Err(Error::Provider(format!(
            "gh is not available here and Stowbox cannot build a pull request link for {}; open it on the host",
            redact_remote(&url)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{fixture, has_bash, has_git, sh, temp_dir, LocalBash};

    #[test]
    fn cwd_words() {
        assert_eq!(cwd_word(None), "\"$HOME\"");
        assert_eq!(cwd_word(Some(" ")), "\"$HOME\"");
        assert_eq!(cwd_word(Some("~")), "\"$HOME\"");
        assert_eq!(cwd_word(Some("~/my app")), "\"$HOME\"/'my app'");
        assert_eq!(cwd_word(Some("/home/user/api")), "/home/user/api");
        assert_eq!(cwd_word(Some("/tmp/it's")), "'/tmp/it'\\''s'");
    }

    #[test]
    fn sections_skip_noise_and_keep_binary() {
        let out = b"Welcome to the box\n\n::stowbox-n:: top\n/repo\n::stowbox-n:: status\na\0b c\0\n::stowbox-n:: end\n";
        let s = sections(out, "n");
        assert_eq!(s.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec!["top", "status", "end"]);
        assert_eq!(s[0].1, b"/repo");
        assert_eq!(s[1].1, b"a\0b c\0");
        // Another nonce is just text.
        assert!(sections(out, "x").is_empty());
    }

    #[test]
    fn status_parses_branch_and_entries() {
        let raw = concat!(
            "# branch.oid 1234abcd\0# branch.head feature/x\0# branch.upstream origin/feature/x\0# branch.ab +2 -1\0",
            "1 .M N... 100644 100644 100644 aaa aaa src/app.ts\0",
            "1 A. N... 000000 100644 100644 000 bbb new file.md\0",
            "2 R. N... 100644 100644 100644 ccc ccc R100 docs/new name.md\0docs/old.md\0",
            "u UU N... 100644 100644 100644 100644 d1 d2 d3 conflict.rs\0",
            "? notes/todo me.txt\0",
        );
        let (b, f) = parse_status(raw.as_bytes(), false);
        assert_eq!(b, Branch { head: Some("feature/x".into()), oid: Some("1234abcd".into()), upstream: Some("origin/feature/x".into()), ahead: 2, behind: 1 });
        let got: Vec<(&str, Option<&str>, &str, bool)> = f.iter().map(|x| (x.path.as_str(), x.orig_path.as_deref(), x.code.as_str(), x.staged)).collect();
        assert_eq!(
            got,
            vec![
                ("src/app.ts", None, "M", false),
                ("new file.md", None, "A", true),
                ("docs/new name.md", Some("docs/old.md"), "R", true),
                ("conflict.rs", None, "U", false),
                ("notes/todo me.txt", None, "?", false),
            ]
        );
    }

    #[test]
    fn status_initial_detached_and_cut() {
        let (b, f) = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0? a\0? b-cut", true);
        assert_eq!(b.oid, None);
        assert_eq!(b.head, None);
        assert_eq!(f.len(), 1, "the cut entry is dropped");
    }

    const DIFF: &str = "diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,3 +1,4 @@
 import x from \"x\";
-const a = 1;
+const a = 2;
+const b = 3;
diff --git a/old.txt b/old.txt
deleted file mode 100644
index 3333333..0000000
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-gone
diff --git a/a b.md b/c d.md
similarity index 100%
rename from a b.md
rename to c d.md
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000..4444444
Binary files /dev/null and b/logo.png differ
diff --git a/notes.md b/notes.md
new file mode 100644
index 0000000..5555555
--- /dev/null
+++ b/notes.md
@@ -0,0 +1,2 @@
+--- a heading that looks like a header
+second
";

    #[test]
    fn diff_splits_files_and_counts() {
        let f = parse_diff(DIFF, false);
        let got: Vec<(&str, Option<&str>, &str, bool, u32, u32)> =
            f.iter().map(|x| (x.path.as_str(), x.old_path.as_deref(), x.change.as_str(), x.binary, x.additions, x.deletions)).collect();
        assert_eq!(
            got,
            vec![
                ("src/app.ts", None, "modified", false, 2, 1),
                ("old.txt", None, "deleted", false, 0, 1),
                ("c d.md", Some("a b.md"), "renamed", false, 0, 0),
                ("logo.png", None, "added", true, 0, 0),
                ("notes.md", None, "added", false, 2, 0),
            ]
        );
        assert!(f[0].patch.starts_with("@@ -1,3 +1,4 @@\n"));
        assert!(f[2].patch.is_empty());
    }

    #[test]
    fn diff_cut_drops_the_partial_line() {
        let f = parse_diff("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new line that was c", true);
        assert_eq!((f[0].additions, f[0].deletions), (0, 1));
    }

    #[test]
    fn header_path_handles_spaces() {
        assert_eq!(header_path("a/my file.txt b/my file.txt").as_deref(), Some("my file.txt"));
        assert_eq!(header_path("a/x b/y"), None);
    }

    #[test]
    fn remotes_and_compare_urls() {
        assert_eq!(remote_web("git@github.com:edumntg/stowbox.git"), Some(("github.com".into(), "edumntg/stowbox".into())));
        assert_eq!(remote_web("https://x-access-token:ghp_abc@github.com/o/r.git"), Some(("github.com".into(), "o/r".into())));
        assert_eq!(remote_web("ssh://git@ssh.github.com:443/o/r.git"), Some(("github.com".into(), "o/r".into())));
        assert_eq!(remote_web("https://gitlab.com/group/sub/proj"), Some(("gitlab.com".into(), "group/sub/proj".into())));
        assert_eq!(remote_web("/srv/git/r.git"), None);
        assert_eq!(remote_web("../r.git"), None);

        assert_eq!(
            compare_url("git@github.com:o/r.git", Some("main"), "fix/login bug").as_deref(),
            Some("https://github.com/o/r/compare/main...fix/login%20bug?expand=1")
        );
        assert_eq!(compare_url("https://github.com/o/r", None, "x").as_deref(), Some("https://github.com/o/r/compare/x?expand=1"));
        assert_eq!(
            compare_url("git@gitlab.com:g/p.git", Some("main"), "x").as_deref(),
            Some("https://gitlab.com/g/p/-/merge_requests/new?merge_request%5Bsource_branch%5D=x&merge_request%5Btarget_branch%5D=main")
        );
        assert_eq!(compare_url("git@bitbucket.org:o/r.git", None, "x").as_deref(), Some("https://bitbucket.org/o/r/pull-requests/new?source=x"));
        assert_eq!(compare_url("git@example.com:o/r.git", None, "x"), None);
    }

    #[test]
    fn remote_credentials_are_not_shown() {
        assert_eq!(redact_remote("https://user:ghp_secret@github.com/o/r.git"), "https://github.com/o/r.git");
        assert_eq!(redact_remote("git@github.com:o/r.git"), "git@github.com:o/r.git");
        assert_eq!(redact_remote("ssh://git@host/o/r"), "ssh://git@host/o/r");
    }

    #[test]
    fn gh_output_urls() {
        assert_eq!(last_url("Creating pull request for x into main\n\nhttps://github.com/o/r/pull/7\n").as_deref(), Some("https://github.com/o/r/pull/7"));
        assert_eq!(
            last_url("a pull request for branch \"x\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/3").as_deref(),
            Some("https://github.com/o/r/pull/3")
        );
        assert_eq!(last_url("no url"), None);
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "box-side script, needs a Linux box's bash; see testing.rs")]
    fn token_helper_answers_for_github_only() {
        if !has_bash() || std::process::Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let home = temp_dir("stowbox-cred");
        let fill = |host: &str| {
            let script = format!("export GIT_TERMINAL_PROMPT=0\n{TOKEN_HELPER}printf 'protocol=https\\nhost={host}\\n\\n' | git \"${{stowbox_auth[@]}}\" credential fill 2>/dev/null");
            let out = std::process::Command::new("bash")
                .arg("-c")
                .arg(script)
                .env("HOME", &home)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GITHUB_TOKEN")
                .env("GH_TOKEN", "tok-123")
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        assert!(fill("github.com").contains("username=x-access-token\npassword=tok-123\n"));
        assert!(!fill("example.com").contains("tok-123"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[cfg_attr(not(unix), ignore = "box-side script, needs a Linux box's bash; see testing.rs")]
    async fn ssh_default_respects_the_users_choice() {
        if !has_git() {
            return;
        }
        let (home, work) = fixture();
        let ssh_of = |env: Vec<(String, String)>| {
            let p = LocalBash { home: home.clone(), env };
            async move {
                let script = prelude(Some("~/work"), "s") + "stowbox_mark ssh; printf '%s' \"${GIT_SSH_COMMAND-unset}\"\n";
                let c = capture(&p, "vm1", script, None, Duration::from_secs(20), "test").await.unwrap();
                text(section(&sections(&c.stdout, "s"), "ssh"))
            }
        };
        // Nothing configured: the batch-mode default.
        assert_eq!(ssh_of(vec![]).await, "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new");
        // An exported GIT_SSH_COMMAND is kept.
        assert_eq!(ssh_of(vec![("GIT_SSH_COMMAND".into(), "ssh -i ~/.ssh/deploy".into())]).await, "ssh -i ~/.ssh/deploy");
        // core.sshCommand in the repository: the variable is left unset so git uses it.
        sh(&work, &home, "git config core.sshCommand 'ssh -i ~/.ssh/work_key'");
        assert_eq!(ssh_of(vec![]).await, "unset");
        // ...and in the global config too.
        sh(&work, &home, "git config --unset core.sshCommand && git config --global core.sshCommand 'ssh -F ~/.ssh/cfg'");
        assert_eq!(ssh_of(vec![]).await, "unset");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[cfg_attr(not(unix), ignore = "box-side script, needs a Linux box's bash; see testing.rs")]
    async fn review_commit_push_end_to_end() {
        if !has_git() {
            return;
        }
        let (home, work) = fixture();
        let p = LocalBash { home: home.clone(), env: vec![] };

        let r = review(&p, "vm1", Some("~/work")).await.unwrap();
        assert_eq!(r.state, RepoState::Ok);
        assert_eq!(r.branch.head.as_deref(), Some("main"));
        assert_eq!(r.branch.upstream.as_deref(), Some("origin/main"));
        assert!(r.files.is_empty() && r.diff.is_empty(), "{r:?}");
        assert_eq!(r.remote.as_deref(), Some("origin"));
        assert!(r.remote_url.as_deref().unwrap().ends_with("remote.git"));

        // A change, an untracked file in a subdirectory, asked for from that subdirectory.
        std::fs::write(work.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::create_dir_all(work.join("sub dir")).unwrap();
        std::fs::write(work.join("sub dir/new.md"), "hello\n").unwrap();
        let r = review(&p, "vm1", Some(work.join("sub dir").to_str().unwrap())).await.unwrap();
        assert_eq!(r.root, work.to_str().unwrap());
        let files: Vec<(&str, &str)> = r.files.iter().map(|f| (f.path.as_str(), f.code.as_str())).collect();
        assert_eq!(files, vec![("a.txt", "M"), ("sub dir/new.md", "?")]);
        let diff: Vec<(&str, &str, u32, u32)> = r.diff.iter().map(|f| (f.path.as_str(), f.change.as_str(), f.additions, f.deletions)).collect();
        assert_eq!(diff, vec![("a.txt", "modified", 2, 1), ("sub dir/new.md", "added", 1, 0)]);
        assert!(!r.diff_truncated);

        let summary = commit(&p, "vm1", Some("~/work"), "Change a, add notes\n\nWith a body that has 'quotes' and $dollars.").await.unwrap();
        assert!(summary.contains("Change a, add notes"), "{summary}");
        let log = sh(&work, &home, "git log -1 --format=%B");
        assert_eq!(log.trim_end(), "Change a, add notes\n\nWith a body that has 'quotes' and $dollars.");
        let r = review(&p, "vm1", Some("~/work")).await.unwrap();
        assert!(r.files.is_empty());
        assert_eq!((r.branch.ahead, r.branch.behind), (1, 0));

        assert!(commit(&p, "vm1", Some("~/work"), "  ").await.is_err());
        let nothing = commit(&p, "vm1", Some("~/work"), "again").await.unwrap_err().to_string();
        assert!(nothing.contains("nothing to commit"), "{nothing}");

        push(&p, "vm1", Some("~/work")).await.unwrap();
        let r = review(&p, "vm1", Some("~/work")).await.unwrap();
        assert_eq!((r.branch.ahead, r.branch.behind), (0, 0));

        // A new branch has no upstream: push sets it.
        sh(&work, &home, "git switch -qc feature/x && echo x > x.txt && git add x.txt && git commit -qm x");
        let r = review(&p, "vm1", Some("~/work")).await.unwrap();
        assert_eq!(r.branch.upstream, None);
        push(&p, "vm1", Some("~/work")).await.unwrap();
        let r = review(&p, "vm1", Some("~/work")).await.unwrap();
        assert_eq!(r.branch.upstream.as_deref(), Some("origin/feature/x"));

        // Someone else pushes to main first: our push is rejected with git's words.
        sh(
            &home,
            &home,
            "git clone -q remote.git other 2>/dev/null && cd other && echo theirs > t.txt && git add t.txt && git commit -qm theirs && git push -q 2>/dev/null",
        );
        sh(&work, &home, "git switch -q main && echo ours > o.txt && git add o.txt && git commit -qm ours");
        let rejected = push(&p, "vm1", Some("~/work")).await.unwrap_err().to_string();
        assert!(rejected.contains("rejected"), "{rejected}");
        assert!(rejected.contains("main -> main"), "{rejected}");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[cfg_attr(not(unix), ignore = "box-side script, needs a Linux box's bash; see testing.rs")]
    async fn review_honest_states_and_caps() {
        if !has_git() {
            return;
        }
        let (home, work) = fixture();
        let p = LocalBash { home: home.clone(), env: vec![] };
        assert_eq!(review(&p, "vm1", None).await.unwrap().state, RepoState::NotRepo);
        assert_eq!(review(&p, "vm1", Some("~/missing")).await.unwrap().state, RepoState::NoDir);
        assert!(commit(&p, "vm1", None, "x").await.unwrap_err().to_string().contains("not inside a git repository"));

        // Bigger than the cap: cut inside the box and said so.
        let line = "x".repeat(99) + "\n";
        std::fs::write(work.join("big.txt"), line.repeat(DIFF_CAP / 100 + 500)).unwrap();
        std::fs::write(work.join("small.txt"), "s\n").unwrap();
        let r = review(&p, "vm1", Some("~/work")).await.unwrap();
        assert!(r.diff_truncated);
        assert_eq!(r.files.len(), 2);
        let total: usize = r.diff.iter().map(|f| f.patch.len()).sum();
        assert!(total <= DIFF_CAP);

        // A repository before its first commit.
        sh(&home, &home, "git init -q fresh && echo a > fresh/a");
        let r = review(&p, "vm1", Some("~/fresh")).await.unwrap();
        assert_eq!(r.branch.oid, None);
        assert_eq!(r.diff.len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)] // box-side script with unix-only test plumbing; see testing.rs
    #[tokio::test(flavor = "multi_thread")]
    async fn pull_request_with_and_without_gh() {
        if !has_git() {
            return;
        }
        let (home, work) = fixture();
        sh(
            &work,
            &home,
            "git remote set-url origin git@github.com:o/r.git && git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main && git switch -qc fix/it",
        );
        // Stub gh binaries in front of PATH, so a real gh never runs here.
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let stub = |body: &str| {
            let f = bin.join("gh");
            std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        };

        // gh present but signed out, no token: the compare page.
        stub("exit 1");
        let p = LocalBash { home: home.clone(), env: vec![("PATH".into(), path.clone())] };
        let out = open_pr(&p, "vm1", Some("~/work")).await.unwrap();
        assert_eq!(out, PrOutcome { url: "https://github.com/o/r/compare/main...fix/it?expand=1".into(), via: PrVia::Compare, existing: false });

        // A token in the environment: gh runs, without a prompt.
        stub("[ \"$1 $2 $3\" = 'pr create --fill' ] && [ \"$GH_PROMPT_DISABLED\" = 1 ] && echo https://github.com/o/r/pull/9");
        let p = LocalBash { home: home.clone(), env: vec![("PATH".into(), path.clone()), ("GH_TOKEN".into(), "tok".into())] };
        let out = open_pr(&p, "vm1", Some("~/work")).await.unwrap();
        assert_eq!(out, PrOutcome { url: "https://github.com/o/r/pull/9".into(), via: PrVia::Gh, existing: false });

        stub("echo 'a pull request for branch \"fix/it\" into branch \"main\" already exists:' >&2; echo https://github.com/o/r/pull/3 >&2; exit 1");
        let out = open_pr(&p, "vm1", Some("~/work")).await.unwrap();
        assert_eq!(out, PrOutcome { url: "https://github.com/o/r/pull/3".into(), via: PrVia::Gh, existing: true });

        stub("echo 'aborted: you must first push the current branch to a remote' >&2; exit 1");
        let err = open_pr(&p, "vm1", Some("~/work")).await.unwrap_err().to_string();
        assert_eq!(err, "aborted: you must first push the current branch to a remote");

        // The review reports the tools and the base branch.
        let r = review(&p, "vm1", Some("~/work")).await.unwrap();
        assert!(r.gh && r.token);
        assert_eq!(r.base.as_deref(), Some("main"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
