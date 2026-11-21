# E2B

Stowbox drives [E2B](https://e2b.dev) sandboxes next to the other
providers. Connect it with the team's API key (`e2b_…`) from the E2B
dashboard under Keys; every sandbox the team has shows up, whoever made it.

## What E2B offers

| | E2B |
| - | - |
| API | REST + JSON at `https://api.e2b.app`, key in `X-API-Key` |
| Create | from a template; Stowbox starts the public `base` template, 2 vCPU and 512 MiB, whose size is fixed |
| Pause and resume | yes; a pause snapshots memory and disk and costs nothing to hold. Stowbox shows it as hibernation |
| Stop, standby, reboot | no |
| Fork | yes: the source is snapshotted in place and a new sandbox boots from it, memory and all |
| Lifetime | a deadline, pushed out on demand; at the deadline the sandbox pauses (`autoPause`). The plan caps one run: an hour on Hobby, a day on Pro |
| Exec with stdin and a TTY | yes, through envd, the agent inside each sandbox, over the Connect protocol |
| HTTPS links, raw ports, editor over SSH | not offered yet |
| Metered usage | not over the API; the dashboard has it |
| Price | per vCPU-second and GiB-second of memory while running, USD; nothing while paused |

## How Stowbox uses E2B

- **Auth.** `X-API-Key` on every REST call. Connecting checks the key with
  `GET /v2/sandboxes?limit=1`. E2B's team call is not used: it answers with
  the team's API keys in it.
- **Names.** E2B sandboxes have no name, so the name given in New box goes
  in the sandbox metadata as `stowbox-name`. A fork shows its id until it is
  renamed in E2B; the fork call takes no metadata.
- **Idle pause.** Every sandbox is made with `autoPause` and a timeout: the
  idle stop picked in New box (30 minutes unless changed), also kept in the
  metadata. While a terminal, an agent or a watch is open, Stowbox calls
  `POST /v2/sandboxes/{id}/connect` with that timeout every four minutes,
  which only ever extends the deadline. The sandbox therefore pauses that
  long after Stowbox stopped using it. Past the plan's cap the extension
  fails and the sandbox pauses even in use; the next exec resumes it.
  Changing the idle stop calls `POST /sandboxes/{id}/timeout`, which sets
  the deadline from now. E2B has no "never".
- **Exec.** `POST /v2/sandboxes/{id}/connect` resumes a paused sandbox and
  returns its envd access token, which stays in memory. envd listens at
  `https://49983-{id}.e2b.app` (or the domain the API names). Stowbox
  starts `sh -c <command>` with `process.Process/Start`, the same command
  line it runs over SSH, as the user `user` (HTTP basic auth with no
  password, plus `X-Access-Token`), and reads the event stream: the pid,
  output chunks, the end with its exit code. A terminal asks for a PTY with
  the window size, or envd starts it at 0x0. Input goes in order from one
  task, one `SendInput` per batch of keystrokes; a resize is `Update`; the
  end of input is `CloseStdin`, or Ctrl-D on a PTY. envd sends a keepalive
  every 30 seconds when asked (`Keepalive-Ping-Interval`), so a stream that
  stays silent for 90 seconds is taken as dead.
- **Ending an exec.** SIGKILL to the pid stops only the shell envd started,
  not what it left running. A command without a TTY therefore runs in a
  process group of its own and reports it on stderr, as over SSH, and
  dropping the exec kills the group from a second, short-lived envd process
  before signalling the pid.
- **Connect.** `src/connectrpc.rs` is a small client for the Connect
  protocol with protobuf messages: unary calls are a POST of one message;
  a server stream is a POST of one enveloped message, answered with
  envelopes (a flags byte and a four-byte length). Each length is checked
  against a 4 MiB cap before anything is kept, compressed envelopes are
  refused, and the end-of-stream envelope's JSON error becomes a Stowbox
  error. The messages come from envd's `process.proto`, vendored in
  `crates/stowbox-core/proto/e2b` (see `third-party/e2b-envd-proto.md`).
- **Price.** The estimate is $0.0504 a vCPU-hour and $0.0162 a GiB-hour of
  memory while running, so the base template is about $0.11 an hour, and
  nothing while paused.

## Testing

`cargo test -p stowbox-core e2b` runs the unit tests: the JSON mapping,
hostile ids and domains, the start request, the event mapping, and the
envelope decoder fed random bytes. With `STOWBOX_LIVE_E2B_KEY` set,
`cargo test -p stowbox-core -- --ignored e2b_live dropping_an_exec_kills`
makes real sandboxes and deletes them: the full conformance run (create,
list, exec with stdin and an exit code, a TTY, a 300 KB upload, pause and
resume, fork, delete), and a check that dropping an exec leaves nothing it
started running.

## What is not there yet

- Other templates and sizes. A template sets the size, and the base one is
  small for a coding agent; templates the team built would need a size
  menu read from `GET /templates`.
- HTTPS links. Every port is reachable at `https://{port}-{id}.e2b.app`,
  but whether a link needs the sandbox's traffic token depends on how the
  sandbox was made; not wired up yet.
- Uploads go over exec stdin like the other providers, not envd's file API.
- Reattaching to a terminal after the app restarts (`process.Process/Connect`).
