# Development

Installers come from the release workflow ([releasing.md](releasing.md)).
To run the app from this repo:

1. Install the toolchain:
   - Rust 1.80+ via [rustup](https://rustup.rs).
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
   [app.boxd.sh](https://app.boxd.sh)) or boat (`boat_…`) API key. Either can
   be added later in Settings. Keys are stored in the OS keychain.

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

The mock connects both providers. `?providers=boxd` or `?providers=boat`
previews one, and `?onboarding` the first-run screen.

Rust side:

```sh
cargo test -p stowbox-core   # parsers, cost math, auth helpers
cargo test -p stowbox-core boat::ssh -- --ignored   # boat's SSH exec against a local sshd
cargo check --workspace
BOXD_API_KEY=bxd_… cargo run -p stowbox-core --example smoke   # against a real account
```

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

## Layout

```
crates/stowbox-core/   provider trait, registry, capabilities, agent protocol, cost, store
  src/boxd/            boxd's gRPC client
  src/boat/            boat's REST client and SSH exec (russh)
  proto/boxd/api.proto boxd's public API, compiled with tonic at build time
src-tauri/             the desktop shell: state, commands, agent turn runner, terminals
src/                   SolidJS UI (bridge/, state/, ui/, icons/, styles/)
```

`stowbox-core` has no Tauri dependency, so it builds and tests on a headless
box. Every provider implements the `Provider` trait and describes itself with
`Capabilities`; the `Registry` holds every connected one and routes each box to
the provider that owns it. The UI turns features on and off from the
capabilities, never from the provider's name. How boat is used, and what it
does not have, is in `docs/boat.md`.

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
