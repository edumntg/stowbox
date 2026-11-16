// Mirrors the Rust types in crates/stowbox-core/src/types.rs, agent/mod.rs
// and agent/session.rs. Everything crosses the bridge camelCase.

export type MachineStatus =
  | "pending"
  | "starting"
  | "running"
  | "suspended"
  | "hibernated"
  | "stopping"
  | "stopped"
  | "failed"
  | "migrating"
  | "unknown";

export interface Machine {
  id: string;
  /** The provider that owns the box: "boxd", "boat" or "daytona". */
  provider: string;
  /** The provider's size name for fixed-size providers ("default"), else "". */
  size: string;
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
  /** When the provider stops the box on its own, unix seconds; 0 is never or not reported. */
  autoStopAt: number;
  /** The provider stops the box after this long with no activity; 0 is off. */
  idleStopSecs: number;
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
  /** Per running hour, for providers that sell fixed sizes. */
  sizes: { size: string; hourlyMicro: number }[];
}

/** One size on a fixed menu. Mirrors core capabilities::SizeOption. */
export interface SizeOption {
  id: string;
  vcpu: number;
  memoryBytes: number;
  diskBytes: number;
  hourlyMicro: number;
  note: string;
}

/** A newer release than the running app. */
export interface UpdateInfo {
  version: string;
  /** The release's notes, as written on GitHub. */
  notes: string;
  /** When it was published, ISO 8601, or "". */
  date: string;
}

/** What a provider supports. Mirrors core capabilities::Capabilities. */
export interface Capabilities {
  label: string;
  consoleUrl: string;
  keyHint: string;
  keyPrefixes: string[];
  currency: string;
  power: PowerAction[];
  fork: boolean;
  isolated: boolean;
  autoSuspend: boolean;
  autoHibernate: boolean;
  autoStop: boolean;
  /** A stop after so long with no activity; setAutoStop takes it in seconds. */
  idleStop: boolean;
  customSize: boolean;
  sizes: SizeOption[];
  httpsLinks: boolean;
  httpsNamed: boolean;
  httpsList: boolean;
  httpsRemove: boolean;
  httpsDefaultRoute: boolean;
  portForwards: boolean;
  editorSsh: boolean;
  /** Largest file one upload may carry, in bytes. */
  maxUploadBytes: number;
  remoteUser: string;
  remoteHome: string;
  meteringNote: string;
  /** Billed usage per box can be read; otherwise only the estimate shows. */
  meteredUsage: boolean;
}

export interface ProviderInfo {
  id: string;
  connected: boolean;
  secretBackend: "keyring" | "file" | null;
  capabilities: Capabilities;
}

/** Every listing call answers with the boxes of the providers that answered. */
export interface MachineList {
  machines: Machine[];
  errors: { provider: string; message: string }[];
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
  /** Which connected provider makes the box. */
  provider?: string | null;
  name?: string | null;
  /** A size from the provider's menu, for providers without free sizes. */
  size?: string | null;
  /** Auto-stop after this many seconds (running for autoStop, idle for idleStop); 0 is never. */
  autoStopSecs?: number | null;
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
  /** `{{name}}` placeholders in the commands; an empty default means a
   * value is required when the preset runs. */
  variables: PresetVariable[];
  createdAt: number;
  updatedAt: number;
  secrets: { name: string; hasValue: boolean }[];
}

export interface PresetVariable {
  name: string;
  default: string;
}

/** Values for a preset's variables by name; empty means "use the default". */
export type VariableValues = Record<string, string>;

export interface PresetInput {
  id?: string | null;
  name: string;
  commands: string[];
  /** `value` null or empty keeps the stored value. */
  secrets: { name: string; value: string | null }[];
  /** Defaults by name. The names themselves come from the commands. */
  variables: PresetVariable[];
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
  /** The values the variables got in this run. Missing on older runs. */
  variables?: { name: string; value: string }[];
  secretsWritten: boolean;
  output: string;
  startedAt: number;
  finishedAt: number | null;
}

/** Mirrors stowbox_core::notify::Notice: set on a push that is a real
 * outcome (an agent stopped working, a setup run ended), once per outcome. */
