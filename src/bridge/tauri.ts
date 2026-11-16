import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Image } from "@tauri-apps/api/image";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isPermissionGranted, onAction, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { openUrl } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import type { Api, LivePush, NotifyPermission, SetupPush, TermEvent } from "./types";

function b64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    s += String.fromCharCode.apply(null, Array.from(bytes.subarray(i, i + 0x8000)));
  }
  return btoa(s);
}

/** WebView2 reports "Win32" here. */
const isWindows = /Win/i.test(navigator.platform || navigator.userAgent);

/** Windows has no badge count on the taskbar, only a small overlay icon on
 * the app's button: the count on an orange square (orange is "waiting for
 * you" in Stowbox), drawn at 32px for Windows to scale. */
async function overlayBadge(n: number): Promise<Image> {
  const size = 32;
  const c = new OffscreenCanvas(size, size);
  const g = c.getContext("2d")!;
  g.fillStyle = "#f97316";
  g.fillRect(0, 0, size, size);
  g.fillStyle = "#0a0a0a";
  g.textAlign = "center";
  g.textBaseline = "middle";
  const label = n > 9 ? "9+" : String(n);
  g.font = `600 ${label.length > 1 ? 19 : 24}px "Segoe UI", sans-serif`;
  g.fillText(label, size / 2, size / 2 + 1);
  return Image.new(g.getImageData(0, 0, size, size).data.buffer, size, size);
}

