import { For, Match, Show, Switch, onCleanup, onMount } from "solid-js";
import { actions, agentBoxGone, agentById, capsOf, demo, hasPrices, machineById, openAgentsOf, selectedAgent, selectedMachine, spendCurrency, state, type TabGroup } from "../state/store";
import * as I from "../icons";
import AgentView from "./AgentView";
import BoxView from "./BoxView";
import FleetView from "./FleetView";
import PresetsView from "./PresetsView";
import Dialogs from "./Dialogs";
import Inspector from "./Inspector";
import Onboarding from "./Onboarding";
import Palette, { paletteShortcut } from "./Palette";
import Sidebar from "./Sidebar";
import Toasts from "./Toasts";
import { AgentDot, BrandMark, Dot, HarnessIcon, ProviderIcon, harnessName, isMac, isWindows } from "./common";
import { dragSort } from "./dragSort";
import { moneyByCurrency } from "./format";

/** The box whose tab is lit: the one shown, or the shown agent's. */
const currentBox = () => {
  const sel = state.ui.selection;
  if (sel.kind === "box") return sel.id;
  if (sel.kind === "agent") return agentById(sel.id)?.machineId;
  return undefined;
};

/** A box in the top row, marked with its provider as in the sidebar. It
 * stays active while one of its agents is shown. */
const BoxTab = (p: { group: TabGroup }) => {
  const id = () => p.group.machineId;
  const machine = () => machineById(id());
  const open = () => openAgentsOf(id());
  const active = () => currentBox() === id();
  const label = () => machine()?.name ?? agentById(open()[0] ?? "")?.machineName ?? "box";
  const provider = () => machine()?.provider ?? "";
  return (
    <div class="tab" classList={{ active: active() }} role="tab" aria-selected={active()} data-sort={id()}>
      <button class="tab-label" style={{ display: "flex", "align-items": "center", gap: "7px", "min-width": 0 }} onClick={() => actions.selectGroup(id())} title={machine() ? `${label()} on ${capsOf(provider())?.label ?? provider()}` : `${label()} (deleted)`}>
        <Dot status={machine()?.status ?? "stopped"} />
        <Show when={provider()} fallback={<I.Box />}>
          {(pr) => <ProviderIcon provider={pr()} />}
        </Show>
        <span class="tab-label">{label()}</span>
        <Show when={open().length}>{(n) => <span class="tab-count mono">{n()}</span>}</Show>
      </button>
      <button class="tab-close" aria-label={`Close ${label()} and its agents`} title="Close the box and its agent tabs" onClick={() => actions.closeTab({ kind: "box", id: id() })}>
        <I.Close />
      </button>
    </div>
  );
};

/** An agent in the agent row, which holds agents from every open box: its
 * state, the coding agent it runs, its title, and, quieter, the provider of
 * the box it runs in. The tooltip names the box. */
const AgentTab = (p: { id: string }) => {
  const agent = () => agentById(p.id);
  const active = () => state.ui.selection.kind === "agent" && state.ui.selection.id === p.id;
  const label = () => agent()?.title || "New agent";
  const box = () => machineById(agent()?.machineId ?? "");
  const where = () => {
    const a = agent();
    if (!a) return label();
    const on = box() ? ` on ${capsOf(box()!.provider)?.label ?? box()!.provider}` : " (box deleted)";
    return `${label()}: ${harnessName(a.harness)} in ${a.machineName}${on}`;
  };
  return (
    <div class="tab" classList={{ active: active() }} role="tab" aria-selected={active()} data-sort={p.id}>
      <button class="tab-label" style={{ display: "flex", "align-items": "center", gap: "7px", "min-width": 0 }} onClick={() => actions.select({ kind: "agent", id: p.id })} title={where()}>
        <Show when={agent()}>
          {(a) => (
            <>
              <Show when={!agentBoxGone(a())} fallback={<Dot status="stopped" />}>
                <AgentDot agent={a()} />
              </Show>
              <HarnessIcon harness={a().harness} />
            </>
          )}
        </Show>
        <span class="tab-label">{label()}</span>
        <Show when={box()}>
          {(m) => (
            <span class="tab-where">
              <ProviderIcon provider={m().provider} />
            </span>
          )}
        </Show>
      </button>
      <button class="tab-close" aria-label={`Close ${label()}`} onClick={() => actions.closeTab({ kind: "agent", id: p.id })}>
        <I.Close />
      </button>
    </div>
  );
};

