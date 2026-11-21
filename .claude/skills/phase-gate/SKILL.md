---
name: phase-gate
description: Quality, security and performance gate for Stowbox work, run at the start and the end of every planned phase of work and before any commit or PR that adds or changes a provider, a transport, a Tauri command, or a dependency. Covers the mechanical checks (gate.sh), the code review, a security review with the security-audit and BugHunter skills, and the performance and readability pass.
---

# Phase gate

Stowbox is open source and holds people's provider keys and a shell into
their boxes. A phase of work (a milestone, or a feature spread over several
PRs) is done when this gate passes, not when the feature works.

## At the start of a phase

1. Re-read the plan for the phase, and `docs/<id>.md` for each provider it
   touches. Write that doc first if it is missing.
2. Name the trust boundaries the phase touches: provider API responses,
   WebSocket and SSH streams, the Tauri IPC surface, files written into a
   box, the keychain, the updater. These pick the review sections below.
3. Branch from main (`feature/<phase-or-provider>`), never work on main.

## While working

- Edit code with the Edit and Write tools, not with `sed` or heredocs in
  Bash: the PostToolUse hook in `.claude/settings.json`
  (`.claude/hooks/post-edit-check.sh`) runs only for them. It blocks on a
  broken build, on clippy findings on the lines you changed (unsafe is
  forbidden; unwrap, expect, panic, indexing and string slicing are flagged),
  on rustfmt drift, on tsc errors, on raw-HTML or eval sinks in the UI, on
  credential-looking strings, and on Tauri leaking into `stowbox-core`.
- When the hook blocks, fix the cause. Do not weaken the hook, add a blanket
  `#[allow]`, or move code around to dodge the changed-line check. An
  `#[allow(clippy::...)]` is fine only on one item, with a comment saying why
  it cannot fail.
- When the hook notes older warnings in a file you touched, clean the ones
  near your change in the same commit. Leave the rest alone.

## At the end of a phase: the gate

Run all of it, in order. Report each step's result as it is: a skipped step
is reported as skipped, with the reason.

### 1. Mechanical checks

```bash
.claude/skills/phase-gate/gate.sh            # base: merge base with main
.claude/skills/phase-gate/gate.sh <base-ref> # or an explicit base
```

It runs rustfmt, clippy on the changed lines, the per-file ceiling on the
safety lints (`clippy-ratchet.sh` against `clippy-baseline.tsv`: a file's
count never rises, and when it falls the baseline is lowered with
`clippy-ratchet.sh --update` in the same change), `cargo test -p stowbox-core`,
`cargo check --workspace`, the core-without-Tauri check, `pnpm build`,
`cargo audit`, `cargo deny check`, `pnpm audit --prod`, a secret scan on the
changed lines, and checks that provider SVGs are inert. Until the tree has
had its formatting-only commit, run it with `STOWBOX_FMT_BASELINE=1`. Every check
must pass before going on.

### 2. Tests that prove the phase

- Every provider passes `testing::conformance` against its fake server.
- Every parser of network input (provider JSON, WebSocket frames, Connect
  envelopes, SSH banners) has tests for truncated, oversized, wrong-type and
  hostile input, and a `cargo fuzz` target when it decodes a binary framing.
- Every new Tauri command has a mock in `src/bridge/mock.ts`, and the UI
  change has been looked at with `pnpm dev`.

### 3. Code review

Run the `code-review` skill on the phase diff at `high`, and fix or answer
every finding. Then run `simplify` for reuse and dead code.

### 4. Security review

Two third-party skill sets make this step sharper. They are not vendored
here; install them at the user level (`~/.claude/skills`) and read them
before trusting them:

- `security-audit` from <https://github.com/cloudflare/security-audit-skill>
  (`skills/security-audit/`).
- The `hunt-*` and `triage-validation` skills from
  <https://github.com/elementalsouls/Claude-BugHunter> (`skills/`). Copy the
  skill folders; its installer also edits shell startup files, which this
  gate does not need.

