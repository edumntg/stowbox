import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { Api, AgentPush, TermEvent } from "./types";

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
  createMachine: (spec) => invoke("create_machine", { spec }),
  forkMachine: (source, name) => invoke("fork_machine", { source, name }),
  machinePower: (id, action) => invoke("machine_power", { id, action }),
  machineUsage: (id) => invoke("machine_usage", { id }),
  orgBilling: (org) => invoke("org_billing", { org }),
  rates: () => invoke("rates"),
  listAgents: () => invoke("list_agents"),
  createAgent: (spec) => invoke("create_agent", { spec }),
  agentEvents: (id) => invoke("agent_events", { id }),
  agentSend: (id, text) => invoke("agent_send", { id, text }),
  agentStop: (id) => invoke("agent_stop", { id }),
  deleteAgent: (id) => invoke("delete_agent", { id }),
  renameAgent: (id, title) => invoke("rename_agent", { id, title }),
  onAgentEvent: (cb) => listen<AgentPush>("agent-event", (e) => cb(e.payload)),
  terminalOpen: (machine, cols, rows, onEvent) => {
    const onData = new Channel<TermEvent>();
    onData.onmessage = onEvent;
    return invoke<string>("terminal_open", { machine, cols, rows, onData });
  },
  terminalWrite: (id, bytes) => invoke("terminal_write", { id, data: b64(bytes) }),
  terminalResize: (id, cols, rows) => invoke("terminal_resize", { id, cols, rows }),
  terminalClose: (id) => invoke("terminal_close", { id }),
  openUrl: (url) => openUrl(url),
  copyText: (text) => writeText(text),
};
