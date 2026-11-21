# Cloudflare

Stowbox drives [Cloudflare Containers](https://developers.cloudflare.com/containers/)
next to the other providers. Cloudflare has no hosted sandbox API: a
container is started by a Durable Object, which runs in a Worker in your own
account. So you deploy a small Worker, `stowbox-bridge`, from
`bridge/cloudflare` in this repository, and Stowbox talks only to it.

Connect it with the Worker's URL and the token you gave the Worker. Stowbox
never holds a Cloudflare API token: wrangler uses yours once, to deploy.

## Setting up the bridge

You need a Workers Paid plan, since Containers are not on the free plan,
and Node.js. In `bridge/cloudflare`:

```sh
pnpm install
pnpm wrangler login
pnpm wrangler deploy
openssl rand -hex 32                      # the token; copy it
pnpm wrangler secret put STOWBOX_TOKEN    # paste it when asked
```

The deploy prints the Worker's URL, such as
`https://stowbox-bridge.<your-subdomain>.workers.dev`. In Stowbox, add
Cloudflare with that URL and the same token. Keep the token as you would an
API key: it gives a root shell in every box the bridge holds. If it leaks,
put a new one with the same `secret put`, and the old one stops working.

No Docker is needed: the image is pulled from Docker Hub by Cloudflare.
`bridge/cloudflare/README.md` has the bridge's protocol and how to update
the image.

## What Cloudflare offers

| | Cloudflare |
| - | - |
| API | the bridge's REST API under `/v1`, the bridge token as a bearer token; exec over a WebSocket at `/v1/boxes/{id}/exec` |
| Create | one image, `node:22-bookworm` pinned by digest; three sizes, Cloudflare's instance types: `standard-2` (1 vCPU, 6 GiB, 12 GB disk), `standard-3` (2 vCPU, 8 GiB, 16 GB) and `standard-4` (4 vCPU, 12 GiB, 20 GB) |
| Idle stop | a box stops after so long with nothing connected: 30 minutes unless New box or the box's settings say otherwise, from a minute to a day. A command running with nothing connected does not keep it up |
| Stop and start | yes. A stopped box keeps **no files**: the next start, by Start or by an exec, is a fresh disk from the image |
| Standby, fork, snapshots | none |
| Delete | yes |
| Exec with stdin and a TTY | yes, as root, through the container's own exec with a PTY, resize and exit codes |
| HTTPS links, raw ports, editor over SSH | not offered yet |
| Metered usage | not over the bridge; Cloudflare's dashboard has it |
| Price | per second while running: vCPU on use, memory and disk on what the size provisions, USD, past the Workers Paid allowance |

## How Stowbox uses the bridge

- **The Worker.** One Durable Object class per size, since an instance type
  belongs to a container class, and a `Registry` Durable Object that lists
  the boxes, since Durable Objects cannot be listed. A bridge keeps at most
  100 boxes and runs at most 10 of each size at once (`max_instances` in
  `wrangler.jsonc`).
- **Auth.** Every request carries `Authorization: Bearer <token>`, also the
  WebSocket upgrade, which a browser page cannot set, so a page cannot reach
  the bridge with your token. The Worker compares SHA-256 digests in
  constant time, and answers 503 until a token of at least 32 characters is
  set. The URL must be `https://` and a bare host; TLS is verified.
- **Create.** A random id (`box-` and 20 letters and digits), the name typed
  in New box, the size, the idle stop and New box's environment variables,
  which the container gets at every start. The container is started at
  once, with internet access and a small bash loop as process 1, so it
  reaps what ended sessions leave behind.
- **Exec.** The first message names the command (`bash -lc` with the
  environment exported in front, as over SSH), whether it gets a PTY, and
  its size. Input is binary messages, a resize or the end of input a text
  message. Output comes back as binary messages that lead with a stream byte
  (1 stdout, 2 stderr), and the end is `{"type":"exit","code":N}`.
- **Ending an exec.** Each exec leads a session of its own. When its socket
  closes or breaks, the bridge sends SIGHUP to the whole session, so a shell
  passes it to its jobs, then SIGKILL a second later, also when the command
  itself has already exited and only its children hold the output open.
  Nothing is left running from a dropped terminal or script. Output waits on
  the command's pipe while more than 1 MiB is queued for a slow client.
- **Status without waking.** Any call to a box's Durable Object counts as
  use and would keep its container up, so listing never calls one. The
  registry keeps when each box started and was last used (an open exec
  tells it every minute), and a box shows as stopped once its idle time has
  passed since then. A box Cloudflare stops early for its own reasons shows
  as running until that time, and the next exec starts it again.
- **Logs.** The Worker never logs a request body or a command line, since
  the environment in a command may hold keys. Errors go to Workers Logs by
  event and message only.
- **Price.** The estimate is $0.072 a vCPU-hour, $0.009 a GiB-hour of memory
  and $0.000252 a GB-hour of disk while running, so `standard-2` is about
  $0.13 an hour with its CPU fully busy. Cloudflare bills CPU on use, so a
  quiet box costs less. The Durable Object adds a little while a terminal is
  open, and Workers Paid is $5 a month.

## Testing

`cargo test -p stowbox-core cloudflare` runs the unit tests: the bridge URL,
errors, the box mapping, ids and names, prices and the exec messages.
`pnpm test` in `bridge/cloudflare` runs the Worker's input checks under
Node.

With a deployed bridge, `STOWBOX_LIVE_CLOUDFLARE_URL` and
`STOWBOX_LIVE_CLOUDFLARE_KEY` (the bridge token) set,
`cargo test -p stowbox-core -- --ignored cloudflare` makes real boxes and
deletes them: the conformance run (create, list, exec with stdin and an exit
code, a TTY, an upload, stop and start, delete), and a check that a terminal
resizes, that dropping a terminal or a script ends what it started, and that
a stopped box comes back on a fresh disk.

## What is not there yet

- Keeping files across a stop: Cloudflare's container snapshots or an R2
  mount.
- HTTPS links to a port, which the Worker could route.
- Other images and sizes, which today mean editing `wrangler.jsonc`.