Without them, use the table below as the checklist and review the diff
against each row by hand.

Use the `security-audit` skill in guidance mode on the phase diff: a focused
review, with no full audit and no report files. Read the sections that
match the boundaries named at the start:

| Boundary in Stowbox | security-audit reference | BugHunter skills |
|---|---|---|
| Tauri commands, webview, local files, keychain, updater | `DESKTOP-MOBILE-AND-LOCAL-IPC.md`, `CLIENT-SIDE.md` | `hunt-xss`, `hunt-dom`, `hunt-lfi` |
| Commands and scripts sent into a box (`shell_quote`, bootstrap, presets) | `ATTACK-CLASSES.md` | `hunt-rce` |
| Uploads, worktrees, paths built from box or provider data | `ATTACK-CLASSES.md` | `hunt-lfi`, `hunt-file-upload` |
| Provider HTTP APIs and URLs built from their data | `WEB-PROTOCOL-AND-AUTH.md` | `hunt-ssrf`, `hunt-exceptional-conditions` |
| WebSocket transports (`wstunnel`, `wspty`), the Cloudflare bridge Worker | `PROTOCOLS-RPC-AND-MESSAGING.md` | `hunt-websocket` |
| gRPC and Connect-RPC (boxd, Modal, E2B envd) | `PROTOCOLS-RPC-AND-MESSAGING.md` | `hunt-grpc` |
| SSH host checks, tokens, key storage | `WEB-PROTOCOL-AND-AUTH.md`, `DATA-ISOLATION-AND-LIFECYCLE.md` | `hunt-session` |
| Timeouts, sizes, background loops, reconnects | `RESOURCE-EXHAUSTION-AND-AVAILABILITY.md` | `hunt-race-condition` |
| Agents in boxes, hook output Stowbox reads back | `AI-AND-LLM.md` | `hunt-llm-ai` |
| Dependencies, CI, release signing | `SUPPLY-CHAIN-AND-RELEASE.md` | `hunt-cicd`, `supply-chain-attack-recon` |

Use the BugHunter skills as checklists of attack patterns against the
source, in the person's own code and with dummy keys only. Never probe a
provider's live service or another account. Run every candidate through
`triage-validation` before calling it a finding.

Run a **full audit** (the security-audit skill's six phases) only at these
milestones, and only after the person says yes, since it runs many agents:
the end of a phase that adds a transport, and before the first release that
ships a new provider. Its output goes outside the repo
(`~/security-audit-skill/stowbox/run-<N>`).

Do not put details of an unfixed vulnerability in a commit, an issue, a PR
or a doc in this public repo. Fix it first, then describe the fix.

### 5. Performance and memory

- No blocking calls on the async runtime: file and keychain I/O, and
  anything that can take more than a millisecond, go through
  `spawn_blocking` or async APIs.
- No lock (`std::sync::Mutex` or `RwLock`) held across `.await`.
- Every network read and every exec has a timeout, and every buffer read
  from the network has a size bound before it is allocated.
- Streams are forwarded in chunks, never collected whole (uploads, logs,
  terminal output).
- Polling has a backoff and stops when nothing is watching; background tasks
  end when their box, tab or provider goes away.
- No per-keystroke or per-frame allocation in terminal paths. Clone `Arc`s,
  not data.
- For a transport, measure one terminal's round trip and an idle box's CPU
  use, and say the numbers in the PR.

### 6. Readability

- Code reads like the code around it: same naming, error type
  (`crate::Error`), module layout (`api.rs`, `convert.rs`, `mod.rs`) and
  comment density. Comments say why, not what.
- No dead code, no commented-out code, no TODO without an issue.
- English everywhere. Public items in core have a doc comment.
- UI follows `DESIGN.md`: square status markers, one accent, real data or an
  honest empty state.

### 7. Close the phase

- Update the plan's status and each `docs/<id>.md` the phase touched.
- Summarize the gate in the PR: each step, pass or fail, with the numbers
  from step 5.
- Commit only when the person asks.
