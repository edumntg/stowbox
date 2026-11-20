# Development

Installers come from the release workflow ([releasing.md](releasing.md)).
To run the app from this repo:

1. Install the toolchain:
   - Rust 1.89+ via [rustup](https://rustup.rs).
   - Node 22+ and pnpm (`npm install -g pnpm`); pnpm switches itself to the
     version pinned in `package.json`.
   - The Tauri 2 system packages for your OS: on macOS,
     `xcode-select --install`; on Linux, follow
     [tauri.app/start/prerequisites](https://tauri.app/start/prerequisites/);
     on Windows, see [windows.md](windows.md).
2. Build and start it:

   ```sh
   git clone https://github.com/edumntg/stowbox && cd stowbox
   pnpm install
   pnpm tauri dev        # desktop app with hot reload; the first run compiles
                         # the Rust side and takes a few minutes
   ```

3. On first launch the app asks for a boxd (`bxd_…`, from
   [app.boxd.sh](https://app.boxd.sh)), boat (`boat_…`), Daytona (`dtn_…`),
   Agent37 (`sk_live_…`) or E2B (`e2b_…`) API key. The others can be added later in Settings. Keys are stored in the OS keychain.

To install it as a regular app instead of running it from a terminal:

```sh
pnpm tauri build      # installers in target/release/bundle/
```

The build is unsigned. It opens fine on the machine that built it; a copy
downloaded onto another Mac needs `xattr -dr com.apple.quarantine Stowbox.app`
before Gatekeeper lets it open (macOS only).

## The browser build and the Rust side

The UI can also run in a plain browser against an in-memory mock, which is how
it is developed and screenshotted without a provider account:

```sh
pnpm dev              # http://localhost:1420, mock data, nothing real
```

The mock connects all three providers. `?providers=boxd`, or a list such as
`?providers=boat,daytona`, previews a subset, and `?onboarding` the first-run screen.

Rust side:

```sh
cargo test -p stowbox-core   # parsers, cost math, auth helpers
cargo test -p stowbox-core sshexec -- --ignored   # the SSH exec (boat, Daytona) against a local sshd
cargo check --workspace
STOWBOX_LIVE_BOAT_KEY=boat_… cargo test -p stowbox-core testing::live -- --ignored --nocapture   # conformance against the real service
BOXD_API_KEY=bxd_… cargo run -p stowbox-core --example smoke   # against a real account
```

The browser mock reads the providers from `src/bridge/providers.generated.json`.
A test fails when it no longer matches core; `STOWBOX_REGENERATE=1 cargo test
-p stowbox-core the_mock_gets_the_descriptors` writes the new one.

The live conformance run covers each provider whose
`STOWBOX_LIVE_<ID>_KEY` (`BOXD`, `BOAT`, `DAYTONA`, `AGENT37`, `E2B`) is set and
skips the rest. For each one it makes a `stowbox-live-*` box in the size New
box starts on (`STOWBOX_LIVE_<ID>_SIZE` picks another), checks exec with
stdin and a TTY, an upload, every power action and fork the provider claims,
and HTTPS links, then deletes every box it made, also when a step fails. It
costs a few minutes of that size, and never runs in CI.

The smoke example makes read-only calls, then creates one `stowbox-smoke-*`
machine, runs every provider operation on it (exec, tty, cancel, standby,
hibernate, usage, fork, a Claude Code turn when the box can authenticate) and
destroys it. It never touches existing machines. boxd refuses to create
machines until the org has a payment method connected in the console, even
with a credit balance.

Screenshots for the README come from that browser build, over the DevTools
protocol, so they always show the same mock data:

```sh
pnpm dev
chromium --headless=new --hide-scrollbars --force-device-scale-factor=2 --window-size=1280,800 \
  --remote-debugging-port=9222 --remote-allow-origins=* http://localhost:1420/
node .github/scripts/screenshots.mjs docs/screenshots
```

## CI

`.github/workflows/ci.yml` runs the three checks from `AGENTS.md` on every
pull request and push to `main`, on Ubuntu, plus the Rust ones on Windows.
`.github/workflows/windows-smoke.yml` builds the real Windows exe and drives
it through onboarding; see [windows.md](windows.md).

## Checks for coding agents

The repo carries a Claude Code setup in `.claude/` that holds agent-written
changes to the bar in `AGENTS.md`:

- `.claude/settings.json` registers `.claude/hooks/post-edit-check.sh` to
  run after every Write and Edit. It blocks the change when the crate stops
  building, when clippy flags the lines it touched (the workspace lints in
  `Cargo.toml` forbid `unsafe` and flag `unwrap`, `expect`, `panic!`,
  indexing and string slicing), on rustfmt drift, on tsc errors, on raw-HTML
  or eval sinks in the UI, on credential-looking strings, and when
  `stowbox-core` starts depending on Tauri.
- `.claude/skills/phase-gate/` is the gate for each phase of planned work
  and for PRs that add a provider, a transport, a Tauri command or a
  dependency: `gate.sh` runs the mechanical checks against the merge base
  with `main`, and `SKILL.md` lists the review steps that follow.

You can run either one without Claude Code:

```bash
echo '{"tool_input":{"file_path":"'"$PWD"'/crates/stowbox-core/src/cost.rs"}}' | bash .claude/hooks/post-edit-check.sh
bash .claude/skills/phase-gate/gate.sh
```

They need bash 3.2 or later, git, jq and awk, plus `cargo install --locked
cargo-audit cargo-deny` for the gate. On macOS, `brew install coreutils`
adds the `gtimeout` the hook uses to bound clippy. Your own Claude Code
settings go in `.claude/settings.local.json`, which git ignores; to turn the
hook off for yourself, set `"disableAllHooks": true` there.

## Layout

```
crates/stowbox-core/   provider trait, registry, capabilities, agent protocol, cost, store
  src/boxd/            boxd's gRPC client
  src/boat/            boat's REST client and how its sandboxes are reached over SSH
  src/daytona/         Daytona's REST client, states and SSH gateway login
  src/agent37/         Agent37's REST client, states, new-box install and SSH through its edge
  src/e2b/             E2B's REST client, states and exec through envd
  src/connectrpc.rs    a Connect protocol client for protobuf services (E2B's envd)
  src/sshexec.rs       exec over SSH (russh) for providers without a usable exec API
  src/wstunnel.rs      a byte stream over a WebSocket, for SSH through a provider's edge
  proto/boxd/api.proto boxd's public API, compiled with tonic at build time
  proto/e2b/           envd's process service, messages only
src-tauri/             the desktop shell: state, commands, agent turn runner, terminals
src/                   SolidJS UI (bridge/, state/, ui/, icons/, styles/)
```

`stowbox-core` has no Tauri dependency, so it builds and tests on a headless
box. Every provider implements the `Provider` trait and describes itself with
`Capabilities`; the `Registry` holds every connected one and routes each box to
the provider that owns it. The UI turns features on and off from the
capabilities, never from the provider's name. How boat, Daytona, Agent37, E2B
and a plain SSH host are used, and what they do not have, is in
`docs/boat.md`, `docs/daytona.md`, `docs/agent37.md`, `docs/e2b.md` and
`docs/sshhost.md`.

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
- On boat: opening a box in an editor, raw TCP ports, saved snapshots and
  environments (see `docs/boat.md`).
- On Daytona: VM-class sandboxes (pause, fork), snapshots, volumes, metered
  usage (see `docs/daytona.md`).
- On Agent37: `vm` and `performance` instances, regions, resize, backups and
  opening a box in an editor (see `docs/agent37.md`).
