# Blaxel

Stowbox drives [Blaxel](https://blaxel.ai) sandboxes next to the other
providers. Connect it with an API key (`bl_…`, from the Blaxel console
under Workspace settings, API keys) and the workspace's name. Every sandbox
in the workspace shows up, whoever made it.

## What Blaxel offers

| | Blaxel |
| - | - |
| API | REST + JSON at `https://api.blaxel.ai/v0`, key as bearer, workspace in `X-Blaxel-Workspace`; each sandbox has its own API at its URL |
| Create | `blaxel/base-image:latest` with 2, 4, 8 or 16 GiB of memory; Blaxel derives the CPUs from memory (1, 2, 4 and 6) |
| Standby | automatic: seconds after its last request a sandbox goes to standby, memory and processes kept, and the next request wakes it in milliseconds. Stowbox shows it as hibernated |
| Stop, start, fork | archive (a stop that keeps the disk) and fork are preview features Blaxel leaves off for workspaces, so Stowbox does not offer them |
| Delete | yes. Blaxel also deletes a sandbox by itself after a while (seven days on the workspace tested) |
| Exec with stdin and a TTY | yes, through the sandbox's process API and its terminal WebSocket |
| Disk | a tmpfs sized from memory, about 3 GiB on 4 GiB; not reported by the API |
| HTTPS links, raw ports, editor over SSH | not offered yet |
| Metered usage | not over the API; the console has it |
| Price | per GB-second of memory while active, CPU included, USD; standby costs no compute |

## How Stowbox uses Blaxel

- **Auth.** `Authorization: Bearer`, `X-Blaxel-Workspace` and
  `Blaxel-Version: 2026-09-22` on every call, to the control plane and to
  sandboxes alike. Connecting lists one sandbox, which checks the key and
  the workspace together. The key goes to each sandbox's own URL, so that
  URL must be `https` on `*.bl.run` or `*.blaxel.ai`; any other is refused.
  Blaxel's edge checks the key and removes `Authorization` before the
  request enters the sandbox, so code in a box cannot read it: on
  2026-11-20 a packet capture inside a sandbox saw the other headers of a
  request in plain HTTP/1.1, and no `Authorization`.
- **Waiting to start.** A sandbox deploys for a few seconds after it is
  made. An exec in that window waits up to a minute for it, since Blaxel
  has no start action to offer instead.
- **Names.** A box's id is the sandbox name: the name given in New box,
  lower-cased into letters, digits and dashes.
- **Exec without a TTY.** `POST {sandbox}/process` starts the command line
  Stowbox runs over SSH, named and with a stdin pipe.
  `GET …/process/{name}/logs/stream` with `Accept: application/x-ndjson`
  streams its output, the history first, with a keepalive every 30
  seconds; a stream silent for 90 seconds is taken as dead. Output that is
  not UTF-8 comes base64-encoded and is decoded. When the stream ends,
  `GET …/process/{name}/wait` gives the exit code. Stdin is `POST
  …/process/{name}/stdin` with raw bytes, in order from one task, and
  `DELETE` on the same path closes it. An open stream keeps the sandbox
  out of standby: a 100-second quiet command finishes.
- **Terminals.** `/terminal/ws` starts a PTY on a program given as a path,
  with no arguments. Stowbox first writes a script (mode 700, in `/tmp`)
  that removes itself, records its pid, runs the command line and records
  the exit code, and opens the terminal on it with the API headers on the
  upgrade. Keystrokes and resizes are `input` and `resize` text messages,
  output arrives as `output` messages, and the socket closes when the
  program exits; Stowbox then reads the exit code back. Blaxel keeps a
  terminal with no client for ten minutes in case it reconnects, so
  dropping one hangs up its process group instead.
- **Ending an exec.** As over SSH, a command without a TTY reports its
  process group on stderr, and dropping it kills the group from a second
  process, then the process itself (`DELETE …/process/{name}/kill`). The
  base image has no `setsid`, so there the group is the caller's and only
  the command's shell is sure to end.
- **Price.** The estimate is $0.0414 a GB-hour of memory while active, so
  4 GiB is about $0.17 an hour, and nothing in standby. When Blaxel's
  answer leaves out the sandbox's `state`, as it did in testing, a sandbox
  in standby shows as running and the estimate is a ceiling.

## Testing

`cargo test -p stowbox-core blaxel` runs the unit tests: the JSON mapping,
names, sandbox URLs, log events and the terminal script. With
`STOWBOX_LIVE_BLAXEL_KEY` and `STOWBOX_LIVE_BLAXEL_WORKSPACE` set,
`cargo test -p stowbox-core -- --ignored blaxel_live runs_as_its_user`
makes real sandboxes and deletes them: the conformance run (create, list,
exec with stdin and an exit code, a TTY, an upload, delete), and a check of
the user and home, of a quiet 100-second command, and that dropping a
terminal hangs up what it runs.

## What is not there yet

- Stop and start (archive) and fork, once Blaxel turns them on.
- Previews (HTTPS links to a port), volumes and other images.
