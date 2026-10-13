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
