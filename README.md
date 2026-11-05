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

- **Fleet**: the landing view. Four tiles (running boxes, working agents,
  finished turns waiting for you, estimated cost per hour), alerts derived
  from the same data (a failed agent, a box billing with nothing to do, a
  box near its memory or disk limit), and one row per box with its agents
  as state squares, CPU, memory, disk free and uptime. The numbers are read
  inside each running box over the exec stream, since boxd has no
  telemetry API; boxes in standby or hibernation are not touched, because
  sampling would wake them, and keep their last reading with its age.
- **Boxes**: list, create (1, 2 or 4 vCPU), fork, standby, resume, hibernate,
  wake, stop, start, reboot, delete. Each row shows size and an estimated cost
  per hour for the box's *current* state, computed from boxd's live rate card.
- **Agents**: a Claude Code or Codex session pinned to a box and a working
  directory. Each message runs one headless turn inside the box; the session
  id is kept so the next message continues the same conversation. Text
  streams as the model types; tool calls and their output fold into cards;
  cost, turns and tokens accumulate per agent. Transcripts persist on disk.
  Turns keep running when Stowbox is closed, the laptop sleeps or the
  network drops: each one runs detached inside the box and writes its
  output to `~/.stowbox/turns/<agent>/<turn>/` there (the last five turns
  per agent are kept). Stowbox follows those files from the last line it
  applied, so after a drop or a restart it reconnects and picks up where it
  left off, with no line shown twice or skipped. While the box is in
  standby or hibernated the agent says so and waits instead of waking it;
  Stop kills the turn's process group in the box.
- **Own branch per agent**: in *New agent*, "Work on its own branch" makes
  a git worktree for the agent before its first turn, so several agents can
  work on one repository in the same box without touching each other's
  files. It runs `git worktree add -b stowbox/<slug> <repo>-wt/<slug> HEAD`
  next to the repository (from another agent's worktree it still lands
  next to the main one), named after the title or first task plus six
  characters of the agent id, and the agent works in the same subdirectory
  of it that it was pointed at. It starts from the commit checked out
  there; uncommitted changes stay where they were. The branch shows in the
  agent's header and box panel, and its Changes tab reads that worktree.
  Removing the agent offers to remove the worktree too (`git worktree
  remove`, the branch is kept); git refuses a worktree with uncommitted or
  untracked files, and the dialog shows git's message and asks before
  discarding them. A worktree already deleted by hand is only pruned from
  git's list.
