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
  box near its memory or disk limit, the daily spend warning, a box the
  idle guard put in standby), and one row per box with its agents
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
- **Queued messages**: while a turn runs, the composer queues what you
  write instead of sending it. Queued messages show under the transcript,
  can be edited or removed until they go out, and are sent one per turn,
  each only once the previous turn has really ended (finished or failed;
  a reconnecting turn is still running). The queue is saved with the
  agent, so it carries on after the window reloads or Stowbox restarts.
  Stop pauses it: nothing more goes out until you press *Send next* (the
  rest then follow turn by turn) or clear it; a queued message that cannot
  be sent goes back to the front and pauses the queue the same way. A turn
  with more queued after it does not notify; the last one does. A queue
  that will send on its own keeps the box from idle standby.
- **Attachments**: attach files with the paperclip, paste images or drop
  files on the composer. Each one is copied into the box right away, to
  `~/.stowbox/uploads/<agent>/<tag>-<name>` (boxd's `UploadFile`, streamed
  with `UploadFileStream` over 3 MB), and the prompt lists the paths, so
  Claude Code or Codex reads the file from there (Claude Code opens images
  from a path). Up to 10 files of 20 MB each per message; a file over the
  limit, or an upload boxd refuses, shows the reason on the file and
  blocks sending until you remove it. Nothing falls back to another route.
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
- **Ports**: a tab on every box for seeing a dev server in the browser.
  It lists the box's HTTPS links (boxd proxies): the box address
  `https://<box>.<domain>`, which serves the first of 80, 443, 8080, 8000,
  3000, 5000 and 5173 that something listens on (else 8000) unless a port
  is pinned, and named links `https://<name>.<box>.<domain>` pinned to one
  port. Open or copy a link, add one (port and name), remove one, pin the
  box address to a port or let boxd detect it again. Raw TCP/UDP forwards
  (boxd's `ExposePort`, up to three per box, no TLS) are listed, added and
  removed in the same tab for databases and other non-HTTP servers. The
  addresses shown are the ones boxd returns; when it returns none the row
  says so instead of building one. The tab also lists what listens inside
  the box (`ss -ltnp`, or `/proc/net/tcp` where `ss` is missing, read
  only), marks a server bound to localhost only, which a link cannot
  reach, and offers *Create link* for the others. Before anything is
  exposed the form says it is public: anyone with the address reaches the
  server, and Stowbox puts no login in front of it. Links and forwards are
  read without waking the box; listing its ports runs a command in it, so
  a box in standby is only read when you ask. The command palette has
  *Ports and links of <box>* and *Open <box> in the browser*.
- **Terminal**: an interactive login shell in the box over boxd's exec
  stream (xterm.js on this side). Standby and hibernated boxes wake on open.
- **Cost**: account balance and not-yet-charged usage in the sidebar, metered
  usage per box with hours in each power state in the Details tab.
- **Cost guards** (Settings, both off by default): *Idle standby* puts a
  running box in standby after N minutes (30 by default) in which nothing
  happened in it: no agent starting, working or reconnecting (a detached
  turn counts until its exit is read, with or without a client attached)
  or holding queued messages it will send,
  no setup run, no terminal open, and no agent update, setup run, terminal
  keystroke or Changes action in the window. The idle clock never starts
  before Stowbox first saw the box running, and restarts when a box wakes.
  A 30 s countdown with Undo comes first, and the box is checked again at
  the end of it; Undo restarts the clock. A box can opt out in its Details
  tab. Stowbox only sees its own agents and terminals: an editor or `ssh`
  session opened elsewhere is invisible to it, so opt such a box out. The
  guard runs while Stowbox is open. *Spend warning* compares today's
  spend with a limit: boxd's metered usage of the org since local
  midnight when the key can read it (org owners and admins; it lags up to
  25 minutes), otherwise the current hourly estimate projected over a full
  day, and the warning says which. It shows in the Fleet alerts and once a
  day as a notification. The boxes the guard put in standby are listed in
  the alerts until they wake.
- **Setup presets**: a named list of commands plus secrets (for example
  "main commands": export `GITHUB_TOKEN`, `git clone`, `docker compose up -d`,
  run a script). Pick one in *New box* and it runs in the box as soon as it
  boots, or run it later from the box's Setup tab. Secret values stay in the
  OS keychain on this computer; in the box they are written to
  `~/.config/stowbox/env.sh` (mode 0600), which `~/.profile` and `~/.bashrc`
  load, so terminals and agents see them too. The script travels over boxd's
  exec stream on stdin, never on a command line, and secret values are
  masked in the setup log. Commands run in order in one shell (`cd` and
  `export` carry over); the first non-zero exit stops the run. A command can
  hold `{{name}}` placeholders (`git checkout {{branch}}`): the editor lists
  them with an optional default, and *New box* or *Run* asks for each value
  (required when there is no default). A value goes in as one single-quoted
  shell word, so keep placeholders outside quotes; other text in braces,
  such as `{{.State}}` for `docker inspect`, is left alone. Values are not
  secrets: the run keeps them and they show in the log.
- **Desktop**: one click opens boxd's live desktop view in the browser.
- **Open in VS Code or Cursor**: *Editor* on a box (and the code button in
  an agent's box panel, at the agent's directory or worktree) opens the
  folder over Remote-SSH with a `vscode://` or `cursor://` link. The dialog
  lists what will change before anything does, and nothing is written until
  you click Open: Stowbox makes its own key at `~/.ssh/stowbox/id_ed25519`
  with `ssh-keygen` if there is none (an existing one is never replaced, and
  your other keys are not read), links it to your boxd account with
  `LinkSshKey` (one Stowbox key per computer: linking again replaces it),
  and writes `Host stowbox-<box>` (the box's address, its SSH port on
  boxd's proxy, user `boxd`, that key) between `# BEGIN stowbox` and
  `# END stowbox` in `~/.ssh/config`. The block goes at the top of the file
  the first time, so its values win over a `Host *` further down; later
  runs replace it in place; nothing outside the markers changes, the file
  before each change is kept as `~/.ssh/config.stowbox-backup`, and a
  symlinked config is written through to its target. The proxy's host key
  goes in `~/.ssh/stowbox/known_hosts`, so the first connection does not
  stop at a fingerprint prompt. A box in standby or hibernated is woken
  first; a stopped box has to be started. The editor needs its Remote - SSH
  extension. The Details tab shows `ssh -p <port> boxd@<box>.boxd.sh` with
  a copy button, and the dialog copies `ssh stowbox-<box>` once the entry
  is there.
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
- **Command palette and transcript search**: ⌘K on macOS, Ctrl+K
  elsewhere (or the search button in the top bar) opens one list to jump to
  Fleet, a box, an agent, a setup preset or Settings, and to run the common
  actions: new box, new agent, open a box's terminal, Changes, Setup or
  Details, standby, resume and the other power actions, open in VS Code or
  Cursor, theme and panels. Matching is fuzzy (words in any order, letters in
  order). From two characters on it also searches every agent's transcript
  on this computer, case-insensitively: messages, replies, reasoning, tool
  calls and their output, errors. Each match shows the agent, its box, the
  turn and a snippet with the match marked; Enter opens the agent's chat
  scrolled to that message, with a folded tool card opened. Transcripts are
  read line by line and files without the text are skipped without parsing;
  results stop at 5 per agent (its latest) and 50 in all. Transcript lines
  carry no time of their own, so the time shown is when the agent last did
  something. Inside the terminal Ctrl+K stays the shell's (kill to the end
  of the line); ⌘K types nothing there and works everywhere.
- Light and dark themes, keyboard reachable, honest empty, loading and error
  states everywhere.

## Install and run on the desktop

Installers come from the release workflow (see [Release builds](#release-builds)):
a `v*` tag attaches them to a draft GitHub release. Or build it from this repo:

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

## Release builds

`.github/workflows/release.yml` builds the installers with
[tauri-action](https://github.com/tauri-apps/tauri-action) on three runners:

| Runner | Output |
| --- | --- |
| `macos-14` | universal `.dmg` (Apple Silicon and Intel in one app) |
| `ubuntu-22.04` | `.AppImage` and `.deb` |
| `windows-2022` | `.msi` and NSIS `-setup.exe` |

It runs only on a `v*` tag and by hand, never on pull requests or pushes,
because macOS minutes bill at 10x and Windows at 2x on a private repo. A
first run with a cold Rust cache took 6 min on macOS, 12 on Linux and 14 on
Windows, about 100 billed minutes in all (6x10 + 12 + 14x2); the cache makes
later runs shorter. A newer run on the same ref cancels the older one.

- **Release**: bump `version` in `src-tauri/tauri.conf.json`, `package.json`
  and the workspace `Cargo.toml`, commit, then
  `git tag v0.2.0 && git push origin v0.2.0`. The workflow creates a draft
  release named after the tag with the installers attached; review it and
  publish it on GitHub.
- **Try a build without releasing**: Actions, Release, Run workflow (or
  `gh workflow run release.yml --ref <branch>`). The installers are attached
  to the run as artifacts for 7 days and no release is created.

### Signing (optional)

Without secrets the builds are unsigned. The Mac app is ad-hoc signed, so on
first launch macOS blocks it as from an unidentified developer instead of
calling it damaged; allow it under System Settings, Privacy & Security, Open
Anyway (or run `xattr -dr com.apple.quarantine /Applications/Stowbox.app`).
Windows SmartScreen warns on first launch; More info, Run anyway. To sign and notarize the Mac build, add
these repository secrets (Settings, Secrets and variables, Actions); the
workflow picks them up when they exist and skips them when they do not:

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | A "Developer ID Application" certificate exported from Keychain Access as `.p12`, then `base64 -i cert.p12 \| pbcopy` |
| `APPLE_CERTIFICATE_PASSWORD` | The password chosen when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | The certificate name, e.g. `Developer ID Application: Your Name (TEAMID)` (`security find-identity -v -p codesigning`) |
| `APPLE_ID` | The Apple ID email of the developer account, for notarization |
| `APPLE_PASSWORD` | An app-specific password for that Apple ID, from [account.apple.com](https://account.apple.com) |
| `APPLE_TEAM_ID` | The 10-character team ID from the developer account |

The certificate needs a paid Apple Developer Program membership. With the
first three set the app is signed; with all six it is also notarized.
Windows code signing is not wired: it needs a certificate from a CA and a
`bundle.windows` section in `tauri.conf.json`
([tauri.app/distribute/sign/windows](https://tauri.app/distribute/sign/windows/)).

### Auto-update (not wired yet)

The Tauri updater is left out on purpose. `tauri-plugin-updater` refuses to
start without `plugins.updater.pubkey` in `tauri.conf.json`, so it cannot be
switched on only when a secret exists, and its download URL must be reachable
without a GitHub login, which release assets of a private repository are not.
To add it once releases are public (or mirrored somewhere public):

1. `pnpm tauri signer generate -w ~/.tauri/stowbox.key` and keep the password.
2. Add the repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the contents of
   `~/.tauri/stowbox.key`) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and pass
   both as `env` to the tauri-action step.
3. Add `tauri-plugin-updater` to `src-tauri/Cargo.toml` and register it in
   `src-tauri/src/lib.rs`; in `tauri.conf.json` set
   `bundle.createUpdaterArtifacts: true` and `plugins.updater` with `pubkey`
   (the contents of `~/.tauri/stowbox.key.pub`) and `endpoints`
   (`https://github.com/edumntg/stowbox/releases/latest/download/latest.json`).
   tauri-action then uploads the signed bundles and `latest.json` to the
   release.

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
