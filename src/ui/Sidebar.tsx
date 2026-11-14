import { For, Show, createMemo } from "solid-js";
import type { AgentMeta, Machine } from "../bridge/types";
import { actions, agentsForMachine, multiProvider, phaseOf, providerLabel, state } from "../state/store";
import * as I from "../icons";
import { AgentDot, Dot } from "./common";
import { agentPhaseLabel, money, relTime, sizeLabel, statusLabel } from "./format";

const AgentRow = (p: { agent: AgentMeta }) => {
  const selected = () => state.ui.selection.kind === "agent" && state.ui.selection.id === p.agent.id;
  return (
    <li>
      <button class="row" classList={{ selected: selected() }} onClick={() => actions.select({ kind: "agent", id: p.agent.id })} title={p.agent.title || "New agent"}>
        <AgentDot agent={p.agent} />
        <span class="row-name">{p.agent.title || "New agent"}</span>
        <span class="row-right">
          <span class="row-time">{relTime(p.agent.updatedAt)}</span>
        </span>
        <span class="row-sub">{agentPhaseLabel[phaseOf(p.agent)]}</span>
      </button>
    </li>
  );
};

const BoxRow = (p: { machine: Machine; agents: AgentMeta[] }) => {
  const selected = () => state.ui.selection.kind === "box" && state.ui.selection.id === p.machine.id;
  const open = () => !state.ui.collapsed[p.machine.id];
  return (
    <li>
      <div class="row" classList={{ selected: selected() }} role="presentation">
        <Dot status={p.machine.status} />
        <button class="row-name" style={{ "text-align": "left", "min-width": 0 }} onClick={() => actions.select({ kind: "box", id: p.machine.id })} title={p.machine.name}>
          {p.machine.name}
        </button>
        <span class="row-right">
          <span class="mono" style={{ "font-size": "var(--text-2xs)" }}>
            {money(p.machine.estHourlyMicro, p.machine.currency)}/h
          </span>
          <Show when={p.agents.length > 0}>
            <button
              class="caret"
              classList={{ open: open() }}
              onClick={() => actions.toggleCollapsed(p.machine.id)}
              aria-label={open() ? "Collapse agents" : "Expand agents"}
              aria-expanded={open()}
            >
              <I.ChevronRight />
            </button>
          </Show>
        </span>
        <span class="row-sub">
          {statusLabel[p.machine.status]} · {sizeLabel(p.machine)}
          {multiProvider() ? ` · ${providerLabel(p.machine.provider)}` : ""}
        </span>
      </div>
      <Show when={open() && p.agents.length > 0}>
        <div class="sub-head">Agents · {p.agents.length}</div>
        <ul class="sub-list">
          <For each={p.agents}>{(a) => <AgentRow agent={a} />}</For>
        </ul>
      </Show>
    </li>
  );
};

export default function Sidebar() {
  const q = () => state.ui.filter.trim().toLowerCase();
  const rows = createMemo(() =>
    state.machines
      .map((m) => {
        const agents = agentsForMachine(m.id);
        if (!q()) return { m, agents };
        const match = m.name.toLowerCase().includes(q());
        const hits = agents.filter((a) => a.title.toLowerCase().includes(q()));
        return match || hits.length ? { m, agents: match ? agents : hits } : null;
      })
      .filter((x): x is { m: Machine; agents: AgentMeta[] } => x !== null),
  );
  return (
    <aside class="sidebar" aria-label="Boxes">
      <div class="side-head">
        <span class="side-title">Boxes</span>
        <span class="side-count">{state.machines.length}</span>
        <span class="spacer" />
        <button class="icon-btn" title="Refresh" aria-label="Refresh boxes" onClick={() => void actions.refreshMachines()}>
          <I.Refresh />
        </button>
        <button class="icon-btn" title="New box" aria-label="New box" onClick={() => actions.openDialog({ kind: "new-box" })}>
          <I.Plus />
        </button>
      </div>
      <nav class="side-nav" aria-label="Views">
        <button class="row nav" classList={{ selected: state.ui.selection.kind === "fleet" }} onClick={() => actions.select({ kind: "fleet" })}>
          <I.Layers />
          <span class="row-name">Fleet overview</span>
          <span class="row-right mono" style={{ "font-size": "var(--text-2xs)" }}>
            {state.machines.filter((m) => m.status === "running").length} running
          </span>
        </button>
        <button class="row nav" classList={{ selected: state.ui.selection.kind === "presets" }} onClick={() => actions.select({ kind: "presets" })}>
          <I.ListChecks />
          <span class="row-name">Setup presets</span>
          <span class="row-right mono" style={{ "font-size": "var(--text-2xs)" }}>
            {state.presets.length}
          </span>
        </button>
      </nav>
      <div class="side-search">
        <I.Search />
        <input type="search" placeholder="Search boxes and agents" value={state.ui.filter} onInput={(e) => actions.setFilter(e.currentTarget.value)} aria-label="Search boxes and agents" />
      </div>

      <nav class="side-list scroll">
        <Show when={!state.machinesLoaded}>
          <div class="side-state">Reading your boxes…</div>
        </Show>
        <Show when={state.machinesLoaded && state.machinesError}>
          <div class="side-state">
            <span>Could not list boxes: {state.machinesError}</span>
            <button class="btn sm" onClick={() => void actions.refreshMachines()}>
              <I.Refresh /> Try again
            </button>
          </div>
        </Show>
        <Show when={state.machinesLoaded && !state.machinesError && state.machines.length === 0}>
          <div class="side-state">
            <span>No boxes in this account yet. A box is a persistent Linux VM; agents run inside one.</span>
            <button class="btn primary sm" onClick={() => actions.openDialog({ kind: "new-box" })}>
              <I.Plus /> Create the first box
            </button>
          </div>
        </Show>
        <Show when={state.machines.length > 0 && rows().length === 0}>
          <div class="side-state">Nothing matches "{state.ui.filter.trim()}".</div>
        </Show>
        <ul>
          <For each={rows()}>{(r) => <BoxRow machine={r.m} agents={r.agents} />}</For>
        </ul>
      </nav>

      <div class="side-foot">
        <button class="btn ghost sm" onClick={() => actions.openDialog({ kind: "new-agent" })} disabled={state.machines.length === 0}>
          <I.Plus /> New agent
        </button>
        <span class="spacer" />
        <button class="icon-btn" title="Settings" aria-label="Settings" onClick={() => actions.openDialog({ kind: "settings" })}>
          <I.Settings />
        </button>
      </div>
    </aside>
  );
}
