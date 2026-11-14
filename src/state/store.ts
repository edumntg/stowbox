import { createStore, produce } from "solid-js/store";
import { api } from "../bridge/api";
import { presetVariables } from "../bridge/variables";
import type {
  TermTarget,
  Account,
  AgentMeta,
  Capabilities,
  AgentLive,
  Attachment,
  LivePush,
  CreateSpec,
  Editor,
  GuardEntry,
  IdlePolicy,
  Machine,
  MachineMetrics,
  NewAgent,
  Notice,
  NoticeTarget,
  NotifyPermission,
  PowerAction,
  Preset,
  PresetInput,
  ProviderStatus,
  RateCard,
  SetupPush,
  SetupRun,
  SpendCheck,
  VariableValues,
} from "../bridge/types";
import { agentPhase, money, spendLine } from "../ui/format";
import { agentTerminalKey, boxTerminalKey, closeTerminal } from "../ui/termSessions";

export type Selection = { kind: "none" } | { kind: "fleet" } | { kind: "presets" } | { kind: "box"; id: string } | { kind: "agent"; id: string };
export type BoxTab = "agents" | "setup" | "changes" | "ports" | "terminal" | "details";
export type AgentTab = "terminal" | "changes";
export type Theme = "dark" | "light";
export type Dialog =
  | null
  | { kind: "new-box" }
  | { kind: "run-setup"; machineId: string; presetId: string }
  | { kind: "new-agent"; machineId?: string }
  | { kind: "settings" }
  | { kind: "delete-agent"; agentId: string }
  /** `dir` is the directory in the box to open; null is the home directory. */
  | { kind: "open-editor"; machineId: string; dir: string | null }
  | { kind: "confirm"; title: string; body: string; confirmLabel: string; danger: boolean; run: () => Promise<void> };
export interface NotifyPrefs {
  /** Notify when an agent finishes or fails, or a setup run ends. */
  enabled: boolean;
  /** Also while the Stowbox window is in front. */
  whenFocused: boolean;
}
/** The cost guards. The numbers are kept while a guard is off, so turning
 * it on again restores the last choice. */
export interface GuardPrefs {
  idleOn: boolean;
  idleMinutes: number;
  /** Boxes the idle guard leaves alone. */
  optedOut: string[];
  spendOn: boolean;
  /** In whole units of the rate card's currency. */
  spendLimit: number;
}
/** One box the idle guard put in standby, or stopped where there is none. */
export interface GuardAction {
  machineId: string;
  /** Stopped instead of put in standby: the provider has no standby. */
  stopped?: boolean;
  machineName: string;
  atMs: number;
  idleSinceMs: number;
  hourlyBeforeMicro: number;
  hourlyAfterMicro: number;
  currency: string;
}
export interface Toast {
  id: number;
  kind: "error" | "info";
  text: string;
  /** A second line under the text. */
  detail?: string;
  /** Shown as a button; the toast stays until it is used or dismissed. */
  action?: { label: string; run: () => void };
  /** A countdown to this moment, shown next to the text. */
  untilMs?: number;
}

interface AppState {
  phase: "booting" | "needs-key" | "ready" | "error";
  bootError: string;
  /** Every provider Stowbox knows, connected or not. */
  providers: ProviderStatus | null;
  /** By provider id, for the connected ones that answered. */
  accounts: Record<string, Account>;
  rateCards: Record<string, RateCard>;
  machines: Machine[];
  machinesLoaded: boolean;
  /** Every connected provider failed to list. */
  machinesError: string;
  /** Providers whose listing failed while others answered. */
  providerErrors: { provider: string; message: string }[];
  agents: AgentMeta[];
  /** What the box's watch saw last, by agent id; missing until it reports. */
  live: Record<string, AgentLive>;
  /** Per box: whether it has tmux, so agents outlive their terminal. */
  tmux: Record<string, boolean>;
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
  notify: NotifyPrefs & { permission: NotifyPermission | null };
  guard: GuardPrefs & {
    /** Countdowns running before a box is suspended, by machine id. */
    pending: Record<string, { untilMs: number; toastId: number }>;
    /** What the guard did, newest first, kept for a day. */
    log: GuardAction[];
    spend: SpendCheck | null;
    spendError: string;
  };
  ui: {
    theme: Theme;
    selection: Selection;
    tabs: Selection[];
    sidebar: boolean;
    inspector: boolean;
    boxTab: BoxTab;
    agentTab: AgentTab;
    dialog: Dialog;
    toasts: Toast[];
    collapsed: Record<string, boolean>;
    filter: string;
    /** The command palette (⌘K / Ctrl+K) is open. */
    palette: boolean;
    /** The preset the presets view should show. */
    presetFocus: string | null;
    /** What the agent's terminal submits first, once: the first prompt of
     * a new agent. `seq` tells two requests apart. */
    agentTerminal: { agentId: string; input: string | null; seq: number } | null;
    /** What the box's Terminal tab runs instead of a shell; `seq` reopens it. */
    boxTerminal: { machineId: string; target: TermTarget; seq: number } | null;
  };
}

