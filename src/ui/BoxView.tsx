import { For, Match, Show, Switch, createEffect, createResource, createSignal, lazy, on } from "solid-js";
import type { Machine, SetupRun, SetupStatus, StepState } from "../bridge/types";
import { api } from "../bridge/api";
import { actions, agentsForMachine, capsFor, multiProvider, phaseOf, providerLabel, setupActive, setupRunFor, state } from "../state/store";
import * as I from "../icons";
import { AgentDot, Dot, PowerActions } from "./common";
import { IDLE_STOP, agentPhaseLabel, gib, hours, money, relTime, sizeLabel, statusLabel } from "./format";
import { boxTerminalKey } from "./termSessions";

const Terminal = lazy(() => import("./Terminal"));
const ClaudeLogin = lazy(() => import("./ClaudeAccount"));
const Changes = lazy(() => import("./Changes"));
const Ports = lazy(() => import("./Ports"));

/** boxd: works with any key linked to the account, and "Open in editor" sets
 * up the `stowbox-<name>` alias. boat: its own CLI links a key on the way. */
const sshCommand = (m: Machine) =>
  m.provider === "boat" ? `boat ssh ${m.id}` : m.provider === "daytona" ? `daytona ssh ${m.id}` : m.provider === "agent37" ? `npx agent37 ssh ${m.id}` : m.sshPort ? `ssh -p ${m.sshPort} boxd@${m.name}.${m.accessDomain || "boxd.sh"}` : "";

/** "in 3 h 20 min" until a unix time, or "now" once it passed. */
const until = (at: number) => {
  const secs = at - Math.floor(Date.now() / 1000);
  if (secs <= 0) return "now";
  const h = Math.floor(secs / 3600);
  const min = Math.round((secs % 3600) / 60);
  return `in ${h ? `${h} h ` : ""}${min} min`;
};

/** The directories this box's agents work in, most recently active first. */
const repoDirs = (id: string) => [...new Set(agentsForMachine(id).sort((a, b) => b.updatedAt - a.updatedAt).map((a) => a.cwd ?? "~"))];
const boxRepoDir = (id: string) => {
  const d = repoDirs(id)[0];
  return d && d !== "~" ? d : null;
};

const AgentsTab = (p: { machine: Machine }) => {
  const agents = () => agentsForMachine(p.machine.id);
  return (
    <>
      <Show
        when={agents().length > 0}
        fallback={
          <div class="empty-state">
            <span>
              <strong>No agents in {p.machine.name} yet.</strong> An agent is a Claude Code or Codex session that runs inside this box; its files and shell state
              stay here between turns.
            </span>
            <button class="btn primary" onClick={() => actions.openDialog({ kind: "new-agent", machineId: p.machine.id })}>
              <I.Plus /> New agent in this box
            </button>
          </div>
        }
      >
        <div style={{ "margin-bottom": "8px" }}>
          <button class="btn sm" onClick={() => actions.openDialog({ kind: "new-agent", machineId: p.machine.id })}>
            <I.Plus /> New agent
          </button>
        </div>
        <div class="list">
          <For each={agents()}>
            {(a) => (
              <button class="list-row" onClick={() => actions.select({ kind: "agent", id: a.id })}>
                <AgentDot agent={a} />
                <span class="name">{a.title || "New agent"}</span>
                <span class="cell">{a.harness === "claude-code" ? "Claude Code" : "Codex"}</span>
                <span class="cell">
                  {agentPhaseLabel[phaseOf(a)]} · {relTime(a.updatedAt)}
                </span>
              </button>
            )}
          </For>
        </div>
      </Show>
      <ClaudeLogin machine={p.machine} />
    </>
  );
};

const setupStatusLabel: Record<SetupStatus, string> = {
  waiting: "Waiting for the box to boot",
  running: "Running",
  done: "Finished",
  failed: "Failed",
  cancelled: "Cancelled",
};
const setupSq: Record<SetupStatus, string> = { waiting: "pending", running: "busy", done: "ok", failed: "failed", cancelled: "idle" };
const stepSq: Record<StepState, string> = { pending: "idle", running: "busy", ok: "ok", failed: "failed", skipped: "idle" };

