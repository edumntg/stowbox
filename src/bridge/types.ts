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

/** Mirrors stowbox_core::metrics::MachineMetrics. Sampled inside a running
 * box; the shell returns null for any other state instead of waking it. */
export interface MachineMetrics {
  sampledAtMs: number;
  cpuPercent: number;
  vcpu: number;
  load1: number;
  memTotalBytes: number;
  memUsedBytes: number;
  diskTotalBytes: number;
  diskUsedBytes: number;
  diskAvailBytes: number;
  uptimeSecs: number;
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

/** Mirrors stowbox_core::provision::Preset plus which secrets have a value.
 * Secret values never come back from the shell; they live in the keychain. */
export interface Preset {
  id: string;
  name: string;
  commands: string[];
  secretNames: string[];
  createdAt: number;
  updatedAt: number;
  secrets: { name: string; hasValue: boolean }[];
}

export interface PresetInput {
  id?: string | null;
  name: string;
  commands: string[];
  /** `value` null or empty keeps the stored value. */
  secrets: { name: string; value: string | null }[];
}

export type SetupStatus = "waiting" | "running" | "done" | "failed" | "cancelled";
export type StepState = "pending" | "running" | "ok" | "failed" | "skipped";

export interface SetupRun {
  id: string;
  machineId: string;
  machineName: string;
  presetId: string;
  presetName: string;
  status: SetupStatus;
  error: string | null;
  steps: { command: string; state: StepState; exitCode: number | null }[];
  secretNames: string[];
  secretsWritten: boolean;
  output: string;
  startedAt: number;
  finishedAt: number | null;
}

/** Mirrors stowbox_core::notify::Notice: set on a push that is a real
 * outcome (a turn finished or failed, a setup run ended), once per outcome. */
export interface Notice {
  kind: "agent-done" | "agent-failed" | "setup-done" | "setup-failed";
  title: string;
  body: string;
  agentId: string | null;
  machineId: string;
}

/** What a click on a notification should open. */
export type NoticeTarget = Pick<Notice, "agentId" | "machineId">;

/** "unsupported": this runtime cannot show notifications at all. */
export type NotifyPermission = "granted" | "denied" | "default" | "unsupported";

/** `run.output` is empty here; `append` carries the new text. */
export interface SetupPush {
  run: SetupRun;
  append: string;
  notice?: Notice | null;
}

/** Mirrors stowbox_core::git. Read inside the box over exec; paths are
 * relative to `root`, the repository's top level. */
export type RepoState = "ok" | "no-dir" | "not-repo" | "no-git";

export interface GitFileChange {
  path: string;
  origPath: string | null;
  /** M, A, D, R, C, T, U (conflict) or ? (untracked). */
  code: string;
  staged: boolean;
}

export interface GitDiffFile {
  path: string;
  oldPath: string | null;
  change: "modified" | "added" | "deleted" | "renamed";
  binary: boolean;
  additions: number;
  deletions: number;
  /** Hunks from the first @@ line on; empty for binary files and pure renames. */
  patch: string;
}

export interface GitReview {
  state: RepoState;
  cwd: string;
  root: string;
  branch: { head: string | null; oid: string | null; upstream: string | null; ahead: number; behind: number };
  remote: string | null;
  remoteUrl: string | null;
  base: string | null;
  gh: boolean;
  token: boolean;
  files: GitFileChange[];
  filesTruncated: boolean;
  diff: GitDiffFile[];
  diffTruncated: boolean;
  sampledAtMs: number;
}

export interface PrOutcome {
  url: string;
  /** "gh": created (or found) by gh in the box; "compare": finish it in the browser. */
  via: "gh" | "compare";
  existing: boolean;
}

export type Harness = "claude-code" | "codex";

export type AgentStatus =
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "thinking" }
  | { kind: "tool"; name: string }
  /** A turn runs detached in the box; its output stream is not attached yet. */
  | { kind: "reconnecting"; message: string }
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
  /** The agent's own worktree and branch; `cwd` is inside it. */
  worktree?: AgentWorktree | null;
}

export interface AgentWorktree {
  /** The worktree's top level, absolute, next to the repository in `<repo>-wt/`. */
  path: string;
  /** `stowbox/<slug>`. */
  branch: string;
  /** The repository's main worktree. */
  repo: string;
}

export interface NewAgent {
  machineId: string;
  machineName: string;
  harness: Harness;
  title?: string | null;
  cwd?: string | null;
  model?: string | null;
  skipPermissions: boolean;
  /** Make a worktree on a new branch from the repository that contains `cwd`. */
  ownBranch?: boolean;
  /** Text the branch is named after when there is no title (the first task). */
  branchHint?: string | null;
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
  notice?: Notice | null;
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
  createMachine(spec: CreateSpec, presetId?: string | null): Promise<Machine>;
  forkMachine(source: string, name: string | null): Promise<Machine>;
  machinePower(id: string, action: PowerAction): Promise<void>;
  machineUsage(id: string): Promise<Usage>;
  machineMetrics(id: string): Promise<MachineMetrics | null>;
  orgBilling(org: string): Promise<OrgBilling>;
  rates(): Promise<RateCard>;
  listAgents(): Promise<AgentMeta[]>;
  createAgent(spec: NewAgent): Promise<AgentMeta>;
  agentEvents(id: string): Promise<AgentEvent[]>;
  agentSend(id: string, text: string): Promise<void>;
  agentStop(id: string): Promise<void>;
  /** `removeWorktree` removes the agent's worktree first (its branch stays); git's refusal keeps the agent. */
  deleteAgent(id: string, removeWorktree?: boolean, force?: boolean): Promise<void>;
  renameAgent(id: string, title: string): Promise<AgentMeta>;
  onAgentEvent(cb: (push: AgentPush) => void): Promise<() => void>;
  listPresets(): Promise<Preset[]>;
  savePreset(input: PresetInput): Promise<Preset>;
  deletePreset(id: string): Promise<void>;
  setupStart(machineId: string, presetId: string): Promise<SetupRun>;
  setupCancel(machineId: string): Promise<void>;
  setupRuns(): Promise<SetupRun[]>;
  onSetupEvent(cb: (push: SetupPush) => void): Promise<() => void>;
  /** `cwd` null means the home directory. Errors carry git's message verbatim. */
  gitReview(machineId: string, cwd: string | null): Promise<GitReview>;
  gitCommit(machineId: string, cwd: string | null, message: string): Promise<string>;
  gitPush(machineId: string, cwd: string | null): Promise<string>;
  gitOpenPr(machineId: string, cwd: string | null): Promise<PrOutcome>;
  terminalOpen(machine: string, cols: number, rows: number, onEvent: (ev: TermEvent) => void): Promise<string>;
  terminalWrite(id: string, bytes: Uint8Array): Promise<void>;
  terminalResize(id: string, cols: number, rows: number): Promise<void>;
  terminalClose(id: string): Promise<void>;
  notifyPermission(): Promise<NotifyPermission>;
  requestNotifyPermission(): Promise<NotifyPermission>;
  showNotification(n: Notice): Promise<void>;
  /** Clicks on notifications, where the platform reports them. */
  onNotificationClick(cb: (target: NoticeTarget) => void): Promise<() => void>;
  windowFocused(): Promise<boolean>;
  onWindowFocus(cb: (focused: boolean) => void): Promise<() => void>;
  focusWindow(): Promise<void>;
  /** 0 clears the dock or taskbar badge. */
  setBadgeCount(n: number): Promise<void>;
  openUrl(url: string): Promise<void>;
  copyText(text: string): Promise<void>;
}