function loadNotifyPrefs(): NotifyPrefs {
  try {
    const raw = JSON.parse(localStorage.getItem("stowbox.notify") ?? "{}") as Partial<NotifyPrefs>;
    return { enabled: raw.enabled !== false, whenFocused: raw.whenFocused === true };
  } catch {
    return { enabled: true, whenFocused: false };
  }
}

const GUARD_DEFAULTS: GuardPrefs = { idleOn: false, idleMinutes: 30, optedOut: [], spendOn: false, spendLimit: 5 };
const LOG_KEEP_MS = 24 * 3_600_000;

function loadGuardPrefs(): GuardPrefs {
  try {
    const raw = JSON.parse(localStorage.getItem("stowbox.guards") ?? "{}") as Partial<GuardPrefs>;
    const pos = (n: unknown, d: number) => (typeof n === "number" && Number.isFinite(n) && n > 0 ? n : d);
    return {
      idleOn: raw.idleOn === true,
      idleMinutes: Math.round(pos(raw.idleMinutes, GUARD_DEFAULTS.idleMinutes)),
      optedOut: Array.isArray(raw.optedOut) ? raw.optedOut.filter((x) => typeof x === "string") : [],
      spendOn: raw.spendOn === true,
      spendLimit: pos(raw.spendLimit, GUARD_DEFAULTS.spendLimit),
    };
  } catch {
    return { ...GUARD_DEFAULTS };
  }
}

