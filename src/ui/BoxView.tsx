import { For, Match, Show, Switch, createResource, lazy } from "solid-js";
import type { Machine } from "../bridge/types";
import { api } from "../bridge/api";
import { actions, agentsForMachine, state } from "../state/store";
import * as I from "../icons";
import { AgentSquare, PowerActions, StatusSquare } from "./common";
import { agentStatusLabel, gib, hours, money, relTime, sizeLabel, statusLabel, tokens, usd } from "./format";

const Terminal = lazy(() => import("./Terminal"));

function sshCommand(m: Machine) {
  return `ssh ${m.name}.boxd`;
}

const AgentsTab = (p: { machine: Machine }) => {
  const agents = () => agentsForMachine(p.machine.id);
  return (
    <Show
      when={agents().length > 0}
      fallback={
        <div class="empty-state">
          <span>
            <strong>No agents in {p.machine.name} yet.</strong> An agent is a Claude Code or Codex session that runs inside this box; its files and
            shell state stay here between turns.
          </span>
          <button class="btn primary" onClick={() => actions.openDialog({ kind: "new-agent", machineId: p.machine.id })}>
            <I.Chat /> New agent in this box
          </button>
        </div>
      }
    >
      <div class="actions" style={{ "margin-bottom": "10px" }}>
        <button class="btn" onClick={() => actions.openDialog({ kind: "new-agent", machineId: p.machine.id })}>
          <I.Plus /> New agent
        </button>
      </div>
      <div class="agent-table">
        <For each={agents()}>
          {(a) => (
            <button class="agent-line" onClick={() => actions.select({ kind: "agent", id: a.id })}>
              <AgentSquare status={a.status} />
              <span class="agent-title">{a.title || "New agent"}</span>
              <span class="cell">{a.harness === "claude-code" ? "Claude Code" : "Codex"}</span>
              <span class="cell mono">{a.totalCostUsd > 0 ? usd(a.totalCostUsd) : ""}</span>
              <span class="cell">{a.status.kind === "done" ? "Ready for you" : agentStatusLabel(a.status)} · {relTime(a.updatedAt)}</span>
            </button>
          )}
        </For>
      </div>
    </Show>
  );
};

const DetailsTab = (p: { machine: Machine }) => {
  const [usage, { refetch }] = createResource(() => p.machine.id, (id) => api.machineUsage(id));
  const m = () => p.machine;
  const timeout = (secs: number) => (secs === 0 ? "off" : secs % 3600 === 0 ? `${secs / 3600} h` : `${Math.round(secs / 60)} min`);
  return (
    <div class="details">
      <section>
        <h3>Machine</h3>
        <dl class="kv">
          <dt>Status</dt>
          <dd>
            <StatusSquare status={m().status} /> {statusLabel[m().status]}
          </dd>
          <dt>Size</dt>
          <dd>
            {sizeLabel(m())} · {gib(m().diskBytes)} disk
          </dd>
          <dt>Estimated now</dt>
          <dd class="mono">{money(m().estHourlyMicro, m().currency)}/h in this state</dd>
          <dt>Address</dt>
          <dd>
            <span class="mono">{m().url.replace("https://", "")}</span>
            <button class="icon-btn" title="Open in the browser" aria-label="Open address" onClick={() => void actions.open(m().url)}>
              <I.External />
            </button>
          </dd>
          <dt>SSH</dt>
          <dd>
            <span class="mono">{sshCommand(m())}</span>
            <button class="icon-btn" title="Copy SSH command" aria-label="Copy SSH command" onClick={() => void actions.copy(sshCommand(m()), "SSH command copied")}>
              <I.Copy />
            </button>
          </dd>
          <dt>Desktop</dt>
          <dd>
            <button class="link" onClick={() => void actions.open(m().desktopUrl)}>
              app.boxd.sh/desktop/{m().name}
            </button>
          </dd>
          <dt>Auto standby</dt>
          <dd>{timeout(m().autoSuspendSecs)}</dd>
          <dt>Auto hibernate</dt>
          <dd>{timeout(m().autoHibernateSecs)}</dd>
          <dt>Source</dt>
          <dd>{m().source || "standalone"}</dd>
          <dt>Isolated</dt>
          <dd>{m().isolated ? "yes: no in-VM CLI, no org integrations" : "no"}</dd>
          <dt>Created</dt>
          <dd>{m().createdAt ? new Date(m().createdAt * 1000).toLocaleString() : "unknown"}</dd>
          <dt>Last connected</dt>
          <dd>{relTime(m().lastConnectedMs)}</dd>
          <Show when={m().billingOrg || m().org}>
            <dt>Billed to</dt>
            <dd>{m().billingOrg || m().org}</dd>
          </Show>
        </dl>
        <p class="note">
          The SSH alias works once the boxd CLI has written its block to <span class="mono">~/.ssh/config</span> (any <span class="mono">boxd connect</span> does
          that). The Terminal tab needs nothing installed.
        </p>
      </section>

      <section>
        <h3>Usage since creation</h3>
        <Switch>
          <Match when={usage.loading}>
            <div class="muted">Asking boxd for the metered usage…</div>
          </Match>
          <Match when={usage.error}>
            <div class="empty-state">
              <span>Could not read usage: {String(usage.error)}</span>
              <button class="btn" onClick={() => void refetch()}>
                <I.Refresh /> Try again
              </button>
            </div>
          </Match>
          <Match when={usage()}>
            {(u) => (
              <>
                <dl class="kv">
                  <dt>Metered cost</dt>
                  <dd class="mono">
                    {money(u().costMicro, u().currency)}
                    <span class="faint">{u().complete ? "" : " · still accruing"}</span>
                  </dd>
                  <dt>Running</dt>
                  <dd>{hours(u().seconds.running)}</dd>
                  <dt>Standby</dt>
                  <dd>{hours(u().seconds.standby)}</dd>
                  <dt>Hibernated</dt>
                  <dd>{hours(u().seconds.hibernated)}</dd>
                  <dt>Stopped</dt>
                  <dd>{hours(u().seconds.stopped)}</dd>
                  <dt>vCPU hours</dt>
                  <dd class="mono">{u().vcpuHours.toFixed(2)}</dd>
                  <dt>RAM GiB hours</dt>
                  <dd class="mono">{u().ramGibHours.toFixed(1)}</dd>
                  <dt>Disk GiB hours</dt>
                  <dd class="mono">{u().diskGibHours.toFixed(0)}</dd>
                  <dt>Rates</dt>
                  <dd class="mono">
                    {money(u().rates.vcpuHourMicro, u().currency)} vCPU·h · {money(u().rates.ramGibHourMicro, u().currency)} GiB·h · {money(u().rates.diskGibHourMicro, u().currency)} disk GiB·h
                  </dd>
                </dl>
                <p class="note">boxd meters with a lag of up to about 25 minutes, so the last half hour may be missing here. The sidebar shows the live estimate.</p>
              </>
            )}
          </Match>
        </Switch>
      </section>
    </div>
  );
};

