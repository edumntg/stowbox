import { createStore, produce } from "solid-js/store";
import { api } from "../bridge/api";
import type {
  Account,
  AgentEvent,
  AgentMeta,
  AgentPush,
  CreateSpec,
  Machine,
  MachineMetrics,
  NewAgent,
  PowerAction,
  Preset,
  PresetInput,
  ProviderStatus,
  RateCard,
  SetupPush,
  SetupRun,
} from "../bridge/types";

export type Selection = { kind: "none" } | { kind: "fleet" } | { kind: "presets" } | { kind: "box"; id: string } | { kind: "agent"; id: string };
export type BoxTab = "agents" | "setup" | "terminal" | "details";
export type Theme = "dark" | "light";
export type Dialog =
  | null
  | { kind: "new-box" }
  | { kind: "new-agent"; machineId?: string }
  | { kind: "settings" }
  | { kind: "confirm"; title: string; body: string; confirmLabel: string; danger: boolean; run: () => Promise<void> };
export interface Toast {
  id: number;
  kind: "error" | "info";
  text: string;
}

interface AppState {
  phase: "booting" | "needs-key" | "ready" | "error";
  bootError: string;
  provider: ProviderStatus | null;
  account: Account | null;
  rateCard: RateCard | null;
  machines: Machine[];
  machinesLoaded: boolean;
  machinesError: string;
  agents: AgentMeta[];
  transcripts: Record<string, AgentEvent[]>;
  live: Record<string, string>;
  pending: Record<string, boolean>;
  /** Last sample per box, kept after the box leaves `running` so the fleet
   * view can say "as of 24 min ago" instead of forgetting. */
  metrics: Record<string, MachineMetrics>;
  metricsError: Record<string, string>;
  metricsAt: number;
  metricsBusy: boolean;
  presets: Preset[];
  presetsLoaded: boolean;
  /** The latest setup run per box id. */
  setupRuns: Record<string, SetupRun>;
  ui: {
    theme: Theme;
    selection: Selection;
    tabs: Selection[];
    sidebar: boolean;
    inspector: boolean;
    boxTab: BoxTab;
    dialog: Dialog;
    toasts: Toast[];
    collapsed: Record<string, boolean>;
    filter: string;
  };
}

function initialTheme(): Theme {
  try {
    const t = localStorage.getItem("stowbox.theme");
    return t === "light" ? "light" : "dark";
  } catch {
    return "dark";
  }
}

export const [state, setState] = createStore<AppState>({
  phase: "booting",
  bootError: "",
  provider: null,
  account: null,
  rateCard: null,
  machines: [],
  machinesLoaded: false,
  machinesError: "",
  agents: [],
  transcripts: {},
  live: {},
  pending: {},
  metrics: {},
  metricsError: {},
  metricsAt: 0,
  metricsBusy: false,
  presets: [],
  presetsLoaded: false,
  setupRuns: {},
  ui: {
    theme: initialTheme(),
    selection: { kind: "fleet" },
    tabs: [],
    sidebar: true,
    inspector: true,
    boxTab: "agents",
    dialog: null,
    toasts: [],
    collapsed: {},
    filter: "",
  },
});

const sameSel = (a: Selection, b: Selection) =>
  a.kind === b.kind && (a.kind === "none" || a.kind === "fleet" || a.kind === "presets" || (a as { id: string }).id === (b as { id: string }).id);
const FLEET: Selection = { kind: "fleet" };

function loadTabs(): Selection[] {
  try {
    const raw = localStorage.getItem("stowbox.tabs");
    const parsed = raw ? (JSON.parse(raw) as Selection[]) : [];
    return Array.isArray(parsed) ? parsed.filter((t) => t && (t.kind === "box" || t.kind === "agent") && typeof (t as { id: unknown }).id === "string") : [];
  } catch {
    return [];
  }
}

