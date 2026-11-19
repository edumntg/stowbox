import { Show } from "solid-js";
import type { AgentMeta } from "../bridge/types";
import { actions, capsFor, machineById, state } from "../state/store";
import * as I from "../icons";
import { BoxState, Dot, PowerActions } from "./common";
import { money, sizeLabel, until } from "./format";

/** The box behind the selected agent: state, live cost and quick controls,
 * so pausing or opening a shell never means leaving the agent. */
export default function Inspector(p: { agent: AgentMeta }) {
  const m = () => machineById(p.agent.machineId);
  return (
    <aside class="panel" aria-label="Box panel">
      <div class="panel-toolbar">
        <Show when={m()}>
          {(box) => (
            <>
              <button class="icon-btn" title="Open a terminal in the box" aria-label="Terminal" onClick={() => actions.openBoxTab(box().id, "terminal")}>
                <I.Terminal />
              </button>
              <Show when={box().desktopUrl}>
                <button class="icon-btn" title="Open the live desktop" aria-label="Desktop" onClick={() => void actions.open(box().desktopUrl)}>
                  <I.Desktop />
                </button>
              </Show>
              <Show when={capsFor(box())?.editorSsh ?? true}>
                <button class="icon-btn" title="Open this agent's directory in VS Code or Cursor" aria-label="Open in editor" onClick={() => actions.openDialog({ kind: "open-editor", machineId: box().id, dir: p.agent.cwd ?? null })}>
                  <I.Code />
                </button>
              </Show>
              <button class="icon-btn" title="Cost and details" aria-label="Details and cost" onClick={() => actions.openBoxTab(box().id, "details")}>
                <I.Coins />
              </button>
            </>
          )}
        </Show>
        <span class="spacer" />
        <button class="icon-btn" title="Hide box panel" aria-label="Hide box panel" onClick={() => actions.toggleInspector()}>
          <I.Close />
        </button>
      </div>
      <div class="panel-body scroll">
        <Show when={m()} fallback={<div class="panel-card muted">This agent's box no longer exists.</div>}>
          {(box) => (
            <div class="panel-card">
              <div class="title">
                <Dot status={box().status} />
                <span>{box().name}</span>
                <span class="meta">
                  <BoxState status={box().status} always />
                </span>
              </div>
              <Show when={box().currency} fallback={<div class="hourly mono">no price</div>}>
                <div class="hourly mono">
                  {money(box().estHourlyMicro, box().currency)}
                  <small>/ h estimated</small>
                </div>
              </Show>
              <div class="muted">{sizeLabel(box())}</div>
              <Show when={capsFor(box())?.autoStop && box().status === "running"}>
                <div class="muted">
                  <Show when={box().autoStopAt > 0} fallback={<>No auto-stop</>}>
                    Auto-stop {until(box().autoStopAt)}
                  </Show>
                  {" · "}
                  <button class="link" disabled={!!state.pending[box().id]} onClick={() => void actions.setAutoStop(box().id, 4 * 3600)} title="Stop it 4 hours from now instead">
                    4 h from now
                  </button>
                </div>
              </Show>
              <div class="actions">
                <PowerActions machine={box()} ghost size="sm" />
              </div>
            </div>
          )}
        </Show>

        <div>
          <h3 class="section-title">Session</h3>
          <dl class="kv" style={{ "grid-template-columns": "76px 1fr" }}>
            <dt>Harness</dt>
            <dd>{p.agent.harness === "claude-code" ? "Claude Code" : "Codex"}</dd>
            <dt>Model</dt>
            <dd>{p.agent.model || "harness default"}</dd>
            <dt>Directory</dt>
            <dd class="mono" style={{ "overflow-wrap": "anywhere" }}>{p.agent.cwd || "home"}</dd>
            <Show when={p.agent.worktree}>
              {(wt) => (
                <>
                  <dt>Branch</dt>
                  <dd class="mono" style={{ "overflow-wrap": "anywhere" }}>
                    {wt().branch}
                    <span class="faint" style={{ display: "block", "font-family": "var(--font-sans)" }}>
                      own worktree of <span class="mono">{wt().repo}</span>
                    </span>
                  </dd>
                </>
              )}
            </Show>
            <dt>Session</dt>
            <dd class="mono">
              <Show when={p.agent.sessionId} fallback={<span class="faint">none yet</span>}>
                <span style={{ "overflow-wrap": "anywhere" }}>{p.agent.sessionId}</span>
                <button class="icon-btn" title="Copy session id" aria-label="Copy session id" onClick={() => void actions.copy(p.agent.sessionId!, "Session id copied")}>
                  <I.Copy />
                </button>
              </Show>
            </dd>
            <dt>Approvals</dt>
            <dd>{p.agent.skipPermissions ? "skipped (box is disposable)" : "asked in the terminal"}</dd>
          </dl>
        </div>
      </div>
    </aside>
  );
}
