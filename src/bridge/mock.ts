// Browser-only stand-in for the desktop shell. Used to develop and screenshot
// the UI with `pnpm dev` outside Tauri. Every value here is a placeholder.

import { uploadCapText } from "../ui/format";
import type {
  Account,
  AgentLive,
  AgentMeta,
  ClaudeAccount,
  Api,
  Capabilities,
  GitDiffFile,
  GitFileChange,
  GitReview,
  GuardVerdict,
  HttpsRoute,
  IdlePolicy,
  ListeningPort,
  LivePush,
  Machine,
  MachineMetrics,
  Notice,
  NoticeTarget,
  PortForward,
  Preset,
  SetupPush,
  SshPlan,
  SetupRun,
  TermEvent,
  VariableValues,
} from "./types";
import { substitute, variablesIn } from "./variables";

const GIB = 1024 ** 3;
let nextSshPort = 21843;

// Mirrors core boxd::capabilities, boat::capabilities and daytona::capabilities.
const BOXD_CAPS: Capabilities = {
  label: "boxd",
  consoleUrl: "https://app.boxd.sh",
  keyHint: "Make one at app.boxd.sh or with `boxd auth keys create stowbox`. Keys are fenced to one organization.",
  keyPrefixes: ["bxd_"],
  currency: "eur",
  power: ["start", "stop", "reboot", "pause", "resume", "hibernate", "wake", "destroy"],
  fork: true,
  isolated: true,
  autoSuspend: true,
  autoHibernate: true,
  autoStop: false,
  idleStop: false,
  customSize: true,
  sizes: [],
  httpsLinks: true,
  httpsNamed: true,
  httpsList: true,
  httpsRemove: true,
  httpsDefaultRoute: true,
  portForwards: true,
  editorSsh: true,
  maxUploadBytes: 20 * 1024 * 1024,
  remoteUser: "boxd",
  remoteHome: "/home/boxd",
  meteringNote: "boxd meters with a lag of up to about 25 minutes, so the last half hour may be missing here.",
  meteredUsage: true,
};
const BOAT_SIZES = [
  { id: "small", vcpu: 2, memoryBytes: 4 * GIB, diskBytes: 51e9, hourlyMicro: 18_000, note: "" },
  { id: "default", vcpu: 4, memoryBytes: 8 * GIB, diskBytes: 51e9, hourlyMicro: 36_000, note: "" },
  { id: "large", vcpu: 8, memoryBytes: 16 * GIB, diskBytes: 111e9, hourlyMicro: 72_000, note: "not on the free trial" },
  { id: "xlarge", vcpu: 16, memoryBytes: 32 * GIB, diskBytes: 251e9, hourlyMicro: 200_000, note: "needs the $100 plan or higher" },
];
const BOAT_CAPS: Capabilities = {
  label: "boat",
  consoleUrl: "https://boat.dev/dashboard",
  keyHint: "Make one with `boat api-key create stowbox` or on the boat dashboard. It needs the ssh action: Stowbox runs every command over SSH.",
  keyPrefixes: ["boat_", "box_"],
  currency: "usd",
  power: ["start", "stop", "destroy"],
  fork: true,
  isolated: false,
  autoSuspend: false,
  autoHibernate: false,
  autoStop: true,
  idleStop: false,
  customSize: false,
  sizes: BOAT_SIZES,
  httpsLinks: true,
  httpsNamed: false,
  httpsList: false,
  httpsRemove: false,
  httpsDefaultRoute: false,
  portForwards: false,
  editorSsh: false,
  maxUploadBytes: 512 * 1024 * 1024,
  remoteUser: "user",
  remoteHome: "/home/user",
  meteringNote: "boat counts by the second at list price; time included in the plan is not taken off here.",
  meteredUsage: true,
};
const DAYTONA_CAPS: Capabilities = {
  label: "daytona",
  consoleUrl: "https://app.daytona.io/dashboard",
  keyHint: "Make one on the Daytona dashboard under Keys, with permission to write and delete sandboxes. Keys are fenced to one organization.",
  keyPrefixes: ["dtn_"],
  currency: "usd",
  power: ["start", "stop", "destroy"],
  fork: false,
  isolated: false,
  autoSuspend: false,
  autoHibernate: false,
  autoStop: false,
  idleStop: true,
  customSize: true,
  sizes: [],
  httpsLinks: true,
  httpsNamed: false,
  httpsList: false,
  httpsRemove: false,
  httpsDefaultRoute: false,
  portForwards: false,
  editorSsh: false,
  maxUploadBytes: 20 * 1024 * 1024,
  remoteUser: "daytona",
  remoteHome: "/home/daytona",
  meteringNote: "Daytona's API does not report what a sandbox has cost; the dashboard does. This is the list-price estimate.",
  meteredUsage: false,
};
const CAPS: Record<string, Capabilities> = { boxd: BOXD_CAPS, boat: BOAT_CAPS, daytona: DAYTONA_CAPS };

// `?providers=boxd` or `?providers=boat,daytona` previews a subset of connected providers.
const connected = new Set((new URLSearchParams(window.location.search).get("providers") ?? "boxd,boat,daytona").split(",").filter((p) => p in CAPS));

/** Daytona's list prices per hour, as in core daytona::RATES. */
const DAYTONA_RATES = { vcpuHourMicro: 50_400, ramGibHourMicro: 16_200, diskGibHourMicro: 108 };

function daytonaMachine(partial: Partial<Machine> & Pick<Machine, "id" | "name" | "status">): Machine {
  const vcpu = partial.vcpu ?? 1;
  const memoryBytes = partial.memoryBytes ?? GIB;
  const diskBytes = partial.diskBytes ?? 3 * GIB;
  const live = partial.status === "running" || partial.status === "starting";
  const est = (live ? vcpu * DAYTONA_RATES.vcpuHourMicro + (memoryBytes / GIB) * DAYTONA_RATES.ramGibHourMicro : 0) + (diskBytes / GIB) * DAYTONA_RATES.diskGibHourMicro;
  return {
    publicIp: "",
    accessDomain: "",
    url: "",
    desktopUrl: "",
    sshPort: 0,
    org: "",
    billingOrg: "org_mock_daytona",
    source: "daytonaio/sandbox",
    isolated: false,
    createdAt: Math.floor(Date.now() / 1000) - 7_200,
    hibernatedAtMs: 0,
    bootTimeMs: 0,
    lastConnectedMs: Date.now() - 4 * 60_000,
    autoSuspendSecs: 0,
    autoHibernateSecs: 0,
    autoDestroySecs: 0,
    idleStopSecs: 3_600,
    ...partial,
    vcpu,
    memoryBytes,
    diskBytes,
    currency: "usd",
    provider: "daytona",
    size: "",
    autoStopAt: 0,
    estHourlyMicro: Math.round(est),
  };
}

function boatMachine(partial: Partial<Machine> & Pick<Machine, "id" | "name" | "status">): Machine {
  const size = BOAT_SIZES.find((z) => z.id === (partial.size || "default")) ?? BOAT_SIZES[1];
  const live = partial.status === "running" || partial.status === "starting";
  return {
    vcpu: size.vcpu,
    memoryBytes: size.memoryBytes,
    diskBytes: size.diskBytes,
    publicIp: "0.0.0.0",
    accessDomain: "on.boat.dev",
    url: "",
    desktopUrl: "",
    sshPort: 0,
    org: "",
    billingOrg: "",
    source: "",
    isolated: false,
    createdAt: Math.floor(Date.now() / 1000) - 86_400,
    hibernatedAtMs: 0,
    bootTimeMs: 0,
    lastConnectedMs: 0,
    autoSuspendSecs: 0,
    autoHibernateSecs: 0,
    autoDestroySecs: 0,
    idleStopSecs: 0,
    currency: "usd",
    ...partial,
    provider: "boat",
    size: size.id,
    autoStopAt: live ? partial.autoStopAt || Math.floor(Date.now() / 1000) + 4 * 3600 : 0,
    estHourlyMicro: live ? size.hourlyMicro : 0,
  };
}

