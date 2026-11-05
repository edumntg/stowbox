//! One git worktree and branch per agent, so several agents can work on one
//! repository in the same box without editing the same checkout.
//!
//! The worktree goes next to the repository, not inside it:
//! `<parent>/<repo>-wt/<slug>` on a new branch `stowbox/<slug>`, started
//! from the commit checked out where the agent was pointed. When that
//! directory is a subdirectory of the repository (`~/app/web`), the agent
//! works in the same subdirectory of its worktree. Removing the worktree
//! keeps the branch, so nothing committed is lost.
//!
//! The scripts start with the Changes tab's prelude (`git::prelude`): no
//! prompts, no editor, a per-call marker, and the honest "not a repository"
//! and "no such directory" states. Each call has a deadline.

use crate::boxd::shell_quote;
use crate::git::{self, capture, sections, RepoState};
use crate::provider::Provider;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const ADD_DEADLINE: Duration = Duration::from_secs(120);
pub const REMOVE_DEADLINE: Duration = Duration::from_secs(60);

/// Where an agent's own checkout lives in the box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// The worktree's top level, absolute.
    pub path: String,
    /// `stowbox/<slug>`.
    pub branch: String,
    /// The repository's main worktree, where `git worktree remove` runs.
    pub repo: String,
}

/// A worktree just made, and the directory the agent should work in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub worktree: Worktree,
    pub cwd: String,
}

/// A branch- and directory-safe name: up to five words of `hint` in lower
/// case, joined by dashes, then six characters of the agent id so two
/// agents with the same task never collide.
pub fn slug(hint: &str, agent_id: &str) -> String {
    let words: Vec<String> = hint
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let mut head = String::new();
    for w in words {
        if head.len() + w.len() + 1 > 32 {
            break;
        }
        if !head.is_empty() {
            head.push('-');
        }
        head.push_str(&w);
    }
    let tail: String = agent_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect::<String>().to_ascii_lowercase();
    match (head.is_empty(), tail.is_empty()) {
        (true, true) => "agent".into(),
        (true, false) => format!("agent-{tail}"),
        (false, true) => head,
        (false, false) => format!("{head}-{tail}"),
    }
}

pub fn branch_name(slug: &str) -> String {
    format!("stowbox/{slug}")
}

/// Creates the worktree for the repository that contains `cwd`. Prints the
/// main worktree, the new directory and the subdirectory prefix, then stops
/// at the first reason it cannot go on, named by a marker.
pub fn add_script(cwd: Option<&str>, slug: &str, nonce: &str) -> String {
    let mut s = git::prelude(cwd, nonce);
    s.push_str(&format!(
        concat!(
            "stowbox_slug={slug}\n",
            "stowbox_branch={branch}\n",
            "git rev-parse -q --verify HEAD >/dev/null || {{ stowbox_mark nocommit; exit 0; }}\n",
            // The main worktree comes first in the list, so an agent started
            // from another agent's worktree still lands next to the repository.
            "stowbox_main=$(git worktree list --porcelain 2>/dev/null | sed -n '1s/^worktree //p')\n",
            "[ -d \"$stowbox_main\" ] || stowbox_main=$stowbox_top\n",
            "stowbox_dir=\"$(dirname -- \"$stowbox_main\")/$(basename -- \"$stowbox_main\")-wt/$stowbox_slug\"\n",
            "stowbox_mark main; printf '%s' \"$stowbox_main\"\n",
            "stowbox_mark dir; printf '%s' \"$stowbox_dir\"\n",
            "stowbox_mark prefix; printf '%s' \"$stowbox_prefix\"\n",
            "[ -e \"$stowbox_dir\" ] && {{ stowbox_mark taken; exit 0; }}\n",
            "git show-ref --verify --quiet \"refs/heads/$stowbox_branch\" && {{ stowbox_mark branch-taken; exit 0; }}\n",
            "stowbox_mark run\n",
            "exec 2>&1 </dev/null\n",
            "mkdir -p -- \"$(dirname -- \"$stowbox_dir\")\" || exit $?\n",
            "git worktree add -b \"$stowbox_branch\" \"$stowbox_dir\" HEAD || exit $?\n",
            "stowbox_mark added\n",
        ),
        slug = shell_quote(slug),
        branch = shell_quote(&branch_name(slug)),
    ));
    s
}