export default function BoxView(p: { machine: Machine }) {
  const m = () => p.machine;
  const tab = () => state.ui.boxTab;
  const agentCount = () => agentsForMachine(m().id).length;
  return (
    <section class="view">
      <header class="view-head">
        <div class="view-title">
          <StatusSquare status={m().status} />
          <h1>{m().name}</h1>
          <span class="pill">{statusLabel[m().status]}</span>
          <span class="meta mono">
            {sizeLabel(m())} · {money(m().estHourlyMicro, m().currency)}/h
          </span>
        </div>
        <div class="actions">
          <PowerActions machine={m()} />
          <button class="btn" title="Copy-on-write fork of this machine, under 200 ms" disabled={!!state.pending[m().id]} onClick={() => void actions.forkBox(m())}>
            <I.Fork /> Fork
          </button>
          <button class="btn" title="Open the live desktop in the browser" onClick={() => void actions.open(m().desktopUrl)}>
            <I.Desktop /> Desktop
          </button>
          <button class="btn danger" title="Destroy the machine and its disk" disabled={!!state.pending[m().id]} onClick={() => actions.confirmDestroy(m())}>
            <I.Trash /> Delete
          </button>
        </div>
      </header>

      <nav class="tabs" role="tablist" aria-label="Box sections">
        <button class="tab" role="tab" aria-selected={tab() === "agents"} onClick={() => actions.setBoxTab("agents")}>
          Agents <span class="count">{agentCount()}</span>
        </button>
        <button class="tab" role="tab" aria-selected={tab() === "terminal"} onClick={() => actions.setBoxTab("terminal")}>
          Terminal
        </button>
        <button class="tab" role="tab" aria-selected={tab() === "details"} onClick={() => actions.setBoxTab("details")}>
          Details & cost
        </button>
      </nav>

      <Switch>
        <Match when={tab() === "agents"}>
          <div class="view-body">
            <AgentsTab machine={m()} />
          </div>
        </Match>
        <Match when={tab() === "terminal"}>
          <div class="view-body flush">
            <Show
              when={m().status !== "stopped" && m().status !== "failed"}
              fallback={
                <div class="view-body">
                  <div class="empty-state">
                    <span>
                      <strong>{m().name} is {statusLabel[m().status].toLowerCase()}.</strong> A shell needs the machine running; standby and hibernated boxes wake on
                      their own when a terminal opens, stopped ones do not.
                    </span>
                    <PowerActions machine={m()} />
                  </div>
                </div>
              }
            >
              <Terminal machine={m().id} key={m().id} />
            </Show>
          </div>
        </Match>
        <Match when={tab() === "details"}>
          <div class="view-body">
            <DetailsTab machine={m()} />
          </div>
        </Match>
      </Switch>
    </section>
  );
}

export { tokens };