/** The open boxes, then, while a box or an agent is in view, the open agents
 * of every box, so moving between agents in different boxes is one click.
 * Both rows can be reordered by dragging. */
const TabRows = () => {
  const current = () => {
    const id = currentBox();
    return id ? state.ui.tabs.find((g) => g.machineId === id) : undefined;
  };
  const sortable = (onMove: (from: string, to: string) => void) => (row: HTMLElement) => {
    const stop = dragSort(row, onMove);
    onCleanup(stop);
  };
  return (
    <>
      <div class="tabstrip" role="tablist" aria-label="Open boxes" ref={sortable((from, to) => actions.moveBoxTab(from, to))}>
        <FleetTab />
        <For each={state.ui.tabs}>{(g) => <BoxTab group={g} />}</For>
        <button class="tab-add" title="New agent" aria-label="New agent" onClick={() => actions.openDialog({ kind: "new-agent" })} disabled={state.machines.length === 0}>
          <I.Plus />
        </button>
      </div>
      <Show when={current()}>
        {(g) => (
          <div class="tabstrip sub" role="tablist" aria-label="Open agents" ref={sortable((from, to) => actions.moveAgentTab(from, to))}>
            <Show when={machineById(g().machineId)}>
              {(m) => (
                <div class="tab pinned" classList={{ active: state.ui.selection.kind === "box" }} role="tab" aria-selected={state.ui.selection.kind === "box"}>
                  <button class="tab-label" style={{ display: "flex", "align-items": "center", gap: "7px", "min-width": 0 }} onClick={() => actions.select({ kind: "box", id: m().id })} title={`${m().name}: agents, setup, changes, ports, terminal and details`}>
                    <I.Box />
                    <span class="tab-label">Box</span>
                  </button>
                </div>
              )}
            </Show>
            <For each={state.ui.agentTabs}>{(id) => <AgentTab id={id} />}</For>
            <Show when={machineById(g().machineId)}>
              <button class="tab-add" title="New agent in this box" aria-label="New agent in this box" onClick={() => actions.openDialog({ kind: "new-agent", machineId: g().machineId })}>
                <I.Plus />
              </button>
            </Show>
          </div>
        )}
      </Show>
    </>
  );
};

/** Always first and never closed: the fleet is where the app lands. */
const FleetTab = () => {
  const active = () => state.ui.selection.kind === "fleet";
  return (
    <div class="tab pinned" classList={{ active: active() }} role="tab" aria-selected={active()}>
      <button class="tab-label" style={{ display: "flex", "align-items": "center", gap: "7px", "min-width": 0 }} onClick={() => actions.select({ kind: "fleet" })} title="Fleet">
        <I.Layers />
        <span class="tab-label">Fleet</span>
      </button>
    </div>
  );
};

const Topbar = () => (
  <header class="topbar" classList={{ mac: isMac }} data-tauri-drag-region>
    <button class="icon-btn" title={state.ui.sidebar ? "Hide sidebar" : "Show sidebar"} aria-label="Toggle sidebar" aria-pressed={state.ui.sidebar} onClick={() => actions.toggleSidebar()}>
      <I.PanelLeft />
    </button>
    {/* Windows draws its own title bar above this one, with the icon and
        the name; repeating them here would make two title bars. */}
    <Show when={!isWindows}>
      <span class="topbar-title" data-tauri-drag-region>
        <BrandMark />
        Stowbox
      </span>
    </Show>
    <div class="topbar-right">
      <button class="icon-btn" title={`Search and commands (${paletteShortcut})`} aria-label="Search and commands" aria-keyshortcuts={isMac ? "Meta+K" : "Control+K"} onClick={() => actions.openPalette()}>
        <I.Search />
      </button>
      <button class="icon-btn" title={state.ui.theme === "dark" ? "Light theme" : "Dark theme"} aria-label="Toggle theme" onClick={() => actions.setTheme(state.ui.theme === "dark" ? "light" : "dark")}>
        <Show when={state.ui.theme === "dark"} fallback={<I.Moon />}>
          <I.Sun />
        </Show>
      </button>
      <button class="icon-btn" title="Settings" aria-label="Open settings" onClick={() => actions.openDialog({ kind: "settings" })}>
        <I.Settings />
      </button>
      <button class="icon-btn" title={state.ui.inspector ? "Hide box panel" : "Show box panel"} aria-label="Toggle box panel" aria-pressed={state.ui.inspector} onClick={() => actions.toggleInspector()}>
        <I.PanelRight />
      </button>
    </div>
  </header>
);

