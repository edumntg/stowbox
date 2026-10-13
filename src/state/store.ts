import { createStore, produce } from "solid-js/store";
import { api } from "../bridge/api";
import type {
  Account,
  AgentEvent,
  AgentMeta,
  AgentPush,
  CreateSpec,
  Machine,
  NewAgent,
  PowerAction,
  ProviderStatus,
  RateCard,
} from "../bridge/types";

export type Selection = { kind: "none" } | { kind: "box"; id: string } | { kind: "agent"; id: string };
export type BoxTab = "agents" | "terminal" | "details";
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
  ui: {
    theme: Theme;
    selection: Selection;
    inspector: boolean;
    boxTab: BoxTab;
    dialog: Dialog;
    toasts: Toast[];
    collapsed: Record<string, boolean>;
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
  ui: {
    theme: initialTheme(),
    selection: { kind: "none" },
    inspector: true,
    boxTab: "agents",
    dialog: null,
    toasts: [],
    collapsed: {},
  },
});

document.documentElement.dataset.theme = state.ui.theme;

let toastSeq = 0;
let unlisten: (() => void) | null = null;
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
    window.clearInterval(pollTimer);
    pollTimer = window.setInterval(() => {
      if (document.visibilityState === "visible" && state.phase === "ready") void actions.refreshMachines();
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
      provider: await api.providerStatus(),
    });
    setState("ui", { selection: { kind: "none" }, dialog: null });
  },

  async refreshMachines() {
    try {
      const machines = await api.listMachines();
      setState({ machines, machinesLoaded: true, machinesError: "" });
      if (state.ui.selection.kind === "box" && !machines.some((m) => m.id === (state.ui.selection as { id: string }).id)) {
        setState("ui", "selection", { kind: "none" });
      }
    } catch (e) {
      setState({ machinesError: message(e), machinesLoaded: true });
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
    setState("ui", "selection", sel);
    if (sel.kind === "agent") void actions.loadTranscript(sel.id);
    if (sel.kind === "box") setState("ui", "boxTab", "agents");
  },

  openBoxTab(id: string, tab: BoxTab) {
    setState("ui", { selection: { kind: "box", id }, boxTab: tab });
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
        if (state.ui.selection.kind === "box" && state.ui.selection.id === id) setState("ui", "selection", { kind: "none" });
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

  async createBox(spec: CreateSpec): Promise<Machine> {
    const m = await api.createMachine(spec);
    setState("machines", (ms) => [...ms.filter((x) => x.id !== m.id), m].sort((a, b) => a.name.localeCompare(b.name)));
    actions.select({ kind: "box", id: m.id });
    window.setTimeout(() => void actions.refreshMachines(), 3000);
    return m;
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
        if (state.ui.selection.kind === "agent" && state.ui.selection.id === a.id) {
          setState("ui", "selection", { kind: "box", id: a.machineId });
        }
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
