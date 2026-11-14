import { For, Match, Show, Switch, onCleanup, onMount } from "solid-js";
import { actions, agentById, machineById, selectedAgent, selectedMachine, state, type Selection } from "../state/store";
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
import { AgentDot, BrandMark, Dot, isMac, isWindows } from "./common";
import { money } from "./format";

const Tab = (p: { sel: Selection }) => {
  const active = () => state.ui.selection.kind === p.sel.kind && (state.ui.selection as { id?: string }).id === (p.sel as { id?: string }).id;
  const label = () => {
    if (p.sel.kind === "box") return machineById(p.sel.id)?.name ?? "box";
    if (p.sel.kind === "agent") return agentById(p.sel.id)?.title || "New agent";
    return "";
  };
  return (
    <div class="tab" classList={{ active: active() }} role="tab" aria-selected={active()}>
      <button class="tab-label" style={{ display: "flex", "align-items": "center", gap: "7px", "min-width": 0 }} onClick={() => actions.select(p.sel)} title={label()}>
        <Switch>
          <Match when={p.sel.kind === "box" && machineById((p.sel as { id: string }).id)}>{(m) => <Dot status={m().status} />}</Match>
          <Match when={p.sel.kind === "agent" && agentById((p.sel as { id: string }).id)}>{(a) => <AgentDot status={a().status} />}</Match>
        </Switch>
        <Show when={p.sel.kind === "agent"} fallback={<I.Box />}>
          <I.Chat />
        </Show>
        <span class="tab-label">{label()}</span>
      </button>
      <button class="tab-close" aria-label={`Close ${label()}`} onClick={() => actions.closeTab(p.sel)}>
        <I.Close />
      </button>
    </div>
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

const Statusbar = () => {
  const running = () => state.machines.filter((m) => m.status === "running" || m.status === "suspended");
  const hourly = () => running().reduce((s, m) => s + m.estHourlyMicro, 0);
  const currency = () => state.machines[0]?.currency ?? state.account?.billing.currency ?? "usd";
  return (
    <footer class="statusbar">
      <Show when={state.account}>
        {(a) => (
          <>
            <span>{a().displayName}</span>
            <Show when={a().billing.mode === "credits"}>
              <span class="mono" style={{ "font-size": "var(--text-2xs)" }}>
                {money(a().billing.balanceMicro, a().billing.currency)} balance
              </span>
            </Show>
          </>
        )}
      </Show>
      <span class="spacer" />
      <span>
        <span class="dot" data-state={running().length ? "running" : "stopped"} />
        {running().length} running or standby
      </span>
      <span class="mono" style={{ "font-size": "var(--text-2xs)" }}>
        {money(hourly(), currency())}/h est.
      </span>
    </footer>
  );
};

const Shell = () => {
  const agent = selectedAgent;
  const machine = selectedMachine;
  const panel = () => state.ui.inspector && !!agent();
  return (
    <div class="app">
      <Topbar />
      <div class="body" classList={{ "has-panel": panel(), "no-sidebar": !state.ui.sidebar }}>
        <Show when={state.ui.sidebar}>
          <Sidebar />
        </Show>
        <main class="main">
          <div class="tabstrip" role="tablist" aria-label="Open items">
            <FleetTab />
            <For each={state.ui.tabs}>{(t) => <Tab sel={t} />}</For>
            <button class="tab-add" title="New agent" aria-label="New agent" onClick={() => actions.openDialog({ kind: "new-agent" })} disabled={state.machines.length === 0}>
              <I.Plus />
            </button>
          </div>
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
