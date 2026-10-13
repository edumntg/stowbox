# ResumeVm fails once when a pause lands right after a wake from hibernation

Bug report for the boxd team. Reproduced on 2026-10-13 between 19:58 and
20:19 UTC against `boxd.sh:9443` (`boxd.api.v1.BoxdApi`, TLS, JWT from an
API key) with machines created by a Rust client (tonic 0.14). All machines
were default-sized (2 vCPU, 8 GiB, standard image) in the hosted EU region,
org on credits billing, and were destroyed after each run.

Machine ids, in case they help find traces:
`e49d9ce7-dc62-4c25-adfe-a3b26092f9a0` (19:58 UTC, first sighting),
`36f33851-eb8a-4a3f-a9de-377e64219a34`,
`a864b48c-15b5-4843-bb71-43ffba6998e8`,
`48693bc2-e58d-400b-b8e6-1be980482920` (20:13 to 20:19 UTC, controlled runs).

## Summary

After `HibernateVm` → `WakeVm`, `GetVm` reports `running` about 50 ms later.
If a `SuspendVm` arrives within roughly the first 300 ms of that `running`
state, it is accepted and the machine reports `suspended`, but the next
`ResumeVm` issued within about 250 ms of the pause fails with
`Internal`. The failure is transient: a second `ResumeVm`, even issued
immediately after the failed one, succeeds, the machine returns to
`running`, and processes that were in memory before the pause are still
there. The machine is left in `suspended` after the failed call, with
`hibernated_at_ms = 0`.

The same resume path through `Exec` (opening a shell on a suspended
machine) fails the same way with a more specific message:

```
resume_vm rpc: status: Internal, message: "resume: vmm exited while a hibernate was pending"
```

The explicit `ResumeVm` RPC returns:

```
Internal: couldn't resume that machine
```

## Steps to reproduce

With the API, all calls on the same machine id, no waits between the last
three:

1. `HibernateVm`, poll `GetVm` until `status == "hibernated"` (5 to 6 s).
2. `WakeVm`, poll `GetVm` until `status == "running"` (about 50 ms).
3. `SuspendVm` immediately. It succeeds; `GetVm` says `suspended`.
4. `ResumeVm` immediately. Fails with `Internal: couldn't resume that machine`.
5. `ResumeVm` again. Succeeds (`resume_us` in the normal 0.3 to 5 ms range).

With the CLI the same sequence is `boxd machine hibernate m`, `boxd machine
wake m`, `boxd machine pause m`, `boxd machine resume m` run back to back;
the CLI's own startup latency may or may not keep each step inside the
window, so the API sequence is the reliable one.

## Measurements

Delay inserted between `GetVm` reporting `running` after the wake and the
`SuspendVm`, with `ResumeVm` sent immediately after the pause. One cycle per
row, each cycle starting from a fresh hibernate:

| Delay running → pause | First `ResumeVm` | Status after | Retry 2 s later |
|---|---|---|---|
| 0 ms | `Internal: couldn't resume that machine` | suspended | ok |
| 25 ms | same error | suspended | ok |
| 50 ms | same error | suspended | ok |
| 100 ms | same error | suspended | ok |
| 150 ms | same error | suspended | ok |
| 200 ms | ok | running | n/a |
| 300 ms | same error | suspended | ok |

The 200 ms row looks like jitter rather than the edge of the window. Polling
`ResumeVm` every 250 ms after a failure showed the first success 261 ms
after the failed call.

Through `Exec` instead of `ResumeVm` (the client opens an exec stream,
which adds roughly 100 to 200 ms before the server-side resume), only the
0 ms row failed; 250 ms and later succeeded. A `sleep 3600` started before
the cycle was still running after every recovery, so memory state survives.

## Controls

Each line is three to five repetitions on one machine.

| Sequence | Result |
|---|---|
| Running for 5 s, then `SuspendVm` → immediate `ResumeVm` (no hibernate) | ok, 5 of 5 |
| `SuspendVm` → 1 s → `ResumeVm` (no hibernate) | ok, 3 of 3 |
| Hibernate → wake → 5 s → `SuspendVm` → immediate `ResumeVm` | ok, 3 of 3 |
| Hibernate → wake → immediate `SuspendVm` → 1 s → `ResumeVm` | ok, 3 of 3 |
| Hibernate → wake → immediate `SuspendVm` → immediate `ResumeVm` | **fails 3 of 3**, immediate second `ResumeVm` ok 3 of 3 |

So neither "resume right after pause" nor "pause right after wake" fails on
its own. The failure needs both: a pause inside the post-wake window and a
resume before the pending hibernate has cleared.

## Expected

One of:

- `SuspendVm` waits for the pending hibernate to settle before returning,
  so a `suspended` status always means a resumable machine.
- `ResumeVm` retries internally when the VMM is gone because a hibernate was
  still pending, since the second attempt always works.
- At minimum, the error is returned with a retryable code
  (`Unavailable` or `FailedPrecondition`) and the same message on both
  paths, so clients can distinguish it from a real internal failure.

## Impact

Low but confusing. A client that reacts to status transitions (a UI power
button pressed right after a wake, or an auto-pause policy) and then opens
a connection gets a 500-class error once, with a message that suggests the
machine is broken. The workaround in our client is to leave a moment after
a wake before pausing and to retry a failed resume once.
