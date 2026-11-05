import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isPermissionGranted, onAction, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { openUrl } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { Api, AgentPush, NotifyPermission, SetupPush, TermEvent } from "./types";

function b64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    s += String.fromCharCode.apply(null, Array.from(bytes.subarray(i, i + 0x8000)));
  }
  return btoa(s);
}

export const tauriApi: Api = {
  providerStatus: () => invoke("provider_status"),
  setApiKey: (key) => invoke("set_api_key", { key }),
  clearApiKey: () => invoke("clear_api_key"),
  whoami: () => invoke("whoami"),
  listMachines: () => invoke("list_machines"),
  getMachine: (id) => invoke("get_machine", { id }),
  createMachine: (spec, presetId, variables) => invoke("create_machine", { spec, presetId: presetId ?? null, variables: variables ?? null }),
  forkMachine: (source, name) => invoke("fork_machine", { source, name }),
  machinePower: (id, action) => invoke("machine_power", { id, action }),
  machineUsage: (id) => invoke("machine_usage", { id }),
  machineMetrics: (id) => invoke("machine_metrics", { id }),
  orgBilling: (org) => invoke("org_billing", { org }),
  rates: () => invoke("rates"),
  guardCheck: (policy) => invoke("guard_check", { policy }),
  guardSuspend: (machineId, policy) => invoke("guard_suspend", { machineId, policy }),
  guardTouch: (machineId) => invoke("guard_touch", { machineId }),
  spendToday: (since, limitMicro) => invoke("spend_today", { since, limitMicro }),
  listAgents: () => invoke("list_agents"),
  createAgent: (spec) => invoke("create_agent", { spec }),
  agentEvents: (id) => invoke("agent_events", { id }),
  searchTranscripts: (query, limit) => invoke("search_transcripts", { query, limit: limit ?? null }),
  agentSend: (id, text, attachments) => invoke("agent_send", { id, text, attachments: attachments ?? null }),
  agentQueueAdd: (id, text, attachments) => invoke("agent_queue_add", { id, text, attachments: attachments ?? null }),
  agentQueueEdit: (id, messageId, text) => invoke("agent_queue_edit", { id, messageId, text }),
  agentQueueRemove: (id, messageId) => invoke("agent_queue_remove", { id, messageId }),
  agentQueueClear: (id) => invoke("agent_queue_clear", { id }),
  agentQueueResume: (id) => invoke("agent_queue_resume", { id }),
  agentUpload: (id, name, bytes) => invoke("agent_upload", { id, name, data: b64(bytes) }),
  agentStop: (id) => invoke("agent_stop", { id }),
  deleteAgent: (id, removeWorktree, force) => invoke("delete_agent", { id, removeWorktree: removeWorktree ?? false, force: force ?? false }),
  renameAgent: (id, title) => invoke("rename_agent", { id, title }),
  onAgentEvent: (cb) => listen<AgentPush>("agent-event", (e) => cb(e.payload)),
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
  sshPlan: (machineId) => invoke("ssh_plan", { machineId }),
  openInEditor: (machineId, editor, dir) => invoke("open_in_editor", { machineId, editor, dir }),
  terminalOpen: (machine, cols, rows, onEvent) => {
    const onData = new Channel<TermEvent>();
    onData.onmessage = onEvent;
    return invoke<string>("terminal_open", { machine, cols, rows, onData });
  },
  terminalWrite: (id, bytes) => invoke("terminal_write", { id, data: b64(bytes) }),
  terminalResize: (id, cols, rows) => invoke("terminal_resize", { id, cols, rows }),
  terminalClose: (id) => invoke("terminal_close", { id }),
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
  setBadgeCount: (n) => getCurrentWindow().setBadgeCount(n > 0 ? n : undefined),
  openUrl: (url) => openUrl(url),
  copyText: (text) => writeText(text),
};
