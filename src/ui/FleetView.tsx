import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import type { AgentMeta, Machine, MachineMetrics } from "../bridge/types";
import { actions, agentsForMachine, state, type Selection } from "../state/store";
import * as I from "../icons";
import { Dot, agentDotState } from "./common";
import { agentStatusLabel, gib, isBusy, money, pct, relTime, sizeLabel, statusLabel, uptime } from "./format";

/** Something a person should look at. Derived from state on every render;
 * nothing is stored, so an alert disappears the moment its cause does. */
interface Alert {
  key: string;
  level: "warn" | "danger";
  title: string;
  body: string;
  sinceMs: number;
  go: Selection;
}

const IDLE_AFTER_MS = 30 * 60_000;
const UNUSED_AFTER_MS = 2 * 3_600_000;
const MEM_WARN = 0.9;
const DISK_WARN = 0.1;

function deriveAlerts(now: number): Alert[] {
  const out: Alert[] = [];
  for (const m of state.machines) {
    const agents = agentsForMachine(m.id);
    const go: Selection = { kind: "box", id: m.id };
    if (m.status === "failed") {
      out.push({ key: `box-failed-${m.id}`, level: "danger", title: `${m.name} failed`, body: "boxd reports the machine as failed. A reboot usually clears it.", sinceMs: 0, go });
    }
    for (const a of agents) {
      if (a.status.kind === "failed") {
        out.push({
          key: `agent-failed-${a.id}`,
          level: "danger",
          title: `${a.title || "An agent"} failed in ${m.name}`,
          body: a.status.message,
          sinceMs: a.updatedAt,
          go: { kind: "agent", id: a.id },
        });
      }
    }
    if (m.status === "running") {
      const busy = agents.some((a) => isBusy(a.status));
      const waiting = agents.filter((a) => a.status.kind === "done" && now - a.updatedAt > IDLE_AFTER_MS);
      if (!busy && waiting.length > 0) {
        const oldest = waiting.reduce((x, y) => (x.updatedAt < y.updatedAt ? x : y));
        out.push({
          key: `box-idle-${m.id}`,
          level: "warn",
          title: `${m.name} is running with nothing to do`,
          body: `${oldest.title || "An agent"} finished ${relTime(oldest.updatedAt, now)}; the box bills ${money(m.estHourlyMicro, m.currency)}/h until it goes to standby.`,
          sinceMs: oldest.updatedAt,
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
  const rank = { danger: 0, warn: 1 };
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
    const working = p.agents.filter((a) => isBusy(a.status)).length;
    const waiting = p.agents.filter((a) => a.status.kind === "done").length;
    const failed = p.agents.filter((a) => a.status.kind === "failed").length;
    const parts: string[] = [];
    if (working) parts.push(`${working} working`);
    if (waiting) parts.push(`${waiting} waiting`);
    if (failed) parts.push(`${failed} failed`);
    const rest = p.agents.length - working - waiting - failed;
    if (rest) parts.push(`${rest} idle`);
    return parts.join(" · ");
  };
  return (
    <Show when={p.agents.length > 0} fallback={<span class="faint">none</span>}>
      <span class="strip" aria-hidden="true">
        <For each={shown()}>{(a) => <span class="sq" data-state={agentDotState(a.status)} title={a.title || "New agent"} />}</For>
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
          <span class="mono faint">{sizeLabel(m())}</span>
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
      <span class="fcell mono" classList={{ faint: stale() }}>
        {stale() ? statusLabel[m().status] : sample() ? uptime(sample()!.uptimeSecs) : "—"}
      </span>
      <span class="fcell mono">{money(m().estHourlyMicro, m().currency)}/h</span>
      <span class="fcell chev">
        <I.ChevronRight />
      </span>
    </button>
  );
};

const AgentLine = (p: { agent: AgentMeta; now: number }) => (
  <button class="aline" onClick={() => actions.select({ kind: "agent", id: p.agent.id })}>
    <span class="sq" data-state={agentDotState(p.agent.status)} />
    <span class="name">{p.agent.title || "New agent"}</span>
    <span class="faint">{p.agent.machineName}</span>
    <span class="aline-right mono">
      {p.agent.status.kind === "done" ? "finished" : agentStatusLabel(p.agent.status).toLowerCase()} · {relTime(p.agent.updatedAt, p.now)}
    </span>
  </button>
);

const statusOrder: Record<Machine["status"], number> = {
  running: 0,
  starting: 1,
  pending: 1,
  migrating: 1,
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
  const working = () => state.agents.filter((a) => isBusy(a.status));
  const waiting = () => state.agents.filter((a) => a.status.kind === "done");
  const hourly = () => billing().reduce((s, m) => s + m.estHourlyMicro, 0);
  const currency = () => state.machines[0]?.currency ?? state.account?.billing.currency ?? "usd";
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
            <Show when={state.account}> · {state.account!.displayName}</Show>
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
        <Show
          when={state.machinesLoaded && state.machines.length > 0}
          fallback={
            <div class="empty-state">
              <Show when={!state.machinesLoaded} fallback={<span>No boxes in this account yet. The fleet view fills in as soon as one exists.</span>}>
                <span>Reading your boxes from boxd…</span>
              </Show>
              <Show when={state.machinesLoaded}>
                <button class="btn primary" onClick={() => actions.openDialog({ kind: "new-box" })}>
                  <I.Plus /> New box
                </button>
              </Show>
            </div>
          }
        >
          <div class="tiles">
            <Tile label="Running" value={String(running().length)} sub={`of ${state.machines.length} boxes`} ratio={state.machines.length ? running().length / state.machines.length : 0} dot="running" />
            <Tile label="Working" value={String(working().length)} sub={`of ${state.agents.length} agents`} dot="busy" />
            <Tile label="Waiting for you" value={String(waiting().length)} sub="finished turns to read" dot="done" />
            <Tile label="Estimated" value={`${money(hourly(), currency())}/h`} sub={`${billing().length} running or standby`} />
          </div>

          <h3 class="fleet-h">
            Alerts <span class="count">{alerts().length}</span>
          </h3>
          <Show when={alerts().length > 0} fallback={<div class="fleet-empty">Nothing needs you. Failed agents, boxes billing with nothing to do, and boxes near their memory or disk limit show up here.</div>}>
            <div class="alerts">
              <For each={alerts()}>
                {(a) => (
                  <button class="alert" data-level={a.level} onClick={() => actions.select(a.go)}>
                    <I.Warning />
                    <span class="alert-body">
                      <span class="alert-title">{a.title}</span>
                      <span>{a.body}</span>
                      <Show when={a.sinceMs > 0}>
                        <span class="faint">First seen {relTime(a.sinceMs, now())}</span>
                      </Show>
                    </span>
                  </button>
                )}
              </For>
            </div>
          </Show>

          <div class="fleet-cols">
            <div>
              <h3 class="fleet-h">
                Working now <span class="count">{working().length}</span>
              </h3>
              <Show when={working().length > 0} fallback={<div class="fleet-empty">No agent is working right now.</div>}>
                <div class="alines">
                  <For each={working()}>{(a) => <AgentLine agent={a} now={now()} />}</For>
                </div>
              </Show>
            </div>
            <div>
              <h3 class="fleet-h">
                Waiting for you <span class="count">{waiting().length}</span>
              </h3>
              <Show when={waiting().length > 0} fallback={<div class="fleet-empty">No finished turn is waiting to be read.</div>}>
                <div class="alines">
                  <For each={waiting()}>{(a) => <AgentLine agent={a} now={now()} />}</For>
                </div>
              </Show>
            </div>
          </div>

          <div class="fleet-table-head">
            <h3 class="fleet-h">
              Boxes <span class="count">{state.machines.length}</span>
            </h3>
            <span class="legend" aria-label="Agent states">
              <span>
                <span class="sq" data-state="busy" /> Working
              </span>
              <span>
                <span class="sq" data-state="done" /> Waiting
              </span>
              <span>
                <span class="sq" data-state="idle" /> Idle
              </span>
              <span>
                <span class="sq" data-state="failed" /> Failed
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
              <span>Uptime</span>
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
