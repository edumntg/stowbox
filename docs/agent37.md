# Agent37

Stowbox drives [Agent37](https://www.agent37.com) instances next to boxd,
boat and Daytona boxes. Connect it with a workspace API key (`sk_live_…`)
made on the Agent37 dashboard under Cloud API, API keys; every box it sees
is an instance of that workspace.

## What Agent37 offers next to the others

| | Agent37 |
| - | - |
| API | REST + JSON at `https://api.agent37.com/v1`, `sk_live_` key as bearer; exec also at `https://sandbox.agent37.app/v1` with the key as `X-Agent37-Key` |
| Create | four shapes (2 vCPU / 4 GB up to 4 vCPU / 32 GB), 4 vCPU at most; types `default`, `vm` (2.5x compute) and `performance` (4x) |
| Stop and start | yes; a stopped instance keeps its whole disk and costs only the disk |
| Sleep | yes: checkpointed, disk billed alone, and the next request (SSH included) wakes it |
| Reboot and fork | yes |
| Auto timers | auto-sleep after an idle timeout of 5 minutes to a day |
| Exec with stdin and a TTY | not on the REST exec, so Stowbox execs over SSH, carried in a WebSocket through Agent37's edge |
| HTTPS links | every port has a URL at the edge, but a browser needs a signed one; not listable |
| Raw TCP and UDP | no (only `ssh -L`) |
| Metered usage | yes, by UTC day and by instance; and the wallet balance |
| Price | per vCPU, GB of memory and GB of disk, USD, metered per minute |

## How Stowbox uses Agent37

- **Auth.** `Authorization: Bearer sk_live_…` on `api.agent37.com`. Connecting
  reads the wallet (`GET /v1/billing`): balance, instance limit and whether
  the workspace is past due. Agent37 has no account name a key can read.
- **Exec.** An instance has no address of its own. SSH goes in a WebSocket
  to `wss://{id}-22022.agent37.app/` with the key in `X-Agent37-Key`, the way
  Agent37's own CLI does it (`agent37 tunnel`); `wstunnel.rs` speaks just
  enough WebSocket for that. Stowbox links its own ed25519 key
  (`ssh/agent37_ed25519` in the app's data directory) to the workspace once
  with `POST /v1/ssh-keys`, matched by fingerprint afterwards. A new key
  takes up to about two minutes to reach running instances, so logins are
  retried for that long after linking. The host key is not checked: it
  changes whenever an instance moves to another host, and TLS plus the API
  key at the edge already say who answers. Commands run as `node`, the
  image's user, in `/home/node`.
- **New boxes.** Instances are made from `agent37-base`, which has Node,
  Python and git but no tmux, no harness and no sudo. Before the create
  returns, a background REST exec as root installs tmux, then Claude Code
  and Codex as `node` into its own npm prefix (so they update themselves),
  and writes `/etc/profile.d/agent37-npm.sh` so SSH logins have that prefix
  on PATH. Stowbox polls `~/.stowbox/bootstrap.exit` with short calls rather
  than holding one request open, which can be dropped while the command
  carries on. It takes about half a minute; the log is
  `~/.stowbox/bootstrap.log`. A failed install does not fail the create.
- **Sleep.** Auto-sleep is the box's auto-stop in Stowbox: one hour unless
  another is picked, changed from Details (`PATCH auto_sleep` and
  `idle_timeout_seconds`). A sleeping instance shows as hibernated. Opening
  a terminal on it starts it first; an open SSH session keeps it awake.
  Stowbox's own idle guard puts Agent37 boxes to sleep instead of stopping
  them.
- **Lifecycle.** `POST /instances/{id}/start`, `stop`, `restart`, `sleep`
  and `fork`, and `DELETE /instances/{id}`. States map onto Stowbox's:
  `running` is running; `provisioning`, `starting`, `restarting` and
  `waking` are starting; `updating` is migrating; `sleeping` is hibernated;
  `stopping` and `deleting` are stopping; deleted instances are left out.
- **Links.** A link is `POST /instances/{id}/signed-url` for the port,
  valid a day (an hour if a day is refused). Agent37 cannot list them, so
  Stowbox shows those made in this session.
- **Cost.** The estimate is the list price: $0.80 a vCPU, $0.70 a GB of
  memory and $0.09 a GB of disk a month, over 730 hours; the disk also while
  stopped or asleep. `vm` and `performance` instances are priced per shape
  with their multiple, at the shape's default disk, while running only.
  Details reads the instance's compute line of `GET /v1/usage` since it was
  made (90 days at most). Agent37 settles minutes at least hourly, so the
  figure can grow; managed model and tool spend is not counted.

## Testing against a real workspace

```sh
AGENT37_API_KEY=sk_live_… AGENT37_INSTANCE=<id> \
  cargo test -p stowbox-core agent37::tests::live_exec -- --ignored   # exec, TTY, upload
AGENT37_API_KEY=sk_live_… \
  cargo test -p stowbox-core agent37::tests::live_lifecycle -- --ignored   # create, fork, delete
```

The second one makes two small instances and deletes them, a few cents.

## What is not there yet

- `vm` and `performance` instances from New box, regions other than `us`,
  other templates, and resize.
- Opening a box in an editor over Remote-SSH: it needs a `ProxyCommand`,
  for now Agent37's CLI (`npx agent37 ssh setup`).
- Backups and restore, public ports, network policies and budgets.
