# Modal

Stowbox drives [Modal](https://modal.com) sandboxes next to the other
providers. Connect it with a token id (`ak-…`) and token secret (`as-…`),
made in Modal's settings under API tokens. Modal is **beta** in Stowbox:
its sandbox API is the gRPC API its own SDKs use, not a published one, and
can change without notice.

Stowbox shows the sandboxes of one Modal app, `stowbox`, in the
workspace's default environment. Sandboxes from other apps are not listed.

## What Modal offers

| | Modal |
| - | - |
| API | gRPC at `api.modal.com:443`, token id and secret as metadata; exec through a second gRPC service on the worker, the task command router |
| Create | one image, `node:22-bookworm`, built once per workspace; 1, 2, 4 or 8 vCPUs with 2 GiB each (Modal counts half a physical core as one vCPU) |
| Lifetime | a sandbox runs until it is deleted or its timeout passes, at most 24 hours, and is then gone, disk and all. Stowbox makes every sandbox with the full day and shows its end as the auto-stop time |
| Stop, start, standby, fork | none. Modal has snapshots, which Stowbox does not use yet |
| Delete | yes |
| Exec with stdin and a TTY | yes, through the task command router, as root |
| Disk | 512 GiB, as `df /` reports it inside a sandbox; not reported by the API and not charged |
| HTTPS links, raw ports, editor over SSH | not offered yet |
| Metered usage | not over the API; Modal's usage page has it |
| Price | per physical-core-second and per GiB-second of memory while running, USD |

## How Stowbox uses Modal

- **Protocol.** The protos are vendored and cut down to what Stowbox calls
  (`proto/modal/`, see `third-party/modal-proto.md`). Every call sends the
  token id and secret as `x-modal-token-id` and `x-modal-token-secret`, and
  identifies itself as Modal's Go SDK 0.11.2, whose protocol the protos
  match: Modal refuses to make apps for a client version it does not know.
  TLS is verified against the web PKI roots.
- **Setup.** The first New box makes the `stowbox` app if it is missing,
  reads the environment's image builder version and builds the image,
  waiting on the build in rounds of under a minute, for at most 15 minutes.
  Later boxes reuse both. Listing never makes the app.
- **Create.** `SandboxCreate` with `sleep infinity` as the entrypoint, the
  size, a 24-hour timeout (or what New box asks, from a minute up), `/root`
  as the working directory, the name typed in New box and a
  `created-by: stowbox` tag. Environment variables go in the sandbox
  definition. Stowbox then waits for it to be ready.
- **Exec.** Decided against the first plan, which ran sshd in the image
  behind an unencrypted Modal tunnel: that needs an image
  with OpenSSH and injected host keys, and a second network path into the
  box. The task command router has a real PTY, stdin and exit codes, so it
  is used instead.
  - `TaskGetCommandRouterAccess` gives the router's URL and a JWT for one
    task. The URL must be `https` on a host under `modal.host` before the
    JWT is sent to it. The token is kept per sandbox and replaced two minutes
    before its `exp`, so a terminal open for hours keeps working.
  - `TaskExecStart` runs `/bin/sh -c` with the command line Stowbox runs
    over SSH. Output is `TaskExecStdioRead`, one stream per file descriptor
    read from a byte offset: a stream that breaks is opened again from the
    last byte, three times at most. Input is `TaskExecStdinWrite` at an
    offset, in order from one task. `TaskExecWait` gives the exit code; a
    death by signal shows as 128 plus the signal, as a shell reports it.
  - With a PTY the router refuses to fold stderr into stdout, so stderr is
    piped and, being empty, not read.
- **Resize and ending an exec.** The router has neither. A terminal's shell
  first writes its pid and tty to `/tmp/.stowbox-<exec id>.tty`; a resize
  runs `stty` on that tty in a second, short-lived exec, which sends the
  terminal its SIGWINCH, and dropping a terminal sends SIGHUP to its
  session the same way. A command without a TTY reports its process group
  on stderr, as over SSH, and dropping it kills the group from a second
  exec.
- **Price.** The estimate is $0.070956 a vCPU-hour and $0.024012 a
  GiB-hour of memory while running, so the default 2 vCPUs and 4 GiB is
  about $0.24 an hour.

## Testing

`cargo test -p stowbox-core modal` runs the unit tests: the sandbox
mapping, ids and names, router URLs, the router token's expiry, the exec
requests and the resize and hang-up scripts. With
`STOWBOX_LIVE_MODAL_TOKEN_ID` and `STOWBOX_LIVE_MODAL_TOKEN_SECRET` set,
`cargo test -p stowbox-core -- --ignored modal_live a_terminal_resizes`
makes real sandboxes and deletes them: the conformance run (create, list,
exec with stdin and an exit code, a TTY, an upload, delete), and a check
that a terminal resizes and that dropping a terminal or a script ends what
it left running.

## What is not there yet

- Snapshots, so a box could outlive its day.
- GPUs, other images, volumes and tunnels to a port.
