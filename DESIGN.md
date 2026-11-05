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

- green: a box running, an agent working, a tool call in progress, a diff
  line added
- orange: standby, hibernated, pending, an agent that finished and
  waits for the person, and an agent reconnecting to a turn that keeps
  running in the box (the transcript says why: the stream dropped, or the
  box is in standby)
- red: failed, a destructive button on hover, a diff line removed
- grey: stopped, idle

No accent colour on chrome. The focus ring is a neutral grey at 50%.

## Type and shape

Geist (bundled, OFL) at 13px body, 12px in rows and controls, 11px for
secondary lines, 10px uppercase with 0.08em tracking for section titles.
System monospace for ids, money, commands and the terminal. Letter-spacing
0.01em on the body, as Orca sets it.

Radius scale from a 10px base: 6px rows and small controls, 8px buttons,
inputs and cards, 10px dialogs, 14px the composer. Borders are 1px, never
shadows, except the floating shadow under dialogs and toasts.

## Layout

- A 38px top bar with the mark, the app name and the panel toggles. On
  macOS it is the title bar: traffic lights sit in it.
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
- The chat composer: attached files sit inside it above the text as
  bordered chips (paperclip, the name in monospace, the size or "copying";
  a failed one turns red with the reason and blocks Send until removed),
  and the paperclip button sits next to Send. While a turn runs the
  buttons are Queue and Stop. Queued messages list between the transcript
  and the composer under a "QUEUED · n" title with one line on when they
  go out and a Clear button: one bordered row per message with a 6x10
  square (orange while the queue will send, grey once Stop paused it),
  the text cut at three lines, its files below, and edit and remove on
  hover. A paused queue says why and offers Send next. Sent files show
  under the message in the transcript, the path in the box on hover.
- An agent has two sub-tabs, Chat and Changes; a box has a Changes tab
  next to Setup, with a path field for the repository. Changes reads top
  down: the branch in monospace with "ahead, behind upstream" and the
  repository root, then one row of actions (commit message input, Commit
  all as the primary button, Push, Open pull request) with a line saying
  how the pull request will open, then the changed files as a bordered
  list (status letter, path, +added in green, -removed in red), then one
  collapsible card per file with the diff in the transcript's diff
  styles and hunk headers on the muted background. Files over 400 lines
  start folded. An action's result is a card under the actions: a neutral
  square and git's output when it worked, a red square, a red-tinted
  border and git's message verbatim when it did not ("Push rejected").
  Empty states name the directory: not a repository, missing, git not
  installed, no changes; a box in standby offers "Wake and read" instead
  of waking on its own.
- New agent has a "Work on its own branch" checkbox under the working
  directory (disabled, with the reason, while the directory is empty).
  An agent with its own worktree shows its branch last in the header's
  second line, a branch icon and the name in monospace, cut with an
  ellipsis before it can widen the view; the box panel lists it under
  the directory with the repository it belongs to. Removing such an
  agent opens its own dialog: one checkbox, on by default, to remove the
  worktree too (the branch stays), disabled with the reason while a turn
  runs or once the box is gone. When git refuses, its message shows
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
  matches" then "In transcripts": the agent's square and title, "box ·
  turn n · kind" muted, the agent's last activity right, and a two-line
  snippet with the match in semibold and underlined in the ring grey, no
  colour. Empty, searching and failed states are one muted line in the
  list. A footer names the keys. Opening a match outlines the message in
  the ring grey for a moment, which fades.
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