function saveTabs(tabs: Selection[]) {
  try {
    localStorage.setItem("stowbox.tabs", JSON.stringify(tabs));
  } catch {
    /* private mode: tabs just do not persist */
  }
}

document.documentElement.dataset.theme = state.ui.theme;

let toastSeq = 0;
let unlisten: (() => void) | null = null;
let unlistenSetup: (() => void) | null = null;
let pollTimer: number | undefined;

function message(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

export const machineById = (id: string) => state.machines.find((m) => m.id === id);
export const agentById = (id: string) => state.agents.find((a) => a.id === id);
export const agentsForMachine = (id: string) => state.agents.filter((a) => a.machineId === id);
export const selectedMachine = () => (state.ui.selection.kind === "box" ? machineById(state.ui.selection.id) : undefined);
export const selectedAgent = () => (state.ui.selection.kind === "agent" ? agentById(state.ui.selection.id) : undefined);

function onPush(p: AgentPush) {
  if (!p || !p.meta || !p.agentId) return;
  setState(
    produce((s) => {
      const i = s.agents.findIndex((a) => a.id === p.agentId);
      if (i >= 0) s.agents[i] = p.meta;
      else s.agents.unshift(p.meta);
      const ev = p.event;
      if (!ev) return;
      if (ev.type === "text_delta") {
        s.live[p.agentId] = (s.live[p.agentId] ?? "") + ev.text;
        return;
      }
      if ((ev.type === "assistant" && ev.blocks.some((b) => b.type === "text")) || ev.type === "result" || ev.type === "exit") {
        delete s.live[p.agentId];
      }
      if (s.transcripts[p.agentId]) s.transcripts[p.agentId].push(ev);
    }),
  );
}

/** Output past this is dropped from the front, as the shell does. */
const SETUP_OUTPUT_CAP = 256 * 1024;

function onSetupPush(p: SetupPush) {
  if (!p || !p.run) return;
  const prev = state.setupRuns[p.run.machineId];
  let output = (prev && prev.id === p.run.id ? prev.output : "") + p.append;
  if (output.length > SETUP_OUTPUT_CAP) output = output.slice(output.length - SETUP_OUTPUT_CAP);
  setState("setupRuns", p.run.machineId, { ...p.run, output });
}

export const setupRunFor = (machineId: string): SetupRun | undefined => state.setupRuns[machineId];
export const setupActive = (r: SetupRun | undefined) => !!r && (r.status === "waiting" || r.status === "running");

export const actions = {
  toast(text: string, kind: Toast["kind"] = "error") {
    const id = ++toastSeq;
    setState("ui", "toasts", (t) => [...t, { id, kind, text }]);
    if (kind === "info") window.setTimeout(() => actions.dismiss(id), 4000);
  },
  dismiss(id: number) {
    setState("ui", "toasts", (t) => t.filter((x) => x.id !== id));
  },

  async boot() {
    setState({ phase: "booting", bootError: "" });
    try {
      const provider = await api.providerStatus();
      setState("provider", provider);
      if (!provider.configured) {
        setState("phase", "needs-key");
        return;
      }
      await actions.loadAll();
      setState("phase", "ready");
    } catch (e) {
      setState({ phase: "error", bootError: message(e) });
    }
  },

  async loadAll() {
    if (!unlisten) unlisten = await api.onAgentEvent(onPush);
    if (!unlistenSetup) unlistenSetup = await api.onSetupEvent(onSetupPush);
    void actions.loadPresets();
    api
      .setupRuns()
      .then((runs) => setState("setupRuns", Object.fromEntries(runs.map((r) => [r.machineId, r]))))
      .catch((e) => actions.toast("Could not read setup runs: " + message(e)));
    const [account, agents] = await Promise.all([
      api.whoami().catch((e) => {
        actions.toast("Could not read the account: " + message(e));
        return null;
      }),
      api.listAgents().catch((e) => {
        actions.toast("Could not read saved agents: " + message(e));
        return [] as AgentMeta[];
      }),
    ]);
    setState({ account, agents });
    api.rates().then((rateCard) => setState("rateCard", rateCard)).catch(() => undefined);
    await actions.refreshMachines();
    if (state.ui.tabs.length === 0) setState("ui", "tabs", loadTabs());
    actions.pruneTabs();
    if (state.ui.selection.kind === "none") setState("ui", "selection", FLEET);
    void actions.refreshMetrics();
    window.clearInterval(pollTimer);
    pollTimer = window.setInterval(() => {
      if (document.visibilityState !== "visible" || state.phase !== "ready") return;
      void actions.refreshMachines().then(() => {
        // Samples cost an exec per running box, so only keep them fresh
        // while someone is looking at the fleet.
        if (state.ui.selection.kind === "fleet") void actions.refreshMetrics();
      });
    }, 15_000);
  },

  async saveKey(key: string) {
    const account = await api.setApiKey(key);
    setState({ account, provider: await api.providerStatus() });
    await actions.loadAll();
    setState("phase", "ready");
  },

  async signOut() {
    await api.clearApiKey();
    window.clearInterval(pollTimer);
    setState({
      phase: "needs-key",
      account: null,
      machines: [],
      machinesLoaded: false,
      agents: [],
      transcripts: {},
      live: {},
      metrics: {},
      metricsError: {},
      metricsAt: 0,
      presets: [],
      presetsLoaded: false,
      setupRuns: {},
      provider: await api.providerStatus(),
    });
    setState("ui", { selection: FLEET, dialog: null, tabs: [] });
    saveTabs([]);
  },

  async refreshMachines() {
    try {
      const machines = await api.listMachines();
      setState({ machines, machinesLoaded: true, machinesError: "" });
      actions.pruneTabs();
    } catch (e) {
      setState({ machinesError: message(e), machinesLoaded: true });
    }
  },

  /** One exec per running box, in parallel. Boxes in any other state are
   * skipped on purpose: sampling would wake them. */
  async refreshMetrics() {
    if (state.metricsBusy) return;
    const targets = state.machines.filter((m) => m.status === "running");
    setState("metricsBusy", true);
    try {
      await Promise.all(
        targets.map(async (m) => {
          try {
            const sample = await api.machineMetrics(m.id);
            if (sample) setState("metrics", m.id, sample);
            setState("metricsError", m.id, undefined!);
          } catch (e) {
            setState("metricsError", m.id, message(e));
          }
        }),
      );
    } finally {
      setState({ metricsBusy: false, metricsAt: Date.now() });
    }
  },

  async refreshAccount() {
    try {
      setState("account", await api.whoami());
    } catch (e) {
      actions.toast("Could not refresh the balance: " + message(e));
    }
  },

  select(sel: Selection) {
    if (sel.kind === "box" || sel.kind === "agent") {
      if (!state.ui.tabs.some((t) => sameSel(t, sel))) {
        setState("ui", "tabs", (tabs) => [...tabs, sel]);
        saveTabs(state.ui.tabs);
      }
    }
    setState("ui", "selection", sel);
    if (sel.kind === "fleet" && Date.now() - state.metricsAt > 5_000) void actions.refreshMetrics();
    if (sel.kind === "agent") void actions.loadTranscript(sel.id);
    if (sel.kind === "box" && !(state.ui.selection.kind === "box" && state.ui.selection.id === sel.id)) setState("ui", "boxTab", "agents");
  },

  closeTab(sel: Selection) {
    const tabs = state.ui.tabs;
    const i = tabs.findIndex((t) => sameSel(t, sel));
    if (i < 0) return;
    const next = tabs.filter((_, j) => j !== i);
    setState("ui", "tabs", next);
    saveTabs(next);
    if (sameSel(state.ui.selection, sel)) {
      const neighbour = next[Math.min(i, next.length - 1)];
      setState("ui", "selection", neighbour ?? FLEET);
      if (neighbour?.kind === "agent") void actions.loadTranscript(neighbour.id);
    }
  },

  /** Drops tabs whose box or agent no longer exists. */
  pruneTabs() {
    const ok = (t: Selection) =>
      (t.kind === "box" && state.machines.some((m) => m.id === t.id)) || (t.kind === "agent" && state.agents.some((a) => a.id === t.id));
    const next = state.ui.tabs.filter(ok);
    if (next.length !== state.ui.tabs.length) {
      setState("ui", "tabs", next);
      saveTabs(next);
    }
    const sel = state.ui.selection;
    if ((sel.kind === "box" || sel.kind === "agent") && !ok(sel)) setState("ui", "selection", next[0] ?? FLEET);
  },

  openBoxTab(id: string, tab: BoxTab) {
    actions.select({ kind: "box", id });
    setState("ui", "boxTab", tab);
  },

  toggleSidebar() {
    setState("ui", "sidebar", (v) => !v);
  },

  setFilter(filter: string) {
    setState("ui", "filter", filter);
  },

  async loadTranscript(id: string) {
    if (state.transcripts[id]) return;
    try {
      const events = await api.agentEvents(id);
      setState("transcripts", id, events);
    } catch (e) {
      actions.toast("Could not load the transcript: " + message(e));
    }
  },

  setTheme(theme: Theme) {
    setState("ui", "theme", theme);
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("stowbox.theme", theme);
    } catch {
      /* private mode: the choice just does not persist */
    }
  },
  toggleInspector() {
    setState("ui", "inspector", (v) => !v);
  },
  setBoxTab(tab: BoxTab) {
    setState("ui", "boxTab", tab);
  },
  toggleCollapsed(id: string) {
    setState("ui", "collapsed", id, (v) => !v);
  },
  openDialog(d: Dialog) {
    setState("ui", "dialog", d);
  },
  closeDialog() {
    setState("ui", "dialog", null);
  },

  async power(id: string, action: PowerAction) {
    setState("pending", id, true);
    try {
      await api.machinePower(id, action);
      if (action === "destroy") {
        setState("machines", (ms) => ms.filter((m) => m.id !== id));
        setState("setupRuns", id, undefined!);
        actions.closeTab({ kind: "box", id });
        actions.toast("Box deleted.", "info");
      }
      // The provider answers once the transition is underway; read back now
      // and again later for states that take a moment (hibernate, reboot).
      await actions.refreshMachines();
      window.setTimeout(() => void actions.refreshMachines(), 2500);
      window.setTimeout(() => void actions.refreshMachines(), 8000);
    } catch (e) {
      actions.toast(`${action} failed: ${message(e)}`);
    } finally {
      setState("pending", id, false);
    }
  },

  confirmDestroy(m: Machine) {
    const running = agentsForMachine(m.id).filter((a) => a.status.kind === "thinking" || a.status.kind === "tool" || a.status.kind === "starting");
    actions.openDialog({
      kind: "confirm",
      title: `Delete ${m.name}?`,
      body:
        `The machine and its disk are destroyed. This cannot be undone.` +
        (running.length ? ` ${running.length} agent${running.length > 1 ? "s are" : " is"} still working in it.` : ""),
      confirmLabel: "Delete box",
      danger: true,
      run: () => actions.power(m.id, "destroy"),
    });
  },

  async createBox(spec: CreateSpec, presetId?: string | null): Promise<Machine> {
    const m = await api.createMachine(spec, presetId ?? null);
    setState("machines", (ms) => [...ms.filter((x) => x.id !== m.id), m].sort((a, b) => a.name.localeCompare(b.name)));
    actions.select({ kind: "box", id: m.id });
    if (presetId) setState("ui", "boxTab", "setup");
    window.setTimeout(() => void actions.refreshMachines(), 3000);
    return m;
  },

  async loadPresets() {
    try {
      setState({ presets: await api.listPresets(), presetsLoaded: true });
    } catch (e) {
      setState("presetsLoaded", true);
      actions.toast("Could not read setup presets: " + message(e));
    }
  },

  async savePreset(input: PresetInput): Promise<Preset> {
    const saved = await api.savePreset(input);
    setState("presets", (ps) => (ps.some((p) => p.id === saved.id) ? ps.map((p) => (p.id === saved.id ? saved : p)) : [...ps, saved]));
    return saved;
  },

  confirmDeletePreset(p: Preset) {
    actions.openDialog({
      kind: "confirm",
      title: `Delete "${p.name}"?`,
      body: `The commands and the ${p.secretNames.length === 1 ? "secret" : `${p.secretNames.length} secrets`} stored with it are removed from this computer. Boxes already set up keep their files and their ~/.config/stowbox/env.sh.`,
      confirmLabel: "Delete preset",
      danger: true,
      run: async () => {
        await api.deletePreset(p.id);
        setState("presets", (ps) => ps.filter((x) => x.id !== p.id));
      },
    });
  },

  async startSetup(machineId: string, presetId: string) {
    try {
      const run = await api.setupStart(machineId, presetId);
      setState("setupRuns", machineId, run);
    } catch (e) {
      actions.toast("Setup could not start: " + message(e));
    }
  },

  async cancelSetup(machineId: string) {
    try {
      await api.setupCancel(machineId);
    } catch (e) {
      actions.toast("Cancel failed: " + message(e));
    }
  },

  async forkBox(source: Machine) {
    setState("pending", source.id, true);
    try {
      const m = await api.forkMachine(source.id, null);
      setState("machines", (ms) => [...ms, m].sort((a, b) => a.name.localeCompare(b.name)));
      actions.select({ kind: "box", id: m.id });
      actions.toast(`Forked ${source.name} as ${m.name}.`, "info");
    } catch (e) {
      actions.toast("Fork failed: " + message(e));
    } finally {
      setState("pending", source.id, false);
    }
  },

  async createAgent(spec: NewAgent, firstMessage?: string): Promise<AgentMeta> {
    const meta = await api.createAgent(spec);
    setState("agents", (as) => [meta, ...as]);
    setState("transcripts", meta.id, []);
    actions.select({ kind: "agent", id: meta.id });
    if (firstMessage && firstMessage.trim()) await actions.send(meta.id, firstMessage);
    return meta;
  },

  async send(id: string, text: string) {
    try {
      await api.agentSend(id, text);
    } catch (e) {
      actions.toast(message(e));
      throw e;
    }
  },

  async stop(id: string) {
    try {
      await api.agentStop(id);
    } catch (e) {
      actions.toast("Stop failed: " + message(e));
    }
  },

  async rename(id: string, title: string) {
    try {
      const meta = await api.renameAgent(id, title);
      setState("agents", (as) => as.map((a) => (a.id === id ? meta : a)));
    } catch (e) {
      actions.toast("Rename failed: " + message(e));
    }
  },

  confirmDeleteAgent(a: AgentMeta) {
    actions.openDialog({
      kind: "confirm",
      title: `Remove "${a.title || "this agent"}"?`,
      body: "The transcript is deleted from this computer. The box and its files are not touched.",
      confirmLabel: "Remove agent",
      danger: true,
      run: async () => {
        await api.deleteAgent(a.id);
        setState("agents", (as) => as.filter((x) => x.id !== a.id));
        actions.closeTab({ kind: "agent", id: a.id });
      },
    });
  },

  async copy(text: string, what = "Copied") {
    try {
      await api.copyText(text);
      actions.toast(what + ".", "info");
    } catch (e) {
      actions.toast("Clipboard failed: " + message(e));
    }
  },

  async open(url: string) {
    try {
      await api.openUrl(url);
    } catch (e) {
      actions.toast("Could not open the browser: " + message(e));
    }
  },
};