function machine(partial: Partial<Machine> & Pick<Machine, "id" | "name" | "status">): Machine {
  if (partial.provider === "boat") return boatMachine(partial);
  if (partial.provider === "daytona") return daytonaMachine(partial);
  const vcpu = partial.vcpu ?? 2;
  const memoryBytes = partial.memoryBytes ?? 8 * GIB;
  const running = partial.status === "running";
  const standby = partial.status === "suspended";
  const est =
    (running ? vcpu * 39_000 : 0) + (running || standby ? (memoryBytes / GIB) * 12_000 : 0) + 100 * 80;
  return {
    vcpu,
    memoryBytes,
    diskBytes: 100 * GIB,
    publicIp: "0.0.0.0",
    accessDomain: "boxd.sh",
    url: `https://${partial.name}.boxd.sh`,
    desktopUrl: `https://app.boxd.sh/desktop/${partial.name}`,
    sshPort: partial.sshPort ?? nextSshPort++,
    org: "",
    billingOrg: "",
    source: "standalone",
    isolated: false,
    createdAt: Math.floor(Date.now() / 1000) - 86_400 * 3,
    hibernatedAtMs: 0,
    bootTimeMs: 0,
    lastConnectedMs: Date.now() - 3_600_000,
    autoSuspendSecs: 0,
    autoHibernateSecs: 14_400,
    autoDestroySecs: 0,
    currency: "eur",
    ...partial,
    provider: "boxd",
    size: "",
    autoStopAt: 0,
    idleStopSecs: 0,
    // Always from the state passed in, so a power change re-prices the box.
    estHourlyMicro: Math.round(est),
  };
}

let machines: Machine[] = [
  machine({ id: "vm_mock_1", name: "mock-api", status: "running" }),
  machine({ id: "vm_mock_2", name: "mock-web", status: "suspended", vcpu: 4, memoryBytes: 16 * GIB }),
  machine({ id: "vm_mock_3", name: "mock-scratch", status: "hibernated", vcpu: 1, memoryBytes: 4 * GIB }),
  machine({ id: "vm_mock_4", name: "mock-old", status: "stopped" }),
  machine({ id: "vm_mock_5", name: "mock-worker", status: "running", vcpu: 4, memoryBytes: 16 * GIB, lastConnectedMs: Date.now() - 3 * 3_600_000 }),
  machine({ id: "sbx_mock_6", name: "boat-ci", status: "running", provider: "boat", size: "large", autoStopAt: Math.floor(Date.now() / 1000) + 12 * 60 }),
  machine({ id: "sbx_mock_7", name: "boat-notes", status: "stopped", provider: "boat", size: "small" }),
  machine({ id: "dt-mock-8", name: "daytona-eval", status: "running", provider: "daytona", vcpu: 2, memoryBytes: 4 * GIB, diskBytes: 10 * GIB }),
].filter((m) => connected.has(m.provider));

// Metrics drift a little per sample so the fleet view shows movement, and
// the worker box sits near its memory ceiling so the pressure colours show.
const metricBase: Record<string, { cpu: number; mem: number; diskUsed: number; up: number }> = {
  vm_mock_1: { cpu: 7, mem: 0.41, diskUsed: 0.22, up: 11 * 86_400 + 4 * 3600 },
  vm_mock_5: { cpu: 61, mem: 0.93, diskUsed: 0.88, up: 14 * 3600 + 54 * 60 },
};
const jitter = (n: number) => n + (Math.random() - 0.5) * n * 0.15;

const account: Account = {
  userId: "user_mock",
  displayName: "mock-user",
  billing: {
    mode: "credits",
    currency: "eur",
    balanceMicro: 24_310_000,
    accruingMicro: 410_000,
    maxVms: 50,
    defaultVcpu: 2,
    defaultMemoryBytes: 8 * GIB,
    ceilingVcpu: 4,
    ceilingMemoryBytes: 16 * GIB,
    subscriptionStatus: "active",
  },
};

const boatAccount: Account = {
  userId: "usr_mock",
  displayName: "mock-boat-user",
  billing: {
    mode: "wallet",
    currency: "usd",
    balanceMicro: 18_500_000,
    accruingMicro: null,
    maxVms: 10,
    defaultVcpu: 4,
    defaultMemoryBytes: 8 * GIB,
    ceilingVcpu: 0,
    ceilingMemoryBytes: 0,
    subscriptionStatus: "pro",
  },
};
const daytonaAccount: Account = {
  userId: "org_mock_daytona",
  displayName: "mock-daytona-org",
  billing: {
    mode: "",
    currency: "usd",
    balanceMicro: 0,
    accruingMicro: null,
    maxVms: 0,
    defaultVcpu: 1,
    defaultMemoryBytes: GIB,
    ceilingVcpu: 4,
    ceilingMemoryBytes: 8 * GIB,
    subscriptionStatus: "",
  },
};
const accounts: Record<string, Account> = { boxd: account, boat: boatAccount, daytona: daytonaAccount };

function ownerCaps(id: string): Capabilities {
  const m = machines.find((m) => m.id === id || m.name === id);
  return CAPS[m?.provider ?? "boxd"];
}

let agents: AgentMeta[] = [
  {
    id: "ag_1",
    title: "Add a /health endpoint and a test for it",
    harness: "claude-code",
    machineId: "vm_mock_1",
    machineName: "mock-api",
    cwd: "/home/user/api",
    model: null,
    sessionId: "6f1c2a4e-0b7d-4d39-9a51-3c2e8f0d1a01",
    skipPermissions: true,
    createdAt: Date.now() - 1_800_000,
    updatedAt: Date.now() - 600_000,
  },
  {
    id: "ag_2",
    title: "Why does the nightly build fail on arm64?",
    harness: "codex",
    machineId: "vm_mock_1",
    machineName: "mock-api",
    cwd: "/home/user/api",
    model: null,
    sessionId: null,
    skipPermissions: true,
    createdAt: Date.now() - 300_000,
    updatedAt: Date.now() - 2_000,
  },
  {
    id: "ag_3",
    title: "Migrate the landing page to the new design tokens",
    harness: "claude-code",
    machineId: "vm_mock_2",
    machineName: "mock-web",
    cwd: null,
    model: null,
    sessionId: "6f1c2a4e-0b7d-4d39-9a51-3c2e8f0d1a03",
    skipPermissions: true,
    createdAt: Date.now() - 7_200_000,
    updatedAt: Date.now() - 7_000_000,
  },
  {
    id: "ag_4",
    title: "Upgrade the image pipeline to sharp 0.34",
    harness: "claude-code",
    machineId: "vm_mock_2",
    machineName: "mock-web",
    cwd: "/home/user/web",
    model: null,
    sessionId: "6f1c2a4e-0b7d-4d39-9a51-3c2e8f0d1a04",
    skipPermissions: true,
    createdAt: Date.now() - 5_400_000,
    updatedAt: Date.now() - 3_000_000,
  },
];

// What the boxes' watches show, by the rules of stowbox_core::agent::session:
// Claude Code writes its state from hooks, Codex writes none, and tmux keeps
// a harness running with no terminal open. mock-web has no tmux, so its
// agents only run while their terminal is open.
const nowSecs = () => Math.floor(Date.now() / 1000);
const live = new Map<string, AgentLive>([
  ["ag_1", { id: "ag_1", state: "working", since: nowSecs() - 40, running: true }],
  ["ag_2", { id: "ag_2", state: null, since: 0, running: true }],
  ["ag_3", { id: "ag_3", state: "waiting", since: nowSecs() - 7_000, running: false }],
]);
const noTmux = new Set(["vm_mock_2"]);
const liveListeners = new Set<(p: LivePush) => void>();
function livePush(machineId: string, notices: Notice[] = []): LivePush {
  const mine = agents.filter((a) => a.machineId === machineId).map((a) => live.get(a.id)).filter((l): l is AgentLive => !!l);
  return { machineId, tmux: !noTmux.has(machineId), agents: mine, notices };
}
function emitLive(machineId: string, notices: Notice[] = []) {
  const p = livePush(machineId, notices);
  for (const l of liveListeners) l(p);
}
function setLive(id: string, f: (l: AgentLive) => Partial<AgentLive>) {
  const a = agents.find((x) => x.id === id);
  if (!a) return;
  const was = live.get(id) ?? { id, state: null, since: 0, running: false };
  const next = { ...was, ...f(was) };
  live.set(id, next);
  // The same rule as stowbox_core::notify: working to waiting notifies once.
  const notices: Notice[] =
    was.state === "working" && next.state === "waiting"
      ? [{ kind: "agent-done", title: `${a.title.trim() || "Claude Code agent"} is waiting for you`, body: `In ${a.machineName}`, agentId: id, machineId: a.machineId }]
      : [];
  emitLive(a.machineId, notices);
}
/** ag_1 finishes the turn it was on a little after the page loads. */
setTimeout(() => setLive("ag_1", () => ({ state: "waiting", since: nowSecs() })), 12_000);

