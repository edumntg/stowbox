//! Setup presets: an ordered list of shell commands plus named secrets that
//! run inside a box right after it is created (or on demand later).
//!
//! The run goes over the provider's exec stream, the same channel the
//! terminal uses, so nothing has to be installed and no SSH key is needed.
//! The whole runner script travels on stdin to `bash -s`, never on the
//! command line, so secret values do not show up in the remote process list
//! or in the provider's exec logs.
//!
//! Inside the box the secrets land in `~/.config/stowbox/env.sh` (mode 0600)
//! as `export NAME='value'` lines. `~/.profile` and `~/.bashrc` source that
//! file, so later terminals and agent turns see the same variables. The
//! steps run in one shell, one after another, with the secrets exported;
//! `cd` and `export` carry over from one step to the next, and the first
//! step that exits non-zero stops the run.
//!
//! A command may hold `{{name}}` placeholders (`git clone {{repo}}`). The
//! value for each one is asked for when the preset runs, falling back to the
//! preset's default, and is inserted as one single-quoted shell word: write
//! the placeholder outside quotes (`cd ~/{{dir}}`, not `cd "~/{{dir}}"`).
//! Only `{{` + a valid variable name + `}}` is a placeholder (spaces inside
//! the braces are allowed); anything else between braces, such as a Go
//! template `{{.State}}` in a `docker inspect -f` command, is left as is.
//! Variable values are not secrets: they are kept in the run record and
//! show up in the log.

use crate::agent::LineBuffer;
use crate::provider::Provider;
use crate::types::*;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Where the secrets file lives inside the box, relative to `$HOME`.
pub const ENV_FILE: &str = ".config/stowbox/env.sh";

/// Kept in the run record; older output is dropped from the front.
pub const OUTPUT_CAP: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub commands: Vec<String>,
    /// Names only. Values live in the OS keychain on the desktop side.
    pub secret_names: Vec<String>,
    /// The `{{name}}` placeholders found in `commands`, in order of first
    /// use, with their defaults. Older presets have none stored.
    #[serde(default)]
    pub variables: Vec<Variable>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    /// Empty when there is none, which makes a value required at run time.
    #[serde(default)]
    pub default: String,
}

/// A value a run used for one variable. Kept in the run record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableValue {
    pub name: String,
    pub value: String,
}

