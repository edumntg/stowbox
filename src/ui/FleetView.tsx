import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import type { AgentMeta, Machine, MachineMetrics } from "../bridge/types";
import { actions, agentWorking, agentsForMachine, capsFor, connectedProviders, machineById, multiProvider, phaseOf, providerLabel, spendCurrency, state, type Selection } from "../state/store";
import * as I from "../icons";
import { Dot, phaseDot } from "./common";
import Composer from "./Composer";
import { agentPhaseLabel, gib, money, moneyByCurrency, pct, relTime, sizeLabel, spendLine, statusLabel, span } from "./format";

/** Something a person should look at. Derived from state on every render;
 * nothing is stored, so an alert disappears the moment its cause does. */
interface Alert {
  key: string;
  /** "info" is something Stowbox did on its own, not a problem. */
  level: "warn" | "danger" | "info";
  title: string;
  body: string;
  sinceMs: number;
  /** What the time line says before the age; "First seen" by default. */
  sinceLabel?: string;
  go: Selection | "settings";
}

const IDLE_AFTER_MS = 30 * 60_000;
const UNUSED_AFTER_MS = 2 * 3_600_000;
const MEM_WARN = 0.9;
const DISK_WARN = 0.1;
/** A box with an agent at work this close to its auto-stop gets an alert. */
const AUTO_STOP_WARN_MS = 20 * 60_000;

/** When the agent's hooks last wrote its state, else when a terminal last opened it. */
const waitingSince = (a: AgentMeta) => (state.live[a.id]?.since ? state.live[a.id].since * 1000 : a.updatedAt);