/// Removes the worktree, keeping its branch. Without `force` git refuses a
/// worktree with uncommitted or untracked files. A directory already gone
/// is only pruned from git's list. The `<repo>-wt` directory goes too once
/// it is empty.
pub fn remove_script(wt: &Worktree, force: bool, nonce: &str) -> String {
    let mut s = git::prelude(Some(&wt.repo), nonce);
    s.push_str(&format!(
        concat!(
            "stowbox_dir={dir}\n",
            "if [ ! -e \"$stowbox_dir\" ]; then git worktree prune; stowbox_mark gone; exit 0; fi\n",
            "stowbox_mark run\n",
            "exec 2>&1 </dev/null\n",
            "git worktree remove {force}\"$stowbox_dir\" || exit $?\n",
            "rmdir -- \"$(dirname -- \"$stowbox_dir\")\" 2>/dev/null\n",
            "stowbox_mark removed\n",
        ),
        dir = shell_quote(&wt.path),
        force = if force { "--force " } else { "" },
    ));
    s
}

fn reason(state: RepoState) -> &'static str {
    match state {
        RepoState::NoGit => "cannot be read: git is not installed in the box",
        RepoState::NoDir => "does not exist in the box",
        RepoState::NotRepo | RepoState::Ok => "is not inside a git repository",
    }
}

fn shown(cwd: Option<&str>) -> &str {
    cwd.map(str::trim).filter(|c| !c.is_empty()).unwrap_or("~")
}

/// Reads the add script's output. `None` when it stopped without saying why.
pub fn parse_add(out: &[u8], nonce: &str, cwd: Option<&str>, slug: &str) -> Result<Option<Added>> {
    let secs = sections(out, nonce);
    if let Some(state) = git::early_state(&secs)? {
        return Err(Error::Provider(format!("{} {}, so the agent cannot have its own branch there", shown(cwd), reason(state))));
    }
    let field = |name: &str| git::text(git::section(&secs, name));
    if git::section(&secs, "nocommit").is_some() {
        return Err(Error::Provider("the repository has no commits yet; commit once, then an agent can branch from it".into()));
    }
    let dir = field("dir");
    if git::section(&secs, "taken").is_some() {
        return Err(Error::Provider(format!("{dir} already exists")));
    }
    if git::section(&secs, "branch-taken").is_some() {
        return Err(Error::Provider(format!("the branch {} already exists", branch_name(slug))));
    }
    if git::section(&secs, "added").is_none() {
        let out = field("run");
        if git::section(&secs, "run").is_some() && !out.is_empty() {
            return Err(Error::Provider(out));
        }
        return Ok(None);
    }
    let prefix = field("prefix");
    let prefix = prefix.trim_end_matches('/');
    let cwd = if prefix.is_empty() { dir.clone() } else { format!("{dir}/{prefix}") };
    Ok(Some(Added { worktree: Worktree { path: dir, branch: branch_name(slug), repo: field("main") }, cwd }))
}

/// Makes a worktree on a new branch `stowbox/<slug>` for the repository that
/// contains `cwd` in the box. Refusals carry git's words.
pub async fn add<P: Provider + ?Sized>(p: &P, machine_id: &str, cwd: Option<&str>, slug: &str) -> Result<Added> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let c = capture(p, machine_id, add_script(cwd, slug, &nonce), None, ADD_DEADLINE, "creating the worktree").await?;
    match parse_add(&c.stdout, &nonce, cwd, slug)? {
        Some(a) => Ok(a),
        None => Err(git::script_failed(&c, "git worktree add")),
    }
}

