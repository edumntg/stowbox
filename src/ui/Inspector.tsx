import { Show } from "solid-js";
import type { AgentMeta } from "../bridge/types";
import { actions, machineById } from "../state/store";
import * as I from "../icons";
import { Dot, PowerActions } from "./common";
import { money, sizeLabel, statusLabel } from "./format";

/** The box behind the selected agent: state, live cost and quick controls,
 * so pausing or opening a shell never means leaving the conversation. */
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
              <button class="icon-btn" title="Open the live desktop" aria-label="Desktop" onClick={() => void actions.open(box().desktopUrl)}>
                <I.Desktop />
              </button>
              <button class="icon-btn" title="Open this agent's directory in VS Code or Cursor" aria-label="Open in editor" onClick={() => actions.openDialog({ kind: "open-editor", machineId: box().id, dir: p.agent.cwd ?? null })}>
                <I.Code />
              </button>
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
        <Show when={m()} fallback={<div class="panel-card muted">This agent's box is no longer in your boxd account.</div>}>
          {(box) => (
            <div class="panel-card">
              <div class="title">
                <Dot status={box().status} />
                <span>{box().name}</span>
                <span class="meta">{statusLabel[box().status]}</span>
              </div>
              <div class="hourly mono">
                {money(box().estHourlyMicro, box().currency)}
                <small>/ h estimated</small>
              </div>
              <div class="muted">{sizeLabel(box())}</div>
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
            <dd>{p.agent.skipPermissions ? "skipped (box is disposable)" : "denied when asked"}</dd>
          </dl>
        </div>
      </div>
    </aside>
  );
}
