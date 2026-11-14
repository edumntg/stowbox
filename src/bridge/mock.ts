// Browser-only stand-in for the desktop shell. Used to develop and screenshot
// the UI with `pnpm dev` outside Tauri. Every value here is a placeholder.

import type {
  Account,
  AgentEvent,
  AgentMeta,
  Attachment,
  AgentPush,
  ClaudeAccount,
  Api,
  GitDiffFile,
  GitFileChange,
  GitReview,
  GuardVerdict,
  HttpsRoute,
  IdlePolicy,
  ListeningPort,
  Machine,
  MachineMetrics,
  Notice,
  NoticeTarget,
  PortForward,
  Preset,
  SearchHit,
  SearchResults,
  SetupPush,
  SshPlan,
  SetupRun,
  TermEvent,
  VariableValues,
} from "./types";
import { substitute, variablesIn } from "./variables";

const GIB = 1024 ** 3;
let nextSshPort = 21843;

function machine(partial: Partial<Machine> & Pick<Machine, "id" | "name" | "status">): Machine {
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
];

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

let agents: AgentMeta[] = [
  {
    id: "ag_1",
    title: "Add a /health endpoint and a test for it",
    harness: "claude-code",
    machineId: "vm_mock_1",
    machineName: "mock-api",
    cwd: "/home/user/api",
    model: null,
    sessionId: "mock-session-1",
    status: { kind: "done" },
    skipPermissions: true,
    createdAt: Date.now() - 1_800_000,
    updatedAt: Date.now() - 600_000,
    totalCostUsd: 0.0412,
    turns: 6,
    inputTokens: 18_204,
    outputTokens: 1_311,
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
    status: { kind: "tool", name: "command" },
    skipPermissions: true,
    createdAt: Date.now() - 300_000,
    updatedAt: Date.now() - 2_000,
    totalCostUsd: 0,
    turns: 0,
    inputTokens: 0,
    outputTokens: 0,
    queue: [
      {
        id: "q_seed_1",
        text: "Once you know why, open an issue draft with the failing log lines.",
        attachments: [{ name: "nightly-arm64.log", path: "/home/boxd/.stowbox/uploads/ag_2/3f9a1c-nightly-arm64.log", size: 48_213 }],
        queuedAt: Date.now() - 60_000,
      },
    ],
  },
  {
    id: "ag_3",
    title: "Migrate the landing page to the new design tokens",
    harness: "claude-code",
    machineId: "vm_mock_2",
    machineName: "mock-web",
    cwd: null,
    model: null,
    sessionId: "mock-session-3",
    status: { kind: "failed", message: "the agent process exited with code 1 before reporting a result" },
    skipPermissions: true,
    createdAt: Date.now() - 7_200_000,
    updatedAt: Date.now() - 7_000_000,
    totalCostUsd: 0.0021,
    turns: 1,
    inputTokens: 900,
    outputTokens: 40,
  },
  {
    // A turn left running when the app closed, on a box that went to standby
    // since: the app waits for the box instead of waking it.
    id: "ag_4",
    title: "Upgrade the image pipeline to sharp 0.34",
    harness: "claude-code",
    machineId: "vm_mock_2",
    machineName: "mock-web",
    cwd: "/home/user/web",
    model: null,
    sessionId: "mock-session-4",
    status: { kind: "reconnecting", message: "the box is in standby; the turn continues when it runs again" },
    skipPermissions: true,
    createdAt: Date.now() - 5_400_000,
    updatedAt: Date.now() - 3_000_000,
    totalCostUsd: 0,
    turns: 0,
    inputTokens: 0,
    outputTokens: 0,
  },
];

