import { createStore, produce } from "solid-js/store";
import { api, isDemo, setDemo, isTauri } from "../bridge/api";
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
  Harness,
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
  UpdateInfo,
  VariableValues,
} from "../bridge/types";
import { agentPhase, firstLine, money, spendLine } from "../ui/format";
import { agentTerminalKey, boxTerminalKey, closeTerminal } from "../ui/termSessions";

export type Selection = { kind: "none" } | { kind: "fleet" } | { kind: "presets" } | { kind: "box"; id: string } | { kind: "agent"; id: string };
/** An open box in the top tab row, with its open agents in the row under
 * it. `last` is the agent shown when the box's tab was left, if any, so
 * coming back to the box lands where it was. */
export interface TabGroup {
  machineId: string;
  agents: string[];
  last?: string;
}
export type BoxTab = "agents" | "setup" | "changes" | "ports" | "terminal" | "details";
export type AgentTab = "terminal" | "changes";
export type Theme = "dark" | "light";
export type Dialog =
  | null
  | { kind: "new-box" }
  | { kind: "run-setup"; machineId: string; presetId: string }
  | { kind: "new-agent"; machineId?: string; prompt?: string; cwd?: string; harness?: Harness }
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
  /** The running app's version, "" until read. */
  version: string;
  update: {
    /** A newer release, from the last check. */
    info: UpdateInfo | null;
    checking: boolean;
    /** Set while downloading: the share done, 0 to 1, or null when the size is unknown. */
    installing: { progress: number | null } | null;
    error: string;
    /** When the last check finished, ms; 0 before the first. */
    checkedAt: number;
  };
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
  /** Per box: operations in flight (power, auto-stop, fork). A count, so
   * one that ends does not re-enable the controls while another runs. */
  pending: Record<string, number>;
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
    tabs: TabGroup[];
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
  version: "",
  update: { info: null, checking: false, installing: null, error: "", checkedAt: 0 },
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

/** The saved groups. Earlier builds saved one flat list of boxes and
 * agents; it is regrouped by box once the agents are known. */
function loadTabs(): TabGroup[] {
  try {
    const raw = localStorage.getItem("stowbox.tabs.v2");
    if (raw) {
      const parsed = JSON.parse(raw) as TabGroup[];
      if (!Array.isArray(parsed)) return [];
      return parsed
        .filter((g) => g && typeof g.machineId === "string" && Array.isArray(g.agents))
        .map((g) => ({ machineId: g.machineId, agents: g.agents.filter((a) => typeof a === "string") }));
    }
    const old = JSON.parse(localStorage.getItem("stowbox.tabs") ?? "[]") as Selection[];
    const groups: TabGroup[] = [];
    const group = (machineId: string) => {
      let g = groups.find((x) => x.machineId === machineId);
      if (!g) groups.push((g = { machineId, agents: [] }));
      return g;
    };
    for (const t of Array.isArray(old) ? old : []) {
      if (t?.kind === "box" && typeof t.id === "string") group(t.id);
      if (t?.kind === "agent" && typeof t.id === "string") {
        const a = state.agents.find((x) => x.id === t.id);
        if (a) group(a.machineId).agents.push(a.id);
      }
    }
    return groups;
  } catch {
    return [];
  }
}

function saveTabs(tabs: TabGroup[]) {
  try {
    localStorage.setItem("stowbox.tabs.v2", JSON.stringify(tabs.map(({ machineId, agents }) => ({ machineId, agents }))));
  } catch {
    /* private mode: tabs just do not persist */
  }
}

