# Vercel

Stowbox drives [Vercel Sandbox](https://vercel.com/docs/vercel-sandbox)
microVMs next to the other providers. Connect it with three things: an
access token (Account Settings, Tokens, scoped to the team), the team id
(`team_…`, under the team's Settings, General) and a project, by name or id.
Every sandbox in that project shows up, whoever made it.

## What Vercel offers

| | Vercel Sandbox |
| - | - |
| API | REST + JSON at `https://api.vercel.com`, token as bearer, `teamId` on every call |
| Create | the default image (Ubuntu with Node and Python), 1, 2, 4 or 8 vCPUs with 2 GiB of memory each (8 needs Pro), and a 64 GiB disk |
| Stop and start | yes; Stowbox makes persistent sandboxes, so a stop snapshots the disk and a start boots a new session from it. Memory and processes do not survive |
| Hibernate, standby, reboot | no |
| Fork | yes, `POST /v2/sandboxes/{name}/fork` makes a new sandbox from the source |
| Lifetime | a session timeout, extended on demand; at the timeout the session stops. The plan caps one session: 45 minutes on Hobby, a day on Pro |
| Exec with stdin and a TTY | yes: commands stream as NDJSON, terminals go over the session's interactive WebSocket |
| HTTPS links, raw ports, editor over SSH | not offered yet |
| Metered usage | not over the API; the dashboard has it |
| Price | active CPU per vCPU-hour and provisioned memory per GB-hour, USD; a stopped sandbox costs only its snapshot's storage |

## How Stowbox uses Vercel

- **Auth.** `Authorization: Bearer` and `?teamId=` on every call.
  Connecting lists one sandbox of the project, which checks the token, the
  team and the project at once.
- **Boxes.** A box is a Vercel *sandbox*, named and kept across sessions;
  its id in Stowbox is that name. The name given in New box is lower-cased
  into letters, digits and dashes; Vercel refuses a name already used in
  the project. Each start is a *session*, one VM boot.
- **Idle stop.** Every sandbox is made with a timeout: the idle stop
  picked in New box (30 minutes unless changed). Vercel keeps it on the
  sandbox and gives each new session that long. While a terminal, an agent
  or a watch is open, Stowbox checks the session every four minutes and
  extends its timeout (`POST …/sessions/{id}/extend-timeout`) so it ends at
  least the idle stop from now. The session therefore stops that long after
  Stowbox stopped using it. Past the plan's cap the extension fails and the
  session stops even in use; the next exec starts the sandbox again.
  Changing the idle stop sets the sandbox's timeout for its next sessions.
- **Exec without a TTY.** `POST /v2/sandboxes/sessions/{id}/cmd` with
  `wait` runs `sh -c <command>`, the same command line Stowbox runs over
  SSH, and streams NDJSON: the command and its id, output lines tagged
  stdout or stderr, and last the command with its exit code. Each line is
  capped at 4 MiB before it is kept. Vercel sends no keepalives on this
  stream, so a quiet command is not timed out. Stdin goes in order from one
  task, one `POST …/cmd/{id}/stdin` per batch of up to 64 KiB, base64 in
  JSON, and ends with `close`. Output reaches Stowbox as JSON strings, so it
  is text: binary output does not come through intact.
- **Terminals.** `POST /v2/sandboxes/sessions/{id}/interactive` returns a
  `wss://` URL and a token for the session's PTY server. Stowbox waits for
  its `/health` check, opens the socket with the token in the query, and
  sends a `start` message with the command line and the window size. Output
  and keystrokes are binary messages, a resize is a `resize` text message,
  and the end is an `exit` text message with the code. The socket is
  `src/websocket.rs`: the upgrade and frames of the Agent37 tunnel, with
  text kept apart from binary and fragments joined up to 4 MiB.
- **Ending an exec.** As over SSH, a command without a TTY runs in a
  process group of its own and reports it on stderr. Dropping the exec
  kills the group from a second, short command, then sends SIGKILL to the
  command (`POST …/cmd/{id}/kill`).
- **Price.** The estimate is $0.128 a vCPU-hour and $0.0212 a GB-hour of
  memory while running: 2 vCPUs are about $0.34 an hour, and nothing while
  stopped. Vercel bills CPU only while it is busy, so a sandbox that mostly
  waits costs less than the estimate.

## Testing

`cargo test -p stowbox-core vercel` runs the unit tests: the JSON mapping,
names, streamed lines, the terminal's messages and the WebSocket. With
`STOWBOX_LIVE_VERCEL_KEY`, `STOWBOX_LIVE_VERCEL_TEAM` and
`STOWBOX_LIVE_VERCEL_PROJECT` set, `cargo test -p stowbox-core -- --ignored
vercel_live runs_as_its_user` makes real sandboxes and deletes them: the
full conformance run (create, list, exec with stdin and an exit code, a
TTY, an upload, stop and start, fork, delete), and a check of the user and
home commands run as and that dropping an exec leaves nothing it started
running.

## What is not there yet

- HTTPS links. A sandbox can expose up to 15 ports at
  `sandbox.domain(port)`, but only ports named when the sandbox is made.
- Other images, network policies and drives.
- Reattaching to a terminal after the app restarts.