const transcripts: Record<string, AgentEvent[]> = {
  ag_1: [
    { type: "user_message", text: "Add a /health endpoint and a test for it" },
    { type: "init", session_id: "mock-session-1", model: "claude-sonnet", cwd: "/home/user/api" },
    { type: "assistant", blocks: [{ type: "text", text: "I'll look at how routes are registered first." }] },
    { type: "assistant", blocks: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "rg -n \"router.get\" src | head" } }] },
    { type: "tool_result", tool_use_id: "t1", content: "src/routes.ts:12:router.get('/version', version)\nsrc/routes.ts:13:router.get('/me', me)", is_error: false },
    { type: "assistant", blocks: [{ type: "tool_use", id: "t2", name: "Edit", input: { file_path: "src/routes.ts" } }] },
    { type: "tool_result", tool_use_id: "t2", content: "The file src/routes.ts has been updated.", is_error: false },
    { type: "assistant", blocks: [{ type: "tool_use", id: "t3", name: "Bash", input: { command: "npm test -- health" } }] },
    { type: "tool_result", tool_use_id: "t3", content: "PASS test/health.test.ts\n  ✓ returns 200 and {ok:true} (18 ms)\n\nTests: 1 passed, 1 total", is_error: false },
    {
      type: "assistant",
      blocks: [
        {
          type: "text",
          text: "Done. `GET /health` returns `{ ok: true }` and `test/health.test.ts` covers it.\n\n```ts\nrouter.get('/health', (_req, res) => res.json({ ok: true }));\n```\n\nNothing else in the router needed changes.",
        },
      ],
    },
    { type: "result", ok: true, subtype: "success", message: "Done.", cost_usd: 0.0412, duration_ms: 48_210, turns: 6, session_id: "mock-session-1", input_tokens: 18_204, output_tokens: 1_311 },
  ],
  ag_4: [{ type: "user_message", text: "Upgrade the image pipeline to sharp 0.34 and fix whatever breaks" }],
  ag_3: [
    { type: "user_message", text: "Migrate the landing page to the new design tokens" },
    { type: "stderr", text: "claude: command not found" },
    { type: "exit", code: 1 },
  ],
};

/** The same rules as stowbox_core::search, over the mock transcripts. */
function eventTexts(ev: AgentEvent): [SearchHit["kind"], string][] {
  const strings = (v: unknown): string[] =>
    typeof v === "string" ? [v] : Array.isArray(v) ? v.flatMap(strings) : v && typeof v === "object" ? Object.values(v).flatMap(strings) : [];
  switch (ev.type) {
    case "user_message":
      return [["user", ev.text]];
    case "assistant":
      return ev.blocks.map((b): [SearchHit["kind"], string] =>
        b.type === "text" ? ["assistant", b.text] : b.type === "thinking" ? ["thinking", b.text] : ["tool-call", [b.name, ...strings(b.input)].join(" ")],
      );
    case "tool_result":
      return [["tool-output", ev.content]];
    case "result":
      return !ev.ok && ev.message ? [["error", ev.message]] : [];
    case "stderr":
      return [["note", ev.text]];
    default:
      return [];
  }
}

function mockSnippet(text: string, start: number, end: number): Pick<SearchHit, "before" | "matched" | "after"> {
  const collapse = (s: string) => s.split(/\s+/).filter(Boolean).join(" ");
  const head = text.slice(0, start);
  let before = head.length > 60 ? "…" + collapse(head.slice(-60)) : collapse(head);
  if (/\s$/.test(head) && before) before += " ";
  const matched = collapse(text.slice(start, end));
  const tail = text.slice(end);
  const room = Math.max(0, 180 - before.length - matched.length);
  const rest = tail.trimStart();
  const after = (/^\s/.test(tail) && rest ? " " : "") + (rest.length > room ? collapse(rest.slice(0, room)) + "…" : collapse(rest));
  return { before, matched, after };
}

function searchMock(query: string, limit: number): SearchResults {
  const q = query.trim().slice(0, 200).toLowerCase();
  const out: SearchResults = { hits: [], truncated: false, scanned: 0 };
  if (!q) return out;
  for (const a of [...agents].sort((x, y) => y.updatedAt - x.updatedAt)) {
    const events = transcripts[a.id];
    if (!events) continue;
    out.scanned++;
    let turn = 0;
    const found: SearchHit[] = [];
    events.forEach((ev, i) => {
      if (ev.type === "user_message") turn++;
      for (const [kind, text] of eventTexts(ev)) {
        const at = text.toLowerCase().indexOf(q);
        if (at < 0) continue;
        found.push({ agentId: a.id, event: i, turn, kind, ...mockSnippet(text, at, at + q.length) });
        break;
      }
    });
    if (found.length > 5) out.truncated = true;
    const latest = found.reverse().slice(0, 5);
    const room = limit - out.hits.length;
    if (latest.length > room) out.truncated = true;
    out.hits.push(...latest.slice(0, room));
    if (out.hits.length >= limit) break;
  }
  return out;
}

