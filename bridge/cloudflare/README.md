# stowbox-bridge

The Worker Stowbox talks to for Cloudflare Containers. You deploy it in your
own Cloudflare account; it starts a container per box and gives Stowbox a
small API over them. `docs/cloudflare.md` says how to set it up and what
Stowbox does with it.

```sh
pnpm install
pnpm wrangler deploy
openssl rand -hex 32                      # the token; copy it
pnpm wrangler secret put STOWBOX_TOKEN    # paste it when asked
```

`pnpm check` type-checks the Worker and `pnpm test` runs its input checks
under Node.

## Protocol, version 1

Every request carries `Authorization: Bearer <STOWBOX_TOKEN>`. Without a
token of at least 32 characters set, every request gets 503; with a wrong
one, 401. Bodies and answers are JSON; an error is `{ "error": "…" }`.

| Request | Answer |
| - | - |
| `GET /v1/info` | `{ "bridge": "stowbox-bridge", "version": 1 }` |
| `GET /v1/boxes` | `{ "boxes": [box, …] }` |
| `POST /v1/boxes` `{ name, size?, idle_secs?, env? }` | 201 and the box; 409 when the bridge holds 100 |
| `GET /v1/boxes/{id}` | the box |
| `POST /v1/boxes/{id}/start`, `/stop` | the box |
| `PUT /v1/boxes/{id}/idle` `{ secs }` | the box |
| `DELETE /v1/boxes/{id}` | 204 |
| `GET /v1/boxes/{id}/exec`, a WebSocket upgrade | see below |

A box is `{ id, name, size, created_at, status, started_at, idle_secs }`:
`status` is `running` or `stopped`, times are Unix seconds, and
`started_at` is 0 while stopped.

- `name`: 1 to 63 lowercase letters, digits or hyphens, starting with a
  letter or digit. It is only stored and shown.
- `size`: `standard-2` (the default), `standard-3` or `standard-4`.
- `idle_secs`: how long the box runs with nothing connected, 60 to 86400,
  1800 by default.
- `env`: up to 64 variables, 64 KiB in all, given to the container at every
  start.

### Exec

The first message is text:
`{ "type": "start", "argv": ["bash", "-lc", "…"], "tty": true, "cols": 80, "rows": 24 }`.
It must come within 30 seconds; `argv` holds 1 to 64 strings. A stopped box
starts first, on a fresh disk.

Then the client sends input as binary messages, and as text
`{ "type": "resize", "cols": N, "rows": N }` or `{ "type": "eof" }`.
The bridge sends output as binary messages whose first byte is the stream
(1 stdout, 2 stderr; with a PTY everything is stdout), then
`{ "type": "exit", "code": N }` and a close with code 1000. A failure is
`{ "type": "error", "message": "…" }` and a close with code 1011.

Each command leads a session of its own. When the socket closes or breaks
before the command exits, the bridge sends SIGHUP to the session, then
SIGKILL a second later.

## The image

`wrangler.jsonc` pins `node:22-bookworm` from Docker Hub by digest, the same
for every size. To move it, read the new digest and put it in all three
places:

```sh
docker buildx imagetools inspect node:22-bookworm --format '{{json .Manifest.Digest}}'
```

Any Debian or Ubuntu image with bash, `pkill` (procps) and `sleep` works.
Cloudflare does not cache Docker Hub pulls, so Docker Hub's pull limits
apply when containers start.

## What it costs

Cloudflare bills a running container per second: vCPU on use, memory and
disk on what the size provisions. The Durable Object is billed while a
socket is open, and Workers Paid is $5 a month with an allowance of each.
`docs/cloudflare.md` has the figures Stowbox uses.