/** A newer release, one click from installed. */
const UpdateNotice = () => (
  <Show when={state.update.info}>
    {(u) => (
      <Show
        when={state.update.installing}
        fallback={
          <button class="statusbar-update" onClick={() => void actions.installUpdate()} title={u().notes || `Stowbox ${u().version} is out`}>
            <I.Refresh /> Stowbox {u().version} is available. Update and restart
          </button>
        }
      >
        {(i) => <span class="statusbar-update busy">Updating to {u().version}{i().progress === null ? "…" : ` ${Math.round(i().progress! * 100)}%`}</span>}
      </Show>
    )}
  </Show>
);

const Statusbar = () => {
  const running = () => state.machines.filter((m) => m.status === "running" || m.status === "suspended");
  const hourly = () => moneyByCurrency(running().map((m) => ({ micro: m.estHourlyMicro, currency: m.currency })), spendCurrency(), "/h");
  return (
    <footer class="statusbar">
      <UpdateNotice />
      <span class="spacer" />
      <span>
        <span class="dot" data-state={running().length ? "running" : "stopped"} />
        {running().length} running or standby
      </span>
      <Show when={hasPrices()}>
        <span class="mono" style={{ "font-size": "var(--text-2xs)" }}>
          {hourly()} est.
        </span>
      </Show>
    </footer>
  );
};

const Shell = () => {
  const agent = selectedAgent;
  const machine = selectedMachine;
  const panel = () => state.ui.inspector && !!agent();
  return (
    <div class="app" classList={{ demo }}>
      <Topbar />
      <Show when={demo}>
        <div class="demo-bar" role="status">
          <span class="sq" data-state="idle" />
          <span>
            <strong>Demo.</strong> Example boxes, agents and costs; nothing here is real and nothing reaches a provider.
          </span>
          <span class="spacer" />
          <button class="btn sm primary" onClick={() => actions.exitDemo()}>
            <I.Key /> Connect a provider
          </button>
        </div>
      </Show>
      <div class="body" classList={{ "has-panel": panel(), "no-sidebar": !state.ui.sidebar }}>
        <Show when={state.ui.sidebar}>
          <Sidebar />
        </Show>
        <main class="main">
          <TabRows />
          <div class="content">
            <Switch fallback={<FleetView />}>
              <Match when={agent()}>{(a) => <AgentView agent={a()} />}</Match>
              <Match when={machine()}>{(m) => <BoxView machine={m()} />}</Match>
              <Match when={state.ui.selection.kind === "presets"}>
                <PresetsView />
              </Match>
            </Switch>
          </div>
        </main>
        <Show when={panel()}>
          <Inspector agent={agent()!} />
        </Show>
      </div>
      <Statusbar />
    </div>
  );
};

/** ⌘K on macOS, Ctrl+K elsewhere, toggles the palette. Inside the terminal
 * Ctrl+K belongs to the shell (kill to the end of the line), so it is left
 * alone there; ⌘K types nothing in a terminal and works everywhere. */
function onShortcut(e: KeyboardEvent) {
  if (e.altKey || e.shiftKey || e.key.toLowerCase() !== "k") return;
  if (!(isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) return;
  if (!isMac && e.target instanceof Element && e.target.closest(".xterm")) return;
  e.preventDefault();
  e.stopPropagation();
  if (state.ui.palette) actions.closePalette();
  else actions.openPalette();
}

export default function App() {
  // Capture phase: the terminal stops the keys it handles from bubbling.
  onMount(() => document.addEventListener("keydown", onShortcut, true));
  onCleanup(() => document.removeEventListener("keydown", onShortcut, true));
  return (
    <>
      <Switch>
        <Match when={state.phase === "booting"}>
          <div class="boot">Starting…</div>
        </Match>
        <Match when={state.phase === "needs-key"}>
          <Onboarding />
        </Match>
        <Match when={state.phase === "error"}>
          <div class="boot boot-error">
            <span>Stowbox could not start: {state.bootError}</span>
            <button class="btn" onClick={() => void actions.boot()}>
              <I.Refresh /> Try again
            </button>
          </div>
        </Match>
        <Match when={state.phase === "ready"}>
          <Shell />
        </Match>
      </Switch>
      <Dialogs />
      <Show when={state.ui.palette}>
        <Palette />
      </Show>
      <Toasts />
    </>
  );
}
