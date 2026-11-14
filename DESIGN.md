# Stowbox design direction

Reading: a desktop cockpit for developers who run coding agents in cloud
boxes, built to sit next to Orca and feel like the same family of tool:
monochrome, dense, quiet. Dial: ENERGY 1 / RHYTHM 1 / MOTION 1. The visual
language follows Orca by the owner's explicit request; the structure is
Stowbox's own.

## Palette

Two scales and nothing else. Dark: coal black surfaces `#0a0a0a` (canvas),
`#171717` (sidebar, cards, popovers), `#262626` (hover, muted), `#404040`
(pressed), borders at 7% white, text `#fafafa` with `#a1a1a1` for secondary.
Light: white canvas, `#fafafa` sidebar, `#f5f5f5` muted, `#e5e5e5` borders,
text `#0a0a0a` with `#737373` secondary. Primary buttons invert: light text
on `#171717`, or dark text on `#e5e5e5`.

Colour means state and only state:

- green: a box running, an agent working (or running, for a harness
  without hooks), a diff line added
- orange: standby, hibernated, pending, and an agent whose turn ended and
  that waits for the person
- red: failed, a destructive button on hover, a diff line removed
- grey: stopped, idle, an agent not running or not checked yet

No accent colour on chrome. The focus ring is a neutral grey at 50%.

## Type and shape

Geist (bundled, OFL) at 13px body, 12px in rows and controls, 11px for
secondary lines, 10px uppercase with 0.08em tracking for section titles.
System monospace for ids, money, commands and the terminal. Letter-spacing
0.01em on the body, as Orca sets it.

Radius scale from a 10px base: 6px rows and small controls, 8px buttons,
inputs and cards, 10px dialogs. Borders are 1px, never
shadows, except the floating shadow under dialogs and toasts.

## Layout

- A 38px top bar with the mark, the app name and the panel toggles. On
  macOS it is the title bar: traffic lights sit in it. On Windows the
  native title bar stays (snap layouts, the system menu and Windows' own
  window buttons keep working) and follows the app's theme; the top bar
  under it drops the mark and the name, which the title bar already shows.
  The taskbar button gets an overlay instead of a badge count: the count on
  an orange square.
- Left sidebar (260px): BOXES header with count and actions, a search field,
  box rows with a 7px state dot, name, and a second line with status and
  size, cost per hour on the right. Agents nest under an AGENTS header with
  their status and relative time.
- Main column: a tab strip with a pinned Fleet tab first, then open boxes
  and agents, each with its state dot, icon and close control, then the
  view.
- Fleet view (the landing page, after Aaron Francis's runner dashboard):
  four stat tiles in one bordered strip, an alerts list, "Working now" and
  "Waiting for you" side by side, then a bordered table with one row per
  box. Agents appear as 6x10 squares in the row, one per agent, coloured
  by state. CPU, memory and disk are a number over a 3px bar; the bar is
  neutral until 70% and turns orange, then red at 90%. Rows for boxes that
  are not running are muted and carry an "as of" age in orange.
- Setup presets view, reached from the sidebar under Fleet overview: a
  240px list of presets on the left, the editor on the right (name,
  secrets as name and masked value rows, commands as numbered monospace
  rows with move and remove controls, then the `{{name}}` variables found
  in the commands as a monospace name next to its default). A box's Setup
  tab shows the last run: one row per command with a 6x10 square (green
  while it runs, a neutral foreground square once it exits 0, red on
  failure, faded grey when pending or skipped) and the output below in a
  monospace well. New box and the Setup tab's Run dialog ask for variable
  values in the same name and value rows, prefilled with the defaults;
  the run shows "With name=value" above its steps, and the steps show the
  commands as they ran.
- An agent is its harness's own interface in a terminal, one terminal per
  agent, each on its own session. It has two sub-tabs, Terminal and
  Changes; a green square on Terminal while it is open. Above the
  terminal, a one-line bar says what runs where and whether it outlives
  the terminal (tmux) or not, with Add files on the right and Open again
  once the harness exits. Files dropped on the terminal or added there
  are copied into the box and their paths typed at the prompt; the bar
  says "Copying…" meanwhile, or the reason in red. The header's End
  session (red, ghost) quits the harness in the box; the terminal then
  gives way to a one-line empty state and "Open … again". The stats line
  says the state from the box (Working, Waiting for you, Running, Not
  running, Not checked yet) and the session id's first 8 characters. An
  agent whose box was deleted shows a grey dot, "Box deleted" in the
  stats, an empty state with the sentence and Remove the agent, and no
  terminal; Changes is disabled. A box's Agents tab ends with CLAUDE CODE LOGIN: one
  bordered row with a square (neutral foreground when a login was found,
  orange when none), which login and the account in monospace, then Sign
  in, the switch, and refresh; a note under it says where the login
  lives. The fleet table's last figure column is Age, since creation.
- A box has a Changes tab
  next to Setup, with a path field for the repository. Changes reads top
  down: the branch in monospace with "ahead, behind upstream" and the
  repository root, then one row of actions (commit message input, Commit
  all as the primary button, Push, Open pull request) with a line saying
  how the pull request will open, then the changed files as a bordered
  list (status letter, path, +added in green, -removed in red), then one
  collapsible card per file with the diff in the diff styles and hunk headers on the muted background. Files over 400 lines
  start folded. An action's result is a card under the actions: a neutral
  square and git's output when it worked, a red square, a red-tinted
  border and git's message verbatim when it did not ("Push rejected").
  Empty states name the directory: not a repository, missing, git not
  installed, no changes; a box in standby offers "Wake and read" instead
  of waking on its own.
- A box's Ports tab, after Changes: one row of buttons (New link,
  Forward a port, Refresh on the right), then three sections, each a
  bordered list. HTTPS LINKS: the host in monospace as a link (the box
  address marked "box address"), the port in monospace with "detected" or
  "pinned" muted, then open, copy, and Remove (red on hover) or Port… on
  the box address, which opens an inline row to pin a port or detect it
  again. LISTENING IN THE BOX: the port, the program, the bound addresses
  in faint monospace, and on the right the link that serves it, "localhost
  only, so boxd cannot reach it", or Create link. RAW FORWARDS with "n of
  3": the address, the protocol as a tag, the port in the box, copy and
  Unexpose. Every row starts with a 6x10 square for what listens on its
  port: green when something reachable does, orange when it listens on
  localhost only, grey when nothing does or the box was not read (the
  title says which). New link and Forward a port open a card above the
  lists with the port, then the name or the protocol, and a "This is
  public." line with the warning icon before the primary button, which
  says "Create public link" or "Expose publicly". A box in standby offers
  "Wake and check" instead of listing its ports on its own.
