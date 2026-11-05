//! Files attached to a message for an agent. Each one is copied into the box
//! under `~/.stowbox/uploads/<agent>/` and the prompt names it by its path,
//! so the harness reads it from there (Claude Code opens images from a
//! path). Nothing is inlined in the prompt.

use crate::boxd::shell_quote;
use crate::provider::Provider;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Upload directories live under `$HOME/<UPLOADS_DIR>/<agent id>/`.
pub const UPLOADS_DIR: &str = ".stowbox/uploads";

/// Largest file accepted, in bytes.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// Most files on one message.
pub const MAX_FILES: usize = 10;

/// Longest file name kept in the box, in characters.
const NAME_MAX: usize = 80;

/// A file already in the box, as a message carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// The name the person's file had.
    pub name: String,
    /// Absolute path in the box.
    pub path: String,
    pub size: u64,
}

/// "20 MB" for the size cap in error messages.
fn mb(bytes: u64) -> String {
    format!("{} MB", bytes / (1024 * 1024))
}

/// Refuses a file over the cap or an empty one, naming the file.
pub fn check_size(name: &str, size: u64) -> Result<()> {
    if size == 0 {
        return Err(Error::Provider(format!("{name} is empty")));
    }
    if size > MAX_BYTES {
        let shown = format!("{:.1} MB", size as f64 / (1024.0 * 1024.0));
        return Err(Error::Provider(format!("{name} is {shown}; attachments are limited to {} each", mb(MAX_BYTES))));
    }
    Ok(())
}

/// The file name used in the box: `<tag>-<name>`, where the name keeps
/// letters, digits, `.`, `-` and `_` from the last path component (others
/// become `_`), never starts with a dot and keeps its extension when cut.
/// `tag` keeps two files with the same name apart.
pub fn box_file_name(original: &str, tag: &str) -> String {
    let base = original.rsplit(['/', '\\']).next().unwrap_or("");
    let mut clean = String::new();
    for c in base.chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' };
        if c == '_' && clean.ends_with('_') {
            continue;
        }
        clean.push(c);
    }
    let clean = clean.trim_start_matches(['.', '_']).to_string();
    let clean = if clean.trim_matches(['.', '_', '-']).is_empty() { "file".to_string() } else { clean };
    let clean = if clean.chars().count() > NAME_MAX {
        match clean.rfind('.').filter(|&i| clean.len() - i <= 10 && i > 0) {
            Some(i) => {
                let ext = &clean[i..];
                let stem: String = clean[..i].chars().take(NAME_MAX - ext.len()).collect();
                format!("{stem}{ext}")
            }
            None => clean.chars().take(NAME_MAX).collect(),
        }
    } else {
        clean
    };
    format!("{tag}-{clean}")
}

/// A short random tag for [`box_file_name`].
pub fn new_tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

/// Makes the agent's upload directory and prints its absolute path, so the
/// upload can name it (the provider's file API takes no `~`).
pub fn dir_script(agent_id: &str) -> String {
    format!("d=\"$HOME\"/{}\nmkdir -p \"$d\" && cd \"$d\" && pwd -P\n", shell_quote(&format!("{UPLOADS_DIR}/{agent_id}")))
}

/// The prompt as the harness gets it: the person's text, then one line per
/// attached file with its path in the box.
pub fn prompt_with(text: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let mut s = text.trim_end().to_string();
    if !s.is_empty() {
        s.push_str("\n\n");
    }
    s.push_str(if attachments.len() == 1 { "Attached file (read it from this path):" } else { "Attached files (read them from these paths):" });
    for a in attachments {
        s.push_str("\n- ");
        s.push_str(&a.path);
    }
    s
}

/// What the provider confirmed against what was sent. 0 means the server
/// did not say, which boxd documents as "unverified".
pub fn confirm_written(written: u64, expected: u64, path: &str) -> Result<u64> {
    if written == 0 || written == expected {
        Ok(expected)
    } else {
        Err(Error::Provider(format!("upload to {path} was cut short: {written} of {expected} bytes written")))
    }
}

/// The exec fallback for providers without a file API: the bytes travel
/// base64-encoded on stdin, land in a temporary file next to `path`, and
/// replace it only once complete. Prints the size written.
pub fn exec_upload_script(path: &str) -> String {
    let p = shell_quote(path);
    format!("set -e\nt={p}.part.$$\nbase64 -d > \"$t\"\nmv -f \"$t\" {p}\nwc -c < {p}\n")
}

/// [`exec_upload_script`] run over the provider's exec stream.
pub async fn upload_over_exec<P: Provider + ?Sized>(p: &P, machine_id: &str, path: &str, data: Vec<u8>) -> Result<u64> {
    use base64::Engine;
    let expected = data.len() as u64;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data).into_bytes();
    let out = crate::git::capture(p, machine_id, exec_upload_script(path), Some(b64), Duration::from_secs(300), "the upload").await?;
    if out.code != 0 {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error::Provider(if msg.is_empty() { format!("upload to {path} failed with exit code {}", out.code) } else { msg }));
    }
    let written = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0);
    confirm_written(written, expected, path)
}

