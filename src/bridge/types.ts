// Mirrors the Rust types in crates/stowbox-core/src/types.rs and agent/mod.rs.
// Machine and AgentMeta are camelCase; transcript events keep snake_case
// because they are stored verbatim on disk.

export type MachineStatus =
  | "pending"
  | "starting"
  | "running"
  | "suspended"
  | "hibernated"
  | "stopped"
  | "failed"
  | "migrating"
  | "unknown";

export interface Machine {
  id: string;
  name: string;
  status: MachineStatus;
  vcpu: number;
  memoryBytes: number;
  diskBytes: number;
  publicIp: string;
  accessDomain: string;
  url: string;
  desktopUrl: string;
  sshPort: number;
  org: string;
  billingOrg: string;
  source: string;
  isolated: boolean;
  createdAt: number;
  hibernatedAtMs: number;
  bootTimeMs: number;
  lastConnectedMs: number;
  autoSuspendSecs: number;
  autoHibernateSecs: number;
  autoDestroySecs: number;
  estHourlyMicro: number;
  currency: string;
}

export interface Billing {
  mode: string;
  currency: string;
  balanceMicro: number;
  accruingMicro: number | null;
  maxVms: number;
  defaultVcpu: number;
  defaultMemoryBytes: number;
  ceilingVcpu: number;
  ceilingMemoryBytes: number;
  subscriptionStatus: string;
}

export interface Account {
  userId: string;
  displayName: string;
  billing: Billing;
}

export interface Rates {
  vcpuHourMicro: number;
  ramGibHourMicro: number;
  diskGibHourMicro: number;
}

export interface Usage {
  periodStart: number;
  periodEnd: number;
  currency: string;
  rates: Rates;
  vcpuHours: number;
  ramGibHours: number;
  diskGibHours: number;
  costMicro: number;
  seconds: { running: number; standby: number; hibernated: number; stopped: number };
  complete: boolean;
}

export interface RateCard {
  currency: string;
  rates: Rates;
}

export interface OrgBilling {
  id: string;
  name: string;
  isAdmin: boolean;
  currency: string;
  mode: string;
  balanceMicro: number;
  accruingMicro: number | null;
  vmCount: number;
  maxVms: number;
}

export type PowerAction = "start" | "stop" | "reboot" | "pause" | "resume" | "hibernate" | "wake" | "destroy";

export interface CreateSpec {
  name?: string | null;
  vcpu?: number | null;
  memoryBytes?: number | null;
  isolated: boolean;
  org?: string | null;
  env?: [string, string][];
}

export type Harness = "claude-code" | "codex";

export type AgentStatus =
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "thinking" }
  | { kind: "tool"; name: string }
  | { kind: "done" }
  | { kind: "failed"; message: string }
  | { kind: "stopped" };

export interface AgentMeta {
  id: string;
  title: string;
  harness: Harness;
  machineId: string;
  machineName: string;
  cwd: string | null;
  model: string | null;
  sessionId: string | null;
  status: AgentStatus;
  skipPermissions: boolean;
  createdAt: number;
  updatedAt: number;
  totalCostUsd: number;
  turns: number;
  inputTokens: number;
  outputTokens: number;
}

export interface NewAgent {
  machineId: string;
  machineName: string;
  harness: Harness;
  title?: string | null;
  cwd?: string | null;
  model?: string | null;
  skipPermissions: boolean;
}

export type Block =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown };

export type AgentEvent =
  | { type: "user_message"; text: string }
  | { type: "init"; session_id: string; model: string | null; cwd: string | null }
  | { type: "text_delta"; text: string }
  | { type: "assistant"; blocks: Block[] }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean }
  | {
      type: "result";
      ok: boolean;
      subtype: string;
      message: string | null;
      cost_usd: number | null;
      duration_ms: number | null;
      turns: number | null;
      session_id: string | null;
      input_tokens: number | null;
      output_tokens: number | null;
    }
  | { type: "stderr"; text: string }
  | { type: "exit"; code: number };

export interface AgentPush {
  agentId: string;
  event: AgentEvent | null;
  meta: AgentMeta;
}

export type TermEvent = { type: "data"; b64: string } | { type: "exit"; code: number } | { type: "error"; message: string };

export interface ProviderStatus {
  configured: boolean;
  provider: string;
  secretBackend: "keyring" | "file" | null;
  consoleUrl: string;
}

export interface Api {
  providerStatus(): Promise<ProviderStatus>;
  setApiKey(key: string): Promise<Account>;
  clearApiKey(): Promise<void>;
  whoami(): Promise<Account>;
  listMachines(): Promise<Machine[]>;
  getMachine(id: string): Promise<Machine>;
  createMachine(spec: CreateSpec): Promise<Machine>;
  forkMachine(source: string, name: string | null): Promise<Machine>;
  machinePower(id: string, action: PowerAction): Promise<void>;
  machineUsage(id: string): Promise<Usage>;
  orgBilling(org: string): Promise<OrgBilling>;
  rates(): Promise<RateCard>;
  listAgents(): Promise<AgentMeta[]>;
  createAgent(spec: NewAgent): Promise<AgentMeta>;
  agentEvents(id: string): Promise<AgentEvent[]>;
  agentSend(id: string, text: string): Promise<void>;
  agentStop(id: string): Promise<void>;
  deleteAgent(id: string): Promise<void>;
  renameAgent(id: string, title: string): Promise<AgentMeta>;
  onAgentEvent(cb: (push: AgentPush) => void): Promise<() => void>;
  terminalOpen(machine: string, cols: number, rows: number, onEvent: (ev: TermEvent) => void): Promise<string>;
  terminalWrite(id: string, bytes: Uint8Array): Promise<void>;
  terminalResize(id: string, cols: number, rows: number): Promise<void>;
  terminalClose(id: string): Promise<void>;
  openUrl(url: string): Promise<void>;
  copyText(text: string): Promise<void>;
}