function deriveAlerts(now: number): Alert[] {
  const out: Alert[] = [];
  for (const e of state.providerErrors) {
    out.push({ key: `provider-${e.provider}`, level: "danger", title: `Could not list the ${providerLabel(e.provider)} boxes`, body: `${e.message}. Its boxes are missing below until it answers again.`, sinceMs: 0, go: "settings" });
  }
  for (const m of state.machines) {
    const agents = agentsForMachine(m.id);
    const go: Selection = { kind: "box", id: m.id };
    const caps = capsFor(m);
    if (m.status === "failed") {
      const fix = caps?.power.includes("reboot") ? "A reboot usually clears it." : "Start it again, or delete it.";
      out.push({ key: `box-failed-${m.id}`, level: "danger", title: `${m.name} failed`, body: `${providerLabel(m.provider)} reports the machine as failed. ${fix}`, sinceMs: 0, go });
    }
    if (m.status === "running" && m.autoStopAt > 0 && m.autoStopAt * 1000 - now < AUTO_STOP_WARN_MS && agents.some(agentWorking)) {
      out.push({
        key: `auto-stop-${m.id}`,
        level: "warn",
        title: `${m.name} stops on its own ${relTime(m.autoStopAt * 1000, now).replace(" ago", "")}`,
        body: "An agent is working in it. Its auto-stop ends every process in the box; push it out from the box's Details.",
        sinceMs: 0,
        go,
      });
    }
    if (m.status === "running") {
      const busy = agents.some(agentWorking);
      const waiting = agents.filter((a) => phaseOf(a) === "waiting" && now - waitingSince(a) > IDLE_AFTER_MS);
      if (!busy && waiting.length > 0) {
        const oldest = waiting.reduce((x, y) => (waitingSince(x) < waitingSince(y) ? x : y));
        out.push({
          key: `box-idle-${m.id}`,
          level: "warn",
          title: `${m.name} is running with nothing to do`,
          body: `${oldest.title || "An agent"} has waited for you since ${relTime(waitingSince(oldest), now)}; the box bills ${money(m.estHourlyMicro, m.currency)}/h until it ${caps?.power.includes("pause") ? "goes to standby" : "stops"}.`,
          sinceMs: waitingSince(oldest),
          go,
        });
      } else if (agents.length === 0 && m.lastConnectedMs && now - m.lastConnectedMs > UNUSED_AFTER_MS) {
        out.push({
          key: `box-unused-${m.id}`,
          level: "warn",
          title: `${m.name} is running unused`,
          body: `No agents and no connection for ${relTime(m.lastConnectedMs, now).replace(" ago", "")}, at ${money(m.estHourlyMicro, m.currency)}/h.`,
          sinceMs: m.lastConnectedMs,
          go,
        });
      }
      const s = state.metrics[m.id];
      if (s) {
        const mem = s.memUsedBytes / s.memTotalBytes;
        if (mem >= MEM_WARN) {
          out.push({ key: `mem-${m.id}`, level: "warn", title: `${m.name} memory at ${pct(mem * 100)}`, body: `${gib(s.memUsedBytes)} of ${gib(s.memTotalBytes)} in use as of ${relTime(s.sampledAtMs, now)}.`, sinceMs: 0, go });
        }
        if (s.diskAvailBytes / s.diskTotalBytes < DISK_WARN) {
          out.push({ key: `disk-${m.id}`, level: "danger", title: `${m.name} disk nearly full`, body: `${gib(s.diskAvailBytes)} free of ${gib(s.diskTotalBytes)}.`, sinceMs: 0, go });
        }
      }
      const err = state.metricsError[m.id];
      if (err) {
        out.push({ key: `sample-${m.id}`, level: "warn", title: `Could not sample ${m.name}`, body: err, sinceMs: 0, go });
      }
    }
  }
  const spend = state.guard.spendOn ? state.guard.spend : null;
  if (spend?.over) {
    out.push({
      key: "spend",
      level: "warn",
      title: spend.basis === "metered" ? `Today's spend passed ${money(spend.limitMicro, spend.currency)}` : `A day at the current rate passes ${money(spend.limitMicro, spend.currency)}`,
      body: spendLine(spend),
      sinceMs: 0,
      go: "settings",
    });
  }
  // The guard's own actions, while the box it put to sleep still sleeps.
  for (const g of state.guard.log) {
    const m = machineById(g.machineId);
    if (!m || m.status !== "suspended" || now - g.atMs > 24 * 3_600_000) continue;
    out.push({
      key: `guard-${g.machineId}`,
      level: "info",
      title: `${g.machineName} was put in standby`,
      body: `Nothing happened in it for ${Math.round((g.atMs - g.idleSinceMs) / 60_000)} min. Estimate ${money(g.hourlyBeforeMicro, g.currency)}/h before, ${money(g.hourlyAfterMicro, g.currency)}/h in standby.`,
      sinceMs: g.atMs,
      sinceLabel: "Idle standby",
      go: { kind: "box", id: g.machineId },
    });
  }
  const rank = { danger: 0, warn: 1, info: 2 };
  return out.sort((a, b) => rank[a.level] - rank[b.level] || a.title.localeCompare(b.title));
}

const level = (ratio: number, warn = 0.7, danger = 0.9) => (ratio >= danger ? "danger" : ratio >= warn ? "warn" : "");

/** A number over a thin bar. The bar carries colour only past the
 * thresholds, like the state dots. */
const Meter = (p: { label: string; ratio: number; warn?: number; danger?: number; stale?: boolean }) => (
  <span class="meter-cell" classList={{ stale: p.stale }}>
    <span class="mono">{p.label}</span>
    <span class="meter" role="presentation">
      <span class="meter-fill" data-level={level(p.ratio, p.warn, p.danger)} style={{ width: `${Math.max(1, Math.min(100, p.ratio * 100))}%` }} />
    </span>
  </span>
);

const Tile = (p: { label: string; value: string; sub: string; ratio?: number; dot?: string }) => (
  <div class="tile">
    <span class="tile-label">{p.label}</span>
    <span class="tile-value">
      <Show when={p.dot}>
        <span class="sq" data-state={p.dot} />
      </Show>
      {p.value}
    </span>
    <Show when={p.ratio !== undefined}>
      <span class="meter" role="presentation">
        <span class="meter-fill" data-level="" style={{ width: `${Math.max(0, Math.min(100, (p.ratio ?? 0) * 100))}%` }} />
      </span>
    </Show>
    <span class="tile-sub">{p.sub}</span>
  </div>
);

