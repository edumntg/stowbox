# SSH host

Stowbox drives any machine it can reach over SSH: a VPS, a home server, a
dev box from another service. Stowbox does not make, stop or delete it; the
box is there while it is connected. It is the provider with the fewest
powers, which keeps the UI honest about what a provider cannot do.

## What it offers next to the cloud providers

| | SSH host |
| - | - |
| API | none: SSH only |
| Create, fork, delete | no; one host per connection, shown as one box |
| Power actions, auto timers | no |
| Exec with stdin and a TTY | yes, over SSH (the shared exec, as boat and Daytona) |
| Uploads | over exec |
| HTTPS links, port forwards, editor over SSH | no |
| Price | none: no estimate, never an invented rate |

## How Stowbox uses it

- **Connect.** Settings or onboarding take the host, the user, an optional
  port (22) and an optional path to a private key. They are kept together
  in the OS keychain, as every provider's credentials are.
- **Key.** With no path, Stowbox uses a key of its own, made the first time
  in `ssh/sshhost_ed25519` in the app's data directory and never one of the
  person's keys. Connecting fails until its public line is in
  `~/.ssh/authorized_keys` on the host, and the message gives that line. A
  path, which may start with `~`, must point to an unencrypted private key
  in the OpenSSH format; Stowbox reads it when connecting and keeps nothing.
- **Host key.** The host announces none through an API, so the first key it
  presents is remembered in `ssh/sshhost_known_hosts` and every later
  connection must present the same one. A different key is refused, with
  the line to remove if the host really changed it.
- **Checks.** The host, user and port are checked before anything
  connects: a host name or address, a POSIX user name, a port from 1 to
  65535. Connecting runs `id -un` and names the account `user@host`.
- **Status.** Listing does not log in. While Stowbox holds a connection the
  box is running. Otherwise a TCP connect to the port, with a short timeout,
  shows it running when it answers and unknown when it does not; Stowbox
  cannot tell an unreachable host from a stopped one. One probe stands for a
  minute, because sshd logs each as a connection dropped before login, and
  sshguard or fail2ban can count those against your address.
- **Exec.** One SSH connection is kept and reused, and every exec runs in a
  fresh channel under `bash -lc`, so the host needs `bash` and `sh`. On
  Linux, dropping an exec kills what it started. A host without util-linux's
  `setsid -w` (macOS, BSD, BusyBox) runs commands the same, but what an
  exec leaves running keeps running.
- **Idle standby.** Never applies: Stowbox cannot stop the host.

## Testing

`cargo test -p stowbox-core -- --ignored local_sshd` starts a throwaway
`sshd` on 127.0.0.1 and runs the conformance checks, an upload and the
reachability probe against it. It needs `sshd` and `ssh-keygen` installed.

## What is not there yet

- More than one host per connection.
- Keys with a passphrase, PEM keys, and the SSH agent.
- A key file that cannot be read when Stowbox starts (an unmounted volume)
  leaves the host disconnected until it is connected again in Settings.
- Jump hosts and `~/.ssh/config`.
