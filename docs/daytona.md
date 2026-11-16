# Daytona

Stowbox drives [Daytona](https://www.daytona.io) sandboxes next to boxd and
boat boxes. Connect it with an API key (`dtn_…`) made on the Daytona
dashboard under Keys; the key belongs to one organization, and every box it
sees is a sandbox of that organization.

## What Daytona offers next to boxd and boat

| | Daytona |
| - | - |
| API | REST + JSON at `https://app.daytona.io/api`, `dtn_` key as bearer |
| Create | any vCPU, memory and disk up to the organization's limit (4 vCPU, 8 GiB, 10 GiB by default) |
| Stop and start | yes; a stopped sandbox keeps its disk and costs only the disk |
| Standby, hibernate, reboot | no |
| Pause and fork | only for VM-class sandboxes; Stowbox creates the default class, so not offered |
| Archive | container sandboxes only; an archived sandbox shows as stopped and starts again |
| Auto timers | auto-stop after an interval with **no activity** (15 min by default) |
| Exec with stdin and a TTY | not on the toolbox API in a usable shape, so Stowbox execs over Daytona's SSH gateway |
| HTTPS links | one preview URL per port, signed, not listable, not removable |
| Raw TCP and UDP, editor over SSH | no |
| Metered usage | not over the API; the dashboard has it |
| Price | per vCPU, GiB of memory and GiB of disk, USD |

## How Stowbox uses Daytona

- **Auth.** `Authorization: Bearer dtn_…` on every call. Connecting checks
  the key with `GET /sandbox?limit=1` and names the account after the first
  organization from `GET /organizations`, with its per-sandbox ceilings.
- **Exec.** Each new connection asks `POST /sandbox/{id}/ssh-access` for a
  token (valid a day) and logs in to `ssh.app.daytona.io` with the token as
  the user name. Daytona does not publish the gateway's host key, so the
  first key seen is remembered in `ssh/daytona_known_hosts` in the app's
  data directory and every later connection must present the same one; a
  different key is refused. The rest (one connection per sandbox, process
  groups, kills) is the shared SSH exec used for boat too.
- **Activity.** Daytona's auto-stop counts API calls and gateway traffic,
  and stops a sandbox even with an agent working in it. While an exec is
  open, Stowbox calls `POST /sandbox/{id}/last-activity` every four
  minutes. Stowbox creates sandboxes with a one-hour auto-stop unless
  another is picked, and the box's Details change it
  (`POST /sandbox/{id}/autostop/{minutes}`).
- **Lifecycle.** Start is `POST /start`, stop is `POST /stop`, delete is
  `DELETE /sandbox/{id}`. States map onto Stowbox's: `started` is running;
  `creating`, `starting`, `restoring`, `pulling_snapshot` and the build
  states are starting; `stopped` and `archived` are stopped; `error` and
  `build_failed` are failed; destroyed sandboxes are left out.
- **Links.** A link is
  `GET /sandbox/{id}/ports/{port}/signed-preview-url`, a URL with the token
  in it, valid a day (an hour where the organization caps it). Daytona
  cannot list them, so Stowbox shows those made in this session; a stop
  forgets them.
- **Cost.** The estimate is the list price: $0.0504 a vCPU-hour, $0.0162 a
  GiB-hour of memory and $0.000108 a GiB-hour of disk, the disk also while
  stopped. Daytona does not bill the first 5 GiB of disk, which the
  estimate does not take off, so it is a slight ceiling. There is no
  metered usage or balance to read, so Details shows the estimate and a
  link to the dashboard, and today's spend counts Daytona boxes by their
  estimate only.

## What is not there yet

- VM-class sandboxes, and with them pause and fork.
- Snapshots other than the default, volumes, and the target region.
- Opening a box in an editor over Remote-SSH.
