import { For, Show } from "solid-js";
import type { AgentMeta, Machine } from "../bridge/types";
import { actions, agentsForMachine, state } from "../state/store";
import * as I from "../icons";
import { AgentSquare, BrandMark, StatusSquare } from "./common";
import { agentStatusLabel, compactSize, money } from "./format";

const BoxRow = (p: { machine: Machine }) => {
  const selected = () => state.ui.selection.kind === "box" && state.ui.selection.id === p.machine.id;
  const agents = () => agentsForMachine(p.machine.id);
  const open = () => !state.ui.collapsed[p.machine.id];
  return (
    <>
      <div class="box-row" classList={{ selected: selected() }}>
        <button class="row-main" onClick={() => actions.select({ kind: "box", id: p.machine.id })} title={p.machine.name}>
          <StatusSquare status={p.machine.status} />
          <span class="row-name">{p.machine.name}</span>
          <span class="row-meta mono">
            {compactSize(p.machine)} · {money(p.machine.estHourlyMicro, p.machine.currency)}/h
          </span>
        </button>
        <Show when={agents().length > 0}>
          <button
            class="caret"
            classList={{ open: open() }}
            onClick={() => actions.toggleCollapsed(p.machine.id)}
            aria-label={open() ? "Collapse agents" : "Expand agents"}
            aria-expanded={open()}
          >
            <I.CaretRight />
          </button>
        </Show>
      </div>
      <Show when={open() && agents().length > 0}>
        <ul class="agent-list">
          <For each={agents()}>{(a) => <AgentRow agent={a} />}</For>
        </ul>
      </Show>
    </>
  );
};

const AgentRow = (p: { agent: AgentMeta }) => {
  const selected = () => state.ui.selection.kind === "agent" && state.ui.selection.id === p.agent.id;
  return (
    <li>
      <button
        class="agent-row"
        classList={{ selected: selected(), attention: p.agent.status.kind === "done" }}
        onClick={() => actions.select({ kind: "agent", id: p.agent.id })}
        title={p.agent.title || "New agent"}
      >
        <AgentSquare status={p.agent.status} />
        <span class="agent-title">{p.agent.title || "New agent"}</span>
        <span class="agent-state">{p.agent.status.kind === "done" ? "Ready for you" : agentStatusLabel(p.agent.status)}</span>
      </button>
    </li>
  );
};

export default function Sidebar() {
  const billing = () => state.account?.billing;
  return (
    <aside class="sidebar">
      <header class="brand">
        <BrandMark />
        <span class="brand-name">Stowbox</span>
        <button class="icon-btn" title="Refresh boxes" aria-label="Refresh boxes" onClick={() => void actions.refreshMachines()}>
          <I.Refresh />
        </button>
      </header>

      <Show when={state.account}>
        {(acct) => (
          <div class="account">
            <div class="account-name">{acct().displayName}</div>
            <Show
              when={billing()?.mode === "credits"}
              fallback={<div class="balance">boxd subscription · {billing()?.subscriptionStatus || "active"}</div>}
            >
              <div class="balance">
                <span>Balance</span>
                <strong class="mono">{money(billing()!.balanceMicro, billing()!.currency)}</strong>
              </div>
              <Show when={(billing()?.accruingMicro ?? 0) > 0}>
                <div class="balance faint">
                  <span>Not yet charged</span>
                  <span class="mono">{money(billing()!.accruingMicro!, billing()!.currency)}</span>
                </div>
              </Show>
            </Show>
          </div>
        )}
      </Show>

      <nav class="boxes" aria-label="Boxes">
        <div class="section-head">
          <span>Boxes</span>
          <span class="count">{state.machines.length}</span>
          <span class="spacer" />
          <button class="icon-btn" title="New box" aria-label="New box" onClick={() => actions.openDialog({ kind: "new-box" })}>
            <I.Plus />
          </button>
        </div>

        <Show when={!state.machinesLoaded}>
          <div class="side-state">Reading your boxes from boxd…</div>
        </Show>
        <Show when={state.machinesLoaded && state.machinesError}>
          <div class="side-state">
            <span>Could not list boxes: {state.machinesError}</span>
            <button class="btn" onClick={() => void actions.refreshMachines()}>
              <I.Refresh /> Try again
            </button>
          </div>
        </Show>
        <Show when={state.machinesLoaded && !state.machinesError && state.machines.length === 0}>
          <div class="side-state">
            <span>No boxes in this account yet. A box is a persistent Linux VM; agents run inside one.</span>
            <button class="btn primary" onClick={() => actions.openDialog({ kind: "new-box" })}>
              <I.Plus /> Create the first box
            </button>
          </div>
        </Show>
        <For each={state.machines}>{(m) => <BoxRow machine={m} />}</For>
      </nav>

      <footer class="sidebar-foot">
        <button class="btn quiet" onClick={() => actions.openDialog({ kind: "new-agent" })} disabled={state.machines.length === 0}>
          <I.Chat /> New agent
        </button>
        <span class="spacer" />
        <button
          class="icon-btn"
          title={state.ui.theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          aria-label="Toggle theme"
          onClick={() => actions.setTheme(state.ui.theme === "dark" ? "light" : "dark")}
        >
          <Show when={state.ui.theme === "dark"} fallback={<I.Hibernate />}>
            <I.Wake />
          </Show>
        </button>
        <button class="icon-btn" title="Settings" aria-label="Settings" onClick={() => actions.openDialog({ kind: "settings" })}>
          <I.Gear />
        </button>
      </footer>
    </aside>
  );
}