export interface Notice {
  /** "spend" is raised by the UI itself, from the daily spend check. */
  kind: "agent-done" | "setup-done" | "setup-failed" | "spend";
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

export interface AgentMeta {
  id: string;
  title: string;
  harness: Harness;
  machineId: string;
  machineName: string;
  cwd: string | null;
  model: string | null;
  /** The harness's own session: set up front for Claude Code, so every
   * terminal resumes the same conversation. */
  sessionId: string | null;
  skipPermissions: boolean;
  createdAt: number;
  /** The last time a terminal was opened on it. */
  updatedAt: number;
  /** The agent's own worktree and branch; `cwd` is inside it. */
  worktree?: AgentWorktree | null;
}

/** Mirrors stowbox_core::agent::session::AgentState, from Claude Code's
 * hooks: "working" from a prompt until the turn stops, then "waiting". */
export type AgentState = "working" | "waiting";

/** One agent as its box shows it. Codex has no hooks, so no `state`. */
export interface AgentLive {
  id: string;
  state: AgentState | null;
  /** Unix seconds the state was written. */
  since: number;
  /** Its tmux session is alive in the box. */
  running: boolean;
}

/** What a box's watch saw last. */
export interface LivePush {
  machineId: string;
  /** The box has tmux, so agents keep running with no terminal open. */
  tmux: boolean;
  agents: AgentLive[];
  /** Agents that just went from working to waiting. */
  notices: Notice[];
}

/** A file already copied into the box (stowbox_core::upload::Attachment). */
export interface Attachment {
  /** The name the person's file had. */
  name: string;
  /** Absolute path in the box, under ~/.stowbox/uploads/<agent>/. */
  path: string;
  size: number;
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
  /** Text the branch is named after when there is no title (the first prompt). */
  branchHint?: string | null;
}

export type TermEvent = { type: "data"; b64: string } | { type: "exit"; code: number } | { type: "error"; message: string };

/** What a terminal runs: a login shell, the agent's harness in its own
 * interactive UI on the agent's session (`input` is submitted first), or
 * Claude Code's sign-in. */
export type TermTarget = { kind: "shell" } | { kind: "agent"; id: string; input?: string | null } | { kind: "claude-login" };

/** The Claude Code login a box uses. `own` means the box keeps its own login
 * apart from the one boxd gives every box in the org. */
export interface ClaudeAccount {
  own: boolean;
  /** A token variable's name, or "stored login"; null when none was found. */
  source: string | null;
  email: string | null;
  organization: string | null;
}

export type Editor = "vscode" | "cursor";

/** What opening a box in an editor would do on this computer; read only. */
export interface SshPlan {
  /** `stowbox-<box>`, the Host written to ~/.ssh/config. */
  alias: string;
  /** `ssh <alias>` once the config is current, else the direct command. */
  sshCommand: string;
  /** `ssh -p <port> boxd@<host>`, which works with any key linked to the account. */
  directCommand: string;
  keyPath: string;
  keyExists: boolean;
  configPath: string;
  backupPath: string;
  config: "add" | "update" | "current";
  /** Why the box cannot be opened over SSH now, e.g. no SSH port yet. */
  blocker: string | null;
}

export interface EditorOpened {
  url: string;
  alias: string;
  /** What changed on this computer or in the account, in plain words. */
  changes: string[];
}

export interface ProviderStatus {
  /** At least one provider is connected. */
  configured: boolean;
  providers: ProviderInfo[];
}

/** Mirrors stowbox_core::guard::IdlePolicy. `idleMinutes` 0 is off. */
export interface IdlePolicy {
  idleMinutes: number;
  optedOut: string[];
}

export type KeepReason = "guard-off" | "not-running" | "opted-out" | "agent-working" | "setup-running" | "terminal-open";

export type GuardVerdict = { kind: "suspend"; idleSinceMs: number } | { kind: "wait"; dueAtMs: number } | { kind: "keep"; reason: KeepReason };

export interface GuardEntry {
  machineId: string;
  machineName: string;
  verdict: GuardVerdict;
}

/** The guard's second look at the end of the countdown. */
export interface GuardOutcome {
  verdict: GuardVerdict;
  suspended: boolean;
  /** The provider has no standby, so the box was stopped instead. */
  stopped: boolean;
  machineName: string;
  hourlyBeforeMicro: number;
  hourlyAfterMicro: number;
  currency: string;
}

/** Mirrors stowbox_core::guard::SpendCheck. "metered" is boxd's meter since
 * local midnight; "projected" is the current hourly estimate over a full
 * day, used when the key cannot read the meter (`why` says why). */
export type SpendCheck = (
  | { basis: "metered"; totalMicro: number; complete: boolean }
  | { basis: "projected"; hourlyMicro: number; why: string }
) & { amountMicro: number; limitMicro: number; currency: string; over: boolean; excluded: string[] };

/** One HTTPS route into a box (a boxd proxy). Mirrors core ports::HttpsRoute. */
export interface HttpsRoute {
  name: string;
  /** The host boxd returned, verbatim. */
  domain: string;
  /** `https://<domain>`, or null when boxd returned no domain. */
  url: string | null;
  /** Where traffic lands: the pinned port, else the detected one. 0 = boxd did not say. */
  port: number;
  /** "locked", "auto", or "" when boxd did not say. */
  portMode: string;
  isDefault: boolean;
}

/** One raw TCP/UDP forward on the box's public address. */
export interface PortForward {
  dns: string;
  publicPort: number;
  vmPort: number;
  protocol: "tcp" | "udp" | "both" | string;
  /** `<dns>:<publicPort>`, or null when boxd returned no host or port. */
  address: string | null;
}

export interface ListeningPort {
  port: number;
  addresses: string[];
  process: string | null;
  /** Bound to loopback only: boxd's proxy cannot reach it. */
  localOnly: boolean;
}

export interface Listening {
  ports: ListeningPort[];
  source: "ss" | "proc" | string;
  sampledAtMs: number;
}

export interface Api {
  providerStatus(): Promise<ProviderStatus>;
  /** Checks the key with the provider and keeps it; other providers stay connected. */
  setApiKey(provider: string, key: string): Promise<Account>;
  /** Forgets one provider's key, or every key when `provider` is null. */
  clearApiKey(provider: string | null): Promise<void>;
  whoami(provider: string): Promise<Account>;
  listMachines(): Promise<MachineList>;
  getMachine(id: string): Promise<Machine>;
  createMachine(spec: CreateSpec, presetId?: string | null, variables?: VariableValues | null): Promise<Machine>;
  forkMachine(source: string, name: string | null): Promise<Machine>;
  machinePower(id: string, action: PowerAction): Promise<void>;
  machineUsage(id: string): Promise<Usage>;
  /** Stops the box `secs` from now; 0 turns auto-stop off. */
  setAutoStop(id: string, secs: number): Promise<Machine>;
  machineMetrics(id: string): Promise<MachineMetrics | null>;
  orgBilling(provider: string, org: string): Promise<OrgBilling>;
  rates(provider: string): Promise<RateCard>;
  /** One verdict per box from the idle guard. Nothing is suspended here. */
  guardCheck(policy: IdlePolicy): Promise<GuardEntry[]>;
  /** Decides again from fresh state and suspends only if still idle. */
  guardSuspend(machineId: string, policy: IdlePolicy): Promise<GuardOutcome>;
  /** Restarts the box's idle clock (Undo on the countdown). */
  guardTouch(machineId: string): Promise<void>;
  /** `since` is local midnight, unix seconds; `currency` is the limit's. */
  spendToday(since: number, limitMicro: number, currency: string | null): Promise<SpendCheck>;
  listAgents(): Promise<AgentMeta[]>;
  createAgent(spec: NewAgent): Promise<AgentMeta>;
  /** Copies a file into the box; the terminal gets its path typed. */
  agentUpload(id: string, name: string, bytes: Uint8Array): Promise<Attachment>;
  /** Ends the agent's tmux session; the conversation stays and resumes on the next terminal. */
  agentStop(id: string): Promise<void>;
  /** What every watched box showed last. */
  agentsLive(): Promise<LivePush[]>;
  /** `removeWorktree` removes the agent's worktree first (its branch stays); git's refusal keeps the agent. */
  deleteAgent(id: string, removeWorktree?: boolean, force?: boolean): Promise<void>;
  renameAgent(id: string, title: string): Promise<AgentMeta>;
  onAgentsLive(cb: (push: LivePush) => void): Promise<() => void>;
  listPresets(): Promise<Preset[]>;
  savePreset(input: PresetInput): Promise<Preset>;
  deletePreset(id: string): Promise<void>;
  setupStart(machineId: string, presetId: string, variables?: VariableValues | null): Promise<SetupRun>;
  setupCancel(machineId: string): Promise<void>;
  setupRuns(): Promise<SetupRun[]>;
  onSetupEvent(cb: (push: SetupPush) => void): Promise<() => void>;
  /** `cwd` null means the home directory. Errors carry git's message verbatim. */
  gitReview(machineId: string, cwd: string | null): Promise<GitReview>;
  gitCommit(machineId: string, cwd: string | null, message: string): Promise<string>;
  gitPush(machineId: string, cwd: string | null): Promise<string>;
  gitOpenPr(machineId: string, cwd: string | null): Promise<PrOutcome>;
  httpsRoutes(machineId: string): Promise<HttpsRoute[]>;
  addHttpsRoute(machineId: string, name: string, port: number): Promise<HttpsRoute>;
  removeHttpsRoute(machineId: string, name: string): Promise<void>;
  /** `port` null goes back to detecting the port. */
  setDefaultRoutePort(machineId: string, port: number | null): Promise<void>;
  portForwards(machineId: string): Promise<PortForward[]>;
  exposePort(machineId: string, port: number, protocol: string): Promise<PortForward>;
  unexposePort(machineId: string, port: number): Promise<void>;
  /** Reads what listens in the box over exec. null when the box is not running and `wake` is false. */
  listeningPorts(machineId: string, wake?: boolean): Promise<Listening | null>;
  sshPlan(machineId: string): Promise<SshPlan>;
  /** Sets up SSH for the box (key, link, ~/.ssh/config block), wakes it, and opens `dir` (null = home) in the editor. */
  openInEditor(machineId: string, editor: Editor, dir: string | null): Promise<EditorOpened>;
  terminalOpen(machine: string, cols: number, rows: number, target: TermTarget, onEvent: (ev: TermEvent) => void): Promise<string>;
  terminalWrite(id: string, bytes: Uint8Array): Promise<void>;
  terminalResize(id: string, cols: number, rows: number): Promise<void>;
  terminalClose(id: string): Promise<void>;
  /** Reads which Claude Code login the box uses; runs in the box, so it wakes it. */
  claudeAccount(machine: string): Promise<ClaudeAccount>;
  /** `own` gives the box its own login (empty until `/login`); false goes back to the org's and deletes the box's own. */
  setClaudeAccount(machine: string, own: boolean): Promise<ClaudeAccount>;
  notifyPermission(): Promise<NotifyPermission>;
  requestNotifyPermission(): Promise<NotifyPermission>;
  showNotification(n: Notice): Promise<void>;
  /** Clicks on notifications, where the platform reports them. */
  onNotificationClick(cb: (target: NoticeTarget) => void): Promise<() => void>;
  windowFocused(): Promise<boolean>;
  onWindowFocus(cb: (focused: boolean) => void): Promise<() => void>;
  focusWindow(): Promise<void>;
  /** 0 clears the dock or taskbar badge (an overlay icon on Windows). */
  setBadgeCount(n: number): Promise<void>;
  /** The theme of the window's native frame, where the OS draws one. */
  setWindowTheme(theme: "light" | "dark"): Promise<void>;
  openUrl(url: string): Promise<void>;
  copyText(text: string): Promise<void>;
  /** The running app's version, from tauri.conf.json. */
  appVersion(): Promise<string>;
  /** The newest published GitHub release, when it is newer than this app. */
  checkUpdate(): Promise<UpdateInfo | null>;
  /** Downloads and installs the update `checkUpdate` found, then restarts
   * the app. `total` is null when the server does not say. */
  installUpdate(onProgress: (done: number, total: number | null) => void): Promise<void>;
}