/** A prompt submitted in a mock Claude Code terminal: a short turn that edits a file. */
function simulateTurn(id: string) {
  const a = agents.find((x) => x.id === id);
  if (!a || a.harness !== "claude-code") return;
  setLive(id, () => ({ state: "working", since: nowSecs(), running: true }));
  setTimeout(() => {
    if (!live.get(id)?.running) return;
    dirtyWorktree(id);
    setLive(id, () => ({ state: "waiting", since: nowSecs() }));
  }, 4_000);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const mb = (n: number) => (n / (1024 * 1024)).toFixed(1) + " MB";

// Setup presets. Secret values are kept here only to mimic the keychain;
// the UI never receives them.
let presets: Preset[] = [
  {
    id: "preset_mock_1",
    name: "main commands",
    commands: [
      "git clone https://$GITHUB_TOKEN@github.com/example/api.git ~/api",
      "cd ~/api && git checkout {{branch}}",
      "cd ~/api && docker compose up -d",
      "bash ~/api/scripts/bootstrap.sh",
    ],
    secretNames: ["GITHUB_TOKEN", "DATABASE_URL"],
    variables: [{ name: "branch", default: "main" }],
    createdAt: Date.now() - 86_400_000,
    updatedAt: Date.now() - 3_600_000,
    secrets: [
      { name: "GITHUB_TOKEN", hasValue: true },
      { name: "DATABASE_URL", hasValue: true },
    ],
  },
  {
    id: "preset_mock_2",
    name: "review a pull request",
    commands: ["gh repo clone {{repo}} ~/review", "cd ~/review && gh pr checkout {{pr}}"],
    secretNames: [],
    variables: [
      { name: "repo", default: "" },
      { name: "pr", default: "" },
    ],
    createdAt: Date.now() - 3 * 86_400_000,
    updatedAt: Date.now() - 2 * 86_400_000,
    secrets: [],
  },
];
const presetValues: Record<string, Record<string, string>> = { preset_mock_1: { GITHUB_TOKEN: "placeholder", DATABASE_URL: "placeholder" } };

/** Mirrors stowbox_core::provision::prepare. */
function prepareMock(p: Preset, given: VariableValues) {
  const names = variablesIn(p.commands);
  const unknown = Object.keys(given).sort().find((k) => !names.includes(k));
  if (unknown) throw new Error(`"${p.name}" has no variable ${unknown}`);
  const values: Record<string, string> = {};
  for (const name of names) {
    const v = given[name] || p.variables.find((x) => x.name === name)?.default || "";
    if (!v) throw new Error(`give ${name} a value`);
    values[name] = v;
  }
  return { commands: p.commands.map((c) => substitute(c, values)), variables: names.map((name) => ({ name, value: values[name] })) };
}

let setupRuns: SetupRun[] = [
  {
    id: "run_mock_1",
    machineId: "vm_mock_1",
    machineName: "mock-api",
    presetId: "preset_mock_1",
    presetName: "main commands",
    status: "done",
    error: null,
    steps: prepareMock(presets[0], {}).commands.map((command) => ({ command, state: "ok" as const, exitCode: 0 })),
    secretNames: ["GITHUB_TOKEN", "DATABASE_URL"],
    variables: [{ name: "branch", value: "main" }],
    secretsWritten: true,
    output: "Cloning into '/home/user/api'...\n[+] Running 2/2\n ✔ Container api-db-1   Started\n ✔ Container api-app-1  Started\nbootstrap: done\n",
    startedAt: Date.now() - 2 * 86_400_000,
    finishedAt: Date.now() - 2 * 86_400_000 + 74_000,
  },
];
const setupListeners = new Set<(p: SetupPush) => void>();
const cancelled = new Set<string>();

const setupNotified = new Set<string>();
function setupNotice(run: SetupRun): Notice | null {
  if ((run.status !== "done" && run.status !== "failed") || setupNotified.has(run.id)) return null;
  setupNotified.add(run.id);
  const base = { agentId: null, machineId: run.machineId };
  const n = run.steps.length;
  if (run.status === "done") return { ...base, kind: "setup-done", title: `Setup finished in ${run.machineName}`, body: `${run.presetName}: ${n} command${n === 1 ? "" : "s"} ran` };
  return { ...base, kind: "setup-failed", title: `Setup failed in ${run.machineName}`, body: `${run.presetName}: ${run.error ?? "it did not finish"}` };
}

function setupEmit(run: SetupRun, append = "") {
  if (append) run.output += append;
  const notice = setupNotice(run);
  for (const l of setupListeners) l({ run: { ...run, output: "", steps: run.steps.map((s) => ({ ...s })) }, append, notice });
}

async function simulateSetup(run: SetupRun) {
  await sleep(900);
  if (cancelled.has(run.id)) return;
  run.status = "running";
  run.secretsWritten = true;
  setupEmit(run);
  for (let i = 0; i < run.steps.length; i++) {
    if (cancelled.has(run.id)) return;
    run.steps[i].state = "running";
    setupEmit(run, `$ ${run.steps[i].command.split("\n")[0]}\n`);
    for (let k = 0; k < 3; k++) {
      await sleep(500);
      if (cancelled.has(run.id)) return;
      setupEmit(run, `mock output ${k + 1}/3 from step ${i + 1}\n`);
    }
    run.steps[i].state = "ok";
    run.steps[i].exitCode = 0;
    setupEmit(run);
  }
  run.status = "done";
  run.finishedAt = Date.now();
  setupEmit(run);
}

/** Throws what the desktop side would refuse, before anything is created. */
function prepareMockSetup(presetId: string, given: VariableValues) {
  const p = presets.find((x) => x.id === presetId);
  if (!p) throw new Error("setup preset not found");
  const missing = p.secretNames.find((n) => !presetValues[p.id]?.[n]);
  if (missing) throw new Error(`the secret ${missing} in "${p.name}" has no value; edit the preset first`);
  return { p, ...prepareMock(p, given) };
}

function startMockSetup(m: Machine, presetId: string, given: VariableValues): SetupRun {
  const { p, commands, variables } = prepareMockSetup(presetId, given);
  if (setupRuns.some((r) => r.machineId === m.id && (r.status === "waiting" || r.status === "running"))) throw new Error(`a setup is already running in ${m.name}`);
  const run: SetupRun = {
    id: "run_mock_" + Date.now(),
    machineId: m.id,
    machineName: m.name,
    presetId: p.id,
    presetName: p.name,
    status: "waiting",
    error: null,
    steps: commands.map((command) => ({ command, state: "pending", exitCode: null })),
    secretNames: [...p.secretNames],
    variables,
    secretsWritten: false,
    output: "",
    startedAt: Date.now(),
    finishedAt: null,
  };
  setupRuns = [...setupRuns.filter((r) => r.machineId !== m.id), run];
  setupEmit(run);
  void simulateSetup(run);
  return run;
}

const clickListeners = new Set<(t: NoticeTarget) => void>();

// The idle guard, by the same rules as stowbox_core::guard. mock-worker has
// sat idle since well before the page loaded, so enabling the guard in
// Settings shows the countdown at once.
const runningSince: Record<string, number> = { vm_mock_5: Date.now() - 3 * 3_600_000 };
const touchedAt: Record<string, number> = {};
const openTerminals = new Map<string, string>();
let terminalSeq = 0;
const terminalEvents = new Map<string, (s: string) => void>();
/** Agent terminals by terminal id; they echo what is typed, like a prompt would. */
const agentTerminals = new Map<string, string>();
const terminalEnds = new Map<string, (code: number) => void>();
function endTerminal(id: string, code: number) {
  terminalEnds.get(id)?.(code);
  terminalEnds.delete(id);
}
/** Sign-in terminals by box: closing one counts as a finished `/login`. */
const loginTerminals = new Map<string, string>();
const claudeAccounts = new Map<string, ClaudeAccount>();
const orgAccount = (): ClaudeAccount => ({ own: false, source: "stored login", email: "team@example.com", organization: "Example Org" });
const touch = (machineId: string) => (touchedAt[machineId] = Math.max(touchedAt[machineId] ?? 0, Date.now()));

function observe() {
  for (const m of machines) {
    if (m.status === "running") runningSince[m.id] ??= Date.now();
    else delete runningSince[m.id];
  }
}

function decide(m: Machine, policy: IdlePolicy, now: number): GuardVerdict {
  if (policy.idleMinutes <= 0) return { kind: "keep", reason: "guard-off" };
  if (m.status !== "running") return { kind: "keep", reason: "not-running" };
  if (policy.optedOut.includes(m.id)) return { kind: "keep", reason: "opted-out" };
  const mine = agents.filter((a) => a.machineId === m.id);
  if (mine.some((a) => live.get(a.id)?.running && live.get(a.id)?.state === "working")) return { kind: "keep", reason: "agent-working" };
  const runs = setupRuns.filter((r) => r.machineId === m.id);
  if (runs.some((r) => r.status === "waiting" || r.status === "running")) return { kind: "keep", reason: "setup-running" };
  if ([...openTerminals.values()].includes(m.id)) return { kind: "keep", reason: "terminal-open" };
  const last = Math.max(runningSince[m.id] ?? now, touchedAt[m.id] ?? 0, ...mine.map((a) => a.updatedAt), ...runs.map((r) => r.finishedAt ?? r.startedAt));
  const due = last + policy.idleMinutes * 60_000;
  return now >= due ? { kind: "suspend", idleSinceMs: last } : { kind: "wait", dueAtMs: due };
}

// Git repositories by "machineId:dir". The api repo has an agent's work
// waiting for review; the worker repo's remote moved on, so a push there is
// rejected the way git words it.
interface MockRepo {
  review: Omit<GitReview, "cwd" | "sampledAtMs">;
  rejectPush: boolean;
}

const healthPatch = `@@ -1,12 +1,15 @@
 import express from "express";
+import { health } from "./health";
 import { users } from "./users";

 const app = express();
 app.use(express.json());

+app.get("/health", health);
 app.use("/users", users);

-app.listen(3000);
+const port = Number(process.env.PORT ?? 3000);
+app.listen(port, () => console.log(\`listening on \${port}\`));

 export default app;
`;

function mockRepos(): Record<string, MockRepo> {
  const files: GitFileChange[] = [
    { path: "src/server.ts", origPath: null, code: "M", staged: false },
    { path: "package.json", origPath: null, code: "M", staged: false },
    { path: "src/health.ts", origPath: null, code: "?", staged: false },
    { path: "test/health.test.ts", origPath: null, code: "?", staged: false },
  ];
  const diff: GitDiffFile[] = [
    { path: "src/server.ts", oldPath: null, change: "modified", binary: false, additions: 4, deletions: 1, patch: healthPatch },
    {
      path: "package.json",
      oldPath: null,
      change: "modified",
      binary: false,
      additions: 1,
      deletions: 1,
      patch: `@@ -6,7 +6,7 @@
   "scripts": {
     "start": "node dist/server.js",
-    "test": "node --test"
+    "test": "node --test test/"
   },
`,
    },
    {
      path: "src/health.ts",
      oldPath: null,
      change: "added",
      binary: false,
      additions: 5,
      deletions: 0,
      patch: `@@ -0,0 +1,5 @@
+import type { Request, Response } from "express";
+
+export function health(_req: Request, res: Response) {
+  res.json({ ok: true, uptime: process.uptime() });
+}
`,
    },
    {
      path: "test/health.test.ts",
      oldPath: null,
      change: "added",
      binary: false,
      additions: 9,
      deletions: 0,
      patch: `@@ -0,0 +1,9 @@
+import { test } from "node:test";
+import assert from "node:assert/strict";
+import request from "supertest";
+import app from "../src/server";
+
+test("GET /health answers ok", async () => {
+  const res = await request(app).get("/health");
+  assert.equal(res.body.ok, true);
+});
`,
    },
  ];
  return {
    "vm_mock_1:/home/user/api": {
      rejectPush: false,
      review: {
        state: "ok",
        root: "/home/user/api",
        branch: { head: "add-health-endpoint", oid: "4f1c2a9e0b7d", upstream: "origin/add-health-endpoint", ahead: 0, behind: 0 },
        remote: "origin",
        remoteUrl: "git@github.com:mock-org/api.git",
        base: "main",
        gh: true,
        token: true,
        files,
        filesTruncated: false,
        diff,
        diffTruncated: false,
      },
    },
    "vm_mock_5:/home/user/worker": {
      rejectPush: true,
      review: {
        state: "ok",
        root: "/home/user/worker",
        branch: { head: "main", oid: "9b0e33c1d2aa", upstream: "origin/main", ahead: 1, behind: 2 },
        remote: "origin",
        remoteUrl: "https://gitlab.com/mock-org/worker.git",
        base: "main",
        gh: false,
        token: false,
        files: [],
        filesTruncated: false,
        diff: [],
        diffTruncated: false,
      },
    },
  };
}
const repos = mockRepos();

function mockRepo(machineId: string, cwd: string | null): MockRepo | GitReview["state"] {
  const dir = (cwd ?? "~").replace(/^~(?=\/|$)/, "/home/user").replace(/\/+$/, "");
  const hit = Object.entries(repos).find(([k]) => {
    const [m, root] = k.split(/:(.*)/s);
    return m === machineId && (dir === root || dir.startsWith(root + "/"));
  });
  if (hit) return hit[1];
  return dir === "/home/user" || dir === "" ? "not-repo" : "no-dir";
}

/** The core's `worktree::slug`: up to five words of the hint, then six characters of the id. */
function slug(hint: string, id: string): string {
  let head = "";
  for (const w of hint.split(/[^A-Za-z0-9]+/).filter(Boolean).slice(0, 5)) {
    if (head.length + w.length + 1 > 32) break;
    head += (head ? "-" : "") + w.toLowerCase();
  }
  const tail = id.replace(/[^A-Za-z0-9]/g, "").slice(0, 6).toLowerCase();
  return [head || "agent", tail].filter(Boolean).join("-");
}

/** Like `git worktree add -b stowbox/<slug> <repo>-wt/<slug> HEAD`: a clean checkout of the repository's commit. */
function addWorktree(machineId: string, cwd: string | null, s: string): { worktree: NonNullable<AgentMeta["worktree"]>; cwd: string } {
  const r = mockRepo(machineId, cwd);
  const shown = cwd?.trim() || "~";
  if (r === "no-dir") throw new Error(`${shown} does not exist in the box, so the agent cannot have its own branch there`);
  if (typeof r === "string") throw new Error(`${shown} is not inside a git repository, so the agent cannot have its own branch there`);
  const root = r.review.root;
  const path = `${root}-wt/${s}`;
  const branch = `stowbox/${s}`;
  if (repos[`${machineId}:${path}`]) throw new Error(`${path} already exists`);
  const dir = (cwd ?? "~").replace(/^~(?=\/|$)/, "/home/user").replace(/\/+$/, "");
  const prefix = dir.slice(root.length);
  repos[`${machineId}:${path}`] = {
    rejectPush: false,
    review: { ...structuredClone(r.review), root: path, branch: { head: branch, oid: r.review.branch.oid, upstream: null, ahead: 0, behind: 0 }, files: [], diff: [] },
  };
  return { worktree: { path, branch, repo: root }, cwd: path + prefix };
}

/** A mock turn edits a file in the agent's worktree, so its Changes tab and removal have something to show. */
function dirtyWorktree(id: string) {
  const a = agents.find((x) => x.id === id);
  const r = a?.worktree && repos[`${a.machineId}:${a.worktree.path}`];
  if (!r || r.review.files.length) return;
  r.review.files = [{ path: "src/app.ts", origPath: null, code: "M", staged: false }];
  r.review.diff = [{ path: "src/app.ts", oldPath: null, change: "modified", binary: false, additions: 4, deletions: 1, patch: healthPatch }];
}

// SSH on "this computer": whether Stowbox's key exists and which Host
// stanzas (alias -> port) the managed block in ~/.ssh/config holds.
let mockSshKey = false;
const mockSshHosts = new Map<string, number>();

function mockSshPlan(m: Machine): SshPlan {
  const alias = "stowbox-" + m.name.toLowerCase().replace(/[^a-z0-9._-]/g, "-");
  const host = `${m.name}.${m.accessDomain || "boxd.sh"}`;
  const directCommand = m.sshPort ? `ssh -p ${m.sshPort} boxd@${host}` : "";
  const had = mockSshHosts.get(alias);
  const config = had === undefined ? "add" : had === m.sshPort ? "current" : "update";
  let blocker: string | null = null;
  if (!m.sshPort) blocker = `boxd has not given ${m.name} an SSH port yet. Try again once the box is running.`;
  else if (m.status === "stopped" || m.status === "failed") blocker = `${m.name} is ${m.status}. Start it first.`;
  return {
    alias,
    sshCommand: config === "current" ? `ssh ${alias}` : directCommand,
    directCommand,
    keyPath: "~/.ssh/stowbox/id_ed25519",
    keyExists: mockSshKey,
    configPath: "~/.ssh/config",
    backupPath: "~/.ssh/config.stowbox-backup",
    config,
    blocker,
  };
}

// Ports per box: what listens inside it, its named HTTPS routes, the
// default route's pinned port (null = detected) and its raw forwards.
const mockListening: Record<string, ListeningPort[]> = {
  vm_mock_1: [
    { port: 22, addresses: ["0.0.0.0", "::"], process: null, localOnly: false },
    { port: 3000, addresses: ["0.0.0.0"], process: "node", localOnly: false },
    { port: 5173, addresses: ["127.0.0.1", "::1"], process: "node", localOnly: true },
    { port: 5432, addresses: ["127.0.0.1"], process: null, localOnly: true },
  ],
  vm_mock_2: [{ port: 22, addresses: ["0.0.0.0"], process: null, localOnly: false }, { port: 8000, addresses: ["0.0.0.0"], process: "python3", localOnly: false }],
  vm_mock_5: [{ port: 22, addresses: ["0.0.0.0"], process: null, localOnly: false }, { port: 9090, addresses: ["0.0.0.0"], process: "python3", localOnly: false }],
};
const mockRoutes: Record<string, { name: string; port: number }[]> = { vm_mock_1: [{ name: "docs", port: 4000 }] };
const mockDefaultPort: Record<string, number | null> = {};
const mockForwards: Record<string, { vmPort: number; publicPort: number; protocol: string }[]> = { vm_mock_1: [{ vmPort: 5432, publicPort: 48211, protocol: "tcp" }] };
/** The ports boxd's default route tries, in order, before falling back to 8000. */
const AUTO_PORTS = [80, 443, 8080, 8000, 3000, 5000, 5173];

function mockRouteList(m: Machine): HttpsRoute[] {
  // boat and Daytona: one tokened link per port, only those made in this session.
  if (m.provider === "boat" || m.provider === "daytona")
    return (mockRoutes[m.id] ?? []).map((r): HttpsRoute => {
      const domain = m.provider === "boat" ? `${r.port}-${m.id.replace(/_/g, "")}.on.boat.dev` : `${r.port}-${m.id}.proxy.daytona.works`;
      return { name: String(r.port), domain, url: `https://${domain}/?token=mock${r.port}`, port: r.port, portMode: "locked", isDefault: false };
    });
  const host = `${m.name}.${m.accessDomain || "boxd.sh"}`;
  const pinned = mockDefaultPort[m.id] ?? null;
  const listening = new Set((mockListening[m.id] ?? []).map((l) => l.port));
  const detected = AUTO_PORTS.find((p) => listening.has(p)) ?? 8000;
  const def: HttpsRoute = { name: "default", domain: host, url: `https://${host}`, port: pinned ?? detected, portMode: pinned === null ? "auto" : "locked", isDefault: true };
  const named = (mockRoutes[m.id] ?? []).map((r): HttpsRoute => ({ name: r.name, domain: `${r.name}.${host}`, url: `https://${r.name}.${host}`, port: r.port, portMode: "locked", isDefault: false }));
  return [def, ...named];
}

function mockForwardList(m: Machine): PortForward[] {
  const dns = `${m.name}.${m.accessDomain || "boxd.sh"}`;
  return (mockForwards[m.id] ?? []).map((f) => ({ dns, publicPort: f.publicPort, vmPort: f.vmPort, protocol: f.protocol, address: `${dns}:${f.publicPort}` }));
}

function mockPort(port: number): number {
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error(`${port} is not a port: use 1 to 65535`);
  return port;
}

function requireRepo(machineId: string, cwd: string | null): MockRepo {
  const r = mockRepo(machineId, cwd);
  if (typeof r === "string") throw new Error(r === "no-dir" ? "the directory does not exist in the box" : "the directory is not inside a git repository");
  return r;
}

export const mockApi: Api = {
  async providerStatus() {
    // `?onboarding` previews the first-run screen.
    if (window.location.search.includes("onboarding")) connected.clear();
    const providers = Object.entries(CAPS).map(([id, capabilities]) => ({ id, connected: connected.has(id), secretBackend: connected.has(id) ? ("file" as const) : null, capabilities }));
    return { configured: connected.size > 0, providers };
  },
  async setApiKey(provider, key) {
    // The same shape check as the desktop shell's set_api_key.
    const caps = CAPS[provider];
    if (!caps) throw new Error(`unknown provider ${provider}`);
    const k = key.trim();
    if (!caps.keyPrefixes.some((p) => k.startsWith(p)) || k.length < 20)
      throw new Error(`That does not look like a ${caps.label} API key. Keys start with ${caps.keyPrefixes.join(" or ")}.`);
    connected.add(provider);
    return accounts[provider];
  },
  async clearApiKey(provider) {
    if (provider) connected.delete(provider);
    else connected.clear();
    machines = machines.filter((m) => connected.has(m.provider));
  },
  async whoami(provider) {
    if (!connected.has(provider)) throw new Error(`${provider} is not connected`);
    return accounts[provider];
  },
  async listMachines() {
    await sleep(150);
    return { machines, errors: [] };
  },
  async getMachine(id) {
    const m = machines.find((m) => m.id === id || m.name === id);
    if (!m) throw new Error("not found: " + id);
    return m;
  },
  async createMachine(spec, presetId, variables) {
    if (presetId) prepareMockSetup(presetId, variables ?? {});
    await sleep(800);
    const provider = spec.provider ?? [...connected][0] ?? "boxd";
    const m =
      provider === "boat"
        ? machine({ id: "sbx_mock_" + Date.now(), createdAt: Math.floor(Date.now() / 1000), name: spec.name || "boat-new", status: "running", provider, size: spec.size ?? "default", autoStopAt: spec.autoStopSecs ? Math.floor(Date.now() / 1000) + spec.autoStopSecs : 0 })
        : provider === "daytona"
          ? machine({ id: "dt-mock-" + Date.now(), createdAt: Math.floor(Date.now() / 1000), lastConnectedMs: Date.now(), name: spec.name || "daytona-new", status: "running", provider, vcpu: spec.vcpu ?? 1, memoryBytes: spec.memoryBytes ?? GIB, idleStopSecs: spec.autoStopSecs ?? 3_600 })
          : machine({ id: "vm_mock_" + Date.now(), createdAt: Math.floor(Date.now() / 1000), name: spec.name || "mock-new", status: "running", vcpu: spec.vcpu ?? 2, memoryBytes: spec.memoryBytes ?? 8 * GIB, isolated: spec.isolated });
    machines = [...machines, m];
    if (presetId) startMockSetup(m, presetId, variables ?? {});
    return m;
  },
  async forkMachine(source, name) {
    await sleep(300);
    const src = await mockApi.getMachine(source);
    const m = machine({ ...src, id: (src.provider === "boat" ? "sbx_mock_" : "vm_mock_") + Date.now(), createdAt: Math.floor(Date.now() / 1000), name: name || src.name + "-fork", status: "running", source: "fork/" + src.name });
    machines = [...machines, m];
    return m;
  },
  async machinePower(id, action) {
    await sleep(400);
    const caps = ownerCaps(id);
    if (!caps.power.includes(action)) throw new Error(`${caps.label} cannot ${action} a box`);
    const next: Record<string, Machine["status"]> = { start: "running", stop: "stopped", reboot: "running", pause: "suspended", resume: "running", hibernate: "hibernated", wake: "running" };
    if (action === "destroy") {
      machines = machines.filter((m) => m.id !== id);
      claudeAccounts.delete(id);
      // Like the app: agents that lived there stay as history.
      for (const a of agents) if (a.machineId === id) live.delete(a.id);
      return;
    }
    machines = machines.map((m) => (m.id === id ? machine({ ...m, status: next[action] }) : m));
    // A stop or reboot ends every process in the box; standby keeps them.
    if (action === "stop" || action === "reboot") for (const a of agents) if (a.machineId === id && live.has(a.id)) setLive(a.id, () => ({ running: false }));
  },
  async setAutoStop(id, secs) {
    await sleep(200);
    if (ownerCaps(id).idleStop) {
      machines = machines.map((m) => (m.id === id ? { ...m, idleStopSecs: secs } : m));
      return mockApi.getMachine(id);
    }
    machines = machines.map((m) => (m.id === id ? { ...m, autoStopAt: secs > 0 && m.status === "running" ? Math.floor(Date.now() / 1000) + secs : 0 } : m));
    return mockApi.getMachine(id);
  },
  async machineUsage(id) {
    const m = await mockApi.getMachine(id);
    if (m.provider === "daytona") throw new Error("Daytona's API does not report what a sandbox has cost; the Daytona dashboard does");
    if (m.provider === "boat")
      return {
        periodStart: m.createdAt,
        periodEnd: Math.floor(Date.now() / 1000),
        currency: "usd",
        rates: { vcpuHourMicro: 0, ramGibHourMicro: 0, diskGibHourMicro: 0 },
        vcpuHours: 0,
        ramGibHours: 0,
        diskGibHours: 0,
        costMicro: 410_000,
        seconds: { running: 20_500, standby: 0, hibernated: 0, stopped: 0 },
        complete: true,
      };
    return {
      periodStart: m.createdAt,
      periodEnd: Math.floor(Date.now() / 1000),
      currency: "eur",
      rates: { vcpuHourMicro: 39_000, ramGibHourMicro: 12_000, diskGibHourMicro: 80 },
      vcpuHours: 11.2,
      ramGibHours: 61.4,
      diskGibHours: 7_200,
      costMicro: 1_750_000,
      seconds: { running: 20_160, standby: 36_000, hibernated: 200_000, stopped: 0 },
      complete: false,
    };
  },
  async machineMetrics(id): Promise<MachineMetrics | null> {
    const m = await mockApi.getMachine(id);
    if (m.status !== "running") return null;
    await sleep(500 + Math.random() * 400);
    const b = metricBase[m.id] ?? { cpu: 3, mem: 0.2, diskUsed: 0.1, up: 1800 };
    const diskTotal = 100 * GIB;
    const used = Math.round(b.diskUsed * diskTotal);
    return {
      sampledAtMs: Date.now(),
      cpuPercent: Math.min(100, Math.max(0, Math.round(jitter(b.cpu) * 10) / 10)),
      vcpu: m.vcpu,
      load1: Math.round(jitter(b.cpu / 100 * m.vcpu) * 100) / 100,
      memTotalBytes: m.memoryBytes,
      memUsedBytes: Math.round(Math.min(0.99, jitter(b.mem)) * m.memoryBytes),
      diskTotalBytes: diskTotal,
      diskUsedBytes: used,
      diskAvailBytes: diskTotal - used - 5 * GIB,
      uptimeSecs: b.up + Math.round((Date.now() - 1_700_000_000_000) / 1000) % 60,
    };
  },
  async rates(provider) {
    if (provider === "daytona") return { currency: "usd", rates: DAYTONA_RATES, sizes: [] };
    if (provider === "boat") return { currency: "usd", rates: { vcpuHourMicro: 0, ramGibHourMicro: 0, diskGibHourMicro: 0 }, sizes: BOAT_SIZES.map((z) => ({ size: z.id, hourlyMicro: z.hourlyMicro })) };
    return { currency: "eur", rates: { vcpuHourMicro: 39_000, ramGibHourMicro: 12_000, diskGibHourMicro: 80 }, sizes: [] };
  },
  async guardCheck(policy) {
    observe();
    const now = Date.now();
    return machines.map((m) => ({ machineId: m.id, machineName: m.name, verdict: decide(m, policy, now) }));
  },
  async guardSuspend(machineId, policy) {
    observe();
    const m = await mockApi.getMachine(machineId);
    const verdict = decide(m, policy, Date.now());
    const act = verdict.kind === "suspend";
    // Like the shell: standby where the provider has it, a stop otherwise.
    const pause = ownerCaps(m.id).power.includes("pause");
    if (act) await mockApi.machinePower(m.id, pause ? "pause" : "stop");
    const after = act ? machine({ ...m, status: pause ? "suspended" : "stopped" }).estHourlyMicro : m.estHourlyMicro;
    return { verdict, suspended: act && pause, stopped: act && !pause, machineName: m.name, hourlyBeforeMicro: m.estHourlyMicro, hourlyAfterMicro: after, currency: m.currency };
  },
  async guardTouch(machineId) {
    touch(machineId);
  },
  // `?spend=projected` previews a key that cannot read boxd's meter.
  async spendToday(_since, limitMicro, currency) {
    await sleep(200);
    const cur = currency ?? "eur";
    // Providers billing in another currency are left out, as in the shell.
    const excluded = [...connected].filter((p) => CAPS[p].currency !== cur).map((p) => CAPS[p].label);
    if (cur === "usd") {
      return { basis: "metered", totalMicro: 410_000, complete: true, amountMicro: 410_000, limitMicro, currency: cur, over: limitMicro > 0 && 410_000 >= limitMicro, excluded };
    }
    const amount = (over: number) => ({ amountMicro: over, limitMicro, currency: cur, over: limitMicro > 0 && over >= limitMicro, excluded });
    if (window.location.search.includes("spend=projected")) {
      const hourlyMicro = machines.filter((m) => m.currency === cur).reduce((s, m) => s + m.estHourlyMicro, 0);
      return { basis: "projected", hourlyMicro, why: "permission denied: usage is visible to org owners and admins", ...amount(hourlyMicro * 24) };
    }
    return { basis: "metered", totalMicro: 1_840_000, complete: false, ...amount(1_840_000) };
  },
  async orgBilling(provider) {
    if (provider === "daytona") throw new Error("Daytona's API does not report a balance");
    if (provider === "boat") return { id: "team_mock", name: "", isAdmin: false, currency: "usd", mode: "wallet", balanceMicro: 18_500_000, accruingMicro: null, vmCount: machines.filter((m) => m.provider === "boat").length, maxVms: 10 };
    return { id: "org_mock", name: "mock-org", isAdmin: true, currency: "eur", mode: "credits", balanceMicro: 24_310_000, accruingMicro: 410_000, vmCount: machines.length, maxVms: 50 };
  },
  async listAgents() {
    return [...agents].sort((a, b) => b.updatedAt - a.updatedAt);
  },
  async createAgent(spec) {
    const id = "ag_" + Math.random().toString(16).slice(2, 10);
    let cwd = spec.cwd ?? null;
    let worktree: AgentMeta["worktree"] = null;
    if (spec.ownBranch) {
      await sleep(700);
      const added = addWorktree(spec.machineId, cwd, slug(spec.title || spec.branchHint || "", id.slice(3)));
      cwd = added.cwd;
      worktree = added.worktree;
    }
    const meta: AgentMeta = {
      id,
      title: spec.title ?? "",
      harness: spec.harness,
      machineId: spec.machineId,
      machineName: spec.machineName,
      cwd,
      worktree,
      model: spec.model ?? null,
      sessionId: spec.harness === "claude-code" ? crypto.randomUUID() : null,
      skipPermissions: spec.skipPermissions,
      createdAt: Date.now(),
      updatedAt: Date.now(),
    };
    agents = [meta, ...agents];
    return meta;
  },
  async agentUpload(id, name, bytes) {
    const a = agents.find((x) => x.id === id);
    if (!a) throw new Error("agent not found");
    if (bytes.length === 0) throw new Error(`${name} is empty`);
    const caps = ownerCaps(a.machineId);
    if (bytes.length > caps.maxUploadBytes) throw new Error(`${name} is ${mb(bytes.length)}; files are limited to ${uploadCapText(caps.maxUploadBytes)} each`);
    await sleep(400 + Math.min(1600, bytes.length / 4000));
    const clean = (name.split(/[\\/]/).pop() ?? "").replace(/[^A-Za-z0-9._-]+/g, "_").replace(/^[._]+/, "") || "file";
    const tag = Math.random().toString(16).slice(2, 8);
    return { name, path: `${caps.remoteHome}/.stowbox/uploads/${id}/${tag}-${clean}`, size: bytes.length };
  },
  async agentStop(id) {
    await sleep(300);
    if (live.has(id)) setLive(id, () => ({ running: false }));
    // The harness exits, so its terminal ends too.
    for (const [term, agent] of agentTerminals) if (agent === id) endTerminal(term, 0);
  },
  async agentsLive() {
    const boxes = new Set(agents.filter((a) => live.has(a.id)).map((a) => a.machineId));
    return [...boxes].map((m) => livePush(m));
  },
  async deleteAgent(id, removeWorktree, force) {
    const a = agents.find((x) => x.id === id);
    const wt = a?.worktree;
    if (a && wt && removeWorktree) {
      await sleep(500);
      const key = `${a.machineId}:${wt.path}`;
      if (repos[key]?.review.files.length && !force) throw new Error(`fatal: '${wt.path}' contains modified or untracked files, use --force to delete it`);
      delete repos[key];
    }
    agents = agents.filter((a) => a.id !== id);
    live.delete(id);
    if (a) emitLive(a.machineId);
  },
  async renameAgent(id, title) {
    agents = agents.map((a) => (a.id === id ? { ...a, title } : a));
    return agents.find((a) => a.id === id)!;
  },
  async onAgentsLive(cb) {
    liveListeners.add(cb);
    return () => liveListeners.delete(cb);
  },
  async listPresets() {
    return presets.map((p) => ({ ...p, secrets: p.secretNames.map((name) => ({ name, hasValue: !!presetValues[p.id]?.[name] })) }));
  },
  async savePreset(input) {
    await sleep(200);
    const name = input.name.trim();
    const commands = input.commands.map((c) => c.trimEnd()).filter((c) => c.trim());
    const names = input.secrets.map((x) => x.name.trim());
    if (!name) throw new Error("give the preset a name");
    if (commands.length === 0 && names.length === 0) throw new Error("add at least one command or one secret");
    const bad = names.find((n) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(n));
    if (bad !== undefined) throw new Error(`"${bad}" is not a valid variable name: use letters, digits and _, not starting with a digit`);
    const dup = names.find((n, i) => names.indexOf(n) !== i);
    if (dup) throw new Error(`the secret ${dup} is listed twice`);
    const id = input.id ?? "preset_mock_" + Date.now();
    const old = presetValues[id] ?? {};
    const values: Record<string, string> = {};
    for (const [i, x] of input.secrets.entries()) {
      const v = x.value || old[names[i]];
      if (!v) throw new Error(`give ${names[i]} a value`);
      values[names[i]] = v;
    }
    presetValues[id] = values;
    const prev = presets.find((p) => p.id === id);
    const next: Preset = {
      id,
      name,
      commands,
      secretNames: names,
      variables: variablesIn(commands).map((n) => ({ name: n, default: input.variables.find((v) => v.name === n)?.default ?? "" })),
      createdAt: prev?.createdAt ?? Date.now(),
      updatedAt: Date.now(),
      secrets: names.map((n) => ({ name: n, hasValue: true })),
    };
    presets = prev ? presets.map((p) => (p.id === id ? next : p)) : [...presets, next];
    return next;
  },
  async deletePreset(id) {
    presets = presets.filter((p) => p.id !== id);
    delete presetValues[id];
  },
  async setupStart(machineId, presetId, variables) {
    const m = await mockApi.getMachine(machineId);
    return startMockSetup(m, presetId, variables ?? {});
  },
  async setupCancel(machineId) {
    const run = setupRuns.find((r) => r.machineId === machineId && (r.status === "waiting" || r.status === "running"));
    if (!run) return;
    cancelled.add(run.id);
    run.status = "cancelled";
    run.finishedAt = Date.now();
    for (const st of run.steps) if (st.state === "pending" || st.state === "running") st.state = "skipped";
    setupEmit(run);
  },
  async setupRuns() {
    return setupRuns.map((r) => ({ ...r, steps: r.steps.map((s) => ({ ...s })) }));
  },
  async onSetupEvent(cb) {
    setupListeners.add(cb);
    return () => setupListeners.delete(cb);
  },
  async gitReview(machineId, cwd) {
    const m = await mockApi.getMachine(machineId);
    // exec wakes a box in standby, as it does in boxd.
    if (m.status === "suspended" || m.status === "hibernated") await mockApi.machinePower(machineId, m.status === "suspended" ? "resume" : "wake");
    if (m.status === "stopped") throw new Error("the box is stopped");
    await sleep(500);
    const r = mockRepo(machineId, cwd);
    const base = { cwd: cwd ?? "~", sampledAtMs: Date.now() };
    if (typeof r === "string") {
      return { ...base, state: r, root: "", branch: { head: null, oid: null, upstream: null, ahead: 0, behind: 0 }, remote: null, remoteUrl: null, base: null, gh: false, token: false, files: [], filesTruncated: false, diff: [], diffTruncated: false };
    }
    return structuredClone({ ...r.review, ...base });
  },
  async gitCommit(machineId, cwd, message) {
    const r = requireRepo(machineId, cwd);
    await sleep(600);
    if (!message.trim()) throw new Error("write a commit message");
    if (r.review.files.length === 0) throw new Error("On branch " + r.review.branch.head + "\nnothing to commit, working tree clean");
    const n = r.review.files.length;
    const adds = r.review.diff.reduce((t, f) => t + f.additions, 0);
    const dels = r.review.diff.reduce((t, f) => t + f.deletions, 0);
    r.review.files = [];
    r.review.diff = [];
    r.review.branch.ahead += 1;
    r.review.branch.oid = Math.random().toString(16).slice(2, 14);
    return `[${r.review.branch.head} ${r.review.branch.oid.slice(0, 7)}] ${message.trim().split("\n")[0]}\n ${n} files changed, ${adds} insertions(+), ${dels} deletions(-)`;
  },
  async gitPush(machineId, cwd) {
    const r = requireRepo(machineId, cwd);
    await sleep(900);
    const b = r.review.branch;
    const url = r.review.remoteUrl ?? "origin";
    if (r.rejectPush) {
      throw new Error(
        `To ${url}\n ! [rejected]        ${b.head} -> ${b.head} (fetch first)\nerror: failed to push some refs to '${url}'\nhint: Updates were rejected because the remote contains work that you do not\nhint: have locally. This is usually caused by another repository pushing to\nhint: the same ref. If you want to integrate the remote changes, use\nhint: 'git pull' before pushing again.`,
      );
    }
    const from = Math.random().toString(16).slice(2, 9);
    b.ahead = 0;
    b.upstream = b.upstream ?? `${r.review.remote}/${b.head}`;
    return `To ${url}\n   ${from}..${(b.oid ?? "").slice(0, 7)}  ${b.head} -> ${b.head}`;
  },
  async gitOpenPr(machineId, cwd) {
    const r = requireRepo(machineId, cwd);
    await sleep(1200);
    const b = r.review.branch;
    if (!b.upstream) throw new Error("aborted: you must first push the current branch to a remote, or use the --head flag");
    if (r.review.gh && r.review.token) return { url: "https://github.com/mock-org/api/pull/42", via: "gh", existing: false };
    return {
      url: `https://gitlab.com/mock-org/worker/-/merge_requests/new?merge_request%5Bsource_branch%5D=${b.head}&merge_request%5Btarget_branch%5D=${r.review.base}`,
      via: "compare",
      existing: false,
    };
  },
  async httpsRoutes(machineId) {
    await sleep(200);
    return mockRouteList(await mockApi.getMachine(machineId));
  },
  async addHttpsRoute(machineId, name, port) {
    const m = await mockApi.getMachine(machineId);
    mockPort(port);
    if (m.provider === "boat" || m.provider === "daytona") {
      await sleep(400);
      mockRoutes[m.id] = [...(mockRoutes[m.id] ?? []).filter((r) => r.port !== port), { name: String(port), port }].sort((a, b) => a.port - b.port);
      return mockRouteList(m).find((r) => r.port === port)!;
    }
    if (!/^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(name)) throw new Error(`"${name}" cannot be a link name: use lowercase letters, digits and hyphens, up to 63, not starting or ending with a hyphen`);
    const list = (mockRoutes[m.id] ??= []);
    if (name === "default" || list.some((r) => r.name === name)) throw new Error(`proxy "${name}" already exists on ${m.name}`);
    await sleep(400);
    list.push({ name, port });
    return mockRouteList(m).find((r) => r.name === name)!;
  },
  async removeHttpsRoute(machineId, name) {
    const m = await mockApi.getMachine(machineId);
    await sleep(250);
    mockRoutes[m.id] = (mockRoutes[m.id] ?? []).filter((r) => r.name !== name);
  },
  async setDefaultRoutePort(machineId, port) {
    const m = await mockApi.getMachine(machineId);
    await sleep(250);
    mockDefaultPort[m.id] = port === null ? null : mockPort(port);
  },
  async portForwards(machineId) {
    await sleep(200);
    return mockForwardList(await mockApi.getMachine(machineId));
  },
  async exposePort(machineId, port, protocol) {
    const m = await mockApi.getMachine(machineId);
    mockPort(port);
    if (!["tcp", "udp", "both"].includes(protocol)) throw new Error(`"${protocol}" is not a protocol: use tcp, udp or both`);
    const list = (mockForwards[m.id] ??= []);
    const had = list.find((f) => f.vmPort === port);
    if (!had && list.length >= 3) throw new Error(`${m.name} already has 3 exposed ports, the most boxd allows; unexpose one first`);
    await sleep(400);
    if (had) had.protocol = protocol;
    else list.push({ vmPort: port, publicPort: 40000 + Math.floor(Math.random() * 20000), protocol });
    return mockForwardList(m).find((f) => f.vmPort === port)!;
  },
  async unexposePort(machineId, port) {
    const m = await mockApi.getMachine(machineId);
    const list = mockForwards[m.id] ?? [];
    if (!list.some((f) => f.vmPort === port)) throw new Error(`nothing is exposed on port ${port}`);
    await sleep(250);
    mockForwards[m.id] = list.filter((f) => f.vmPort !== port);
  },
  async listeningPorts(machineId, wake) {
    const m = await mockApi.getMachine(machineId);
    const asleep = m.status === "suspended" || m.status === "hibernated";
    if (m.status !== "running" && !(wake && asleep)) return null;
    // exec wakes a box in standby, as it does in boxd.
    if (asleep) await mockApi.machinePower(machineId, m.status === "suspended" ? "resume" : "wake");
    await sleep(350);
    return { ports: structuredClone(mockListening[m.id] ?? []), source: "ss", sampledAtMs: Date.now() };
  },
  async sshPlan(machineId) {
    await sleep(120);
    return mockSshPlan(await mockApi.getMachine(machineId));
  },
  async openInEditor(machineId, editor, dir) {
    const m = await mockApi.getMachine(machineId);
    const plan = mockSshPlan(m);
    if (plan.blocker) throw new Error(plan.blocker);
    await sleep(700);
    const changes: string[] = [];
    if (!mockSshKey) {
      mockSshKey = true;
      changes.push(`Created an SSH key for Stowbox at ${plan.keyPath} (your other keys are untouched)`, "Linked it to your boxd account");
    }
    if (plan.config !== "current") {
      const hadFile = mockSshHosts.size > 0;
      mockSshHosts.set(plan.alias, m.sshPort);
      changes.push(`${plan.config === "add" ? "Added" : "Updated"} Host ${plan.alias} in ${plan.configPath}${hadFile ? `; the previous file is at ${plan.backupPath}` : ""}`);
    }
    if (m.status === "suspended" || m.status === "hibernated") {
      await mockApi.machinePower(machineId, m.status === "suspended" ? "resume" : "wake");
      changes.push(`Woke ${m.name}`);
    }
    const d = (dir ?? "").trim();
    const path = !d || d === "~" ? "/home/boxd" : d.startsWith("~/") ? "/home/boxd/" + d.slice(2) : d.startsWith("/") ? d : "/home/boxd/" + d;
    const url = `${editor}://vscode-remote/ssh-remote+${plan.alias}${encodeURI(path.replace(/\/+$/, "") || "/")}?windowId=_blank`;
    // The browser preview does not hand vscode:// or cursor:// links to the OS.
    console.info("mock: would open", url);
    return { url, alias: plan.alias, changes };
  },
  async terminalOpen(machineId, _cols, _rows, target, onEvent) {
    const enc = new TextEncoder();
    const send = (s: string) => onEvent({ type: "data", b64: btoa(String.fromCharCode(...enc.encode(s))) } as TermEvent);
    const id = `term_mock_${++terminalSeq}`;
    if (target.kind === "agent") {
      const a = agents.find((x) => x.id === target.id);
      if (!a) throw new Error("no such agent");
      const cmd = a.harness === "claude-code" ? `claude --resume ${a.sessionId}` : `codex${a.sessionId ? ` resume ${a.sessionId}` : ""}`;
      const where = noTmux.has(machineId) ? "" : " in tmux, so it keeps running when this terminal closes";
      setTimeout(() => send(`mock terminal: the desktop build runs ${cmd} inside the box${where}.\r\n\r\n> ${target.input ? `${target.input}\r\n> ` : ""}`), 100);
      agentTerminals.set(id, a.id);
      terminalEnds.set(id, (code) => onEvent({ type: "exit", code }));
      agents = agents.map((x) => (x.id === a.id ? { ...x, updatedAt: Date.now() } : x));
      if (target.input?.trim()) simulateTurn(a.id);
      else setLive(a.id, () => ({ running: true }));
    } else if (target.kind === "claude-login") {
      // Like the real sign-in: a long address that wraps over rows, under an
      // app that tracks the mouse (which is what made selections vanish).
      const url = `https://example.com/oauth/authorize?code=true&client_id=mock-client&response_type=code&redirect_uri=https%3A%2F%2Fexample.com%2Fcallback&scope=user%3Ainference&state=${"m0ck".repeat(12)}`;
      setTimeout(
        () =>
          send(
            `\x1b[?1003h\x1b[?1006hmock terminal: the desktop build runs claude /login inside the box.\r\nLeave this tab to pretend the sign-in worked.\r\n\r\nBrowser didn't open? Use the url below to sign in:\r\n\r\n${url}\r\n\r\nPaste code here if prompted > `,
          ),
        100,
      );
      loginTerminals.set(id, machineId);
    } else {
      setTimeout(() => send("mock terminal: the desktop build runs a real shell inside the box.\r\n$ "), 100);
    }
    openTerminals.set(id, machineId);
    terminalEvents.set(id, send);
    touch(machineId);
    return id;
  },
  async terminalWrite(id, bytes) {
    const m = openTerminals.get(id);
    if (m) touch(m);
    const agent = agentTerminals.get(id);
    if (!agent) return;
    const text = new TextDecoder().decode(bytes);
    terminalEvents.get(id)?.(text.replace(/\r/g, "\r\n> ").replace(/\x7f/g, "\b \b"));
    if (text.includes("\r")) simulateTurn(agent);
  },
  async terminalResize() {},
  async terminalClose(id) {
    const m = openTerminals.get(id);
    openTerminals.delete(id);
    terminalEvents.delete(id);
    terminalEnds.delete(id);
    const agent = agentTerminals.get(id);
    agentTerminals.delete(id);
    // Without tmux the harness dies with its terminal.
    if (agent && m && noTmux.has(m)) setLive(agent, () => ({ running: false }));
    const login = loginTerminals.get(id);
    loginTerminals.delete(id);
    if (login) {
      const acc = claudeAccounts.get(login) ?? orgAccount();
      claudeAccounts.set(login, { ...acc, source: "stored login", email: acc.own ? "personal@example.com" : acc.email });
    }
    if (m) touch(m);
  },
  async claudeAccount(machine) {
    const m = machines.find((x) => x.id === machine);
    if (!m) throw new Error("no such box");
    await sleep(300);
    return claudeAccounts.get(machine) ?? orgAccount();
  },
  async setClaudeAccount(machine, own) {
    await sleep(400);
    const acc: ClaudeAccount = own ? { own: true, source: null, email: null, organization: null } : orgAccount();
    claudeAccounts.set(machine, acc);
    touch(machine);
    return acc;
  },
  // The browser's own Notification API stands in for the OS notifications.
  async notifyPermission() {
    return "Notification" in window ? Notification.permission : "unsupported";
  },
  async requestNotifyPermission() {
    return "Notification" in window ? Notification.requestPermission() : "unsupported";
  },
  async showNotification(n) {
    if (!("Notification" in window) || Notification.permission !== "granted") return;
    const shown = new Notification(n.title, { body: n.body, tag: `${n.kind}:${n.agentId ?? n.machineId}` });
    shown.onclick = () => {
      for (const cb of clickListeners) cb({ agentId: n.agentId, machineId: n.machineId });
    };
  },
  async onNotificationClick(cb) {
    clickListeners.add(cb);
    return () => clickListeners.delete(cb);
  },
  async windowFocused() {
    return document.hasFocus();
  },
  async onWindowFocus(cb) {
    const on = () => cb(true);
    const off = () => cb(false);
    window.addEventListener("focus", on);
    window.addEventListener("blur", off);
    return () => {
      window.removeEventListener("focus", on);
      window.removeEventListener("blur", off);
    };
  },
  async focusWindow() {
    window.focus();
  },
  async setBadgeCount(n) {
    const nav = navigator as Navigator & { setAppBadge?: (n?: number) => Promise<void>; clearAppBadge?: () => Promise<void> };
    try {
      if (n > 0) await nav.setAppBadge?.(n);
      else await nav.clearAppBadge?.();
    } catch {
      /* not an installed app: browsers refuse badges, which is fine here */
    }
  },
  async setWindowTheme() {
    /* a browser tab has no window frame of its own */
  },
  async openUrl(url) {
    window.open(url, "_blank", "noopener");
  },
  async copyText(text) {
    await navigator.clipboard.writeText(text);
  },
};