- **Changes**: a tab on every agent (and on every box, pointed at any
  directory) that reads the repository in the agent's working directory
  inside the box: the branch, how far it is ahead of or behind its
  upstream, the changed files from `git status`, and the full diff against
  the last commit with untracked files shown as new. From there: commit
  everything with a message (`git add -A`, hooks run), push (setting the
  upstream on a branch's first push), and open a pull request. The pull
  request is created by `gh pr create --fill` in the box when `gh` is
  installed and signed in, a `GH_TOKEN` or `GITHUB_TOKEN` from a setup
  preset counts; otherwise the host's compare page opens in the browser
  (GitHub, GitLab, Bitbucket). With a token set, pushes to
  `https://github.com` remotes use it too. Nothing waits for a prompt: git
  runs with terminal prompts off, ssh in batch mode (unless you set
  `GIT_SSH_COMMAND` or `core.sshCommand`, which are kept) and no editor, and every
  call has a deadline. The diff is cut at 512 KB inside the box and the tab
  says so; refusals such as a rejected push show git's message verbatim. A
  box in standby or hibernated is only read when you ask, since that wakes
  it; the tab reads again when the agent's turn ends.
- **Terminal**: an interactive login shell in the box over boxd's exec
  stream (xterm.js on this side). Standby and hibernated boxes wake on open.
- **Cost**: account balance and not-yet-charged usage in the sidebar, metered
  usage per box with hours in each power state in the Details tab.
- **Setup presets**: a named list of commands plus secrets (for example
  "main commands": export `GITHUB_TOKEN`, `git clone`, `docker compose up -d`,
  run a script). Pick one in *New box* and it runs in the box as soon as it
  boots, or run it later from the box's Setup tab. Secret values stay in the
  OS keychain on this computer; in the box they are written to
  `~/.config/stowbox/env.sh` (mode 0600), which `~/.profile` and `~/.bashrc`
  load, so terminals and agents see them too. The script travels over boxd's
  exec stream on stdin, never on a command line, and secret values are
  masked in the setup log. Commands run in order in one shell (`cd` and
  `export` carry over); the first non-zero exit stops the run.
- **Desktop**: one click opens boxd's live desktop view in the browser.
- **Notifications**: when an agent's turn finishes or fails, or a setup run
  ends, the OS shows a notification, by default only while Stowbox is not
  the window in front (Settings has both switches). One turn notifies once;
  reconnecting and a pressed Stop do not notify, and nothing is announced
  for the first 10 s after launch, when reattached turns that ended while
  Stowbox was closed replay their last lines. Clicking a notification
  brings the window back and opens the agent or the box's Setup tab, where
  the notification server reports clicks. The dock or taskbar badge counts
  notifications shown since the window was last in front (macOS and Linux
  desktops that support badges).
- Light and dark themes, keyboard reachable, honest empty, loading and error
  states everywhere.

## Install and run on the desktop

There are no prebuilt installers yet; the app is built from this repo.

1. Install the toolchain:
   - Rust 1.80+ via [rustup](https://rustup.rs).
   - Node 22+ and pnpm (`npm install -g pnpm`); pnpm switches itself to the
     version pinned in `package.json`.
   - The Tauri 2 system packages for your OS: on macOS,
     `xcode-select --install`; on Linux and Windows, follow
     [tauri.app/start/prerequisites](https://tauri.app/start/prerequisites/).
2. Build and start it:

   ```sh
   git clone https://github.com/edumntg/stowbox && cd stowbox
   pnpm install
   pnpm tauri dev        # desktop app with hot reload; the first run compiles
                         # the Rust side and takes a few minutes
   ```

3. On first launch the app asks for a boxd API key (`bxd_…`), created at
   [app.boxd.sh](https://app.boxd.sh). It is stored in the OS keychain.

To install it as a regular app instead of running it from a terminal:

```sh
pnpm tauri build      # installers in target/release/bundle/
```

The build is unsigned. It opens fine on the machine that built it; a copy
downloaded onto another Mac needs `xattr -dr com.apple.quarantine Stowbox.app`
before Gatekeeper lets it open.

## Develop

The UI can also run in a plain browser against an in-memory mock, which is how
it is developed and screenshotted without a boxd account:

```sh
pnpm dev              # http://localhost:1420, mock data, nothing real
```

Rust side:

```sh
cargo test -p stowbox-core   # parsers, cost math, auth helpers
cargo check --workspace
BOXD_API_KEY=bxd_… cargo run -p stowbox-core --example smoke   # against a real account
```

The smoke example makes read-only calls, then creates one `stowbox-smoke-*`
machine, runs every provider operation on it (exec, tty, cancel, standby,
hibernate, usage, fork, a Claude Code turn when the box can authenticate) and
destroys it. It never touches existing machines. boxd refuses to create
machines until the org has a payment method connected in the console, even
with a credit balance.

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
  kills the remote process group, which is how a terminal closes. Agent
  turns are started with `setsid` in their own session so they outlive the
  stream that launched them; a second exec follows their output files with
  `tail -f`, and Stop kills the turn's process group by the pid it wrote.
- Costs: the sidebar estimate multiplies the box's vCPU, RAM and disk by the
  rate card from `GetOrgUsage`; running boxes pay all three, standby pays RAM
  and disk, hibernated and stopped pay disk only. Metered numbers come from
  `GetVmUsage` and lag by up to about 25 minutes.
- The API key is kept in the OS keychain; when no keychain is reachable it is
  written to a file only your user can read, and Settings says which.

Measured against a real account on 2026-10-13 with the smoke example: a box
is created and running in about 0.3 s (boot time reported as 4 to 5 ms),
standby and resume are instant, hibernate takes 5 to 6 s, wake about 0.1 s,
and dropping an exec stream kills the whole remote process group within
half a second. `GetOrgBilling` is refused for API keys; the sidebar balance
comes from `Whoami`. The rate card is in the org's currency, which was USD
there, so the published EUR card is only a fallback. Pausing a box within
about 300 ms of waking it from hibernation, then resuming within about
250 ms, fails the first resume with an Internal error; the second attempt
works and nothing is lost. Details in `docs/boxd-resume-after-wake-race.md`.

## Not yet

- Interactive tool approvals mid-turn (Claude Code's stdio control protocol).
- Snapshots, checkpoints, env and secret editing, port exposure.
- Other providers behind the `Provider` trait.

## Credits

Icons are [Phosphor Icons](https://phosphoricons.com) (bold weight), MIT,
inlined in `src/icons`. Terminal rendering by [xterm.js](https://xtermjs.org).