const AgentStrip = (p: { agents: AgentMeta[] }) => {
  const shown = () => p.agents.slice(0, 8);
  const summary = () => {
    const count = (phase: string) => p.agents.filter((a) => phaseOf(a) === phase).length;
    const working = count("working");
    const waiting = count("waiting");
    const running = count("running");
    const parts: string[] = [];
    if (working) parts.push(`${working} working`);
    if (waiting) parts.push(`${waiting} waiting`);
    if (running) parts.push(`${running} running`);
    const rest = p.agents.length - working - waiting - running;
    if (rest) parts.push(`${rest} not running`);
    return parts.join(" · ");
  };
  return (
    <Show when={p.agents.length > 0} fallback={<span class="faint">none</span>}>
      <span class="strip" aria-hidden="true">
        <For each={shown()}>{(a) => <span class="sq" data-state={phaseDot[phaseOf(a)]} title={a.title || "New agent"} />}</For>
        <Show when={p.agents.length > 8}>
          <span class="faint mono">+{p.agents.length - 8}</span>
        </Show>
      </span>
      <span class="mono">{summary()}</span>
    </Show>
  );
};

const Row = (p: { machine: Machine; now: number }) => {
  const m = () => p.machine;
  const agents = () => agentsForMachine(m().id);
  const sample = (): MachineMetrics | undefined => state.metrics[m().id];
  const stale = () => m().status !== "running";
  const memRatio = () => (sample() ? sample()!.memUsedBytes / sample()!.memTotalBytes : 0);
  const diskRatio = () => (sample() ? sample()!.diskUsedBytes / sample()!.diskTotalBytes : 0);
  return (
    <button class="frow" classList={{ stale: stale() }} onClick={() => actions.select({ kind: "box", id: m().id })} title={`Open ${m().name}`}>
      <span class="fcell box">
        <Dot status={m().status} />
        <span class="fname">
          <span class="name">{m().name}</span>
          <span class="mono faint">
            {sizeLabel(m())}
            <Show when={multiProvider()}> · {providerLabel(m().provider)}</Show>
          </span>
          <Show when={stale() && sample()}>
            <span class="asof">
              {statusLabel[m().status]} · as of {relTime(sample()!.sampledAtMs, p.now)}
            </span>
          </Show>
        </span>
      </span>
      <span class="fcell agents">
        <AgentStrip agents={agents()} />
      </span>
      <Show
        when={sample()}
        fallback={
          <>
            <span class="fcell faint">{stale() ? "not sampled" : state.metricsError[m().id] ? "sample failed" : state.metricsBusy ? "sampling…" : "—"}</span>
            <span class="fcell faint">—</span>
            <span class="fcell faint">—</span>
          </>
        }
      >
        {(s) => (
          <>
            <span class="fcell">
              <Meter label={pct(s().cpuPercent)} ratio={s().cpuPercent / 100} stale={stale()} />
            </span>
            <span class="fcell">
              <Meter label={pct(memRatio() * 100)} ratio={memRatio()} stale={stale()} />
            </span>
            <span class="fcell">
              <Meter label={gib(s().diskAvailBytes)} ratio={diskRatio()} warn={0.8} danger={0.9} stale={stale()} />
            </span>
          </>
        )}
      </Show>
      <span class="fcell mono" classList={{ faint: stale() }} title={m().createdAt ? `Created ${new Date(m().createdAt * 1000).toLocaleString()}` : `${providerLabel(m().provider)} did not say when`}>
        {m().createdAt ? span(Math.max(0, p.now / 1000 - m().createdAt)) : "—"}
      </span>
      <span class="fcell mono">{money(m().estHourlyMicro, m().currency)}/h</span>
      <span class="fcell chev">
        <I.ChevronRight />
      </span>
    </button>
  );
};

const AgentLine = (p: { agent: AgentMeta; now: number; open?: boolean }) => (
  <button class="aline" onClick={() => actions.select({ kind: "agent", id: p.agent.id })}>
    <span class="sq" data-state={phaseDot[phaseOf(p.agent)]} />
    <span class="name">{p.agent.title || "New agent"}</span>
    <span class="faint">{p.agent.machineName}</span>
    <span class="aline-right mono">
      {agentPhaseLabel[phaseOf(p.agent)].toLowerCase()} · {relTime(waitingSince(p.agent), p.now)}
    </span>
    <Show when={p.open}>
      <span class="aline-go">
        Reply <I.ChevronRight />
      </span>
    </Show>
  </button>
);