- New agent has a "Work on its own branch" checkbox under the working
  directory (disabled, with the reason, while the directory is empty).
  An agent with its own worktree shows its branch last in the header's
  second line, a branch icon and the name in monospace, cut with an
  ellipsis before it can widen the view; the box panel lists it under
  the directory with the repository it belongs to. Removing such an
  agent opens its own dialog: one checkbox, on by default, to remove the
  worktree too (the branch stays), disabled with the reason once the box
  is gone. When git refuses, its message shows
  verbatim in the red error style and the actions become "Keep the
  worktree" and "Discard changes and remove".
- Right panel (280px) when an agent is open: a toolbar of icon buttons
  (terminal, desktop, open in editor, details), the box card with the
  hourly estimate and power controls, the session facts.
- Open in editor (Editor in a box's header, the code icon in the agent's
  box panel and in the Details tab's SSH row) is a dialog: VS Code or
  Cursor as two choices (the last one is remembered), the folder in the
  box as a monospace input, then "What happens" as a numbered bordered
  list of exactly what will change on this computer and in the account
  (key, link, the `Host` entry in `~/.ssh/config` and its backup, waking
  the box), paths in monospace, then the SSH command with a copy button.
  When the box cannot be opened (stopped, no SSH port yet) the list gives
  way to the reason in red and Open is disabled. The result is an info
  toast naming what changed.
- Command palette (⌘K / Ctrl+K, and a search icon in the top bar): a
  620px panel near the top of the window over the dimmed app, one input
  with no ring of its own, then rows of 12px text: an icon or the state
  square, the label with the matched letters in semibold, a muted detail,
  and the kind (Action, View, Box, Agent, Preset) right-aligned. The
  active row has the hover background; the mouse and the arrow keys move
  the same highlight. With no query it lists what fits the open box or
  agent first, then views, boxes and recent agents; with one, "Best
  matches" across every box. No match is one muted line in the list. A
  footer names the keys.
- A 24px status bar: account, balance, how many boxes are running and the
  combined hourly estimate.
- Settings dialog: a key/value list. Notifications are two checkboxes
  (notify on outcomes; also while in front, disabled when the first is off)
  and, when the OS has not allowed or blocks notifications, one hint line
  saying so.
  Idle standby and Spend warning are a checkbox each with one number
  under it (minutes; a limit in the rate card's currency), indented to the
  checkbox text and faded while off, then a hint line on what the guard
  can and cannot see. The spend warning shows the current figure and where
  it came from (metered, or the hourly estimate over a day) in a hint line.
- Idle standby countdown: an info toast that stays ("mock-api goes to
  standby in 24 s", the count in monospace), a muted second line with the
  reason, and an Undo button in place of the dismiss control. A box kept
  running at the end of it gets a short info toast with the reason. The
  box's Details tab has an "Idle standby" row with the per-box checkbox,
  or "off" and a link to Settings.
- Fleet alerts have a third, neutral level for what Stowbox did on its
  own: a box the idle guard put in standby, with a moon icon in the muted
  colour, the estimate before and in standby, and "Idle standby 4 min
  ago". It stays while the box sleeps. The spend warning is an orange
  alert that opens Settings.
- OS notifications: the title names the agent and the outcome
  ("Fix the login bug finished", "Setup failed in api-dev"); the body is
  the box, or the provider's error verbatim, cut at 180 characters.

## Icons

Lucide, 1.75px stroke, 14 to 16px, inlined per glyph. The app mark is one
cube drawn with the same stroke.

## Motion

150ms background and colour transitions on hover, a 150ms rotate on the
disclosure caret, 160ms pop-in on dialogs and toasts. Hover reveals row
controls; touch devices always show them. Nothing loops.

## Copy

Short, specific, no marketing words. Empty states say why they are empty
and what fills them. Errors carry the provider's message verbatim.
