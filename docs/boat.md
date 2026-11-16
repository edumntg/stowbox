# boat.dev

Stowbox drives [boat.dev](https://boat.dev) sandboxes next to boxd boxes.
Both providers can be connected at once, each with its own API key; every
box belongs to one of them and every call about a box goes to its owner.

## What boat offers next to boxd

| | boxd | boat.dev |
| - | - | - |
| API | gRPC, `bxd_` key traded for a JWT | REST + JSON, `boat_` key as bearer |
| Create | any vCPU and RAM up to the plan ceiling | four sizes: small, default, large, xlarge |
| Stop and start | yes | stop saves the disk as a snapshot; resume restores it |
| Standby, hibernate, wake | yes | no |
| Reboot | yes | no |
| Fork | yes | yes, from the newest snapshot |
| Delete | yes | yes, as a background operation |
| Auto timers | auto suspend, auto hibernate | auto-stop (TTL), 1 hour by default |
| Large files | `UploadFileStream`, 20 MB in Stowbox | signed upload URL, 512 MB in Stowbox |
| Exec with stdin and a TTY | yes, one gRPC stream | no: the command API has neither, so Stowbox execs over SSH |
| SSH | one key per account, a shared proxy, a fixed host key | one key per sandbox, its own address and host key, both new after every resume or fork |
| HTTPS links | default route plus named routes, listable | one URL per port, token-gated, not listable, not removable over the API |
| Raw TCP and UDP | ExposePort | not over the API |
| Isolated sandboxes | yes | no |
| Price | per vCPU, GiB of RAM and GiB of disk, EUR or USD | one hourly price per size, USD; stopped costs nothing |

## How Stowbox uses boat

- **Auth.** `Authorization: Bearer boat_…` on every call to
  `https://boat.dev/api/v1`. Older `box_` keys are accepted too. The key
  needs the `ssh` action, because every exec is an SSH session.
- **Exec.** Each exec asks `POST /sandboxes/{id}/sshkey` to authorize
  Stowbox's own ed25519 key on the sandbox, which answers with the address,
  the user and the host key. Stowbox connects with an in-process SSH client
  (`russh`, so Windows needs no OpenSSH), checks the host key against that
  answer, and runs `bash -lc` in a fresh channel. One connection per sandbox
  is reused; a failed connect or a changed host key asks boat again once.
  Dropping the exec kills the remote process group, as boxd does: the
  command runs under `setsid`, reports its group id on stderr first, and a
  short second channel sends it `SIGTERM` then `SIGKILL`.
- **Lifecycle.** Stop is `POST /stop`, start and resume are `POST /resume`,
  delete is `DELETE` with the `X-Ascii-Confirm-Delete` header. Standby,
  hibernate, wake and reboot are not offered for boat sandboxes. The idle
  guard stops a boat sandbox where it would put a boxd box in standby.
- **Auto-stop.** New sandboxes get the auto-stop picked in the New box
  dialog; Details shows when it falls due and can push it out. A stopped
  sandbox loses running processes about ten minutes after the stop, agents
  included.
- **Costs.** The estimate is the size's hourly price while the sandbox runs
  and zero otherwise. Details shows boat's metered seconds and dollars for
  the sandbox. The sidebar balance is the wallet from `GET /limits`.
- **Files.** A file dropped on an agent's terminal goes through a signed
  upload URL: `POST /sandboxes/{id}/files/upload-url` with the path, then a
  `PUT` of the raw bytes to the URL it returns, without the API key. That
  skips the 5 MiB cap of the JSON file call and the base64-over-SSH path, so
  boat takes files up to 512 MB (boxd: 20 MB). The URL is a credential until
  it expires, so it never appears in an error. The key needs `file.write`.
- **Links.** `POST /host` makes or returns the URL of a port. boat has no
  call that lists or hides them, so Ports shows the links made from this
  Stowbox session and says so.

## Plan

1. Core: a capability record per provider, so the UI turns features on and
   off from data instead of from the provider's name.
2. Core: `Machine.provider`, a `Stopping` state, rate cards with per-size
   prices, and an `auto_stop_at` on the machine.
3. Core: a registry that holds the connected providers and routes each
   machine id to its owner, with tests.
4. Core: the boat client (REST, error envelope, retries on `Retry-After`,
   `Idempotency-Key` on create and fork, cursor pages) with fixture tests.
5. Core: exec over SSH for boat, with process-group cleanup.
6. Shell: one key per provider in the keychain, commands routed through the
   registry, provider status listing both providers and their capabilities.
7. UI: onboarding and Settings for both keys, a provider column and badge,
   the New box dialog by provider, power actions, Ports and Details from
   capabilities, provider names instead of hard-coded "boxd".
8. Mock: boat sandboxes and a boat account in `src/bridge/mock.ts`.

Not in this round: boat's saved snapshots, environments and integrated
agents, raw TCP ports, and opening a boat sandbox in an editor over
Remote-SSH (its address and host key change on every resume, which needs
its own `~/.ssh/config` handling).