/// Copies one file into the agent's upload directory in the box and
/// returns where it landed. Errors carry the provider's message.
pub async fn upload<P: Provider + ?Sized>(p: &P, machine_id: &str, agent_id: &str, name: &str, data: Vec<u8>) -> Result<Attachment> {
    let size = data.len() as u64;
    check_size(name, size)?;
    let dir = crate::git::capture(p, machine_id, dir_script(agent_id), None, Duration::from_secs(60), "making the upload directory").await?;
    let dir_path = String::from_utf8_lossy(&dir.stdout).lines().map(str::trim).filter(|l| l.starts_with('/')).last().unwrap_or("").to_string();
    if dir.code != 0 || dir_path.is_empty() {
        let msg = String::from_utf8_lossy(&dir.stderr).trim().to_string();
        return Err(Error::Provider(if msg.is_empty() { "could not make the upload directory in the box".into() } else { msg }));
    }
    let path = format!("{dir_path}/{}", box_file_name(name, &new_tag()));
    let written = p.upload_file(machine_id, &path, data).await.map_err(|e| Error::Provider(format!("upload of {name} failed: {e}")))?;
    confirm_written(written, size, &path)?;
    Ok(Attachment { name: name.to_string(), path, size })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{has_bash, temp_dir, LocalBash};

    #[test]
    fn file_names_are_safe_and_tagged() {
        assert_eq!(box_file_name("screenshot.png", "abc123"), "abc123-screenshot.png");
        assert_eq!(box_file_name("../../etc/passwd", "t"), "t-passwd");
        assert_eq!(box_file_name("C:\\Users\\me\\My Shot (2).PNG", "t"), "t-My_Shot_2_.PNG");
        assert_eq!(box_file_name(".bashrc", "t"), "t-bashrc");
        assert_eq!(box_file_name("..", "t"), "t-file");
        assert_eq!(box_file_name("", "t"), "t-file");
        assert_eq!(box_file_name("$(rm -rf ~)'; x", "t"), "t-rm_-rf_x");
        let long = format!("{}.jpeg", "a".repeat(200));
        let cut = box_file_name(&long, "t");
        assert!(cut.ends_with(".jpeg"));
        assert_eq!(cut.chars().count(), 2 + NAME_MAX);
    }

    #[test]
    fn size_cap_is_honest() {
        assert!(check_size("a.png", 1).is_ok());
        assert!(check_size("a.png", MAX_BYTES).is_ok());
        let e = check_size("big.mov", MAX_BYTES + 1).unwrap_err().to_string();
        assert!(e.contains("big.mov") && e.contains("20 MB"), "{e}");
        assert!(check_size("empty.txt", 0).unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn prompt_names_each_path() {
        assert_eq!(prompt_with("fix it", &[]), "fix it");
        let a = |p: &str| Attachment { name: "x".into(), path: p.into(), size: 1 };
        assert_eq!(prompt_with("fix it\n", &[a("/h/a.png")]), "fix it\n\nAttached file (read it from this path):\n- /h/a.png");
        assert_eq!(prompt_with("", &[a("/h/a.png"), a("/h/b.log")]), "Attached files (read them from these paths):\n- /h/a.png\n- /h/b.log");
    }

    #[test]
    fn written_count_is_checked() {
        assert_eq!(confirm_written(0, 10, "/p").unwrap(), 10);
        assert_eq!(confirm_written(10, 10, "/p").unwrap(), 10);
        assert!(confirm_written(4, 10, "/p").unwrap_err().to_string().contains("4 of 10"));
    }

    #[tokio::test]
    async fn uploads_end_to_end_over_exec() {
        if !has_bash() {
            return;
        }
        let home = temp_dir("stowbox-upload");
        let p = LocalBash { home: home.clone(), env: vec![] };
        // Every byte value, so nothing is lost to text handling.
        let data: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
        let a = upload(&p, "vm", "agent-1", "../shot one.png", data.clone()).await.unwrap();
        let dir = std::fs::canonicalize(home.join(UPLOADS_DIR).join("agent-1")).unwrap();
        assert!(a.path.starts_with(dir.to_str().unwrap()), "{} not under {}", a.path, dir.display());
        assert!(a.path.ends_with("-shot_one.png"), "{}", a.path);
        assert_eq!(a.size, 300_000);
        assert_eq!(a.name, "../shot one.png");
        assert_eq!(std::fs::read(&a.path).unwrap(), data);
        // No temporary file left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // The same name again lands next to it, not over it.
        let b = upload(&p, "vm", "agent-1", "shot one.png", b"second".to_vec()).await.unwrap();
        assert_ne!(a.path, b.path);
        assert_eq!(std::fs::read(&a.path).unwrap(), data);
        // Over the cap: refused before anything is written.
        let e = upload(&p, "vm", "agent-2", "huge.bin", vec![0; MAX_BYTES as usize + 1]).await.unwrap_err();
        assert!(e.to_string().contains("limited to 20 MB"), "{e}");
        assert!(!home.join(UPLOADS_DIR).join("agent-2").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn exec_upload_reports_failure_verbatim() {
        if !has_bash() {
            return;
        }
        let home = temp_dir("stowbox-upload-fail");
        let p = LocalBash { home: home.clone(), env: vec![] };
        let e = upload_over_exec(&p, "vm", &format!("{}/no/such/dir/f", home.display()), b"x".to_vec()).await.unwrap_err();
        assert!(e.to_string().contains("No such file or directory"), "{e}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
