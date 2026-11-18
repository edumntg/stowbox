# Working in this repo

- Everything in the repo is in English: code, comments, commits, docs.
- `crates/stowbox-core` must keep building without Tauri or a GUI; put
  anything provider- or protocol-shaped there, with tests.
- The UI runs in a browser against `src/bridge/mock.ts` with `pnpm dev`.
  Every new command gets a mock implementation so screenshots and reviews do
  not need a boxd account.
- UI changes follow `DESIGN.md`. Status markers are squares. One accent.
  Real data or an honest empty state; never invented numbers.
- Before a commit: `pnpm build`, `cargo test -p stowbox-core`,
  `cargo check --workspace`.

## Security, performance and code quality

Stowbox is open source and holds people's provider keys and a shell into
their machines. Hold every change to this bar:

- **Memory safety.** No `unsafe`: the workspace lints forbid it. No
  `unwrap`, `expect`, `panic!`, indexing or string slicing on anything that
  came from the network, a file or the user. Return `crate::Error` instead.
  An `#[allow]` goes on one item, with a comment saying why it cannot fail.
  Tests may unwrap.
- **Untrusted input.** Provider responses, WebSocket and SSH streams, box
  output and agent hook output are hostile until parsed. Parse into typed
  structs, bound sizes before allocating, and validate names and paths
  before they reach a shell (`shell_quote`) or the filesystem. The webview
  renders text, never provider data as HTML.
- **Secrets.** Keys and tokens live in the OS keychain only. They never go
  in the store file, logs, errors, URLs or the repo, and a type that holds
  one prints `***` in `Debug`. TLS is always verified; SSH host keys are
  pinned or remembered, never ignored, except for an edge-authenticated
  WebSocket tunnel.
- **Performance.** No blocking work on the async runtime, no lock held
  across `.await`, a timeout on every network call, streaming instead of
  collecting, and background tasks that stop when nothing needs them.
- **Readability.** Match the surrounding code's naming, module layout
  (`api.rs`, `convert.rs`, `mod.rs` per provider), error type and comment
  density. Comments say why. No dead or commented-out code.
- **Dependencies.** Few, maintained, from crates.io or npm. They must pass
  `cargo audit`, `cargo deny check` and `pnpm audit --prod`.
- **Checks.** `.claude/hooks/post-edit-check.sh` runs after every Write and
  Edit; fix what it reports instead of working around it. Every phase of
  planned work, and every PR that adds a provider, a
  transport, a Tauri command or a dependency, passes the gate in
  `.claude/skills/phase-gate/SKILL.md`.
- **Disclosure.** Never describe an unfixed vulnerability in a commit,
  issue, PR or doc here; fix it first.