const AlertCard = (p: { alert: Alert; now: number }) => (
  <button class="alert" data-level={p.alert.level} onClick={() => (p.alert.go === "settings" ? actions.openDialog({ kind: "settings" }) : actions.select(p.alert.go))}>
    <Show when={p.alert.level === "info"} fallback={<I.Warning />}>
      <I.Moon />
    </Show>
    <span class="alert-body">
      <span class="alert-title">{p.alert.title}</span>
      <span>{p.alert.body}</span>
      <Show when={p.alert.sinceMs > 0}>
        <span class="faint">
          {p.alert.sinceLabel ?? "First seen"} {relTime(p.alert.sinceMs, p.now)}
        </span>
      </Show>
    </span>
  </button>
);

const statusOrder: Record<Machine["status"], number> = {
  running: 0,
  starting: 1,
  pending: 1,
  migrating: 1,
  stopping: 4,
  suspended: 2,
  hibernated: 3,
  failed: 4,
  stopped: 5,
  unknown: 6,
};

export default function FleetView() {
  const [now, setNow] = createSignal(Date.now());
  const tick = window.setInterval(() => setNow(Date.now()), 5_000);
  onCleanup(() => window.clearInterval(tick));

  const running = () => state.machines.filter((m) => m.status === "running");
  const billing = () => state.machines.filter((m) => m.status === "running" || m.status === "suspended");
  const working = () => state.agents.filter(agentWorking);
  const waiting = () => state.agents.filter((a) => phaseOf(a) === "waiting");
  /** The hourly estimate in the main currency, big; any other currency goes in the line under it. */
  const hourlyIn = (currency: string) => billing().filter((m) => m.currency === currency).reduce((sum, m) => sum + m.estHourlyMicro, 0);
  const others = () => [...new Set(billing().map((m) => m.currency))].filter((c) => c !== spendCurrency());
  const costSub = () => {
    const parts: string[] = [];
    const rest = others().map((c) => ({ micro: hourlyIn(c), currency: c }));
    if (rest.length) parts.push(`+ ${moneyByCurrency(rest, spendCurrency(), "/h")}`);
    const spend = state.guard.spend;
    if (spend) parts.push(`today ${money(spend.amountMicro, spend.currency)}${spend.basis === "metered" ? "" : " projected"}`);
    parts.push(`${billing().length} running or standby`);
    return parts.join(" · ");
  };
  /** What needs a person: agents waiting for a reply, then the problems. */
  const needsCount = () => waiting().length + alerts().filter((a) => a.level !== "info").length;
  const accountNames = () =>
    connectedProviders()
      .map((p) => state.accounts[p.id] && (multiProvider() ? `${p.capabilities.label} ${state.accounts[p.id].displayName}` : state.accounts[p.id].displayName))
      .filter(Boolean)
      .join(", ");
  const alerts = createMemo(() => deriveAlerts(now()));
  const rows = createMemo(() => [...state.machines].sort((a, b) => statusOrder[a.status] - statusOrder[b.status] || a.name.localeCompare(b.name)));
  const sampled = () => {
    if (state.metricsBusy) return "Sampling…";
    if (!state.metricsAt) return "";
    if (running().length === 0) return "Nothing running to sample";
    return `Sampled ${relTime(state.metricsAt, now())}`;
  };
  const check = () => {
    void actions.refreshMachines().then(() => actions.refreshMetrics());
  };

  return (
    <section class="view fleet">
      <header class="view-head">
        <div class="view-title">
          <I.Layers />
          <h1>Fleet</h1>
          <span class="meta">
            {state.machines.length} {state.machines.length === 1 ? "box" : "boxes"} · {state.agents.length} {state.agents.length === 1 ? "agent" : "agents"}
            <Show when={accountNames()}> · {accountNames()}</Show>
          </span>
        </div>
        <div class="view-actions">
          <span class="meta muted" style={{ "font-size": "var(--text-2xs)" }}>
            {sampled()}
          </span>
          <button class="btn ghost" onClick={check} disabled={state.metricsBusy} title="Re-read the boxes and sample the running ones">
            <I.Refresh /> Check now
          </button>
        </div>
      </header>

      <div class="view-body scroll">
        <Show when={connectedProviders().length > 0}>
          <Composer />
        </Show>
        <Show
          when={state.machinesLoaded && state.machines.length > 0}
          fallback={
            <div class="fleet-empty">
              <Show when={state.machinesLoaded} fallback={<span>Reading your boxes…</span>}>
                <span>
                  No boxes in this account yet. Describe a task above and Stowbox makes one, or{" "}
                  <button class="link" onClick={() => actions.openDialog({ kind: "new-box" })}>
                    make a box
                  </button>{" "}
                  with every option.
                </span>
              </Show>
            </div>
          }
        >
          <div class="tiles">
            <Tile label="Needs you" value={String(needsCount())} sub={needsCount() ? "replies and problems below" : "nothing right now"} dot={needsCount() ? "waiting" : undefined} />
            <Tile label="Working" value={String(working().length)} sub={`of ${state.agents.length} agents`} dot={working().length ? "busy" : undefined} />
            <Tile label="Running" value={String(running().length)} sub={`of ${state.machines.length} boxes`} ratio={state.machines.length ? running().length / state.machines.length : 0} dot={running().length ? "running" : undefined} />
            <Tile label="Cost now" value={`${money(hourlyIn(spendCurrency()), spendCurrency())}/h`} sub={costSub()} />
          </div>

          <section class="fleet-section">
            <h3 class="fleet-h">
              Needs you <span class="count">{needsCount()}</span>
            </h3>
            <Show
              when={waiting().length > 0 || alerts().length > 0}
              fallback={
                <div class="fleet-empty">
                  Nothing needs you. Agents waiting for a reply, boxes billing with nothing to do, boxes near their memory or disk limit and your daily spend
                  warning show up here.
                </div>
              }
            >
              <Show when={waiting().length > 0}>
                <div class="alines">
                  <For each={waiting()}>{(a) => <AgentLine agent={a} now={now()} open />}</For>
                </div>
              </Show>
              <Show when={alerts().length > 0}>
                <div class="alerts">
                  <For each={alerts()}>{(a) => <AlertCard alert={a} now={now()} />}</For>
                </div>
              </Show>
            </Show>
          </section>

          <section class="fleet-section">
            <h3 class="fleet-h">
              Working now <span class="count">{working().length}</span>
            </h3>
            <Show when={working().length > 0} fallback={<div class="fleet-empty">No agent is working right now.</div>}>
              <div class="alines">
                <For each={working()}>{(a) => <AgentLine agent={a} now={now()} />}</For>
              </div>
            </Show>
          </section>

          <div class="fleet-table-head">
            <h3 class="fleet-h">
              Boxes <span class="count">{state.machines.length}</span>
            </h3>
            <span class="legend" aria-label="Agent states">
              <span>
                <span class="sq" data-state="busy" /> Working or running
              </span>
              <span>
                <span class="sq" data-state="waiting" /> Waiting
              </span>
              <span>
                <span class="sq" data-state="idle" /> Not running or not checked
              </span>
            </span>
          </div>
          <div class="ftable" role="table" aria-label="Boxes">
            <div class="fhead" role="row">
              <span>Box</span>
              <span>Agents</span>
              <span>CPU</span>
              <span>Memory</span>
              <span>Disk free</span>
              <span title="Time since the box was created">Age</span>
              <span>Est.</span>
              <span />
            </div>
            <For each={rows()}>{(m) => <Row machine={m} now={now()} />}</For>
          </div>
          <p class="note">
            CPU, memory and disk are read inside each running box over a half-second window. Standby, hibernated and stopped boxes are not touched, since a sample
            would wake them; their last reading stays with its age.
          </p>
        </Show>
      </div>
    </section>
  );
}
