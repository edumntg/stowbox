# Stowbox design direction

Reading: a desktop cockpit for developers who run coding agents in cloud
boxes. Dense, quiet, dark by default like the tools it sits next to, with one
warm accent for the thing that needs the person. Dial: ENERGY 2 / RHYTHM 1 /
MOTION 1.

## Identity

- **The box is the motif.** Every status marker is a small square, not a dot:
  machine state in the sidebar, agent state in rows and headers, the app mark
  itself (a box seen from above with an amber lid). One shape, repeated.
- **Palette.** Two cores and one accent. Dark: charcoal surfaces
  (`#151515` → `#333333`) with off-white text. Light: warm paper
  (`#f4f2ee` → `#ffffff`) with near-black text. Accent: cargo amber
  (`#f2b43c` dark, `#b8780a` light), the colour of shipping labels and
  warehouse tape, used only where the person is wanted: agents that finished
  and wait for review, the primary button, the text cursor in the terminal.
- **Status colours are semantic, not palette.** Green running, blue standby,
  slate hibernated, grey stopped, red failed. They mark real state and nothing
  else; no glow, no pulse.
- **Type.** The system UI stack and the system monospace stack. A desktop
  tool should feel native and start instantly; the person's own fonts cost
  nothing to load. Base 13px, ids and money in monospace.
- **Density.** 30px rows in the sidebar, 36px in tables, 44px headers. The
  window is a workbench, not a page.

## Layout

Three columns, built around the two decisions the person makes here: *which
agent needs me* and *what is this costing*.

- Left: boxes, each with its agents nested under it. Size and live €/h on
  every row; balance at the top.
- Centre: the selected agent's conversation, or the selected box with its
  Agents, Terminal and Details tabs.
- Right (when an agent is selected): the box behind it, with power controls
  and the hourly estimate, so pausing a box never means leaving the chat.

## Motion

Hover and focus states, a rotating caret, nothing on a loop. Liveness is
shown by content changing (streaming text, a status label, an elapsed cost),
not by animation.

## Copy

Short, specific, no marketing words. Empty states say why they are empty and
what fills them. Errors carry the provider's message verbatim.
