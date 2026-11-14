# Windows

Windows 10 and 11, 64-bit.

- **Running it**: the `.msi` or `-setup.exe` from a release. It needs the
  WebView2 runtime, which Windows 11 has; on Windows 10 the installer
  downloads it if it is missing. SmartScreen warns about an unsigned app on
  first launch: More info, Run anyway.
- **Building it**: Rust with the MSVC toolchain (the rustup default on
  Windows), the Visual Studio Build Tools with "Desktop development with
  C++" (MSVC and a Windows SDK), Node 22+ and pnpm, then the same
  `pnpm install` and `pnpm tauri dev` as in [development.md](development.md), in PowerShell.
- **Open in VS Code or Cursor** needs Windows' OpenSSH Client: Settings,
  System, Optional features (Windows 10: Settings, Apps, Optional features),
  or `Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0` in
  PowerShell as administrator. Stowbox runs its `ssh-keygen` from
  `C:\Windows\System32\OpenSSH`, the OpenSSH VS Code uses, and writes
  `%USERPROFILE%\.ssh\config`, the file that OpenSSH and Remote-SSH read.
  The entries name files as `~/.ssh/...`, so a user name with spaces or
  accents never ends up in the file. ssh-keygen gives the key an ACL for your
  user only, which Windows' OpenSSH requires.
- **Secrets**: the API key goes in Windows Credential Manager. A preset's
  secrets over 1280 characters in all (a private key as a value, say) do
  not fit there and go to a file in `%APPDATA%\sh.stowbox.desktop`, which
  your user, SYSTEM and administrators can read; Windows has no `chmod
  600`.
- **CI**: every pull request runs `cargo test -p stowbox-core` and
  `cargo check --workspace` on `windows-latest`. Tests of the scripts that
  run inside a box are ignored there: boxes are Linux, and `bash` on
  Windows is not the box's. `.github/workflows/windows-smoke.yml` builds
  the app with WebView2's DevTools port open, starts it and drives it over
  the DevTools protocol (`.github/scripts/windows-smoke.mjs`, no driver to
  install) through onboarding with an invalid key, with screenshots
  attached to the run; it runs on pull requests that change `src/`,
  `src-tauri/` or `package.json`, and by hand. The same script runs here
  against `pnpm dev` and a Chromium started with
  `--remote-debugging-port=9222 --remote-allow-origins=*` (see its header).