/// Environment variable names a POSIX shell accepts in `export`.
pub fn valid_secret_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Checks a preset before it is saved. Commands are trimmed and empty ones
/// dropped by the caller; this only rejects what cannot run.
pub fn validate(name: &str, commands: &[String], secret_names: &[String]) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::Provider("give the preset a name".into()));
    }
    if commands.iter().all(|c| c.trim().is_empty()) && secret_names.is_empty() {
        return Err(Error::Provider(
            "add at least one command or one secret".into(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for n in secret_names {
        if !valid_secret_name(n) {
            return Err(Error::Provider(format!(
                "\"{n}\" is not a valid variable name: use letters, digits and _, not starting with a digit"
            )));
        }
        if !seen.insert(n.as_str()) {
            return Err(Error::Provider(format!("the secret {n} is listed twice")));
        }
    }
    Ok(())
}

fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Byte ranges and names of the `{{name}}` placeholders in one command.
fn placeholders(cmd: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = cmd[i..].find("{{") {
        let start = i + off;
        let Some(len) = cmd[start + 2..].find("}}") else { break };
        let name = cmd[start + 2..start + 2 + len].trim_matches(' ');
        if valid_secret_name(name) {
            let end = start + 2 + len + 2;
            out.push((start..end, name));
            i = end;
        } else {
            // `{` is one byte, so this stays on a char boundary.
            i = start + 1;
        }
    }
    out
}

/// The variable names used in `commands`, once each, in order of first use.
pub fn variables_in(commands: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for c in commands {
        for (_, n) in placeholders(c) {
            if !names.iter().any(|x| x == n) {
                names.push(n.to_string());
            }
        }
    }
    names
}

/// The variables a preset with these commands has, keeping the default
/// given for each name. Defaults for names no command uses are dropped.
pub fn preset_variables(commands: &[String], defaults: &[Variable]) -> Vec<Variable> {
    variables_in(commands)
        .into_iter()
        .map(|name| {
            let default = defaults.iter().find(|d| d.name == name).map(|d| d.default.clone()).unwrap_or_default();
            Variable { name, default }
        })
        .collect()
}

/// Replaces every placeholder with its value as one single-quoted shell
/// word. A placeholder without a value is an error.
pub fn substitute(cmd: &str, values: &[VariableValue]) -> Result<String> {
    let mut out = String::with_capacity(cmd.len());
    let mut last = 0;
    for (range, name) in placeholders(cmd) {
        let Some(v) = values.iter().find(|v| v.name == name) else {
            return Err(Error::Provider(format!("{{{{{name}}}}} has no value")));
        };
        out.push_str(&cmd[last..range.start]);
        out.push_str(&single_quote(&v.value));
        last = range.end;
    }
    out.push_str(&cmd[last..]);
    Ok(out)
}

/// The commands of one run with the values filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub commands: Vec<String>,
    pub variables: Vec<VariableValue>,
}

/// Picks a value for every variable the preset's commands use: the one
/// given, else the preset's default. An empty value counts as not given.
/// A value for a name no command uses is an error, so is a variable left
/// without a value.
pub fn prepare(preset: &Preset, given: &std::collections::HashMap<String, String>) -> Result<Prepared> {
    let names = variables_in(&preset.commands);
    let mut unknown: Vec<&String> = given.keys().filter(|k| !names.contains(k)).collect();
    unknown.sort();
    if let Some(k) = unknown.first() {
        return Err(Error::Provider(format!("\"{}\" has no variable {k}", preset.name)));
    }
    let mut variables = Vec::with_capacity(names.len());
    for name in names {
        let default = preset.variables.iter().find(|v| v.name == name).map(|v| v.default.as_str()).unwrap_or("");
        let value = given.get(&name).map(String::as_str).filter(|v| !v.is_empty()).unwrap_or(default);
        if value.is_empty() {
            return Err(Error::Provider(format!("give {name} a value")));
        }
        variables.push(VariableValue { name, value: value.to_string() });
    }
    let commands = preset.commands.iter().map(|c| substitute(c, &variables)).collect::<Result<Vec<_>>>()?;
    Ok(Prepared { commands, variables })
}

/// The script fed to `bash -s` inside the box. `nonce` makes the progress
/// markers impossible to fake from a command's own output by accident.
pub fn runner_script(commands: &[String], secrets: &[(String, String)], nonce: &str) -> String {
    let mark = marker(nonce);
    let mut s = String::new();
    s.push_str("set -u\n");
    s.push_str(&format!("STOWBOX_ENV=\"$HOME/{ENV_FILE}\"\n"));
    s.push_str("mkdir -p \"$(dirname \"$STOWBOX_ENV\")\"\n");
    s.push_str("( umask 077; : > \"$STOWBOX_ENV\" )\n");
    s.push_str("chmod 600 \"$STOWBOX_ENV\"\n");
    // Each value is single-quoted and appended with printf, so no heredoc
    // delimiter can collide with a value.
    for (k, v) in secrets {
        let line = format!("export {k}={}\n", single_quote(v));
        s.push_str(&format!(
            "printf '%s' {} >> \"$STOWBOX_ENV\"\n",
            single_quote(&line)
        ));
    }
    s.push_str(concat!(
        "for rc in \"$HOME/.profile\" \"$HOME/.bashrc\"; do\n",
        "  grep -qsF 'stowbox/env.sh' \"$rc\" || printf '\\n# Added by Stowbox: secrets from the setup preset\\n[ -f \"$HOME/.config/stowbox/env.sh\" ] && . \"$HOME/.config/stowbox/env.sh\"\\n' >> \"$rc\"\n",
        "done\n",
        ". \"$STOWBOX_ENV\"\n",
        "cd \"$HOME\"\n",
    ));
    s.push_str(&format!(
        "printf '{mark} secrets %d\\n' {}\n",
        secrets.len()
    ));
    s.push_str("set +u\n");
    // A step that calls `exit` ends this shell; the trap still reports it.
    s.push_str("stowbox_step=-1\n");
    s.push_str(&format!(
        "trap 'stowbox_rc=$?; [ \"$stowbox_step\" -ge 0 ] && printf \"{mark} exit %d %d\\\\n\" \"$stowbox_step\" \"$stowbox_rc\"' EXIT\n"
    ));
    for (i, cmd) in commands.iter().enumerate() {
        s.push_str(&format!("stowbox_step={i}\nprintf '{mark} start {i}\\n'\n"));
        // A brace group, not a subshell, so `cd` and `export` carry over.
        // stdin is closed so a command cannot swallow the rest of this script.
        s.push_str("{\n");
        s.push_str(cmd.trim_end());
        s.push_str("\n} </dev/null 2>&1\n");
        s.push_str("stowbox_rc=$?\nstowbox_step=-1\n");
        s.push_str(&format!("printf '{mark} exit {i} %d\\n' \"$stowbox_rc\"\n"));
        s.push_str("[ \"$stowbox_rc\" -eq 0 ] || exit \"$stowbox_rc\"\n");
    }
    s.push_str(&format!("printf '{mark} done\\n'\n"));
    s
}

fn marker(nonce: &str) -> String {
    format!("::stowbox-{nonce}::")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Secrets(usize),
    Start(usize),
    Exit(usize, i32),
    Done,
    Output(String),
}

/// Reads one line of runner output.
pub fn parse_line(line: &str, nonce: &str) -> Line {
    let mark = marker(nonce);
    let Some(rest) = line.strip_prefix(&mark) else {
        return Line::Output(line.to_string());
    };
    let parts: Vec<&str> = rest.split_whitespace().collect();
    match parts.as_slice() {
        ["secrets", n] => n
            .parse()
            .map(Line::Secrets)
            .unwrap_or_else(|_| Line::Output(line.into())),
        ["start", i] => i
            .parse()
            .map(Line::Start)
            .unwrap_or_else(|_| Line::Output(line.into())),
        ["exit", i, c] => match (i.parse(), c.parse()) {
            (Ok(i), Ok(c)) => Line::Exit(i, c),
            _ => Line::Output(line.into()),
        },
        ["done"] => Line::Done,
        _ => Line::Output(line.to_string()),
    }
}

/// Replaces every secret value in `text` with `[NAME]`. Short values are
/// left alone: masking "1" everywhere would wreck the log for nothing.
pub fn redact(text: &str, secrets: &[(String, String)]) -> String {
    let mut out = text.to_string();
    let mut by_len: Vec<&(String, String)> = secrets.iter().filter(|(_, v)| v.len() >= 4).collect();
    by_len.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (k, v) in by_len {
        out = out.replace(v.as_str(), &format!("[{k}]"));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Waiting for the box to boot.
    Waiting,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Pending,
    Running,
    Ok,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRun {
    pub command: String,
    pub state: StepState,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionRun {
    pub id: String,
    pub machine_id: String,
    pub machine_name: String,
    pub preset_id: String,
    pub preset_name: String,
    pub status: RunStatus,
    pub error: Option<String>,
    pub steps: Vec<StepRun>,
    /// Names of the secrets written into the box; never the values.
    pub secret_names: Vec<String>,
    /// The values the `{{name}}` placeholders got in this run.
    #[serde(default)]
    pub variables: Vec<VariableValue>,
    pub secrets_written: bool,
    pub output: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// One change to a run, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    Running,
    SecretsWritten,
    StepStart(usize),
    StepExit(usize, i32),
    Output(String),
    /// The remote process ended with this code.
    Finished(i32),
    Failed(String),
    Cancelled,
}

impl ProvisionRun {
    /// `prepared` holds the commands with their values filled in, as they
    /// will run; the steps show those.
    pub fn new(id: String, machine: &Machine, preset: &Preset, prepared: &Prepared, now_ms: i64) -> Self {
        Self {
            id,
            machine_id: machine.id.clone(),
            machine_name: machine.name.clone(),
            preset_id: preset.id.clone(),
            preset_name: preset.name.clone(),
            status: RunStatus::Waiting,
            error: None,
            steps: prepared
                .commands
                .iter()
                .map(|c| StepRun {
                    command: c.clone(),
                    state: StepState::Pending,
                    exit_code: None,
                })
                .collect(),
            secret_names: preset.secret_names.clone(),
            variables: prepared.variables.clone(),
            secrets_written: false,
            output: String::new(),
            started_at: now_ms,
            finished_at: None,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, RunStatus::Waiting | RunStatus::Running)
    }

    /// Folds one event into the run. `now_ms` stamps the end.
    pub fn apply(&mut self, ev: &RunEvent, now_ms: i64) {
        match ev {
            RunEvent::Running => self.status = RunStatus::Running,
            RunEvent::SecretsWritten => self.secrets_written = true,
            RunEvent::StepStart(i) => {
                if let Some(s) = self.steps.get_mut(*i) {
                    s.state = StepState::Running;
                }
            }
            RunEvent::StepExit(i, code) => {
                if let Some(s) = self.steps.get_mut(*i) {
                    s.exit_code = Some(*code);
                    s.state = if *code == 0 {
                        StepState::Ok
                    } else {
                        StepState::Failed
                    };
                }
            }
            RunEvent::Output(text) => {
                self.output.push_str(text);
                if self.output.len() > OUTPUT_CAP {
                    let mut cut = self.output.len() - OUTPUT_CAP;
                    while !self.output.is_char_boundary(cut) {
                        cut += 1;
                    }
                    self.output.drain(..cut);
                }
            }
            RunEvent::Finished(code) => {
                let all_ok = self.steps.iter().all(|s| s.state == StepState::Ok);
                if *code == 0 && all_ok {
                    self.status = RunStatus::Done;
                } else {
                    self.status = RunStatus::Failed;
                    if self.error.is_none() {
                        self.error = Some(
                            match self.steps.iter().position(|s| s.state == StepState::Failed) {
                                Some(i) => format!(
                                    "step {} exited with code {}",
                                    i + 1,
                                    self.steps[i].exit_code.unwrap_or(*code)
                                ),
                                None => format!("the setup script exited with code {code}"),
                            },
                        );
                    }
                }
                self.finish(now_ms);
            }
            RunEvent::Failed(message) => {
                self.status = RunStatus::Failed;
                self.error = Some(message.clone());
                self.finish(now_ms);
            }
            RunEvent::Cancelled => {
                self.status = RunStatus::Cancelled;
                self.finish(now_ms);
            }
        }
    }

    fn finish(&mut self, now_ms: i64) {
        self.finished_at = Some(now_ms);
        for s in &mut self.steps {
            if matches!(s.state, StepState::Pending | StepState::Running) {
                s.state = StepState::Skipped;
            }
        }
    }
}

/// How long to wait for a new box to reach a state exec can use.
pub const BOOT_DEADLINE: Duration = Duration::from_secs(180);

/// Waits for the box, runs the preset in it and reports every change
/// through `emit`. Returns once the remote process is gone. Dropping the
/// future kills the remote process group (the exec stream closes).
pub async fn run<P, F>(
    p: &P,
    machine_id: &str,
    commands: &[String],
    secrets: &[(String, String)],
    mut emit: F,
) where
    P: Provider + ?Sized,
    F: FnMut(RunEvent),
{
    if let Err(e) = wait_until_reachable(p, machine_id).await {
        emit(RunEvent::Failed(e.to_string()));
        return;
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let script = runner_script(commands, secrets, &nonce);

    // A box that just booted can refuse exec for a few seconds.
    let start = Instant::now();
    let mut handle = loop {
        match p
            .exec(
                machine_id,
                ExecSpec {
                    script: "exec bash -s".into(),
                    tty: false,
                    cols: 0,
                    rows: 0,
                    env: vec![],
                },
            )
            .await
        {
            Ok(h) => break h,
            Err(_) if start.elapsed() < Duration::from_secs(60) => {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(e) => {
                emit(RunEvent::Failed(format!(
                    "could not open a shell in the box: {e}"
                )));
                return;
            }
        }
    };
    emit(RunEvent::Running);
    if handle
        .input
        .send(ExecInput::Data(script.into_bytes()))
        .await
        .is_err()
        || handle.input.send(ExecInput::Close).await.is_err()
    {
        emit(RunEvent::Failed(
            "the box closed the shell before the script was sent".into(),
        ));
        return;
    }

    let mut out = LineBuffer::default();
    let mut err = LineBuffer::default();
    let handle_line = |line: String, emit: &mut F| match parse_line(&line, &nonce) {
        Line::Secrets(_) => emit(RunEvent::SecretsWritten),
        Line::Start(i) => {
            emit(RunEvent::StepStart(i));
            // A header in the log so each block of output has its command.
            let cmd = commands.get(i).map(|c| c.trim()).unwrap_or("");
            let first = cmd.lines().next().unwrap_or("");
            let more = if cmd.lines().count() > 1 { " …" } else { "" };
            emit(RunEvent::Output(format!("$ {first}{more}\n")));
        }
        Line::Exit(i, c) => emit(RunEvent::StepExit(i, c)),
        Line::Done => {}
        Line::Output(text) => emit(RunEvent::Output(redact(&text, secrets) + "\n")),
    };
    while let Some(chunk) = handle.output.recv().await {
        match chunk {
            ExecOutput::Stdout(b) => out
                .push(&b)
                .into_iter()
                .for_each(|l| handle_line(l, &mut emit)),
            ExecOutput::Stderr(b) => err
                .push(&b)
                .into_iter()
                .for_each(|l| handle_line(l, &mut emit)),
            ExecOutput::Exit(code) => {
                for l in out.flush().into_iter().chain(err.flush()) {
                    handle_line(l, &mut emit);
                }
                emit(RunEvent::Finished(code));
                return;
            }
            ExecOutput::Error(e) => {
                emit(RunEvent::Failed(e));
                return;
            }
        }
    }
    emit(RunEvent::Failed(
        "the exec stream ended without an exit code".into(),
    ));
}

async fn wait_until_reachable<P: Provider + ?Sized>(p: &P, id: &str) -> Result<()> {
    let end = Instant::now() + BOOT_DEADLINE;
    loop {
        let m = p.get_machine(id).await?;
        match m.status {
            // exec wakes standby and hibernated boxes on its own.
            s if s.is_reachable() => return Ok(()),
            MachineStatus::Stopped => {
                return Err(Error::Provider(
                    "the box is stopped; start it and run the preset again".into(),
                ))
            }
            MachineStatus::Failed => return Err(Error::Provider("the box failed to boot".into())),
            _ => {}
        }
        if Instant::now() >= end {
            return Err(Error::Provider(
                "the box did not finish booting within 3 minutes".into(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names() {
        assert!(valid_secret_name("GITHUB_TOKEN"));
        assert!(valid_secret_name("_x1"));
        assert!(!valid_secret_name("1X"));
        assert!(!valid_secret_name("A-B"));
        assert!(!valid_secret_name(""));
        assert!(!valid_secret_name("A B"));
    }

    #[test]
    fn validate_rejects_bad_presets() {
        assert!(validate("", &["ls".into()], &[]).is_err());
        assert!(validate("x", &["  ".into()], &[]).is_err());
        assert!(validate("x", &[], &["A".into(), "A".into()]).is_err());
        assert!(validate("x", &[], &["bad-name".into()]).is_err());
        assert!(validate("x", &[], &["TOKEN".into()]).is_ok());
        assert!(validate("main", &["git clone x".into()], &[]).is_ok());
    }

    #[test]
    fn script_quotes_values_and_marks_steps() {
        let s = runner_script(
            &[
                "git clone https://example.com/r.git\ncd r".into(),
                "docker compose up -d".into(),
            ],
            &[("TOKEN".into(), "it's $secret\nline2".into())],
            "n1",
        );
        // The secret value never appears unquoted.
        assert!(!s.contains("TOKEN=it's"));
        assert!(s.contains("{\ngit clone https://example.com/r.git\ncd r\n} </dev/null 2>&1\n"));
        assert!(s.contains("printf '::stowbox-n1:: exit 1 %d\\n'"));
        assert!(s.trim_end().ends_with("printf '::stowbox-n1:: done\\n'"));
        assert!(s.contains("chmod 600"));
    }

    #[test]
    fn script_runs_in_bash() {
        // Only where bash exists; proves quoting and step markers end to end.
        if std::process::Command::new("bash")
            .arg("-c")
            .arg("true")
            .status()
            .is_err()
        {
            return;
        }
        let home =
            std::env::temp_dir().join(format!("stowbox-prov-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let script = runner_script(
            &[
                "mkdir -p work && cd work".into(),
                "echo \"[$PWD] $TOKEN\"".into(),
                "exit 3".into(),
                "echo never".into(),
            ],
            &[("TOKEN".into(), "a'b $c\"d".into())],
            "t",
        );
        let run = || {
            std::process::Command::new("bash")
                .arg("-s")
                .env("HOME", &home)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut c| {
                    use std::io::Write;
                    c.stdin.take().unwrap().write_all(script.as_bytes())?;
                    c.wait_with_output()
                })
                .unwrap()
        };
        let _ = run();
        let out = run();
        let text = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<Line> = text.lines().map(|l| parse_line(l, "t")).collect();
        let workdir = home.join("work");
        assert_eq!(
            lines,
            vec![
                Line::Secrets(1),
                Line::Start(0),
                Line::Exit(0, 0),
                Line::Start(1),
                Line::Output(format!("[{}] a'b $c\"d", workdir.display())),
                Line::Exit(1, 0),
                Line::Start(2),
                Line::Exit(2, 3),
            ]
        );
        assert_eq!(out.status.code(), Some(3));
        let env = std::fs::read_to_string(home.join(ENV_FILE)).unwrap();
        assert_eq!(env, "export TOKEN='a'\\''b $c\"d'\n");
        let profile = std::fs::read_to_string(home.join(".profile")).unwrap();
        assert_eq!(
            profile.matches("Added by Stowbox").count(),
            1,
            "the rc hook is added once"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    fn preset_with(commands: &[&str], variables: &[(&str, &str)]) -> Preset {
        Preset {
            id: "p1".into(),
            name: "main".into(),
            commands: commands.iter().map(|c| c.to_string()).collect(),
            secret_names: vec![],
            variables: variables.iter().map(|(n, d)| Variable { name: n.to_string(), default: d.to_string() }).collect(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn given(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn finds_placeholders_in_order_once() {
        let cmds = vec![
            "git clone {{repo}} ~/app && cd ~/app".to_string(),
            "git checkout {{ branch }} && echo {{repo}}".to_string(),
            "docker inspect -f '{{.State.Running}}' db {{json .}} {{1x}} {{a-b}} {{}}".to_string(),
            "echo {{{inner}}} {{unclosed".to_string(),
        ];
        assert_eq!(variables_in(&cmds), vec!["repo", "branch", "inner"]);
        assert!(variables_in(&["echo ${HOME} {x}".to_string()]).is_empty());
    }

    #[test]
    fn preset_variables_keep_defaults_by_name() {
        let cmds = vec!["git checkout {{branch}} {{repo}}".to_string()];
        let defaults = vec![
            Variable { name: "repo".into(), default: "r".into() },
            Variable { name: "gone".into(), default: "x".into() },
        ];
        assert_eq!(
            preset_variables(&cmds, &defaults),
            vec![Variable { name: "branch".into(), default: String::new() }, Variable { name: "repo".into(), default: "r".into() }]
        );
    }

    #[test]
    fn substitute_single_quotes_values() {
        let v = vec![VariableValue { name: "b".into(), value: "it's $(rm -rf ~); `x`".into() }];
        assert_eq!(substitute("git checkout {{b}} && echo {{ b }}!", &v).unwrap(), r"git checkout 'it'\''s $(rm -rf ~); `x`' && echo 'it'\''s $(rm -rf ~); `x`'!");
        assert_eq!(substitute("echo '{{.Name}}' {{}}", &[]).unwrap(), "echo '{{.Name}}' {{}}");
        let e = substitute("echo {{missing}}", &v).unwrap_err().to_string();
        assert!(e.contains("{{missing}} has no value"), "{e}");
    }

    #[test]
    fn prepare_uses_given_then_default_and_rejects_the_rest() {
        let p = preset_with(&["git clone {{repo}}", "git checkout {{branch}}"], &[("branch", "main"), ("repo", "")]);
        let r = prepare(&p, &given(&[("repo", "https://x/y.git")])).unwrap();
        assert_eq!(r.commands, vec!["git clone 'https://x/y.git'", "git checkout 'main'"]);
        assert_eq!(
            r.variables,
            vec![VariableValue { name: "repo".into(), value: "https://x/y.git".into() }, VariableValue { name: "branch".into(), value: "main".into() }]
        );
        // A value given beats the default; an empty one falls back to it.
        let r = prepare(&p, &given(&[("repo", "a"), ("branch", "dev")])).unwrap();
        assert_eq!(r.commands[1], "git checkout 'dev'");
        let r = prepare(&p, &given(&[("repo", "a"), ("branch", "")])).unwrap();
        assert_eq!(r.commands[1], "git checkout 'main'");
        // No value and no default.
        assert_eq!(prepare(&p, &given(&[])).unwrap_err().to_string(), Error::Provider("give repo a value".into()).to_string());
        assert!(prepare(&p, &given(&[("repo", "")])).is_err());
        // A name no command uses.
        let e = prepare(&p, &given(&[("repo", "a"), ("nope", "1")])).unwrap_err().to_string();
        assert!(e.contains("has no variable nope"), "{e}");
        // A placeholder the stored list misses (an older preset) is still asked for.
        let old = preset_with(&["echo {{x}}"], &[]);
        assert!(prepare(&old, &given(&[])).is_err());
        assert_eq!(prepare(&old, &given(&[("x", "1")])).unwrap().commands, vec!["echo '1'"]);
    }

    #[test]
    fn older_records_still_load() {
        let p: Preset = serde_json::from_str(r#"{"id":"p","name":"n","commands":["ls"],"secretNames":[],"createdAt":1,"updatedAt":2}"#).unwrap();
        assert!(p.variables.is_empty());
        let r: ProvisionRun = serde_json::from_str(
            r#"{"id":"r","machineId":"m","machineName":"b","presetId":"p","presetName":"n","status":"done","error":null,"steps":[],"secretNames":[],"secretsWritten":true,"output":"","startedAt":1,"finishedAt":2}"#,
        )
        .unwrap();
        assert!(r.variables.is_empty());
    }

    #[test]
    fn substituted_values_reach_bash_verbatim() {
        if std::process::Command::new("bash").arg("-c").arg("true").status().is_err() {
            return;
        }
        let nasty = "a'b \"c\" $HOME `id` $(echo no); *\nline2 \\";
        let p = preset_with(&["printf '[%s]\\n' {{v}} x{{v}}y"], &[]);
        let r = prepare(&p, &given(&[("v", nasty)])).unwrap();
        let out = std::process::Command::new("bash").arg("-c").arg(&r.commands[0]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("[{nasty}]\n[x{nasty}y]\n"));
    }

    #[test]
    fn markers_need_the_nonce() {
        assert_eq!(
            parse_line("::stowbox-abc:: exit 2 127", "abc"),
            Line::Exit(2, 127)
        );
        assert_eq!(
            parse_line("::stowbox-xyz:: exit 2 127", "abc"),
            Line::Output("::stowbox-xyz:: exit 2 127".into())
        );
        assert_eq!(
            parse_line("::stowbox-abc:: exit x", "abc"),
            Line::Output("::stowbox-abc:: exit x".into())
        );
    }

    #[test]
    fn redact_masks_long_values_only() {
        let s = vec![
            ("TOKEN".to_string(), "ghp_abcdef".to_string()),
            ("N".to_string(), "1".to_string()),
        ];
        assert_eq!(
            redact("push with ghp_abcdef to 1 remote", &s),
            "push with [TOKEN] to 1 remote"
        );
    }

    fn sample_machine() -> Machine {
        Machine {
            id: "vm1".into(),
            name: "box".into(),
            status: MachineStatus::Running,
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

    fn sample_run() -> ProvisionRun {
        let m = sample_machine();
        let p = Preset {
            id: "p1".into(),
            name: "main".into(),
            commands: vec!["a".into(), "b".into(), "c".into()],
            secret_names: vec!["TOKEN".into()],
            variables: vec![],
            created_at: 0,
            updated_at: 0,
        };
        let prepared = prepare(&p, &Default::default()).unwrap();
        ProvisionRun::new("r1".into(), &m, &p, &prepared, 10)
    }

    #[test]
    fn run_reducer_marks_failure_and_skips_the_rest() {
        let mut r = sample_run();
        for ev in [
            RunEvent::Running,
            RunEvent::SecretsWritten,
            RunEvent::StepStart(0),
            RunEvent::StepExit(0, 0),
            RunEvent::StepStart(1),
            RunEvent::StepExit(1, 2),
            RunEvent::Finished(2),
        ] {
            r.apply(&ev, 99);
        }
        assert_eq!(r.status, RunStatus::Failed);
        assert!(r.secrets_written);
        assert_eq!(r.error.as_deref(), Some("step 2 exited with code 2"));
        assert_eq!(
            r.steps.iter().map(|s| s.state).collect::<Vec<_>>(),
            vec![StepState::Ok, StepState::Failed, StepState::Skipped]
        );
        assert_eq!(r.finished_at, Some(99));
    }

    #[test]
    fn run_reducer_done_and_output_cap() {
        let mut r = sample_run();
        r.apply(&RunEvent::Running, 0);
        for i in 0..3 {
            r.apply(&RunEvent::StepStart(i), 0);
            r.apply(&RunEvent::StepExit(i, 0), 0);
        }
        r.apply(&RunEvent::Output("é".repeat(OUTPUT_CAP)), 0);
        assert!(r.output.len() <= OUTPUT_CAP);
        r.apply(&RunEvent::Finished(0), 5);
        assert_eq!(r.status, RunStatus::Done);
        assert!(r.error.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_end_to_end_with_local_bash() {
        if std::process::Command::new("bash").arg("-c").arg("true").status().is_err() {
            return;
        }
        let home = std::env::temp_dir().join(format!("stowbox-run-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let p = crate::testing::LocalBash { home: home.clone(), env: vec![] };
        let commands = vec!["echo token is $API_TOKEN".to_string(), "false".to_string(), "echo never".to_string()];
        let secrets = vec![("API_TOKEN".to_string(), "tok-12345".to_string())];
        let mut events = Vec::new();
        run(&p, "vm1", &commands, &secrets, |ev| events.push(ev)).await;

        let mut r = sample_run();
        r.steps = commands
            .iter()
            .map(|c| StepRun { command: c.clone(), state: StepState::Pending, exit_code: None })
            .collect();
        for ev in &events {
            r.apply(ev, 1);
        }
        assert_eq!(r.status, RunStatus::Failed, "{events:?}");
        assert!(r.secrets_written);
        assert_eq!(r.steps.iter().map(|s| s.state).collect::<Vec<_>>(), vec![StepState::Ok, StepState::Failed, StepState::Skipped]);
        assert_eq!(r.error.as_deref(), Some("step 2 exited with code 1"));
        assert!(r.output.contains("$ echo token is $API_TOKEN\ntoken is [API_TOKEN]\n"), "{}", r.output);
        assert!(!r.output.contains("tok-12345"));
        assert!(!r.output.contains("never"));
        let _ = std::fs::remove_dir_all(&home);
    }

}