function loadGuardLog(): GuardAction[] {
  try {
    const raw = JSON.parse(localStorage.getItem("stowbox.guardLog") ?? "[]") as GuardAction[];
    return Array.isArray(raw) ? raw.filter((a) => a && typeof a.machineId === "string" && Date.now() - a.atMs < LOG_KEEP_MS) : [];
  } catch {
    return [];
  }
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
  providers: null,
  accounts: {},
  rateCards: {},
  machines: [],
  machinesLoaded: false,
  machinesError: "",
  providerErrors: [],
  agents: [],
  live: {},
  tmux: {},
  pending: {},
  metrics: {},
  metricsError: {},
  metricsAt: 0,
  metricsBusy: false,
  presets: [],
  presetsLoaded: false,
  setupRuns: {},
  notify: { ...loadNotifyPrefs(), permission: null },
  guard: { ...loadGuardPrefs(), pending: {}, log: loadGuardLog(), spend: null, spendError: "" },
  ui: {
    theme: initialTheme(),
    selection: { kind: "fleet" },
    tabs: [],
    sidebar: true,
    inspector: true,
    boxTab: "agents",
    agentTab: "terminal",
    dialog: null,
    toasts: [],
    collapsed: {},
    filter: "",
    palette: false,
    presetFocus: null,
    agentTerminal: null,
    boxTerminal: null,
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
void api.setWindowTheme(state.ui.theme).catch(() => undefined);

let toastSeq = 0;
let unlisten: (() => void) | null = null;
let unlistenSetup: (() => void) | null = null;
let unlistenNotify: (() => void) | null = null;
/** Notifications shown since the window was last in front; the badge count. */
let unseen = 0;
let pollTimer: number | undefined;
let guardTimer: number | undefined;
let spendTimer: number | undefined;
let guardBusy = false;
const countdowns = new Map<string, number>();
/** When Undo was last pressed per box, so a check already in flight cannot
 * start a new countdown right after it. */
const keptAt = new Map<string, number>();

/** How long the countdown with Undo runs before a box is suspended. */
export const GUARD_COUNTDOWN_MS = 30_000;
const GUARD_EVERY_MS = 30_000;
const SPEND_EVERY_MS = 5 * 60_000;

export const guardPolicy = (): IdlePolicy => ({ idleMinutes: state.guard.idleOn ? state.guard.idleMinutes : 0, optedOut: [...state.guard.optedOut] });

const keepWhy: Record<string, string> = {
  "agent-working": "an agent started working",
  "setup-running": "a setup run started",
  "terminal-open": "a terminal is open",
  "opted-out": "it was excluded from idle standby",
  "not-running": "it is no longer running",
  "guard-off": "idle standby was turned off",
};

function saveGuard() {
  try {
    const g = state.guard;
    localStorage.setItem("stowbox.guards", JSON.stringify({ idleOn: g.idleOn, idleMinutes: g.idleMinutes, optedOut: g.optedOut, spendOn: g.spendOn, spendLimit: g.spendLimit }));
    localStorage.setItem("stowbox.guardLog", JSON.stringify(g.log));
  } catch {
    /* private mode: the choice just does not persist */
  }
}

function localMidnightSecs(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}

function startCountdown(e: GuardEntry) {
  if (state.guard.pending[e.machineId] || Date.now() - (keptAt.get(e.machineId) ?? 0) < GUARD_EVERY_MS * 2) return;
  const untilMs = Date.now() + GUARD_COUNTDOWN_MS;
  const m = machineById(e.machineId);
  const stops = !!m && !capsFor(m)?.power.includes("pause");
  const toastId = actions.toast(stops ? `${e.machineName} stops` : `${e.machineName} goes to standby`, "info", {
    untilMs,
    detail: `Nothing has happened in it for ${state.guard.idleMinutes} min: no agent working, no setup, no terminal open.`,
    action: { label: "Undo", run: () => void actions.keepRunning(e.machineId) },
  });
  setState("guard", "pending", e.machineId, { untilMs, toastId });
  countdowns.set(e.machineId, window.setTimeout(() => void finishCountdown(e.machineId), GUARD_COUNTDOWN_MS));
}

function dropCountdown(machineId: string) {
  window.clearTimeout(countdowns.get(machineId));
  countdowns.delete(machineId);
  const p = state.guard.pending[machineId];
  if (p) actions.dismiss(p.toastId);
  setState("guard", "pending", machineId, undefined!);
}

async function finishCountdown(machineId: string) {
  if (!state.guard.pending[machineId]) return;
  dropCountdown(machineId);
  try {
    const r = await api.guardSuspend(machineId, guardPolicy());
    if (r.suspended && r.verdict.kind === "suspend") {
      const entry: GuardAction = {
        machineId,
        stopped: r.stopped,
        machineName: r.machineName,
        atMs: Date.now(),
        idleSinceMs: r.verdict.idleSinceMs,
        hourlyBeforeMicro: r.hourlyBeforeMicro,
        hourlyAfterMicro: r.hourlyAfterMicro,
        currency: r.currency,
      };
      setState("guard", "log", (l) => [entry, ...l.filter((a) => a.machineId !== machineId && Date.now() - a.atMs < LOG_KEEP_MS)].slice(0, 50));
      saveGuard();
      await actions.refreshMachines();
    } else if (r.verdict.kind === "keep") {
      actions.toast(`Kept ${r.machineName} running: ${keepWhy[r.verdict.reason] ?? r.verdict.reason}.`, "info");
    }
  } catch (e) {
    actions.toast("Idle standby failed: " + message(e));
  }
}

function message(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

export const machineById = (id: string) => state.machines.find((m) => m.id === id);
/** The agent's box is gone from its provider: the agent is history and
 * nothing can run. Not decided while any provider failed to list, since
 * the box may be one of that provider's. */
export const agentBoxGone = (a: AgentMeta) => state.machinesLoaded && !state.machinesError && state.providerErrors.length === 0 && !machineById(a.machineId);

/** Providers with a key, in Stowbox's order. */
export const connectedProviders = () => (state.providers?.providers ?? []).filter((p) => p.connected);
export const capsOf = (provider: string): Capabilities | undefined => state.providers?.providers.find((p) => p.id === provider)?.capabilities;
export const capsFor = (m: Machine): Capabilities | undefined => capsOf(m.provider);
/** The short name of a provider for the UI: "boxd", "boat". */
export const providerLabel = (provider: string) => capsOf(provider)?.label ?? provider;
/** More than one provider is connected, so boxes carry their provider's name. */
export const multiProvider = () => connectedProviders().length > 1;
/** The label of a box's provider, or of the only one connected. */
export const boxProvider = (m: Machine | undefined) => (m ? providerLabel(m.provider) : connectedProviders().map((p) => p.capabilities.label).join(" or ") || "the provider");
/** The currency the spend limit is in: the first connected provider's. */
export const spendCurrency = () => {
  const first = connectedProviders()[0];
  return (first && state.rateCards[first.id]?.currency) || first?.capabilities.currency || "usd";
};
export const agentById = (id: string) => state.agents.find((a) => a.id === id);
export const agentsForMachine = (id: string) => state.agents.filter((a) => a.machineId === id);
export const selectedMachine = () => (state.ui.selection.kind === "box" ? machineById(state.ui.selection.id) : undefined);
export const selectedAgent = () => (state.ui.selection.kind === "agent" ? agentById(state.ui.selection.id) : undefined);

/** The harness is running in the box and Claude Code's hooks say it is on a turn. */
/** The agent's box as the app knows it: an agent whose box is gone, from
 * the app or elsewhere, runs nowhere, whatever its box showed last. */
const boxKnown = (a: AgentMeta) => !!machineById(a.machineId);

export const phaseOf = (a: AgentMeta) => (state.machinesLoaded && !boxKnown(a) ? "stopped" : agentPhase(state.live[a.id]));

export const agentWorking = (a: AgentMeta) => {
  const l = state.live[a.id];
  return boxKnown(a) && !!l && l.running && l.state === "working";
};

/** A box's watch reported: its agents' states replace what was known. */
function onLive(p: LivePush) {
  if (!p || !p.machineId) return;
  setState(
    produce((s) => {
      s.tmux[p.machineId] = p.tmux;
      for (const a of s.agents) if (a.machineId === p.machineId) delete s.live[a.id];
      for (const l of p.agents) s.live[l.id] = l;
    }),
  );
  for (const n of p.notices ?? []) void deliver(n);
}

/** Shows a notice the shell flagged as an outcome, if the settings and the
 * OS allow it. */
async function deliver(n: Notice) {
  const prefs = state.notify;
  if (!prefs.enabled || prefs.permission !== "granted") return;
  try {
    const focused = await api.windowFocused();
    if (focused && !prefs.whenFocused) return;
    await api.showNotification(n);
    if (!focused) await api.setBadgeCount(++unseen);
  } catch (e) {
    console.warn("notification failed", e);
  }
}

function openTarget(t: NoticeTarget) {
  void api.focusWindow().catch(() => undefined);
  if (t.agentId && agentById(t.agentId)) actions.select({ kind: "agent", id: t.agentId });
  else if (t.machineId && machineById(t.machineId)) actions.openBoxTab(t.machineId, "setup");
}

async function listenNotify() {
  const offClick = await api.onNotificationClick(openTarget);
  const offFocus = await api.onWindowFocus((focused) => {
    if (focused && unseen > 0) {
      unseen = 0;
      void api.setBadgeCount(0).catch(() => undefined);
    }
  });
  unlistenNotify = () => {
    offClick();
    offFocus();
  };
}

function saveNotifyPrefs() {
  try {
    localStorage.setItem("stowbox.notify", JSON.stringify({ enabled: state.notify.enabled, whenFocused: state.notify.whenFocused }));
  } catch {
    /* private mode: the choice just does not persist */
  }
}

/** Output past this is dropped from the front, as the shell does. */
const SETUP_OUTPUT_CAP = 256 * 1024;

function onSetupPush(p: SetupPush) {
  if (!p || !p.run) return;
  const prev = state.setupRuns[p.run.machineId];
  let output = (prev && prev.id === p.run.id ? prev.output : "") + p.append;
  if (output.length > SETUP_OUTPUT_CAP) output = output.slice(output.length - SETUP_OUTPUT_CAP);
  setState("setupRuns", p.run.machineId, { ...p.run, output });
  if (p.notice) void deliver(p.notice);
}

export const setupRunFor = (machineId: string): SetupRun | undefined => state.setupRuns[machineId];
export const setupActive = (r: SetupRun | undefined) => !!r && (r.status === "waiting" || r.status === "running");

export const actions = {
  toast(text: string, kind: Toast["kind"] = "error", extra: Pick<Toast, "detail" | "action" | "untilMs"> = {}): number {
    const id = ++toastSeq;
    setState("ui", "toasts", (t) => [...t, { id, kind, text, ...extra }]);
    if (kind === "info" && !extra.action) window.setTimeout(() => actions.dismiss(id), 4000);
    return id;
  },
  dismiss(id: number) {
    setState("ui", "toasts", (t) => t.filter((x) => x.id !== id));
  },

  async boot() {
    setState({ phase: "booting", bootError: "" });
    try {
      const provider = await api.providerStatus();
      setState("providers", provider);
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
    if (!unlisten) unlisten = await api.onAgentsLive(onLive);
    api
      .agentsLive()
      .then((pushes) => pushes.forEach((p) => onLive({ ...p, notices: [] })))
      .catch(() => undefined);
    if (!unlistenSetup) unlistenSetup = await api.onSetupEvent(onSetupPush);
    if (!unlistenNotify) await listenNotify().catch((e) => console.warn("notification listeners failed", e));
    api
      .notifyPermission()
      .then((permission) => setState("notify", "permission", permission))
      .catch(() => setState("notify", "permission", "unsupported"));
    void actions.loadPresets();
    api
      .setupRuns()
      .then((runs) => setState("setupRuns", Object.fromEntries(runs.map((r) => [r.machineId, r]))))
      .catch((e) => actions.toast("Could not read setup runs: " + message(e)));
    const agents = await api.listAgents().catch((e) => {
      actions.toast("Could not read saved agents: " + message(e));
      return [] as AgentMeta[];
    });
    setState({ agents });
    await actions.refreshAccount();
    for (const p of connectedProviders()) {
      api
        .rates(p.id)
        .then((card) => setState("rateCards", p.id, card))
        .catch(() => undefined);
    }
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
    // The guards run whether or not the window is visible: walking away is
    // when a box sits idle.
    window.clearInterval(guardTimer);
    guardTimer = window.setInterval(() => void actions.runGuard(), GUARD_EVERY_MS);
    window.clearInterval(spendTimer);
    spendTimer = window.setInterval(() => void actions.checkSpend(), SPEND_EVERY_MS);
    void actions.runGuard();
    void actions.checkSpend();
  },

  /** Starts a countdown for every box the idle guard says is due, and
   * drops countdowns for boxes that are no longer idle. */
  async runGuard() {
    if (guardBusy) return;
    if (!state.guard.idleOn) {
      for (const id of Object.keys(state.guard.pending)) dropCountdown(id);
      return;
    }
    guardBusy = true;
    try {
      const entries = await api.guardCheck(guardPolicy());
      for (const e of entries) {
        if (e.verdict.kind === "suspend") startCountdown(e);
        else if (state.guard.pending[e.machineId]) dropCountdown(e.machineId);
      }
    } catch (e) {
      console.warn("idle guard check failed", e);
    } finally {
      guardBusy = false;
    }
  },

  /** Undo on the countdown: the box stays up and its idle clock restarts. */
  async keepRunning(machineId: string) {
    keptAt.set(machineId, Date.now());
    dropCountdown(machineId);
    try {
      await api.guardTouch(machineId);
    } catch (e) {
      actions.toast("Could not restart the idle clock: " + message(e));
    }
  },

  async checkSpend() {
    const g = state.guard;
    if (!g.spendOn) {
      setState("guard", { spend: null, spendError: "" });
      return;
    }
    try {
      const check = await api.spendToday(localMidnightSecs(), Math.round(g.spendLimit * 1_000_000), spendCurrency());
      setState("guard", { spend: check, spendError: "" });
      const today = new Date().toDateString();
      let warned = "";
      try {
        warned = localStorage.getItem("stowbox.spendWarned") ?? "";
      } catch {
        /* private mode */
      }
      if (check.over && warned !== today) {
        try {
          localStorage.setItem("stowbox.spendWarned", today);
        } catch {
          /* private mode: it may warn again after a restart */
        }
        void deliver({ kind: "spend", title: `Today's spend passed ${money(check.limitMicro, check.currency)}`, body: spendLine(check), agentId: null, machineId: "" });
      }
    } catch (e) {
      setState("guard", { spend: null, spendError: message(e) });
    }
  },

  setGuard(patch: Partial<GuardPrefs>) {
    setState("guard", patch);
    saveGuard();
    if ("idleOn" in patch || "idleMinutes" in patch || "optedOut" in patch) {
      for (const id of state.guard.optedOut) if (state.guard.pending[id]) dropCountdown(id);
      void actions.runGuard();
    }
    if ("spendOn" in patch || "spendLimit" in patch) void actions.checkSpend();
  },

  setOptOut(machineId: string, out: boolean) {
    const rest = state.guard.optedOut.filter((id) => id !== machineId);
    actions.setGuard({ optedOut: out ? [...rest, machineId] : rest });
  },

  /** Connects `provider`; the others stay connected. */
  async saveKey(provider: string, key: string) {
    const account = await api.setApiKey(provider, key);
    setState("accounts", provider, account);
    setState("providers", await api.providerStatus());
    setState("rateCards", provider, undefined!);
    await actions.loadAll();
    setState("phase", "ready");
  },

  /** Forgets one provider's key. The last one going is a sign-out. */
  async disconnect(provider: string) {
    if (connectedProviders().length <= 1) return actions.signOut();
    await api.clearApiKey(provider);
    setState("providers", await api.providerStatus());
    setState("accounts", provider, undefined!);
    setState("rateCards", provider, undefined!);
    await actions.refreshMachines();
  },

  async signOut() {
    await api.clearApiKey(null);
    window.clearInterval(pollTimer);
    window.clearInterval(guardTimer);
    window.clearInterval(spendTimer);
    for (const id of Object.keys(state.guard.pending)) dropCountdown(id);
    setState("guard", { spend: null, spendError: "" });
    setState({
      phase: "needs-key",
      accounts: {},
      rateCards: {},
      machines: [],
      machinesLoaded: false,
      providerErrors: [],
      agents: [],
      live: {},
      tmux: {},
      metrics: {},
      metricsError: {},
      metricsAt: 0,
      presets: [],
      presetsLoaded: false,
      setupRuns: {},
      providers: await api.providerStatus(),
    });
    setState("ui", { selection: FLEET, dialog: null, tabs: [] });
    saveTabs([]);
  },

  async refreshMachines() {
    try {
      const { machines, errors } = await api.listMachines();
      setState({ machines, machinesLoaded: true, machinesError: "", providerErrors: errors });
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

  /** Every connected provider's account, side by side. */
  async refreshAccount() {
    await Promise.all(
      connectedProviders().map(async (p) => {
        try {
          setState("accounts", p.id, await api.whoami(p.id));
        } catch (e) {
          actions.toast(`Could not read the ${p.capabilities.label} account: ` + message(e));
        }
      }),
    );
  },

  select(sel: Selection) {
    const prev = state.ui.selection;
    if (sel.kind === "box" || sel.kind === "agent") {
      if (!state.ui.tabs.some((t) => sameSel(t, sel))) {
        setState("ui", "tabs", (tabs) => [...tabs, sel]);
        saveTabs(state.ui.tabs);
      }
    }
    // Another agent opens on its terminal; the Changes tab is per visit. Set
    // before the view shows, so it never mounts on the previous agent's tab.
    if (sel.kind === "agent" && !(prev.kind === "agent" && prev.id === sel.id)) setState("ui", "agentTab", "terminal");
    setState("ui", "selection", sel);
    if (sel.kind === "fleet" && Date.now() - state.metricsAt > 5_000) void actions.refreshMetrics();
    if (sel.kind === "box" && !(state.ui.selection.kind === "box" && state.ui.selection.id === sel.id)) setState("ui", "boxTab", "agents");
  },

  closeTab(sel: Selection) {
    // A closed tab ends the terminal session it kept running in the box.
    if (sel.kind === "agent") closeTerminal(agentTerminalKey(sel.id));
    if (sel.kind === "box") closeTerminal(boxTerminalKey(sel.id));
    const tabs = state.ui.tabs;
    const i = tabs.findIndex((t) => sameSel(t, sel));
    if (i < 0) return;
    const next = tabs.filter((_, j) => j !== i);
    setState("ui", "tabs", next);
    saveTabs(next);
    if (sameSel(state.ui.selection, sel)) {
      const neighbour = next[Math.min(i, next.length - 1)];
      setState("ui", "selection", neighbour ?? FLEET);
    }
  },

  /** Drops tabs whose box or agent no longer exists. */
  pruneTabs() {
    const ok = (t: Selection) =>
      (t.kind === "box" && state.machines.some((m) => m.id === t.id)) || (t.kind === "agent" && state.agents.some((a) => a.id === t.id));
    const next = state.ui.tabs.filter(ok);
    for (const t of state.ui.tabs) {
      if (ok(t)) continue;
      if (t.kind === "agent") closeTerminal(agentTerminalKey(t.id));
      if (t.kind === "box") closeTerminal(boxTerminalKey(t.id));
    }
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

  /** Turning notifications on asks the OS once, if it has not been asked. */
  async setNotify(patch: Partial<NotifyPrefs>) {
    setState("notify", patch);
    saveNotifyPrefs();
    if (patch.enabled && state.notify.permission === "default") await actions.allowNotifications();
  },

  async allowNotifications() {
    try {
      setState("notify", "permission", await api.requestNotifyPermission());
    } catch (e) {
      actions.toast("Could not ask for notification permission: " + message(e));
    }
  },

  setTheme(theme: Theme) {
    setState("ui", "theme", theme);
    document.documentElement.dataset.theme = theme;
    void api.setWindowTheme(theme).catch(() => undefined);
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
    setState("ui", { boxTab: tab, boxTerminal: null });
  },
  setAgentTab(tab: AgentTab) {
    setState("ui", "agentTab", tab);
  },
  /** Shows the agent's terminal and submits `input` there. */
  openAgentTerminal(agentId: string, input: string | null = null) {
    actions.select({ kind: "agent", id: agentId });
    setState("ui", { agentTab: "terminal", agentTerminal: { agentId, input, seq: (state.ui.agentTerminal?.seq ?? 0) + 1 } });
  },
  /** Opens the box's Terminal tab on `target`; a shell when null. */
  openBoxTerminal(machineId: string, target: TermTarget | null) {
    actions.openBoxTab(machineId, "terminal");
    setState("ui", "boxTerminal", target ? { machineId, target, seq: (state.ui.boxTerminal?.seq ?? 0) + 1 } : null);
  },
  toggleCollapsed(id: string) {
    setState("ui", "collapsed", id, (v) => !v);
  },
  openDialog(d: Dialog) {
    setState("ui", "palette", false);
    setState("ui", "dialog", d);
  },
  /** The palette does not open over a dialog. */
  openPalette() {
    if (state.phase === "ready" && !state.ui.dialog) setState("ui", "palette", true);
  },
  closePalette() {
    setState("ui", "palette", false);
  },
  openPreset(id: string) {
    actions.select({ kind: "presets" });
    // Through null, so picking the same preset again still reaches the view.
    setState("ui", "presetFocus", null);
    setState("ui", "presetFocus", id);
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
        // Its agents stay as history, stopped; their tabs say the box is gone.
        setState(
          produce((s) => {
            for (const a of s.agents) if (a.machineId === id) delete s.live[a.id];
            delete s.tmux[id];
          }),
        );
        void api.listAgents().then((agents) => setState({ agents })).catch(() => undefined);
      }
      // The provider answers once the transition is underway; read back now
      // and again later for states that take a moment (hibernate, reboot).
      await actions.refreshMachines();
      window.setTimeout(() => void actions.refreshMachines(), 2500);
      window.setTimeout(() => void actions.refreshMachines(), 8000);
      // A boat stop saves the disk first, which takes longer.
      window.setTimeout(() => void actions.refreshMachines(), 25_000);
    } catch (e) {
      actions.toast(`${action} failed: ${message(e)}`);
    } finally {
      setState("pending", id, false);
    }
  },

  /** Moves the box's auto-stop to `secs` from now; 0 turns it off. */
  async setAutoStop(id: string, secs: number) {
    setState("pending", id, true);
    try {
      const m = await api.setAutoStop(id, secs);
      setState("machines", (ms) => ms.map((x) => (x.id === id ? m : x)));
      actions.toast(secs === 0 ? `${m.name} no longer stops on its own.` : `${m.name} stops on its own in ${Math.round(secs / 3600)} h.`, "info");
    } catch (e) {
      actions.toast("Could not change the auto-stop: " + message(e));
    } finally {
      setState("pending", id, false);
    }
  },

  confirmDestroy(m: Machine) {
    const running = agentsForMachine(m.id).filter(agentWorking);
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

  async createBox(spec: CreateSpec, presetId?: string | null, variables?: VariableValues | null): Promise<Machine> {
    const m = await api.createMachine(spec, presetId ?? null, variables ?? null);
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

  /** Runs a preset in a box, asking for its variables first if it has any. */
  runPreset(machineId: string, presetId: string) {
    const p = state.presets.find((x) => x.id === presetId);
    if (p && presetVariables(p).length > 0) return actions.openDialog({ kind: "run-setup", machineId, presetId });
    actions.startSetup(machineId, presetId).catch((e) => actions.toast("Setup could not start: " + message(e)));
  },

  /** Throws when the run cannot start, so a dialog can show why. */
  async startSetup(machineId: string, presetId: string, variables?: VariableValues | null) {
    const run = await api.setupStart(machineId, presetId, variables ?? null);
    setState("setupRuns", machineId, run);
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

  /** Opens the new agent's terminal, which submits `firstPrompt` once the harness starts. */
  async createAgent(spec: NewAgent, firstPrompt?: string): Promise<AgentMeta> {
    const meta = await api.createAgent(spec);
    setState("agents", (as) => [meta, ...as]);
    actions.openAgentTerminal(meta.id, firstPrompt?.trim() || null);
    return meta;
  },

  /** Throws the provider's message; the terminal view shows it. */
  upload(id: string, name: string, bytes: Uint8Array): Promise<Attachment> {
    return api.agentUpload(id, name, bytes);
  },

  /** Ends the harness in the box; the next terminal resumes the conversation. */
  async stop(id: string) {
    try {
      closeTerminal(agentTerminalKey(id));
      await api.agentStop(id);
      setState("live", id, (l) => (l ? { ...l, running: false } : l));
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
    // An agent with its own worktree gets the dialog that offers to remove it.
    if (a.worktree) return actions.openDialog({ kind: "delete-agent", agentId: a.id });
    actions.openDialog({
      kind: "confirm",
      title: `Remove "${a.title || "this agent"}"?`,
      body: "Its session in the box is ended and Stowbox forgets it. The conversation and the box's files are not touched.",
      confirmLabel: "Remove agent",
      danger: true,
      run: () => actions.deleteAgent(a.id),
    });
  },

  /** Throws git's message when the worktree cannot be removed; the agent is kept then. */
  async deleteAgent(id: string, removeWorktree = false, force = false) {
    await api.deleteAgent(id, removeWorktree, force);
    setState("agents", (as) => as.filter((x) => x.id !== id));
    setState("live", id, undefined!);
    actions.closeTab({ kind: "agent", id });
  },

  /** Throws the reason when the box cannot be opened; the dialog shows it. */
  async openInEditor(machineId: string, editor: Editor, dir: string | null) {
    const r = await api.openInEditor(machineId, editor, dir);
    const name = editor === "vscode" ? "VS Code" : "Cursor";
    actions.toast([`Opening ${r.alias.replace(/^stowbox-/, "")} in ${name}.`, ...r.changes.map((c) => c + ".")].join(" "), "info");
    if (r.changes.some((c) => c.startsWith("Woke "))) void actions.refreshMachines();
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
