# Release builds

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

## Signing (optional)

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

## Auto-update (not wired yet)

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
