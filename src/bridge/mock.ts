// Browser-only stand-in for the desktop shell. Used to develop and screenshot
// the UI with `pnpm dev` outside Tauri. Every value here is a placeholder.

import type { Account, AgentEvent, AgentMeta, AgentPush, Api, Machine, TermEvent } from "./types";

const GIB = 1024 ** 3;

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
    sshPort: 0,
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
    estHourlyMicro: Math.round(est),
    currency: "eur",
    ...partial,
  };
}

let machines: Machine[] = [
  machine({ id: "vm_mock_1", name: "mock-api", status: "running" }),
  machine({ id: "vm_mock_2", name: "mock-web", status: "suspended", vcpu: 4, memoryBytes: 16 * GIB }),
  machine({ id: "vm_mock_3", name: "mock-scratch", status: "hibernated", vcpu: 1, memoryBytes: 4 * GIB }),
  machine({ id: "vm_mock_4", name: "mock-old", status: "stopped" }),
];

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
  ag_3: [
    { type: "user_message", text: "Migrate the landing page to the new design tokens" },
    { type: "stderr", text: "claude: command not found" },
    { type: "exit", code: 1 },
  ],
};

const listeners = new Set<(p: AgentPush) => void>();
function emit(agentId: string, event: AgentEvent | null) {
  const meta = agents.find((a) => a.id === agentId);
  if (!meta) return; // the agent was removed while a simulated turn was running
  for (const l of listeners) l({ agentId, event, meta });
}
function setStatus(id: string, status: AgentMeta["status"]) {
  agents = agents.map((a) => (a.id === id ? { ...a, status, updatedAt: Date.now() } : a));
}
function record(id: string, ev: AgentEvent) {
  if (ev.type !== "text_delta") (transcripts[id] ??= []).push(ev);
  emit(id, ev);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function simulateTurn(id: string, text: string) {
  record(id, { type: "user_message", text });
  setStatus(id, { kind: "starting" });
  emit(id, null);
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
  record(id, { type: "tool_result", tool_use_id: "x1", content: " M src/app.ts", is_error: false });
  setStatus(id, { kind: "thinking" });
  await sleep(700);
  record(id, { type: "assistant", blocks: [{ type: "text", text: "One modified file. Tell me whether to commit it or keep going." }] });
  agents = agents.map((a) => (a.id === id ? { ...a, totalCostUsd: a.totalCostUsd + 0.0031, turns: a.turns + 2, status: { kind: "done" } } : a));
  record(id, { type: "result", ok: true, subtype: "success", message: null, cost_usd: 0.0031, duration_ms: 3200, turns: 2, session_id: "mock-session", input_tokens: 1200, output_tokens: 80 });
}

export const mockApi: Api = {
  async providerStatus() {
    // `?onboarding` previews the first-run screen.
    const configured = !window.location.search.includes("onboarding");
    return { configured, provider: "boxd", secretBackend: "file", consoleUrl: "https://app.boxd.sh" };
  },
  async setApiKey(key) {
    if (!key.startsWith("bxd_")) throw new Error("That does not look like a boxd API key. Keys start with bxd_.");
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
  async createMachine(spec) {
    await sleep(800);
    const m = machine({ id: "vm_mock_" + Date.now(), name: spec.name || "mock-new", status: "running", vcpu: spec.vcpu ?? 2, memoryBytes: spec.memoryBytes ?? 8 * GIB, isolated: spec.isolated });
    machines = [...machines, m];
    return m;
  },
  async forkMachine(source, name) {
    await sleep(300);
    const src = await mockApi.getMachine(source);
    const m = machine({ ...src, id: "vm_mock_" + Date.now(), name: name || src.name + "-fork", status: "running", source: "fork/" + src.name });
    machines = [...machines, m];
    return m;
  },
  async machinePower(id, action) {
    await sleep(400);
    const next: Record<string, Machine["status"]> = { start: "running", stop: "stopped", reboot: "running", pause: "suspended", resume: "running", hibernate: "hibernated", wake: "running" };
    if (action === "destroy") {
      machines = machines.filter((m) => m.id !== id);
      return;
    }
    machines = machines.map((m) => (m.id === id ? machine({ ...m, status: next[action] }) : m));
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
  async rates() {
    return { currency: "eur", rates: { vcpuHourMicro: 39_000, ramGibHourMicro: 12_000, diskGibHourMicro: 80 } };
  },
  async orgBilling() {
    return { id: "org_mock", name: "mock-org", isAdmin: true, currency: "eur", mode: "credits", balanceMicro: 24_310_000, accruingMicro: 410_000, vmCount: machines.length, maxVms: 50 };
  },
  async listAgents() {
    return [...agents].sort((a, b) => b.updatedAt - a.updatedAt);
  },
  async createAgent(spec) {
    const meta: AgentMeta = {
      id: "ag_" + Date.now(),
      title: spec.title ?? "",
      harness: spec.harness,
      machineId: spec.machineId,
      machineName: spec.machineName,
      cwd: spec.cwd ?? null,
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
    return transcripts[id] ?? [];
  },
  async agentSend(id, text) {
    const a = agents.find((a) => a.id === id);
    if (!a) throw new Error("agent not found");
    if (!a.title) agents = agents.map((x) => (x.id === id ? { ...x, title: text.slice(0, 64) } : x));
    void simulateTurn(id, text);
  },
  async agentStop(id) {
    setStatus(id, { kind: "stopped" });
    record(id, { type: "exit", code: -1 });
  },
  async deleteAgent(id) {
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
  async terminalOpen(_machine, _cols, _rows, onEvent) {
    const enc = new TextEncoder();
    const send = (s: string) => onEvent({ type: "data", b64: btoa(String.fromCharCode(...enc.encode(s))) } as TermEvent);
    setTimeout(() => send("mock terminal: the desktop build runs a real shell inside the box.\r\n$ "), 100);
    return "term_mock";
  },
  async terminalWrite() {},
  async terminalResize() {},
  async terminalClose() {},
  async openUrl(url) {
    window.open(url, "_blank", "noopener");
  },
  async copyText(text) {
    await navigator.clipboard.writeText(text);
  },
};
