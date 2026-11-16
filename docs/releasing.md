# Release builds

`.github/workflows/release.yml` builds the installers with
[tauri-action](https://github.com/tauri-apps/tauri-action) on three runners:

| Runner | Output |
| --- | --- |
| `macos-14` | universal `.dmg` (Apple Silicon and Intel in one app) |
| `ubuntu-22.04` | `.AppImage` and `.deb` |
| `windows-2022` | `.msi` and NSIS `-setup.exe` |

It runs only on a `v*` tag and by hand, never on pull requests or pushes:
a first run with a cold Rust cache took 6 min on macOS, 12 on Linux and 14
on Windows; the cache makes later runs shorter. A newer run on the same ref cancels the older one.

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

## Auto-update

Installed apps read
`https://github.com/edumntg/stowbox/releases/latest/download/latest.json`
when they start and every 6 hours, and offer the release it names in the
status bar when it is newer than their own version
(`tauri-plugin-updater`). Each update bundle is signed; the app refuses one
whose signature does not match the public key in `src-tauri/tauri.conf.json`
(`plugins.updater.pubkey`).

- **Where the files come from.** `release.yml` passes
  `--config src-tauri/tauri.updater.conf.json`, which turns on
  `bundle.createUpdaterArtifacts`, and signs with the repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
  tauri-action uploads the signed bundles (`.app.tar.gz` on macOS, the
  AppImage on Linux, the NSIS and MSI installers on Windows) and
  `latest.json` to the release. Builds without the overlay, local ones and
  the smoke test, need no key.
- **When apps see it.** `releases/latest` skips drafts: installed apps see a
  release once it is published, not when the workflow makes the draft.
- **Platforms.** macOS, Windows, and the Linux AppImage. A `.deb` cannot
  replace itself; its users install the next `.deb`.
- **The key.** It was made with
  `pnpm tauri signer generate -w ~/.tauri/stowbox.key -p <password>`. Keep
  the key and its password somewhere safe besides the secrets: a lost key
  means a new one, and apps that trust the old public key will not install
  updates signed with the new one; they need one manual install.
- **Releases must be public.** The updater downloads without a GitHub login,
  so the repository (or wherever releases are published) must be public.