/// Removes the agent's worktree and keeps its branch. Without `force`, a
/// worktree with changes is refused with git's message.
pub async fn remove<P: Provider + ?Sized>(p: &P, machine_id: &str, wt: &Worktree, force: bool) -> Result<()> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let c = capture(p, machine_id, remove_script(wt, force, &nonce), None, REMOVE_DEADLINE, "removing the worktree").await?;
    let secs = sections(&c.stdout, &nonce);
    if let Some(state) = git::early_state(&secs)? {
        return Err(Error::Provider(format!("the repository {} {}; remove {} by hand", wt.repo, reason(state), wt.path)));
    }
    if git::section(&secs, "gone").is_some() || git::section(&secs, "removed").is_some() {
        return Ok(());
    }
    let out = git::text(git::section(&secs, "run"));
    Err(if out.is_empty() { git::script_failed(&c, "git worktree remove") } else { Error::Provider(out) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{fixture, has_git, sh, LocalBash};

    #[test]
    fn slugs() {
        assert_eq!(slug("Add a /health endpoint and a test for it", "3F2A91c0-x"), "add-a-health-endpoint-and-3f2a91");
        assert_eq!(slug("", "ab12cd34"), "agent-ab12cd");
        assert_eq!(slug("¿Qué pasa? ñandú", "ab12cd34"), "qu-pasa-and-ab12cd");
        let long = slug("supercalifragilisticexpialidocious", "ab12cd");
        assert_eq!(long, "agent-ab12cd", "a single word over the limit is dropped");
        assert_eq!(branch_name("x-1"), "stowbox/x-1");
        // Every slug is a valid branch component.
        for h in ["--x--", "..", "a.lock", "@{u}", "ünïcödé", "x y z"] {
            let s = slug(h, "ab12cd");
            assert!(s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{s}");
            assert!(!s.starts_with('-') && !s.ends_with('-'), "{s}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn add_review_and_remove_end_to_end() {
        if !has_git() {
            return;
        }
        let (home, work) = fixture();
        std::fs::create_dir_all(work.join("web")).unwrap();
        sh(&work, &home, "echo w > web/w.txt && git add web && git commit -qm web");
        let p = LocalBash { home: home.clone(), env: vec![] };

        // From a subdirectory: the agent works in the same subdirectory of its worktree.
        let a = add(&p, "vm1", Some("~/work/web"), "fix-login-ab12cd").await.unwrap();
        let wt_dir = home.join("work-wt/fix-login-ab12cd");
        assert_eq!(a.worktree, Worktree { path: wt_dir.to_str().unwrap().into(), branch: "stowbox/fix-login-ab12cd".into(), repo: work.to_str().unwrap().into() });
        assert_eq!(a.cwd, format!("{}/web", wt_dir.display()));
        assert!(wt_dir.join("web/w.txt").exists());

        // The Changes tab reads the agent's own branch there.
        let r = git::review(&p, "vm1", Some(&a.cwd)).await.unwrap();
        assert_eq!(r.branch.head.as_deref(), Some("stowbox/fix-login-ab12cd"));
        assert_eq!(r.root, wt_dir.to_str().unwrap());

        // A second agent, started from the first one's worktree, still lands next to the repository.
        let b = add(&p, "vm1", Some(&a.worktree.path), "second-ef34ab").await.unwrap();
        assert_eq!(b.worktree.path, home.join("work-wt/second-ef34ab").to_str().unwrap());
        assert_eq!(b.worktree.repo, work.to_str().unwrap());
        assert_eq!(b.cwd, b.worktree.path);

        // The same slug again: the directory is taken. Same branch, other directory: the branch is.
        let taken = add(&p, "vm1", Some("~/work"), "fix-login-ab12cd").await.unwrap_err().to_string();
        assert!(taken.ends_with("work-wt/fix-login-ab12cd already exists"), "{taken}");
        sh(&work, &home, "git branch stowbox/only-branch");
        let taken = add(&p, "vm1", Some("~/work"), "only-branch").await.unwrap_err().to_string();
        assert_eq!(taken, "the branch stowbox/only-branch already exists");

        // Uncommitted work: removal is refused with git's words, then forced.
        std::fs::write(wt_dir.join("web/new.txt"), "draft\n").unwrap();
        let refused = remove(&p, "vm1", &a.worktree, false).await.unwrap_err().to_string();
        assert!(refused.contains("--force"), "{refused}");
        assert!(wt_dir.exists());
        remove(&p, "vm1", &a.worktree, true).await.unwrap();
        assert!(!wt_dir.exists());
        // The branch is kept.
        sh(&work, &home, "git show-ref --verify --quiet refs/heads/stowbox/fix-login-ab12cd");

        // A clean worktree goes without force, and the empty -wt directory with it.
        remove(&p, "vm1", &b.worktree, false).await.unwrap();
        assert!(!home.join("work-wt").exists());

        // Deleted by hand already: pruned from git's list, not an error.
        let c = add(&p, "vm1", Some("~/work"), "by-hand-0000aa").await.unwrap();
        std::fs::remove_dir_all(&c.worktree.path).unwrap();
        remove(&p, "vm1", &c.worktree, false).await.unwrap();
        assert!(!sh(&work, &home, "git worktree list").contains("by-hand"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn add_refuses_honestly() {
        if !has_git() {
            return;
        }
        let (home, _work) = fixture();
        let p = LocalBash { home: home.clone(), env: vec![] };
        let e = add(&p, "vm1", None, "x").await.unwrap_err().to_string();
        assert_eq!(e, "~ is not inside a git repository, so the agent cannot have its own branch there");
        let e = add(&p, "vm1", Some("~/missing"), "x").await.unwrap_err().to_string();
        assert_eq!(e, "~/missing does not exist in the box, so the agent cannot have its own branch there");
        sh(&home, &home, "git init -q fresh");
        let e = add(&p, "vm1", Some("~/fresh"), "x").await.unwrap_err().to_string();
        assert!(e.contains("no commits yet"), "{e}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