function duration(run: SetupRun): string {
  const ms = (run.finishedAt ?? Date.now()) - run.startedAt;
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${s % 60} s`;
}

const SetupTab = (p: { machine: Machine }) => {
  const run = () => setupRunFor(p.machine.id);
  const active = () => setupActive(run());
  const [presetId, setPresetId] = createSignal(run()?.presetId ?? state.presets[0]?.id ?? "");
  const chosen = () => (state.presets.some((x) => x.id === presetId()) ? presetId() : (state.presets[0]?.id ?? ""));
  let log: HTMLPreElement | undefined;
  // Follow the output while it grows, unless the person scrolled up.
  createEffect(
    on(
      () => run()?.output.length,
      () => {
        if (!log) return;
        const nearEnd = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
        if (nearEnd) queueMicrotask(() => log && (log.scrollTop = log.scrollHeight));
      },
    ),
  );

  return (
    <div class="setup">
      <div class="setup-bar">
        <Show
          when={state.presets.length > 0}
          fallback={
            <span class="muted">
              No setup presets yet.{" "}
              <button class="link" onClick={() => actions.select({ kind: "presets" })}>
                Write one
              </button>{" "}
              with the commands and secrets a new box needs.
            </span>
          }
        >
          <select class="input" aria-label="Setup preset" value={chosen()} onChange={(e) => setPresetId(e.currentTarget.value)} disabled={active()}>
            <For each={state.presets}>{(x) => <option value={x.id}>{x.name}</option>}</For>
          </select>
          <Show
            when={active()}
            fallback={
              <button class="btn" disabled={!chosen() || p.machine.status === "stopped"} title={p.machine.status === "stopped" ? "Start the box first" : "Run this preset in the box now"} onClick={() => actions.runPreset(p.machine.id, chosen())}>
                <I.Play /> {run() ? "Run again" : "Run preset"}
              </button>
            }
          >
            <button class="btn danger" onClick={() => void actions.cancelSetup(p.machine.id)}>
              <I.StopCircle /> Cancel
            </button>
          </Show>
        </Show>
      </div>

      <Show
        when={run()}
        fallback={
          <div class="empty-state">
            <span>
              <strong>No setup has run in {p.machine.name}.</strong> A setup preset exports its secrets in the box, then runs its commands in order: clone a repo,
              start containers, run a script. Pick one when creating a box, or run one here.
            </span>
          </div>
        }
      >
        {(r) => (
          <>
            <div class="setup-head">
              <span class="sq" data-state={setupSq[r().status]} />
              <strong>{r().presetName}</strong>
              <span class="muted">{setupStatusLabel[r().status]}</span>
              <span class="faint mono">
                {relTime(r().startedAt)} · {duration(r())}
              </span>
            </div>
            <Show when={r().error}>
              <div class="form-error">{r().error}</div>
            </Show>
            <Show when={(r().variables ?? []).length > 0}>
              <p class="note">
                With{" "}
                <For each={r().variables}>
                  {(v, i) => (
                    <>
                      {i() > 0 ? ", " : ""}
                      <span class="mono">
                        {v.name}={v.value}
                      </span>
                    </>
                  )}
                </For>
              </p>
            </Show>
            <Show when={r().secretNames.length > 0}>
              <p class="note">
                {r().secretsWritten ? "Exported" : "Will export"} <span class="mono">{r().secretNames.join(", ")}</span> from{" "}
                <span class="mono">~/.config/stowbox/env.sh</span>
              </p>
            </Show>
            <ol class="steps">
              <For each={r().steps}>
                {(st, i) => (
                  <li class="step" data-state={st.state}>
                    <span class="sq" data-state={stepSq[st.state]} />
                    <span class="step-n mono">{i() + 1}</span>
                    <span class="step-cmd mono" title={st.command}>
                      {st.command.split("\n")[0]}
                      {st.command.includes("\n") ? " …" : ""}
                    </span>
                    <span class="faint mono">
                      {st.state === "skipped" ? "skipped" : st.exitCode !== null ? `exit ${st.exitCode}` : st.state === "running" ? "running" : ""}
                    </span>
                  </li>
                )}
              </For>
            </ol>
            <pre class="setup-log mono scroll" ref={log} aria-label="Setup output" tabindex="0">
              {r().output || (active() ? "Waiting for output…" : "No output.")}
            </pre>
          </>
        )}
      </Show>
    </div>
  );
};

const DetailsTab = (p: { machine: Machine }) => {
  // Providers that cannot report billed usage are not asked.
  const [usage, { refetch }] = createResource(() => (capsFor(p.machine)?.meteredUsage ?? true) && p.machine.id, (id) => api.machineUsage(id));
  const m = () => p.machine;
  const timeout = (secs: number) => (secs === 0 ? "off" : secs % 3600 === 0 ? `${secs / 3600} h` : `${Math.round(secs / 60)} min`);
  const caps = () => capsFor(m());
  const label = () => providerLabel(m().provider);
  const canPause = () => caps()?.power.includes("pause") ?? true;
  const resourceMetered = () => (usage()?.rates.vcpuHourMicro ?? 0) > 0;
  return (
    <div class="details">
      <section>
        <h3 class="section-title">Machine</h3>
        <dl class="kv">
          <dt>Status</dt>
          <dd>
            <Dot status={m().status} /> {statusLabel[m().status]}
          </dd>
          <dt>Provider</dt>
          <dd>
            {label()}
            <Show when={m().size}>
              <span class="faint"> · {m().size}</span>
            </Show>
          </dd>
          <dt>Size</dt>
          <dd>
            {sizeLabel(m())} · {gib(m().diskBytes)} disk
          </dd>
          <dt>Estimated now</dt>
          <dd class="mono">{money(m().estHourlyMicro, m().currency)}/h in this state</dd>
          <Show when={m().url}>
            <dt>Address</dt>
            <dd>
              <span class="mono">{m().url.replace("https://", "")}</span>
              <button class="icon-btn" title="Open in the browser" aria-label="Open address" onClick={() => void actions.open(m().url)}>
                <I.External />
              </button>
            </dd>
          </Show>
          <dt>SSH</dt>
          <dd>
            <Show when={sshCommand(m())} fallback={<span class="faint">no SSH port from {label()} yet</span>}>
              <span class="mono">{sshCommand(m())}</span>
              <button class="icon-btn" title="Copy SSH command" aria-label="Copy SSH command" onClick={() => void actions.copy(sshCommand(m()), "SSH command copied")}>
                <I.Copy />
              </button>
              <Show when={caps()?.editorSsh ?? true}>
                <button class="icon-btn" title="Open in VS Code or Cursor" aria-label="Open in editor" onClick={() => actions.openDialog({ kind: "open-editor", machineId: m().id, dir: boxRepoDir(m().id) })}>
                  <I.Code />
                </button>
              </Show>
            </Show>
          </dd>
          <Show when={m().desktopUrl}>
            <dt>Desktop</dt>
            <dd>
              <button class="link" onClick={() => void actions.open(m().desktopUrl)}>
                {m().desktopUrl.replace(/^https?:\/\//, "")}
              </button>
            </dd>
          </Show>
          <Show when={caps()?.autoStop}>
            <dt>Auto-stop</dt>
            <dd>
              <Show when={m().autoStopAt > 0} fallback={<span>{m().status === "running" ? "off" : "only while it runs"}</span>}>
                <span>stops {until(m().autoStopAt)}</span>
              </Show>
              <Show when={m().status === "running"}>
                {" · "}
                <button class="link" disabled={!!state.pending[m().id]} onClick={() => void actions.setAutoStop(m().id, 4 * 3600)}>
                  4 h from now
                </button>
                {" · "}
                <button class="link" disabled={!!state.pending[m().id]} onClick={() => void actions.setAutoStop(m().id, 86_400)}>
                  1 day
                </button>
                <Show when={m().autoStopAt > 0}>
                  {" · "}
                  <button class="link" disabled={!!state.pending[m().id]} onClick={() => void actions.setAutoStop(m().id, 0)}>
                    never
                  </button>
                </Show>
              </Show>
            </dd>
          </Show>
          <Show when={caps()?.idleStop}>
            <dt>Auto-stop</dt>
            <dd>
              <span>{m().idleStopSecs > 0 ? `after ${timeout(m().idleStopSecs)} with no activity` : "off"}</span>
              <For each={IDLE_STOP.filter((o) => o.secs !== m().idleStopSecs)}>
                {(o) => (
                  <>
                    {" · "}
                    <button class="link" disabled={!!state.pending[m().id]} onClick={() => void actions.setAutoStop(m().id, o.secs)}>
                      {o.secs ? o.label : "never"}
                    </button>
                  </>
                )}
              </For>
            </dd>
          </Show>
          <Show when={caps()?.autoSuspend ?? true}>
            <dt>Auto standby</dt>
            <dd>{timeout(m().autoSuspendSecs)}</dd>
          </Show>
          <dt>{canPause() ? "Idle standby" : "Idle stop"}</dt>
          <dd>
            <Show
              when={state.guard.idleOn}
              fallback={
                <span>
                  off ·{" "}
                  <button class="link" onClick={() => actions.openDialog({ kind: "settings" })}>
                    Settings
                  </button>
                </span>
              }
            >
              <label class="check">
                <input type="checkbox" checked={!state.guard.optedOut.includes(m().id)} onChange={(e) => actions.setOptOut(m().id, !e.currentTarget.checked)} />
                <span>
                  Stowbox {canPause() ? "puts it in standby" : "stops it"} after {state.guard.idleMinutes} min with nothing happening
                </span>
              </label>
            </Show>
          </dd>
          <Show when={caps()?.autoHibernate ?? true}>
            <dt>Auto hibernate</dt>
            <dd>{timeout(m().autoHibernateSecs)}</dd>
          </Show>
          <dt>Source</dt>
          <dd>{m().source || "standalone"}</dd>
          <Show when={caps()?.isolated ?? true}>
            <dt>Isolated</dt>
            <dd>{m().isolated ? "yes: no in-VM CLI, no org integrations" : "no"}</dd>
          </Show>
          <dt>Created</dt>
          <dd>{m().createdAt ? new Date(m().createdAt * 1000).toLocaleString() : "unknown"}</dd>
          <Show when={m().lastConnectedMs > 0}>
            <dt>{caps()?.idleStop ? "Last activity" : "Last connected"}</dt>
            <dd>{relTime(m().lastConnectedMs)}</dd>
          </Show>
          <Show when={m().billingOrg || m().org}>
            <dt>Billed to</dt>
            <dd>{m().billingOrg || m().org}</dd>
          </Show>
        </dl>
        <Show
          when={caps()?.editorSsh ?? true}
          fallback={
            <p class="note">
              The SSH command needs the {label()} CLI, signed in. Stowbox opens its own SSH connection, so the Terminal tab needs nothing installed.
            </p>
          }
        >
          <p class="note">
            The SSH command works with a key linked to your {label()} account. Open in editor makes Stowbox's own key, links it and adds a{" "}
            <span class="mono">Host stowbox-{m().name}</span> entry to <span class="mono">~/.ssh/config</span>. The Terminal tab needs nothing installed.
          </p>
        </Show>
      </section>

      <section>
        <h3 class="section-title">Usage since creation</h3>
        <Switch>
          <Match when={!(caps()?.meteredUsage ?? true)}>
            <p class="note">
              {caps()?.meteringNote}{" "}
              <button class="link" onClick={() => void actions.open(caps()!.consoleUrl)}>
                Open the {label()} dashboard
              </button>
            </p>
          </Match>
          <Match when={usage.loading}>
            <div class="muted">Asking {label()} for the metered usage…</div>
          </Match>
          <Match when={usage.error}>
            <div class="empty-state">
              <span>Could not read usage: {String(usage.error)}</span>
              <button class="btn sm" onClick={() => void refetch()}>
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
                  <dt>{resourceMetered() ? "Running" : "Billable time"}</dt>
                  <dd>{hours(u().seconds.running)}</dd>
                  <Show when={resourceMetered()}>
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
                  </Show>
                </dl>
                <p class="note">{caps()?.meteringNote} The sidebar shows the live estimate.</p>
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
          <Dot status={m().status} />
          <h1>{m().name}</h1>
          <span class="tag">{statusLabel[m().status]}</span>
          <Show when={multiProvider()}>
            <span class="tag">{providerLabel(m().provider)}</span>
          </Show>
          <span class="meta mono">
            {sizeLabel(m())} · {money(m().estHourlyMicro, m().currency)}/h
          </span>
        </div>
        <div class="view-actions">
          <PowerActions machine={m()} ghost />
          <Show when={capsFor(m())?.fork ?? true}>
            <button class="btn ghost" title="Copy-on-write fork of this machine" disabled={!!state.pending[m().id]} onClick={() => void actions.forkBox(m())}>
              <I.Fork /> Fork
            </button>
          </Show>
          <Show when={m().desktopUrl}>
            <button class="btn ghost" title="Open the live desktop in the browser" onClick={() => void actions.open(m().desktopUrl)}>
              <I.Desktop /> Desktop
            </button>
          </Show>
          <Show when={capsFor(m())?.editorSsh ?? true}>
            <button class="btn ghost" title="Open in VS Code or Cursor over Remote-SSH" onClick={() => actions.openDialog({ kind: "open-editor", machineId: m().id, dir: boxRepoDir(m().id) })}>
              <I.Code /> Editor
            </button>
          </Show>
          <button class="btn ghost danger" title="Destroy the machine and its disk" disabled={!!state.pending[m().id]} onClick={() => actions.confirmDestroy(m())}>
            <I.Trash /> Delete
          </button>
        </div>
      </header>

      <nav class="subtabs" role="tablist" aria-label="Box sections">
        <button class="subtab" role="tab" aria-selected={tab() === "agents"} onClick={() => actions.setBoxTab("agents")}>
          Agents <span class="count">{agentCount()}</span>
        </button>
        <button class="subtab" role="tab" aria-selected={tab() === "setup"} onClick={() => actions.setBoxTab("setup")}>
          <Show when={setupRunFor(m().id)}>{(r) => <span class="sq" data-state={setupSq[r().status]} />}</Show>
          Setup
        </button>
        <button class="subtab" role="tab" aria-selected={tab() === "changes"} onClick={() => actions.setBoxTab("changes")}>
          Changes
        </button>
        <button class="subtab" role="tab" aria-selected={tab() === "ports"} onClick={() => actions.setBoxTab("ports")}>
          Ports
        </button>
        <button class="subtab" role="tab" aria-selected={tab() === "terminal"} onClick={() => actions.setBoxTab("terminal")}>
          Terminal
        </button>
        <button class="subtab" role="tab" aria-selected={tab() === "details"} onClick={() => actions.setBoxTab("details")}>
          Details & cost
        </button>
      </nav>

      <Switch>
        <Match when={tab() === "agents"}>
          <div class="view-body scroll">
            <AgentsTab machine={m()} />
          </div>
        </Match>
        <Match when={tab() === "setup"}>
          <div class="view-body scroll">
            <SetupTab machine={m()} />
          </div>
        </Match>
        <Match when={tab() === "changes"}>
          <div class="view-body scroll">
            <Show when={m().id} keyed>
              <Changes machine={m()} cwd={boxRepoDir(m().id)} editable suggestions={repoDirs(m().id)} />
            </Show>
          </div>
        </Match>
        <Match when={tab() === "ports"}>
          <div class="view-body scroll">
            <Show when={m().id} keyed>
              <Ports machine={m()} />
            </Show>
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
                    <div style={{ display: "flex", gap: "4px" }}>
                      <PowerActions machine={m()} />
                    </div>
                  </div>
                </div>
              }
            >
              <Show when={state.ui.boxTerminal?.machineId === m().id ? state.ui.boxTerminal : null} keyed fallback={
                  <Terminal
                    machine={m().id}
                    key={m().id}
                    persist={boxTerminalKey(m().id)}
                    hint="Connected. The shell keeps running while you look elsewhere; closing the box's tab ends it."
                  />
                }>
                {(t) => (
                  <div class="term-pane">
                    <div class="term-tools">
                      <span>Claude Code's sign-in. Open the link it prints, paste the code back here, then type /exit.</span>
                      <button class="btn sm ghost" onClick={() => actions.openBoxTerminal(m().id, null)}>
                        Open a shell instead
                      </button>
                    </div>
                    <Terminal machine={m().id} target={t.target} hint="The Agents tab shows which account signed in." />
                  </div>
                )}
              </Show>
            </Show>
          </div>
        </Match>
        <Match when={tab() === "details"}>
          <div class="view-body scroll">
            <DetailsTab machine={m()} />
          </div>
        </Match>
      </Switch>
    </section>
  );
}