export const tauriApi: Api = {
  providerStatus: () => invoke("provider_status"),
  setApiKey: (provider, key) => invoke("set_api_key", { provider, key }),
  clearApiKey: (provider) => invoke("clear_api_key", { provider }),
  whoami: (provider) => invoke("whoami", { provider }),
  listMachines: () => invoke("list_machines"),
  getMachine: (id) => invoke("get_machine", { id }),
  createMachine: (spec, presetId, variables) => invoke("create_machine", { spec, presetId: presetId ?? null, variables: variables ?? null }),
  forkMachine: (source, name) => invoke("fork_machine", { source, name }),
  machinePower: (id, action) => invoke("machine_power", { id, action }),
  machineUsage: (id) => invoke("machine_usage", { id }),
  setAutoStop: (id, secs) => invoke("set_auto_stop", { id, secs }),
  machineMetrics: (id) => invoke("machine_metrics", { id }),
  orgBilling: (provider, org) => invoke("org_billing", { provider, org }),
  rates: (provider) => invoke("rates", { provider }),
  guardCheck: (policy) => invoke("guard_check", { policy }),
  guardSuspend: (machineId, policy) => invoke("guard_suspend", { machineId, policy }),
  guardTouch: (machineId) => invoke("guard_touch", { machineId }),
  spendToday: (since, limitMicro, currency) => invoke("spend_today", { since, limitMicro, currency }),
  listAgents: () => invoke("list_agents"),
  createAgent: (spec) => invoke("create_agent", { spec }),
  // Raw bytes, not base64 JSON: a large file would not fit in one string.
  agentUpload: (id, name, bytes) => invoke("agent_upload", bytes, { headers: { "x-agent": id, "x-name": encodeURIComponent(name) } }),
  agentStop: (id) => invoke("agent_stop", { id }),
  agentsLive: () => invoke("agents_live"),
  deleteAgent: (id, removeWorktree, force) => invoke("delete_agent", { id, removeWorktree: removeWorktree ?? false, force: force ?? false }),
  renameAgent: (id, title) => invoke("rename_agent", { id, title }),
  onAgentsLive: (cb) => listen<LivePush>("agents-live", (e) => cb(e.payload)),
  listPresets: () => invoke("list_presets"),
  savePreset: (input) => invoke("save_preset", { input }),
  deletePreset: (id) => invoke("delete_preset", { id }),
  setupStart: (machineId, presetId, variables) => invoke("setup_start", { machineId, presetId, variables: variables ?? null }),
  setupCancel: (machineId) => invoke("setup_cancel", { machineId }),
  setupRuns: () => invoke("setup_runs"),
  onSetupEvent: (cb) => listen<SetupPush>("setup-event", (e) => cb(e.payload)),
  gitReview: (machineId, cwd) => invoke("git_review", { machineId, cwd }),
  gitCommit: (machineId, cwd, message) => invoke("git_commit", { machineId, cwd, message }),
  gitPush: (machineId, cwd) => invoke("git_push", { machineId, cwd }),
  gitOpenPr: (machineId, cwd) => invoke("git_open_pr", { machineId, cwd }),
  httpsRoutes: (machineId) => invoke("https_routes", { machineId }),
  addHttpsRoute: (machineId, name, port) => invoke("add_https_route", { machineId, name, port }),
  removeHttpsRoute: (machineId, name) => invoke("remove_https_route", { machineId, name }),
  setDefaultRoutePort: (machineId, port) => invoke("set_default_route_port", { machineId, port }),
  portForwards: (machineId) => invoke("port_forwards", { machineId }),
  exposePort: (machineId, port, protocol) => invoke("expose_port", { machineId, port, protocol }),
  unexposePort: (machineId, port) => invoke("unexpose_port", { machineId, port }),
  listeningPorts: (machineId, wake) => invoke("listening_ports", { machineId, wake: wake ?? false }),
  sshPlan: (machineId) => invoke("ssh_plan", { machineId }),
  openInEditor: (machineId, editor, dir) => invoke("open_in_editor", { machineId, editor, dir }),
  terminalOpen: (machine, cols, rows, target, onEvent) => {
    const onData = new Channel<TermEvent>();
    onData.onmessage = onEvent;
    return invoke<string>("terminal_open", { machine, cols, rows, target, onData });
  },
  terminalWrite: (id, bytes) => invoke("terminal_write", { id, data: b64(bytes) }),
  terminalResize: (id, cols, rows) => invoke("terminal_resize", { id, cols, rows }),
  terminalClose: (id) => invoke("terminal_close", { id }),
  claudeAccount: (machine) => invoke("claude_account", { machine }),
  setClaudeAccount: (machine, own) => invoke("set_claude_account", { machine, own }),
  notifyPermission: async (): Promise<NotifyPermission> => ((await isPermissionGranted()) ? "granted" : "default"),
  requestNotifyPermission: () => requestPermission(),
  showNotification: async (n) => sendNotification({ title: n.title, body: n.body, extra: { agentId: n.agentId, machineId: n.machineId } }),
  onNotificationClick: async (cb) => {
    // Desktop reports a click on the notification itself as "tap", with the
    // payload it was sent with, where the notification server reports clicks.
    const l = await onAction((ev) => {
      const extra = ev.notification?.extra ?? {};
      cb({ agentId: typeof extra.agentId === "string" ? extra.agentId : null, machineId: typeof extra.machineId === "string" ? extra.machineId : "" });
    });
    return () => void l.unregister();
  },
  windowFocused: () => getCurrentWindow().isFocused(),
  onWindowFocus: (cb) => getCurrentWindow().onFocusChanged((e) => cb(e.payload)),
  focusWindow: async () => {
    const w = getCurrentWindow();
    await w.unminimize();
    await w.show();
    await w.setFocus();
  },
  setBadgeCount: async (n) => {
    const w = getCurrentWindow();
    if (isWindows) await w.setOverlayIcon(n > 0 ? await overlayBadge(n) : undefined);
    else await w.setBadgeCount(n > 0 ? n : undefined);
  },
  // Only Windows draws the native title bar from this: it follows the
  // system theme otherwise, a white bar over a dark window.
  setWindowTheme: async (theme) => {
    if (isWindows) await getCurrentWindow().setTheme(theme);
  },
  openUrl: (url) => openUrl(url),
  copyText: (text) => writeText(text),
  appVersion: () => getVersion(),
  checkUpdate: async () => {
    pending = await check();
    return pending ? { version: pending.version, notes: pending.body ?? "", date: pending.date ?? "" } : null;
  },
  installUpdate: async (onProgress) => {
    if (!pending) throw new Error("no update to install; check again");
    let done = 0;
    let total: number | null = null;
    await pending.downloadAndInstall((ev) => {
      if (ev.event === "Started") total = ev.data.contentLength ?? null;
      if (ev.event === "Progress") done += ev.data.chunkLength;
      onProgress(done, total);
    });
    // Windows quits into the installer inside downloadAndInstall; elsewhere
    // the new version runs from the next start.
    await relaunch();
  },
};

/** The update the last check found, kept for the install. */
let pending: Update | null = null;
