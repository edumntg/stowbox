# Features in detail

Everything the app does today, in full. The README has the short version.

- **Fleet**: the landing view. Four tiles (running boxes, working agents,
  finished turns waiting for you, estimated cost per hour), alerts derived
  from the same data (a failed agent, a box billing with nothing to do, a
  box near its memory or disk limit, the daily spend warning, a box the
  idle guard put in standby), and one row per box with its agents
  as state squares, CPU, memory, disk free and age (time since boxd created
  the box; the guest's own uptime is not used, because a box restored from
  an image carries that image's kernel uptime). The numbers are read
  inside each running box over the exec stream, since boxd has no
  telemetry API; boxes in standby or hibernation are not touched, because
  sampling would wake them, and keep their last reading with its age.
- **Boxes**: list, create (1, 2 or 4 vCPU), fork, standby, resume, hibernate,
  wake, stop, start, reboot, delete. Each row shows size and an estimated cost
  per hour for the box's *current* state, computed from boxd's live rate card.
- **Agents**: a Claude Code or Codex session pinned to a box and a working
  directory. Each message runs one headless turn inside the box; the session
  id is kept so the next message continues the same conversation. Text
  streams as the model types; tool calls and their output fold into cards;
  cost, turns and tokens accumulate per agent. Transcripts persist on disk.
  Turns keep running when Stowbox is closed, the laptop sleeps or the
  network drops: each one runs detached inside the box and writes its
  output to `~/.stowbox/turns/<agent>/<turn>/` there (the last five turns
  per agent are kept). Stowbox follows those files from the last line it
  applied, so after a drop or a restart it reconnects and picks up where it
  left off, with no line shown twice or skipped. While the box is in
  standby or hibernated the agent says so and waits instead of waking it;
  Stop kills the turn's process group in the box. When a box is deleted its
  agents stay as read-only history: the composer gives way to a "was
  deleted" line with Remove.
- **Agent terminal**: an agent's Terminal tab runs the harness's own
  interactive interface (`claude --resume <session>`, `codex resume`) in
  the agent's directory, on the same session, for what a headless turn
  cannot do: `/login`, `/status`, `/config`, `/model` and the like. Typing
  one of those in the chat runs it there instead (the button turns into
  *Run in terminal*); for Codex every slash command goes there. The
  session keeps running while you look at the chat, another agent or a
  box, and coming back to the agent opens it where it was; it ends when a
  chat turn starts, so one process at a time works on the session, or
  when the agent's tab is closed. A turn
  that answers "isn't available in this environment", or fails because
  Claude Code is not signed in, offers the way into the terminal.
- **Claude Code login per box**: boxd gives every box in an org the same
  agent login. A box's Agents tab shows which login its Claude Code uses
  (and the account's email when Claude Code recorded one) and can switch
  that box to a login of its own: `~/.stowbox/claude` in the box becomes
  `CLAUDE_CONFIG_DIR` for every Claude Code Stowbox starts there, with the
  token variables unset, and *Sign in* runs `claude /login` in the box's
  terminal. Sessions stay shared (`projects/` links back to
  `~/.claude/projects`), so agents keep their conversations; going back to
  the org login deletes the box's own.
- **Queued messages**: while a turn runs, the composer queues what you
  write instead of sending it. Queued messages show under the transcript,
  can be edited or removed until they go out, and are sent one per turn,
  each only once the previous turn has really ended (finished or failed;
  a reconnecting turn is still running). The queue is saved with the
  agent, so it carries on after the window reloads or Stowbox restarts.
  Stop pauses it: nothing more goes out until you press *Send next* (the
  rest then follow turn by turn) or clear it; a queued message that cannot
  be sent goes back to the front and pauses the queue the same way. A turn
  with more queued after it does not notify; the last one does. A queue
  that will send on its own keeps the box from idle standby.
- **Attachments**: attach files with the paperclip, paste images or drop
  files on the composer. Each one is copied into the box right away, to
  `~/.stowbox/uploads/<agent>/<tag>-<name>` (boxd's `UploadFile`, streamed
  with `UploadFileStream` over 3 MB), and the prompt lists the paths, so
  Claude Code or Codex reads the file from there (Claude Code opens images
  from a path). Up to 10 files of 20 MB each per message; a file over the
  limit, or an upload boxd refuses, shows the reason on the file and
  blocks sending until you remove it. Nothing falls back to another route.
- **Own branch per agent**: in *New agent*, "Work on its own branch" makes
  a git worktree for the agent before its first turn, so several agents can
  work on one repository in the same box without touching each other's
  files. It runs `git worktree add -b stowbox/<slug> <repo>-wt/<slug> HEAD`
  next to the repository (from another agent's worktree it still lands
  next to the main one), named after the title or first task plus six
  characters of the agent id, and the agent works in the same subdirectory
  of it that it was pointed at. It starts from the commit checked out
  there; uncommitted changes stay where they were. The branch shows in the
  agent's header and box panel, and its Changes tab reads that worktree.
  Removing the agent offers to remove the worktree too (`git worktree
  remove`, the branch is kept); git refuses a worktree with uncommitted or
  untracked files, and the dialog shows git's message and asks before
  discarding them. A worktree already deleted by hand is only pruned from
  git's list.
- **Changes**: a tab on every agent (and on every box, pointed at any
  directory) that reads the repository in the agent's working directory
  inside the box: the branch, how far it is ahead of or behind its
  upstream, the changed files from `git status`, and the full diff against
  the last commit with untracked files shown as new. From there: commit
  everything with a message (`git add -A`, hooks run), push (setting the
  upstream on a branch's first push), and open a pull request. The pull
  request is created by `gh pr create --fill` in the box when `gh` is
  installed and signed in, a `GH_TOKEN` or `GITHUB_TOKEN` from a setup
  preset counts; otherwise the host's compare page opens in the browser
  (GitHub, GitLab, Bitbucket). With a token set, pushes to
  `https://github.com` remotes use it too. Nothing waits for a prompt: git
  runs with terminal prompts off, ssh in batch mode (unless you set
  `GIT_SSH_COMMAND` or `core.sshCommand`, which are kept) and no editor, and every
  call has a deadline. The diff is cut at 512 KB inside the box and the tab
  says so; refusals such as a rejected push show git's message verbatim. A
  box in standby or hibernated is only read when you ask, since that wakes
  it; the tab reads again when the agent's turn ends.
- **Ports**: a tab on every box for seeing a dev server in the browser.
  It lists the box's HTTPS links (boxd proxies): the box address
  `https://<box>.<domain>`, which serves the first of 80, 443, 8080, 8000,
  3000, 5000 and 5173 that something listens on (else 8000) unless a port
  is pinned, and named links `https://<name>.<box>.<domain>` pinned to one
  port. Open or copy a link, add one (port and name), remove one, pin the
  box address to a port or let boxd detect it again. Raw TCP/UDP forwards
  (boxd's `ExposePort`, up to three per box, no TLS) are listed, added and
  removed in the same tab for databases and other non-HTTP servers. The
  addresses shown are the ones boxd returns; when it returns none the row
  says so instead of building one. The tab also lists what listens inside
  the box (`ss -ltnp`, or `/proc/net/tcp` where `ss` is missing, read
  only), marks a server bound to localhost only, which a link cannot
  reach, and offers *Create link* for the others. Before anything is
  exposed the form says it is public: anyone with the address reaches the
  server, and Stowbox puts no login in front of it. Links and forwards are
  read without waking the box; listing its ports runs a command in it, so
  a box in standby is only read when you ask. The command palette has
  *Ports and links of <box>* and *Open <box> in the browser*.
- **Terminal**: an interactive login shell in the box over boxd's exec
  stream (xterm.js on this side). Standby and hibernated boxes wake on open.
  The shell keeps running, scrollback and all, while you look at other
  tabs or boxes, and ends when the box's tab is closed. An open shell or
  agent terminal counts as activity for idle standby.
- **Cost**: account balance and not-yet-charged usage in the sidebar, metered
  usage per box with hours in each power state in the Details tab.
- **Cost guards** (Settings, both off by default): *Idle standby* puts a
  running box in standby after N minutes (30 by default) in which nothing
  happened in it: no agent starting, working or reconnecting (a detached
  turn counts until its exit is read, with or without a client attached)
  or holding queued messages it will send,
  no setup run, no terminal open, and no agent update, setup run, terminal
  keystroke or Changes action in the window. The idle clock never starts
  before Stowbox first saw the box running, and restarts when a box wakes.
  A 30 s countdown with Undo comes first, and the box is checked again at
  the end of it; Undo restarts the clock. A box can opt out in its Details
  tab. Stowbox only sees its own agents and terminals: an editor or `ssh`
  session opened elsewhere is invisible to it, so opt such a box out. The
  guard runs while Stowbox is open. *Spend warning* compares today's
  spend with a limit: boxd's metered usage of the org since local
  midnight when the key can read it (org owners and admins; it lags up to
  25 minutes), otherwise the current hourly estimate projected over a full
  day, and the warning says which. It shows in the Fleet alerts and once a
  day as a notification. The boxes the guard put in standby are listed in
  the alerts until they wake.
- **Setup presets**: a named list of commands plus secrets (for example
  "main commands": export `GITHUB_TOKEN`, `git clone`, `docker compose up -d`,
  run a script). Pick one in *New box* and it runs in the box as soon as it
  boots, or run it later from the box's Setup tab. Secret values stay in the
  OS keychain on this computer; in the box they are written to
  `~/.config/stowbox/env.sh` (mode 0600), which `~/.profile` and `~/.bashrc`
  load, so terminals and agents see them too. The script travels over boxd's
  exec stream on stdin, never on a command line, and secret values are
  masked in the setup log. Commands run in order in one shell (`cd` and
  `export` carry over); the first non-zero exit stops the run. A command can
  hold `{{name}}` placeholders (`git checkout {{branch}}`): the editor lists
  them with an optional default, and *New box* or *Run* asks for each value
  (required when there is no default). A value goes in as one single-quoted
  shell word, so keep placeholders outside quotes; other text in braces,
  such as `{{.State}}` for `docker inspect`, is left alone. Values are not
  secrets: the run keeps them and they show in the log.
- **Desktop**: one click opens boxd's live desktop view in the browser.
- **Open in VS Code or Cursor**: *Editor* on a box (and the code button in
  an agent's box panel, at the agent's directory or worktree) opens the
  folder over Remote-SSH with a `vscode://` or `cursor://` link. The dialog
  lists what will change before anything does, and nothing is written until
  you click Open: Stowbox makes its own key at `~/.ssh/stowbox/id_ed25519`
  with `ssh-keygen` if there is none (an existing one is never replaced, and
  your other keys are not read), links it to your boxd account with
  `LinkSshKey` (one Stowbox key per computer: linking again replaces it),
  and writes `Host stowbox-<box>` (the box's address, its SSH port on
  boxd's proxy, user `boxd`, that key) between `# BEGIN stowbox` and
  `# END stowbox` in `~/.ssh/config`. The block goes at the top of the file
  the first time, so its values win over a `Host *` further down; later
  runs replace it in place; nothing outside the markers changes, the file
  before each change is kept as `~/.ssh/config.stowbox-backup`, and a
  symlinked config is written through to its target. The proxy's host key
  goes in `~/.ssh/stowbox/known_hosts`, so the first connection does not
  stop at a fingerprint prompt. A box in standby or hibernated is woken
  first; a stopped box has to be started. The editor needs its Remote - SSH
  extension. The Details tab shows `ssh -p <port> boxd@<box>.boxd.sh` with
  a copy button, and the dialog copies `ssh stowbox-<box>` once the entry
  is there.
- **Notifications**: when an agent's turn finishes or fails, or a setup run
  ends, the OS shows a notification, by default only while Stowbox is not
  the window in front (Settings has both switches). One turn notifies once;
  reconnecting and a pressed Stop do not notify, and nothing is announced
  for the first 10 s after launch, when reattached turns that ended while
  Stowbox was closed replay their last lines. Clicking a notification
  brings the window back and opens the agent or the box's Setup tab, where
  the notification server reports clicks. The dock or taskbar badge counts
  notifications shown since the window was last in front (macOS and Linux
  desktops that support badges; on Windows an overlay on the taskbar
  button, the count on an orange square). On Windows a click is reported
  only while the toast is on screen, not from the Action Center, and the
  toasts carry Stowbox's name only in the installed app: run from
  `target\release` or `pnpm tauri dev` they show as Windows PowerShell.
- **Command palette and transcript search**: ⌘K on macOS, Ctrl+K
  elsewhere (or the search button in the top bar) opens one list to jump to
  Fleet, a box, an agent, a setup preset or Settings, and to run the common
  actions: new box, new agent, open a box's terminal, Changes, Setup or
  Details, standby, resume and the other power actions, open in VS Code or
  Cursor, theme and panels. Matching is fuzzy (words in any order, letters in
  order). From two characters on it also searches every agent's transcript
  on this computer, case-insensitively: messages, replies, reasoning, tool
  calls and their output, errors. Each match shows the agent, its box, the
  turn and a snippet with the match marked; Enter opens the agent's chat
  scrolled to that message, with a folded tool card opened. Transcripts are
  read line by line and files without the text are skipped without parsing;
  results stop at 5 per agent (its latest) and 50 in all. Transcript lines
  carry no time of their own, so the time shown is when the agent last did
  something. Inside the terminal Ctrl+K stays the shell's (kill to the end
  of the line); ⌘K types nothing there and works everywhere. On Windows the
  terminal copies a selection with Ctrl+C (with nothing selected Ctrl+C is
  the interrupt, as always) and pastes with Ctrl+V, as Windows Terminal does.
- Light and dark themes, keyboard reachable, honest empty, loading and error
  states everywhere.
