# Stowbox

A desktop cockpit for cloud boxes and the coding agents running in them.

Stowbox talks to [boxd](https://boxd.sh) with **your** API key, from your
computer. It lists your boxes with a live cost estimate, starts Claude Code or
Codex sessions inside them, streams the conversation back as chat, opens a
shell in any box without installing anything, and lets you pause, hibernate,
fork or delete a box from the same window. No Stowbox server exists: the key,
the boxes and the transcripts never leave your machine and boxd.

Built with Tauri 2 (Rust) and SolidJS, so it starts in a blink and idles at a
few tens of megabytes instead of an Electron process per window.

## What it does today

- **Boxes**: list, create (1, 2 or 4 vCPU), fork, standby, resume, hibernate,
  wake, stop, start, reboot, delete. Each row shows size and an estimated cost
  per hour for the box's *current* state, computed from boxd's live rate card.
- **Agents**: a Claude Code or Codex session pinned to a box and a working
  directory. Each message runs one headless turn inside the box; the session
  id is kept so the next message continues the same conversation. Text
  streams as the model types; tool calls and their output fold into cards;
  cost, turns and tokens accumulate per agent. Transcripts persist on disk.
- **Terminal**: an interactive login shell in the box over boxd's exec
  stream (xterm.js on this side). Standby and hibernated boxes wake on open.
- **Cost**: account balance and not-yet-charged usage in the sidebar, metered
  usage per box with hours in each power state in the Details tab.
- **Desktop**: one click opens boxd's live desktop view in the browser.
- Light and dark themes, keyboard reachable, honest empty, loading and error
  states everywhere.

## What the agent needs inside the box

Stowbox does not handle Anthropic or OpenAI credentials. The harness signs in
with whatever the box already has:

- an `ANTHROPIC_API_KEY` (or `OPENAI_API_KEY`) set as a boxd secret:
  `boxd env set ANTHROPIC_API_KEY sk-ant-… --secret`
- or a `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`
- or the Claude Code / Codex login boxd stores for your organization

Claude Code, Codex and OpenCode come preinstalled on boxd machines. Turns run
with `--dangerously-skip-permissions` by default because the box is a
disposable VM; untick "Run tools without approval prompts" when creating the
agent to have permission-needing tools denied instead.

## Run it

Requirements: Rust 1.80+, Node 22+, pnpm, and the
[Tauri 2 system packages](https://tauri.app/start/prerequisites/) for your OS.

```sh
pnpm install
pnpm tauri dev        # desktop app with hot reload
pnpm tauri build      # installers in src-tauri/target/release/bundle
```

The UI can also run in a plain browser against an in-memory mock, which is how
it is developed and screenshotted without a boxd account:

```sh
pnpm dev              # http://localhost:1420, mock data, nothing real
```

Rust side:

```sh
cargo test -p stowbox-core   # parsers, cost math, auth helpers
cargo check --workspace
```

## Layout

```
crates/stowbox-core/   provider trait, boxd gRPC client, agent protocol, cost, store
  proto/boxd/api.proto boxd's public API, compiled with tonic at build time
src-tauri/             the desktop shell: state, commands, agent turn runner, terminals
src/                   SolidJS UI (bridge/, state/, ui/, icons/, styles/)
```

`stowbox-core` has no Tauri dependency, so it builds and tests on a headless
box. The `Provider` trait is the seam for adding boat.dev or Cloudflare
Sandboxes later.

## How boxd is used

- Auth: the `bxd_…` API key is exchanged for a one-hour JWT at
  `app.boxd.sh/api/v1/auth/token`; the JWT rides on every gRPC call to
  `boxd.sh:9443` as `authorization: Bearer …` and is refreshed early.
- Exec: one bidirectional `Exec` stream per terminal or agent turn. Commands
  are wrapped in `bash -lc`, so `cd`, pipes and env exports behave the same
  whether the server execs directly or through a shell. Dropping the stream
  kills the remote process group, which is how Stop works.
- Costs: the sidebar estimate multiplies the box's vCPU, RAM and disk by the
  rate card from `GetOrgUsage`; running boxes pay all three, standby pays RAM
  and disk, hibernated and stopped pay disk only. Metered numbers come from
  `GetVmUsage` and lag by up to about 25 minutes.
- The API key is kept in the OS keychain; when no keychain is reachable it is
  written to a file only your user can read, and Settings says which.

## Not yet

- Interactive tool approvals mid-turn (Claude Code's stdio control protocol).
- Snapshots, checkpoints, env and secret editing, port exposure.
- Other providers behind the `Provider` trait.

## Credits

Icons are [Phosphor Icons](https://phosphoricons.com) (bold weight), MIT,
inlined in `src/icons`. Terminal rendering by [xterm.js](https://xtermjs.org).
