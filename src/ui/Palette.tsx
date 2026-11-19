import { For, Show, createEffect, createMemo, createSignal, on, onMount, type JSX } from "solid-js";
import type { Machine } from "../bridge/types";
import { actions, machineById, phaseOf, selectedAgent, selectedMachine, state } from "../state/store";
import * as I from "../icons";
import { ProviderIcon, dotState, isMac, phaseDot, powerButtons } from "./common";
import { agentPhaseLabel, sizeLabel, statusLabel } from "./format";
import { highlight, rank, type Match } from "./fuzzy";

/** Something the palette can do: go somewhere or run an action. */
interface Command {
  id: string;
  kind: "View" | "Box" | "Agent" | "Preset" | "Action";
  label: string;
  /** Matched too, never highlighted. */
  keywords?: string;
  detail?: string;
  icon: () => JSX.Element;
  /** The provider of a box, next to its status. */
  mark?: () => JSX.Element;
  run: () => void;
}

interface Row {
  cmd: Command;
  match: Match;
}

const COMMANDS_SHOWN = 12;
export const paletteShortcut = isMac ? "⌘K" : "Ctrl+K";

const Sq = (p: { state: string }) => <span class="sq" data-state={p.state} />;

function boxCommands(m: Machine, all: boolean): Command[] {
  const where = `${m.name} box`;
  const cmds: Command[] = [];
  if (all) {
    cmds.push(
      { id: `term:${m.id}`, kind: "Action", label: `Open terminal in ${m.name}`, keywords: "shell ssh console", icon: () => <I.Terminal />, run: () => actions.openBoxTab(m.id, "terminal") },
      { id: `changes:${m.id}`, kind: "Action", label: `Changes in ${m.name}`, keywords: "git diff commit push", icon: () => <I.FileDiff />, run: () => actions.openBoxTab(m.id, "changes") },
      { id: `ports:${m.id}`, kind: "Action", label: `Ports and links of ${m.name}`, keywords: "open port expose forward preview browser https url localhost dev server", icon: () => <I.Link />, run: () => actions.openBoxTab(m.id, "ports") },
      { id: `setup:${m.id}`, kind: "Action", label: `Setup in ${m.name}`, keywords: "preset run", icon: () => <I.ListChecks />, run: () => actions.openBoxTab(m.id, "setup") },
      { id: `details:${m.id}`, kind: "Action", label: `Details of ${m.name}`, keywords: "usage cost ssh", icon: () => <I.Info />, run: () => actions.openBoxTab(m.id, "details") },
    );
  }
  cmds.push({
    id: `newagent:${m.id}`,
    kind: "Action",
    label: `New agent in ${m.name}`,
    keywords: "claude codex terminal session",
    icon: () => <I.Plus />,
    run: () => actions.openDialog({ kind: "new-agent", machineId: m.id }),
  });
  if (!all) {
    cmds.push({ id: `ports:${m.id}`, kind: "Action", label: `Ports and links of ${m.name}`, keywords: "open port expose forward preview browser https url localhost dev server", icon: () => <I.Link />, run: () => actions.openBoxTab(m.id, "ports") });
  }
  if (m.url) {
    cmds.push({ id: `browse:${m.id}`, kind: "Action", label: `Open ${m.name} in the browser`, detail: m.url.replace("https://", ""), keywords: "port preview https url link dev server", icon: () => <I.External />, run: () => void actions.open(m.url) });
  }
  if (!all) cmds.push({ id: `term:${m.id}`, kind: "Action", label: `Open terminal in ${m.name}`, keywords: "shell ssh console", icon: () => <I.Terminal />, run: () => actions.openBoxTab(m.id, "terminal") });
  for (const b of powerButtons(m)) {
    cmds.push({ id: `power:${b.action}:${m.id}`, kind: "Action", label: `${b.label} ${m.name}`, detail: b.title, keywords: where, icon: () => <b.icon />, run: () => void actions.power(m.id, b.action) });
  }
  if (m.status !== "stopped") {
    cmds.push({ id: `editor:${m.id}`, kind: "Action", label: `Open ${m.name} in VS Code or Cursor`, keywords: "editor remote ssh", icon: () => <I.Code />, run: () => actions.openDialog({ kind: "open-editor", machineId: m.id, dir: null }) });
  }
  if (all) cmds.push({ id: `delete:${m.id}`, kind: "Action", label: `Delete ${m.name}`, detail: "Asks first", keywords: "destroy remove", icon: () => <I.Trash />, run: () => actions.confirmDestroy(m) });
  return cmds;
}