/** Moves `from` to where `to` is, in place of a drag-and-drop. */
function moved<T>(list: T[], from: T, to: T): T[] {
  const i = list.indexOf(from);
  const j = list.indexOf(to);
  if (i < 0 || j < 0 || i === j) return list;
  const next = list.filter((_, k) => k !== i);
  next.splice(j, 0, from);
  return next;
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
let updateTimer: number | undefined;
/** How often a running app looks for a new release. */
const UPDATE_EVERY_MS = 6 * 3600_000;
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

function beginPending(id: string) {
  setState("pending", id, (n) => (n ?? 0) + 1);
}

function endPending(id: string) {
  setState("pending", id, (n) => Math.max(0, (n ?? 1) - 1));
}

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
/** The app runs on example data; see bridge/api. */
export const demo = isDemo;

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
  /** Looks for a newer GitHub release. A failed check in the background
   * stays quiet (offline, rate limited); one asked for shows why. */
  async checkUpdate(asked = false) {
    if (state.update.checking || state.update.installing) return;
    setState("update", { checking: true, error: "" });
    try {
      const info = await api.checkUpdate();
      setState("update", { info, checkedAt: Date.now() });
    } catch (e) {
      if (asked) setState("update", "error", message(e));
    } finally {
      setState("update", "checking", false);
    }
  },

  /** Downloads the update, installs it and restarts the app. */
  async installUpdate() {
    if (!state.update.info || state.update.installing) return;
    setState("update", { installing: { progress: null }, error: "" });
    try {
      await api.installUpdate((done, total) => setState("update", "installing", { progress: total ? Math.min(1, done / total) : null }));
    } catch (e) {
      setState("update", "error", message(e));
      actions.toast("Could not install the update: " + message(e));
    } finally {
      setState("update", "installing", null);
    }
  },

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
    void api.appVersion().then((v) => setState("version", v), () => undefined);
    // `pnpm tauri dev` would offer the latest release over itself; it checks
    // only when asked. The browser preview checks the mock.
    if (!(isTauri && import.meta.env.DEV) && !updateTimer) {
      void actions.checkUpdate();
      updateTimer = window.setInterval(() => void actions.checkUpdate(), UPDATE_EVERY_MS);
    }
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
    // A box opens a tab in the top row; an agent opens its box's tab there
    // and its own in the row under it.
    const machineId = sel.kind === "box" ? sel.id : sel.kind === "agent" ? agentById(sel.id)?.machineId : undefined;
    if (machineId) {
      const agent = sel.kind === "agent" ? sel.id : undefined;
      const i = state.ui.tabs.findIndex((g) => g.machineId === machineId);
      if (i < 0) setState("ui", "tabs", (tabs) => [...tabs, { machineId, agents: agent ? [agent] : [], last: agent }]);
      else {
        if (agent && !state.ui.tabs[i].agents.includes(agent)) setState("ui", "tabs", i, "agents", (as) => [...as, agent]);
        setState("ui", "tabs", i, "last", agent);
      }
      saveTabs(state.ui.tabs);
    }
    // Another agent opens on its terminal; the Changes tab is per visit. Set
    // before the view shows, so it never mounts on the previous agent's tab.
    if (sel.kind === "agent" && !(prev.kind === "agent" && prev.id === sel.id)) setState("ui", "agentTab", "terminal");
    setState("ui", "selection", sel);
    if (sel.kind === "fleet" && Date.now() - state.metricsAt > 5_000) void actions.refreshMetrics();
    if (sel.kind === "box" && !(state.ui.selection.kind === "box" && state.ui.selection.id === sel.id)) setState("ui", "boxTab", "agents");
  },

  /** Goes back to a box's tab: the agent last shown there, or the box. */
  selectGroup(machineId: string): void {
    const g = state.ui.tabs.find((x) => x.machineId === machineId);
    const last = g?.last && g.agents.includes(g.last) ? g.last : undefined;
    if (last) return actions.select({ kind: "agent", id: last });
    if (machineById(machineId)) return actions.select({ kind: "box", id: machineId });
    if (g?.agents[0]) return actions.select({ kind: "agent", id: g.agents[0] });
  },

  /** Closing a box's tab closes its agents' tabs too; closing an agent's
   * keeps the box open. A closed tab ends the terminal session it kept
   * running in the box. */
  closeTab(sel: Selection): void {
    const tabs = state.ui.tabs;
    if (sel.kind === "agent") {
      closeTerminal(agentTerminalKey(sel.id));
      const i = tabs.findIndex((g) => g.agents.includes(sel.id));
      if (i < 0) return;
      const g = tabs[i];
      const at = g.agents.indexOf(sel.id);
      const agents = g.agents.filter((a) => a !== sel.id);
      // A box that is gone has nothing left to show once its last agent closes.
      if (agents.length === 0 && !machineById(g.machineId)) return actions.closeTab({ kind: "box", id: g.machineId });
      setState("ui", "tabs", i, { agents, last: g.last === sel.id ? undefined : g.last });
      saveTabs(state.ui.tabs);
      if (sameSel(state.ui.selection, sel)) {
        const neighbour = agents[Math.min(at, agents.length - 1)];
        actions.select(neighbour ? { kind: "agent", id: neighbour } : { kind: "box", id: g.machineId });
      }
      return;
    }
    if (sel.kind !== "box") return;
    const i = tabs.findIndex((g) => g.machineId === sel.id);
    if (i < 0) return;
    const g = tabs[i];
    closeTerminal(boxTerminalKey(g.machineId));
    for (const a of g.agents) closeTerminal(agentTerminalKey(a));
    const next = tabs.filter((_, j) => j !== i);
    setState("ui", "tabs", next);
    saveTabs(next);
    const cur = state.ui.selection;
    const inGroup = (cur.kind === "box" && cur.id === g.machineId) || (cur.kind === "agent" && g.agents.includes(cur.id));
    if (inGroup) {
      const neighbour = next[Math.min(i, next.length - 1)];
      if (neighbour) actions.selectGroup(neighbour.machineId);
      else setState("ui", "selection", FLEET);
    }
  },

  /** Drag-and-drop in the top row: box `from` takes box `to`'s place. */
  moveBoxTab(from: string, to: string) {
    const ids = moved(
      state.ui.tabs.map((g) => g.machineId),
      from,
      to,
    );
    setState("ui", "tabs", (tabs) => ids.map((id) => tabs.find((g) => g.machineId === id)!));
    saveTabs(state.ui.tabs);
  },

  /** Drag-and-drop in a box's agent row. */
  moveAgentTab(machineId: string, from: string, to: string) {
    const i = state.ui.tabs.findIndex((g) => g.machineId === machineId);
    if (i < 0) return;
    setState("ui", "tabs", i, "agents", (as) => moved(as, from, to));
    saveTabs(state.ui.tabs);
  },

  /** Drops tabs whose agent no longer exists, and boxes that are gone with
   * no agent tab left to show. Agents of a deleted box stay, as history. */
  pruneTabs() {
    const prev = state.ui.tabs;
    const next = prev
      .map((g) => ({ ...g, agents: g.agents.filter((id) => state.agents.some((a) => a.id === id)) }))
      .filter((g) => g.agents.length > 0 || state.machines.some((m) => m.id === g.machineId));
    for (const g of prev) {
      const kept = next.find((x) => x.machineId === g.machineId);
      if (!kept) closeTerminal(boxTerminalKey(g.machineId));
      for (const a of g.agents) if (!kept?.agents.includes(a)) closeTerminal(agentTerminalKey(a));
    }
    if (JSON.stringify(next) !== JSON.stringify(prev)) {
      setState("ui", "tabs", next);
      saveTabs(next);
    }
    const sel = state.ui.selection;
    const ok =
      (sel.kind === "box" && state.machines.some((m) => m.id === sel.id)) || (sel.kind === "agent" && next.some((g) => g.agents.includes(sel.id)));
    if ((sel.kind === "box" || sel.kind === "agent") && !ok) {
      const g = next.find((x) => sel.kind === "box" && x.machineId === sel.id) ?? next[0];
      if (g) actions.selectGroup(g.machineId);
      else setState("ui", "selection", FLEET);
    }
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
    beginPending(id);
    try {
      await api.machinePower(id, action);
      if (action === "destroy") {
        setState("machines", (ms) => ms.filter((m) => m.id !== id));
        setState("setupRuns", id, undefined!);
        // Its agent tabs stay; the box's own tab closes with its last one.
        actions.pruneTabs();
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
      endPending(id);
    }
  },

  /** Moves the box's auto-stop to `secs` from now; 0 turns it off. */
  async setAutoStop(id: string, secs: number) {
    beginPending(id);
    try {
      const m = await api.setAutoStop(id, secs);
      setState("machines", (ms) => ms.map((x) => (x.id === id ? m : x)));
      const span = secs % 3600 === 0 ? `${secs / 3600} h` : `${Math.round(secs / 60)} min`;
      const idle = capsFor(m)?.idleStop;
      actions.toast(secs === 0 ? `${m.name} no longer stops on its own.` : idle ? `${m.name} stops after ${span} with no activity.` : `${m.name} stops on its own in ${span}.`, "info");
    } catch (e) {
      actions.toast("Could not change the auto-stop: " + message(e));
    } finally {
      endPending(id);
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
    beginPending(source.id);
    try {
      const m = await api.forkMachine(source.id, null);
      setState("machines", (ms) => [...ms, m].sort((a, b) => a.name.localeCompare(b.name)));
      actions.select({ kind: "box", id: m.id });
      actions.toast(`Forked ${source.name} as ${m.name}.`, "info");
    } catch (e) {
      actions.toast("Fork failed: " + message(e));
    } finally {
      endPending(source.id);
    }
  },

  /** Opens the new agent's terminal, which submits `firstPrompt` once the harness starts. */
  async createAgent(spec: NewAgent, firstPrompt?: string): Promise<AgentMeta> {
    const meta = await api.createAgent(spec);
    setState("agents", (as) => [meta, ...as]);
    actions.openAgentTerminal(meta.id, firstPrompt?.trim() || null);
    return meta;
  },

  /** The composer's one step: a box (an existing one, or a new one that
   * is waited for until it runs), then an agent in it with the prompt.
   * `onStep` names what is happening, for the button. Throws the
   * provider's message. */
  async startTask(
    task: { prompt: string; harness: Harness; cwd: string | null; box: { existing: string } | { create: CreateSpec } },
    onStep: (step: string) => void,
  ): Promise<AgentMeta> {
    let m: Machine | undefined;
    if ("existing" in task.box) {
      m = machineById(task.box.existing);
      if (!m) throw new Error("That box is gone; pick another.");
    } else {
      onStep("Creating the box…");
      m = await api.createMachine(task.box.create, null, null);
      setState("machines", (ms) => [...ms.filter((x) => x.id !== m!.id), m!].sort((a, b) => a.name.localeCompare(b.name)));
      onStep(`Starting ${m.name}…`);
      // A new box can take a moment; the agent needs it running.
      const deadline = Date.now() + 180_000;
      while (m.status !== "running") {
        if (m.status === "failed" || m.status === "stopped") throw new Error(`${m.name} did not start: ${providerLabel(m.provider)} reports it ${m.status}.`);
        if (Date.now() > deadline) throw new Error(`${m.name} is still ${m.status} after three minutes. It stays in the list; start the agent from it once it runs.`);
        await new Promise((r) => window.setTimeout(r, 2000));
        m = await api.getMachine(m.id);
        setState("machines", (ms) => ms.map((x) => (x.id === m!.id ? m! : x)));
      }
    }
    onStep("Opening the agent…");
    return actions.createAgent(
      {
        machineId: m.id,
        machineName: m.name,
        harness: task.harness,
        title: firstLine(task.prompt) || null,
        cwd: task.cwd,
        model: null,
        skipPermissions: true,
        ownBranch: false,
        branchHint: task.prompt.trim() || null,
      },
      task.prompt,
    );
  },

  /** Back to the Fleet view with the cursor in the composer. */
  focusComposer() {
    actions.select({ kind: "fleet" });
    window.setTimeout(() => document.getElementById("task-prompt")?.focus(), 0);
  },

  startDemo() {
    setDemo(true);
  },
  exitDemo() {
    setDemo(false);
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
