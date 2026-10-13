import { Show } from "solid-js";
import type { AgentMeta } from "../bridge/types";
import { actions, machineById } from "../state/store";
import * as I from "../icons";
import { PowerActions, StatusSquare } from "./common";
import { money, sizeLabel, statusLabel } from "./format";

/** The box behind the selected agent: state, live cost and quick controls,
 * so pausing or opening a shell never means leaving the conversation. */
export default function Inspector(p: { agent: AgentMeta }) {
  const m = () => machineById(p.agent.machineId);
  return (
    <aside class="inspector" aria-label="Box panel">
      <h3>Box</h3>
      <Show when={m()} fallback={<div class="card muted">This agent's box is no longer in your boxd account.</div>}>
        {(box) => (
          <div class="card">
            <div class="card-title">
              <StatusSquare status={box().status} />
              <span>{box().name}</span>
              <span class="meta">{statusLabel[box().status]}</span>
            </div>
            <div class="hourly mono">
              {money(box().estHourlyMicro, box().currency)}
              <small>/ h estimated</small>
            </div>
            <div class="faint">{sizeLabel(box())}</div>
            <div class="actions">
              <PowerActions machine={box()} quiet />
            </div>
            <div class="actions">
              <button class="btn quiet" onClick={() => actions.openBoxTab(box().id, "terminal")}>
                <I.Terminal /> Terminal
              </button>
              <button class="btn quiet" onClick={() => void actions.open(box().desktopUrl)}>
                <I.Desktop /> Desktop
              </button>
              <button class="btn quiet" onClick={() => actions.openBoxTab(box().id, "details")}>
                <I.Coins /> Cost
              </button>
            </div>
          </div>
        )}
      </Show>

      <h3>Session</h3>
      <div class="card">
        <dl class="kv" style={{ "grid-template-columns": "80px 1fr" }}>
          <dt>Harness</dt>
          <dd>{p.agent.harness === "claude-code" ? "Claude Code" : "Codex"}</dd>
          <dt>Model</dt>
          <dd>{p.agent.model || "harness default"}</dd>
          <dt>Directory</dt>
          <dd class="mono">{p.agent.cwd || "home"}</dd>
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
    </aside>
  );
}