/** Everything the palette offers. With an empty query, only what fits the
 * current box or agent; with one, the actions of every box too. */
function commands(query: boolean): Command[] {
  const ctxAgent = selectedAgent();
  const ctxBox = selectedMachine() ?? (ctxAgent ? machineById(ctxAgent.machineId) : undefined);
  const out: Command[] = [];

  if (ctxAgent && !query) {
    out.push({ id: `agentchanges:${ctxAgent.id}`, kind: "Action", label: "Changes of this agent", keywords: "git diff", icon: () => <I.FileDiff />, run: () => actions.setAgentTab("changes") });
  }
  if (ctxBox && !query) out.push(...boxCommands(ctxBox, false));
  if (query) for (const m of state.machines) out.push(...boxCommands(m, true));
  if (query || !ctxBox) {
    out.push({ id: "newtask", kind: "Action", label: "New task", detail: "Describe it; Stowbox finds or makes the box", keywords: "prompt agent start run claude codex", icon: () => <I.Play />, run: () => actions.focusComposer() });
  }
  if (state.machines.length > 0 && (query || !ctxBox)) {
    out.push({ id: "newagent", kind: "Action", label: "New agent", keywords: "claude codex terminal session", icon: () => <I.Terminal />, run: () => actions.openDialog({ kind: "new-agent" }) });
  }
  out.push({ id: "newbox", kind: "Action", label: "New box", keywords: "create machine vm", icon: () => <I.Plus />, run: () => actions.openDialog({ kind: "new-box" }) });

  out.push(
    { id: "fleet", kind: "View", label: "Fleet", keywords: "overview dashboard home", icon: () => <I.Layers />, run: () => actions.select({ kind: "fleet" }) },
    { id: "presets", kind: "View", label: "Setup presets", keywords: "commands secrets", icon: () => <I.ListChecks />, run: () => actions.select({ kind: "presets" }) },
    { id: "settings", kind: "View", label: "Settings", keywords: "preferences notifications idle standby spend key", icon: () => <I.Settings />, run: () => actions.openDialog({ kind: "settings" }) },
  );

  for (const m of state.machines) {
    out.push({
      id: `box:${m.id}`,
      kind: "Box",
      label: m.name,
      detail: `${statusLabel[m.status]} · ${sizeLabel(m)}`,
      keywords: "box",
      icon: () => <Sq state={dotState(m.status)} />,
      mark: () => <ProviderIcon provider={m.provider} />,
      run: () => actions.select({ kind: "box", id: m.id }),
    });
  }
  const agents = [...state.agents].sort((a, b) => b.updatedAt - a.updatedAt);
  for (const a of query ? agents : agents.slice(0, 6)) {
    out.push({
      id: `agent:${a.id}`,
      kind: "Agent",
      label: a.title || "New agent",
      detail: `${a.machineName} · ${agentPhaseLabel[phaseOf(a)]}`,
      keywords: `agent ${a.machineName} ${a.worktree?.branch ?? ""}`,
      icon: () => <Sq state={phaseDot[phaseOf(a)]} />,
      run: () => actions.select({ kind: "agent", id: a.id }),
    });
  }
  if (query) {
    for (const p of state.presets) {
      out.push({ id: `preset:${p.id}`, kind: "Preset", label: p.name, detail: `${p.commands.length} command${p.commands.length === 1 ? "" : "s"}`, keywords: "preset setup", icon: () => <I.ListChecks />, run: () => actions.openPreset(p.id) });
    }
  }
  out.push(
    { id: "theme", kind: "Action", label: state.ui.theme === "dark" ? "Light theme" : "Dark theme", keywords: "appearance mode", icon: () => (state.ui.theme === "dark" ? <I.Sun /> : <I.Moon />), run: () => actions.setTheme(state.ui.theme === "dark" ? "light" : "dark") },
    { id: "sidebar", kind: "Action", label: state.ui.sidebar ? "Hide sidebar" : "Show sidebar", keywords: "toggle panel", icon: () => <I.PanelLeft />, run: () => actions.toggleSidebar() },
  );
  if (ctxAgent) {
    out.push({ id: "inspector", kind: "Action", label: state.ui.inspector ? "Hide box panel" : "Show box panel", keywords: "toggle inspector right", icon: () => <I.PanelRight />, run: () => actions.toggleInspector() });
  }
  return out;
}