const listeners = new Set<(p: AgentPush) => void>();
// The same rule as stowbox_core::notify: an outcome notifies when the status
// seen just before it was busy, so one turn notifies once.
const lastStatus = new Map(agents.map((a) => [a.id, a.status.kind]));
const busyKinds = new Set(["starting", "thinking", "tool", "reconnecting"]);
function agentNotice(meta: AgentMeta): Notice | null {
  const prev = lastStatus.get(meta.id);
  lastStatus.set(meta.id, meta.status.kind);
  if (!prev || !busyKinds.has(prev)) return null;
  const name = meta.title.trim() || (meta.harness === "codex" ? "Codex agent" : "Claude Code agent");
  const base = { agentId: meta.id, machineId: meta.machineId };
  if (meta.status.kind === "done") return { ...base, kind: "agent-done", title: `${name} finished`, body: `Waiting for you in ${meta.machineName}` };
  if (meta.status.kind === "failed") return { ...base, kind: "agent-failed", title: `${name} failed`, body: `${meta.machineName}: ${meta.status.message}` };
  return null;
}
function emit(agentId: string, event: AgentEvent | null) {
  const meta = agents.find((a) => a.id === agentId);
  if (!meta) return; // the agent was removed while a simulated turn was running
  const notice = agentNotice(meta);
  for (const l of listeners) l({ agentId, event, meta, notice });
}
function setStatus(id: string, status: AgentMeta["status"]) {
  agents = agents.map((a) => (a.id === id ? { ...a, status, updatedAt: Date.now() } : a));
}
function record(id: string, ev: AgentEvent) {
  if (ev.type !== "text_delta") (transcripts[id] ??= []).push(ev);
  emit(id, ev);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const stopped = (id: string) => agents.find((a) => a.id === id)?.status.kind === "stopped";

/** Mimics the follower losing the stream and picking the turn up again. */
async function simulateReconnect(id: string, resume: AgentMeta["status"]) {
  setStatus(id, { kind: "reconnecting", message: "the stream dropped (connection reset by peer)" });
  emit(id, null);
  await sleep(2500);
  if (stopped(id)) return false;
  setStatus(id, resume);
  emit(id, null);
  return true;
}

/** The shell's rule (AgentMeta::queue_ready): after a turn really ends,
 * the next queued message goes out, unless Stop paused the queue.
 * `resume` is the person asking for it after a Stop. */
function drainQueue(id: string, resume = false) {
  const a = agents.find((x) => x.id === id);
  if (!a || !a.queue?.length || busyKinds.has(a.status.kind)) return;
  if (!resume && (a.queuePaused || a.status.kind === "stopped")) return;
  const [next, ...rest] = a.queue;
  agents = agents.map((x) => (x.id === id ? { ...x, queue: rest } : x));
  void simulateTurn(id, next.text, next.attachments);
}

function setQueue(id: string, f: (a: AgentMeta) => Partial<AgentMeta>): AgentMeta {
  const a = agents.find((x) => x.id === id);
  if (!a) throw new Error("agent not found");
  agents = agents.map((x) => (x.id === id ? { ...x, ...f(x), updatedAt: Date.now() } : x));
  emit(id, null);
  return agents.find((x) => x.id === id)!;
}

const UPLOAD_MAX = 20 * 1024 * 1024;
const mb = (n: number) => (n / (1024 * 1024)).toFixed(1) + " MB";

async function simulateTurn(id: string, text: string, attachments: Attachment[] = []) {
  setStatus(id, { kind: "starting" });
  record(id, { type: "user_message", text, ...(attachments.length ? { attachments } : {}) });
  await sleep(600);
  record(id, { type: "init", session_id: "mock-session", model: "claude-sonnet", cwd: null });
  setStatus(id, { kind: "thinking" });
  const reply = "Looking into it. I'll check the current state before changing anything.";
  for (const word of reply.split(" ")) {
    await sleep(60);
    record(id, { type: "text_delta", text: word + " " });
  }
  record(id, { type: "assistant", blocks: [{ type: "text", text: reply }] });
  setStatus(id, { kind: "tool", name: "Bash" });
  record(id, { type: "assistant", blocks: [{ type: "tool_use", id: "x1", name: "Bash", input: { command: "git status --short" } }] });
  await sleep(900);
  if (stopped(id) || !(await simulateReconnect(id, { kind: "tool", name: "Bash" }))) return;
  record(id, { type: "tool_result", tool_use_id: "x1", content: " M src/app.ts", is_error: false });
  dirtyWorktree(id);
  setStatus(id, { kind: "thinking" });
  await sleep(700);
  record(id, { type: "assistant", blocks: [{ type: "text", text: "One modified file. Tell me whether to commit it or keep going." }] });
  agents = agents.map((a) => (a.id === id ? { ...a, totalCostUsd: a.totalCostUsd + 0.0031, turns: a.turns + 2, status: { kind: "done" } } : a));
  record(id, { type: "result", ok: true, subtype: "success", message: null, cost_usd: 0.0031, duration_ms: 3200, turns: 2, session_id: "mock-session", input_tokens: 1200, output_tokens: 80 });
  await sleep(300);
  drainQueue(id);
}

/** A box came back: turns that waited for it are followed again. */
async function resumeWaitingTurns(machineId: string) {
  const waiting = agents.filter((a) => a.machineId === machineId && a.status.kind === "reconnecting");
  for (const a of waiting) {
    setStatus(a.id, { kind: "reconnecting", message: "following the turn again" });
    emit(a.id, null);
  }
  await sleep(1200);
  for (const a of waiting) {
    if (stopped(a.id)) continue;
    setStatus(a.id, { kind: "thinking" });
    record(a.id, { type: "init", session_id: a.sessionId ?? "mock-session", model: "claude-sonnet", cwd: a.cwd });
    await sleep(800);
    if (stopped(a.id)) continue;
    record(a.id, { type: "assistant", blocks: [{ type: "text", text: "sharp is on 0.34 now; the resize tests pass." }] });
    agents = agents.map((x) => (x.id === a.id ? { ...x, turns: x.turns + 1, totalCostUsd: x.totalCostUsd + 0.0112, status: { kind: "done" } } : x));
    record(a.id, { type: "result", ok: true, subtype: "success", message: null, cost_usd: 0.0112, duration_ms: 812_000, turns: 1, session_id: a.sessionId, input_tokens: 4100, output_tokens: 220 });
    drainQueue(a.id);
  }
}

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
/** Agent terminals echo what is typed, like a prompt would. */
const echoing = new Set<string>();
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
  const queueGoesOn = (a: AgentMeta) => !!a.queue?.length && !a.queuePaused && a.status.kind !== "stopped";
  if (mine.some((a) => busyKinds.has(a.status.kind) || queueGoesOn(a))) return { kind: "keep", reason: "agent-working" };
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
    const configured = !window.location.search.includes("onboarding");
    return { configured, provider: "boxd", secretBackend: "file", consoleUrl: "https://app.boxd.sh" };
  },
  async setApiKey(key) {
    // The same shape check, and message, as the desktop shell's set_api_key.
    const k = key.trim();
    if (!k.startsWith("bxd_") || k.length < 20) throw new Error("That does not look like a boxd API key. Keys start with bxd_ and come from app.boxd.sh or `boxd auth keys create`.");
    return account;
  },
  async clearApiKey() {},
  async whoami() {
    return account;
  },
  async listMachines() {
    await sleep(150);
    return machines;
  },
  async getMachine(id) {
    const m = machines.find((m) => m.id === id || m.name === id);
    if (!m) throw new Error("not found: " + id);
    return m;
  },
  async createMachine(spec, presetId, variables) {
    if (presetId) prepareMockSetup(presetId, variables ?? {});
    await sleep(800);
    const m = machine({ id: "vm_mock_" + Date.now(), createdAt: Math.floor(Date.now() / 1000), name: spec.name || "mock-new", status: "running", vcpu: spec.vcpu ?? 2, memoryBytes: spec.memoryBytes ?? 8 * GIB, isolated: spec.isolated });
    machines = [...machines, m];
    if (presetId) startMockSetup(m, presetId, variables ?? {});
    return m;
  },
  async forkMachine(source, name) {
    await sleep(300);
    const src = await mockApi.getMachine(source);
    const m = machine({ ...src, id: "vm_mock_" + Date.now(), createdAt: Math.floor(Date.now() / 1000), name: name || src.name + "-fork", status: "running", source: "fork/" + src.name });
    machines = [...machines, m];
    return m;
  },
  async machinePower(id, action) {
    await sleep(400);
    const next: Record<string, Machine["status"]> = { start: "running", stop: "stopped", reboot: "running", pause: "suspended", resume: "running", hibernate: "hibernated", wake: "running" };
    if (action === "destroy") {
      machines = machines.filter((m) => m.id !== id);
      claudeAccounts.delete(id);
      // Like the app: agents that lived there stay as history, stopped.
      agents = agents.map((a) => (a.machineId === id ? { ...a, status: ["starting", "thinking", "tool", "reconnecting"].includes(a.status.kind) ? { kind: "stopped" } : a.status } : a));
      return;
    }
    machines = machines.map((m) => (m.id === id ? machine({ ...m, status: next[action] }) : m));
    if (next[action] === "running") void resumeWaitingTurns(id);
  },
  async machineUsage(id) {
    const m = await mockApi.getMachine(id);
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
  async rates() {
    return { currency: "eur", rates: { vcpuHourMicro: 39_000, ramGibHourMicro: 12_000, diskGibHourMicro: 80 } };
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
    const suspended = verdict.kind === "suspend";
    if (suspended) await mockApi.machinePower(m.id, "pause");
    const after = suspended ? machine({ ...m, status: "suspended" }).estHourlyMicro : m.estHourlyMicro;
    return { verdict, suspended, machineName: m.name, hourlyBeforeMicro: m.estHourlyMicro, hourlyAfterMicro: after, currency: m.currency };
  },
  async guardTouch(machineId) {
    touch(machineId);
  },
  // `?spend=projected` previews a key that cannot read boxd's meter.
  async spendToday(_since, limitMicro) {
    await sleep(200);
    const amount = (over: number) => ({ amountMicro: over, limitMicro, currency: "eur", over: limitMicro > 0 && over >= limitMicro });
    if (window.location.search.includes("spend=projected")) {
      const hourlyMicro = machines.reduce((s, m) => s + m.estHourlyMicro, 0);
      return { basis: "projected", hourlyMicro, why: "permission denied: usage is visible to org owners and admins", ...amount(hourlyMicro * 24) };
    }
    return { basis: "metered", totalMicro: 1_840_000, complete: false, ...amount(1_840_000) };
  },
  async orgBilling() {
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
      sessionId: null,
      status: { kind: "idle" },
      skipPermissions: spec.skipPermissions,
      createdAt: Date.now(),
      updatedAt: Date.now(),
      totalCostUsd: 0,
      turns: 0,
      inputTokens: 0,
      outputTokens: 0,
    };
    agents = [meta, ...agents];
    return meta;
  },
  async agentEvents(id) {
    // A copy: the store keeps what it gets and pushes live events onto it,
    // so handing out this array made every event show up twice.
    return [...(transcripts[id] ?? [])];
  },
  async searchTranscripts(query, limit) {
    await sleep(120);
    return searchMock(query, Math.min(Math.max(limit ?? 50, 1), 200));
  },
  async agentSend(id, text, attachments) {
    const a = agents.find((a) => a.id === id);
    if (!a) throw new Error("agent not found");
    if (busyKinds.has(a.status.kind)) throw new Error("this agent is still working on the previous message");
    const title = text.trim() || attachments?.[0]?.name || "";
    if (!a.title) agents = agents.map((x) => (x.id === id ? { ...x, title: title.slice(0, 64) } : x));
    void simulateTurn(id, text, attachments ?? []);
  },
  async agentQueueAdd(id, text, attachments) {
    if (!text.trim() && !attachments?.length) throw new Error("write something for the agent first");
    const a = agents.find((x) => x.id === id);
    if ((a?.queue?.length ?? 0) >= 20) throw new Error("the queue holds 20 messages; wait for some to be sent or remove one");
    const msg = { id: "q_" + Math.random().toString(16).slice(2, 10), text: text.trimEnd(), attachments: attachments ?? [], queuedAt: Date.now() };
    const meta = setQueue(id, (x) => ({ queue: [...(x.queue ?? []), msg] }));
    drainQueue(id);
    return agents.find((x) => x.id === id) ?? meta;
  },
  async agentQueueEdit(id, messageId, text) {
    const a = agents.find((x) => x.id === id);
    const m = a?.queue?.find((q) => q.id === messageId);
    if (!m) throw new Error("not found: that message was already sent");
    const remove = !text.trim() && !m.attachments.length;
    return setQueue(id, (x) => ({ queue: remove ? x.queue!.filter((q) => q.id !== messageId) : x.queue!.map((q) => (q.id === messageId ? { ...q, text: text.trimEnd() } : q)) }));
  },
  async agentQueueRemove(id, messageId) {
    return setQueue(id, (x) => {
      const queue = (x.queue ?? []).filter((q) => q.id !== messageId);
      return { queue, queuePaused: queue.length ? x.queuePaused : false };
    });
  },
  async agentQueueClear(id) {
    return setQueue(id, () => ({ queue: [], queuePaused: false }));
  },
  async agentQueueResume(id) {
    setQueue(id, () => ({ queuePaused: false }));
    drainQueue(id, true);
  },
  async agentUpload(id, name, bytes) {
    const a = agents.find((x) => x.id === id);
    if (!a) throw new Error("agent not found");
    if (bytes.length === 0) throw new Error(`${name} is empty`);
    if (bytes.length > UPLOAD_MAX) throw new Error(`${name} is ${mb(bytes.length)}; attachments are limited to 20 MB each`);
    await sleep(400 + Math.min(1600, bytes.length / 4000));
    const clean = (name.split(/[\\/]/).pop() ?? "").replace(/[^A-Za-z0-9._-]+/g, "_").replace(/^[._]+/, "") || "file";
    const tag = Math.random().toString(16).slice(2, 8);
    return { name, path: `/home/boxd/.stowbox/uploads/${id}/${tag}-${clean}`, size: bytes.length };
  },
  async agentStop(id) {
    const a = agents.find((x) => x.id === id);
    agents = agents.map((x) => (x.id === id ? { ...x, queuePaused: !!a?.queue?.length } : x));
    setStatus(id, { kind: "stopped" });
    record(id, { type: "exit", code: -1 });
  },
  async deleteAgent(id, removeWorktree, force) {
    const a = agents.find((x) => x.id === id);
    const wt = a?.worktree;
    if (a && wt && removeWorktree) {
      if (a.status.kind === "starting" || a.status.kind === "thinking" || a.status.kind === "tool" || a.status.kind === "reconnecting")
        throw new Error("stop the agent's turn before removing its worktree");
      await sleep(500);
      const key = `${a.machineId}:${wt.path}`;
      if (repos[key]?.review.files.length && !force) throw new Error(`fatal: '${wt.path}' contains modified or untracked files, use --force to delete it`);
      delete repos[key];
    }
    agents = agents.filter((a) => a.id !== id);
    delete transcripts[id];
  },
  async renameAgent(id, title) {
    agents = agents.map((a) => (a.id === id ? { ...a, title } : a));
    return agents.find((a) => a.id === id)!;
  },
  async onAgentEvent(cb) {
    listeners.add(cb);
    return () => listeners.delete(cb);
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
      if (["starting", "thinking", "tool", "reconnecting"].includes(a.status.kind)) throw new Error("a turn is running in the chat; wait for it to end or stop it first");
      const cmd = a.harness === "claude-code" ? `claude${a.sessionId ? ` --resume ${a.sessionId}` : ""}` : `codex${a.sessionId ? ` resume ${a.sessionId}` : ""}`;
      setTimeout(() => send(`mock terminal: the desktop build runs ${cmd} inside the box.\r\n\r\n> ${target.input ? `${target.input}\r\n> ` : ""}`), 100);
      echoing.add(id);
    } else if (target.kind === "claude-login") {
      setTimeout(() => send("mock terminal: the desktop build runs claude /login inside the box.\r\nLeave this tab to pretend the sign-in worked.\r\n"), 100);
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
    if (echoing.has(id)) terminalEvents.get(id)?.(new TextDecoder().decode(bytes).replace(/\r/g, "\r\n> ").replace(/\x7f/g, "\b \b"));
  },
  async terminalResize() {},
  async terminalClose(id) {
    const m = openTerminals.get(id);
    openTerminals.delete(id);
    terminalEvents.delete(id);
    echoing.delete(id);
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
