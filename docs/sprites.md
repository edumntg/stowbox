# Sprites

Stowbox drives [Sprites](https://sprites.dev), Fly.io's persistent
microVMs, next to the other providers. Connect it with an access token for
your Sprites organization (it looks like `org/id/secret`).

## What Sprites offers

| | Sprites |
| - | - |
| API | REST at `api.sprites.dev/v1`, the token as a bearer token; exec over a WebSocket at `/v1/sprites/{name}/exec` |
| Create | a name; one shape for every sprite: 8 vCPUs and a 100 GB disk, memory managed by Sprites. New box's environment variables go in as the sprite's environment |
| Pause | by itself, about 30 seconds after its last use: `warm` keeps memory and wakes in under a second, `cold` keeps only the disk. The next exec wakes it. An open terminal or command counts as use |
| Stop, start, standby settings | none to set: pausing is Sprites' own |
| Disk | persists across pauses until the sprite is deleted |
| Fork, snapshots | Sprites has checkpoints; Stowbox does not offer them yet |
| Delete | yes |
| Exec with stdin and a TTY | yes, as `sprite` in `/home/sprite`, with resize and exit codes |
| HTTPS links, raw ports, editor over SSH | not offered yet |
| Metered usage | not in the API; sprites.dev has it |
| Price | CPU time, memory and storage, each on use, USD |

## How Stowbox uses the API

- **Names.** A sprite's name is its id. New box's name is made into
  lowercase letters, digits and hyphens, up to 40 characters, since it is
  also the front of the sprite's host name; with no name, `box-` and eight
  random characters. A name from the list that has anything but letters,
  digits, `-` and `_` is left out rather than put into a path.
- **List.** Pages of 100, up to 20 pages.
- **Status.** `running` shows as running, `warm` as standby and `cold` as
  hibernated.
- **Exec.** The program is the `cmd` query parameters, but the command line
  never goes in the URL: it can hold an agent's prompt and run to many
  kilobytes. A short first exec writes it, from its stdin, to a private file
  in `/tmp`; the real exec runs a fixed wrapper that reads the file,
  removes it and runs it. The command line is the one SSH runs, with the
  environment exported in front. Without a TTY, each binary message leads
  with its stream (0 stdin, 1 stdout, 2 stderr, 3 the exit code, 4 the end
  of stdin). With a TTY, binary messages are the terminal's bytes and a
  resize is `{"type":"resize","cols","rows"}`. The exit comes as the `3`
  message or as `{"type":"exit","exit_code":N}`.
- **Ending an exec.** Sprites keeps a command running after its socket
  closes. So when Stowbox drops or loses the socket, it ends what the exec
  started itself: a command reports its process group on stderr, as over
  SSH, and a terminal writes its pid to a file in `/tmp`; a short second
  exec signals that group, and Sprites' kill call ends the session.
- **Secrets.** The token is only in the `Authorization` header, never in a
  URL or a log, and command lines go over the socket, not in the URL.
- **Pausing.** Any exec counts as use, so the Fleet view does not sample
  CPU and memory in an awake sprite with no agent in it; a sample every 15
  seconds would keep it from ever pausing. A new sprite that comes back
  paused is ready for an agent: its first command wakes it.
- **Price.** The estimate is $0.0385 a CPU-hour with all 8 vCPUs busy,
  about $0.31 an hour, while the sprite is awake, and nothing while it is
  paused. Memory ($0.021875 a GB-hour) and storage are billed on use, which
  the API does not report, so they are left out of the estimate.

## Testing

`cargo test -p stowbox-core sprites` runs the unit tests: errors, the
sprite mapping, names, prices, the exec URL and messages, and the terminal
wrapper and hang-up command, run under `sh`.

With `STOWBOX_LIVE_SPRITES_KEY` set,
`cargo test -p stowbox-core -- --ignored sprites` makes real sprites and
deletes them: the conformance run (create, list, exec with stdin and an exit
code, a TTY, an upload, delete), and a check that a terminal resizes and
gives its exit code, that dropping a terminal or a script ends what it
started, and that the disk is kept while the sprite pauses.

## What is not there yet

- Checkpoints and restore, which could back fork and snapshots.
- HTTPS links: a sprite has its own URL, and Stowbox could show it.
- Services, which keep a process running across pauses.