const Label = (p: { text: string; match: Match }) => (
  <For each={highlight(p.text, p.match.indices)}>{(r) => (r.hit ? <mark>{r.text}</mark> : r.text)}</For>
);

export default function Palette() {
  let input!: HTMLInputElement;
  let list!: HTMLDivElement;
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  const rows = createMemo<Row[]>(() => {
    const q = query().trim();
    const ranked = rank(commands(!!q), q, (c) => ({ label: c.label, keywords: [c.keywords, c.detail, c.kind].filter(Boolean).join(" ") }));
    return (q ? ranked.slice(0, COMMANDS_SHOWN) : ranked).map(({ item, match }) => ({ cmd: item, match }));
  });
  createEffect(on(query, () => setActive(0), { defer: true }));
  createEffect(
    on(rows, (rs) => {
      if (active() >= rs.length) setActive(Math.max(0, rs.length - 1));
    }),
  );
  createEffect(
    on(active, (i) => {
      list?.querySelector(`#pal-opt-${i}`)?.scrollIntoView({ block: "nearest" });
    }),
  );

  const close = () => {
    actions.closePalette();
    if (returnFocus?.isConnected) returnFocus.focus();
  };
  const run = (row: Row | undefined) => {
    if (!row) return;
    actions.closePalette();
    row.cmd.run();
  };

  const onKey = (e: KeyboardEvent) => {
    const n = rows().length;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
      e.preventDefault();
      if (n) setActive((i) => (i + 1) % n);
    } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
      e.preventDefault();
      if (n) setActive((i) => (i - 1 + n) % n);
    } else if (e.key === "PageDown") {
      e.preventDefault();
      if (n) setActive((i) => Math.min(n - 1, i + 8));
    } else if (e.key === "PageUp") {
      e.preventDefault();
      setActive((i) => Math.max(0, i - 8));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!e.isComposing) run(rows()[active()]);
    } else if (e.key === "Tab") {
      // The input is the only stop; the list is driven from it.
      e.preventDefault();
    }
  };

  onMount(() => input.focus());

  const optionId = (i: number) => `pal-opt-${i}`;

  return (
    <div class="overlay palette-overlay" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div class="palette" role="dialog" aria-modal="true" aria-label="Search and commands">
        <div class="palette-input">
          <I.Search />
          <input
            ref={input}
            class="palette-field"
            type="text"
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-autocomplete="list"
            aria-activedescendant={rows().length ? optionId(active()) : undefined}
            placeholder="Search boxes, agents and actions"
            spellcheck={false}
            autocomplete="off"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={onKey}
          />
          <kbd class="palette-kbd">Esc</kbd>
        </div>
        <div class="palette-list scroll" id="palette-list" role="listbox" aria-label="Results" ref={list}>
          <Show when={rows().length > 0}>
            <div class="palette-group" role="presentation">
              {query().trim() ? "Best matches" : "Go to and do"}
            </div>
          </Show>
          <For each={rows()}>
            {(r, i) => {
              return (
                <div
                  id={optionId(i())}
                  class="palette-row"
                  classList={{ active: active() === i() }}
                  role="option"
                  aria-selected={active() === i()}
                  onMouseMove={() => active() !== i() && setActive(i())}
                  onClick={() => run(r)}
                >
                  <span class="palette-icon">{r.cmd.icon()}</span>
                  <span class="palette-label">
                    {r.cmd.mark?.()}
                    <Label text={r.cmd.label} match={r.match} />
                  </span>
                  <span class="palette-detail">{r.cmd.detail}</span>
                  <span class="palette-kind">{r.cmd.kind}</span>
                </div>
              );
            }}
          </For>
          <Show when={rows().length === 0}>
            <p class="palette-note">Nothing matches “{query().trim()}”.</p>
          </Show>
        </div>
        <div class="palette-foot">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> move
          </span>
          <span>
            <kbd>Enter</kbd> open
          </span>
          <span>
            <kbd>Esc</kbd> close
          </span>
          <span class="spacer" />
          <span class="palette-foot-hint">{isMac ? "⌘K opens this anywhere" : "Ctrl+K opens this outside the terminal"}</span>
        </div>
      </div>
    </div>
  );
}
