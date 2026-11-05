import { For, Show, createEffect, createMemo, createSignal, lazy, on } from "solid-js";
import type { AgentMeta } from "../bridge/types";
import { actions, machineById, state } from "../state/store";
import * as I from "../icons";
import { AgentDot, Dot } from "./common";
import { diffFor } from "./diff";
import { agentStatusLabel, duration, isBusy, statusLabel, tokens, usd } from "./format";
import { renderMarkdown } from "./markdown";
import { toItems, toolSummary, type Item } from "./transcript";

const Changes = lazy(() => import("./Changes"));

const ToolCard = (p: { item: Extract<Item, { kind: "tool" }> }) => {
  const summary = () => toolSummary(p.item.name, p.item.input);
  const stateLabel = () => (p.item.result === undefined ? "running" : p.item.isError ? "error" : "done");
  const diff = createMemo(() => diffFor(p.item.name, p.item.input));
  const Icon = () => (diff() ? <I.FileDiff /> : p.item.name === "Bash" || p.item.name === "command" ? <I.Terminal /> : <I.Wrench />);
  return (
    <details class="tool" open={!!diff()}>
      <summary>
        <Icon />
        <span class="tool-name">{p.item.name}</span>
        <span class="tool-summary mono">{summary()}</span>
        <span class="tool-state" data-state={stateLabel()}>
          {stateLabel()}
        </span>
      </summary>
      <Show when={diff()}>
        {(lines) => (
          <div class="diff">
            <For each={lines()}>{(l) => <div class={l.kind}>{l.text}</div>}</For>
          </div>
        )}
      </Show>
      <Show when={!diff() && p.item.input && !summary()}>
        <pre>{JSON.stringify(p.item.input, null, 2)}</pre>
      </Show>
      <Show when={p.item.result !== undefined && (p.item.isError || !diff() || p.item.result)}>
        <pre>{p.item.result || "(no output)"}</pre>
      </Show>
    </details>
  );
};

const Transcript = (p: { agent: AgentMeta }) => {
  let scroller!: HTMLDivElement;
  const events = () => state.transcripts[p.agent.id];
  const items = createMemo(() => toItems(events() ?? []));
  const live = () => state.live[p.agent.id] ?? "";
  const machine = () => machineById(p.agent.machineId);

  createEffect(
    on(
      () => [items().length, live().length],
      () => {
        const nearBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 160;
        if (nearBottom) scroller.scrollTop = scroller.scrollHeight;
      },
    ),
  );

  return (
    <div class="transcript scroll" ref={scroller}>
      <Show when={events()} fallback={<div class="chat-empty">Loading the transcript…</div>}>
        <Show
          when={items().length > 0 || live()}
          fallback={
            <div class="chat-empty">
              <strong>Describe the task.</strong>
              <span>
                {p.agent.harness === "claude-code" ? "Claude Code" : "Codex"} will run inside {p.agent.machineName}
                {p.agent.cwd ? ` from ${p.agent.cwd}` : ""}. Each message is one turn; the session id is kept so the next message continues the same
                conversation.
              </span>
            </div>
          }
        >
          <div class="chat">
            <For each={items()}>
              {(it) => {
                switch (it.kind) {
                  case "user":
                    return <div class="msg-user">{it.text}</div>;
                  case "text":
                    return <div class="msg-text" innerHTML={renderMarkdown(it.text)} />;
                  case "thinking":
                    return (
                      <details class="msg-thinking">
                        <summary>Reasoning</summary>
                        {it.text}
                      </details>
                    );
                  case "tool":
                    return <ToolCard item={it} />;
                  case "result":
                    return (
                      <div class="turn-result" classList={{ failed: !it.ok }}>
                        <Show when={it.ok} fallback={<I.Warning />}>
                          <I.Check />
                        </Show>
                        <span>{it.ok ? "Turn finished" : `Turn failed: ${it.message || it.subtype.replace(/_/g, " ")}`}</span>
                        <Show when={it.durationMs}>
                          <span>{duration(it.durationMs!)}</span>
                        </Show>
                        <Show when={it.costUsd != null}>
                          <span class="mono">{usd(it.costUsd!)}</span>
                        </Show>
                        <Show when={it.turns}>
                          <span>{it.turns} model turns</span>
                        </Show>
                      </div>
                    );
                  case "note":
                    return <div class="msg-note">{it.text}</div>;
                  case "exit":
                    return (
                      <Show when={it.code !== 0}>
                        <div class="msg-note">{it.code === -1 ? "Stopped." : `Process exited with code ${it.code}.`}</div>
                      </Show>
                    );
                }
              }}
            </For>
            <Show when={live()}>
              <div class="msg-text msg-live" innerHTML={renderMarkdown(live())} />
            </Show>
            <Show when={p.agent.status.kind === "reconnecting" && (p.agent.status as { message: string }).message}>
              {(msg) => <div class="msg-note">Reconnecting to the turn, which keeps running in the box: {msg()}.</div>}
            </Show>
            <Show when={!live() && p.agent.status.kind === "starting"}>
              <div class="msg-note">Starting {p.agent.harness === "claude-code" ? "claude" : "codex"} in {machine()?.name ?? p.agent.machineName}…</div>
            </Show>
          </div>
        </Show>
      </Show>
    </div>
  );
};

const Composer = (p: { agent: AgentMeta }) => {
  let box!: HTMLTextAreaElement;
  const [text, setText] = createSignal("");
  const [sending, setSending] = createSignal(false);
  const machine = () => machineById(p.agent.machineId);
  const busy = () => isBusy(p.agent.status) || sending();
  const blocked = () => {
    const m = machine();
    if (!m) return "This box no longer exists in boxd.";
    if (m.status === "stopped") return `${m.name} is stopped. Start it to run the agent.`;
    if (m.status === "failed") return `${m.name} is in a failed state.`;
    return "";
  };

  const grow = () => {
    box.style.height = "auto";
    box.style.height = Math.min(box.scrollHeight, 200) + "px";
  };

  const submit = async () => {
    const t = text().trim();
    if (!t || busy() || blocked()) return;
    setSending(true);
    try {
      await actions.send(p.agent.id, t);
      setText("");
      queueMicrotask(grow);
    } catch {
      /* the store already showed the error */
    } finally {
      setSending(false);
      box.focus();
    }
  };

  return (
    <div class="composer">
      <div class="composer-box">
        <textarea
          ref={box}
          rows={1}
          placeholder={p.agent.turns === 0 ? "What should the agent do?" : "Reply, or give the next task"}
          value={text()}
          disabled={!!blocked()}
          onInput={(e) => {
            setText(e.currentTarget.value);
            grow();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void submit();
            }
          }}
          aria-label="Message to the agent"
        />
        <Show
          when={!isBusy(p.agent.status)}
          fallback={
            <button class="btn danger" onClick={() => void actions.stop(p.agent.id)} title="Kill the running turn">
              <I.Stop /> Stop
            </button>
          }
        >
          <button class="btn primary" disabled={!text().trim() || busy() || !!blocked()} onClick={() => void submit()} aria-label="Send">
            <I.Send /> Send
          </button>
        </Show>
      </div>
      <div class="composer-foot">
        <Show when={blocked()} fallback={<span>Enter sends, Shift+Enter adds a line.</span>}>
          <span>{blocked()}</span>
        </Show>
        <span>{p.agent.skipPermissions ? "Tools run without approval prompts." : "Tools that need approval are denied."}</span>
        <Show when={p.agent.cwd}>
          <span class="mono" style={{ "font-size": "var(--text-3xs)" }}>
            {p.agent.cwd}
          </span>
        </Show>
      </div>
    </div>
  );
};

export default function AgentView(p: { agent: AgentMeta }) {
  const a = () => p.agent;
  const machine = () => machineById(a().machineId);
  const [editing, setEditing] = createSignal(false);
  const [draft, setDraft] = createSignal("");

  const startEdit = () => {
    setDraft(a().title);
    setEditing(true);
  };
  const commit = () => {
    const t = draft().trim();
    setEditing(false);
    if (t && t !== a().title) void actions.rename(a().id, t);
  };

  return (
    <section class="view agent-view">
      <header class="view-head">
        <div class="view-title">
          <AgentDot status={a().status} />
          <Show
            when={editing()}
            fallback={
              <>
                <h1 title={a().title}>{a().title || "New agent"}</h1>
                <button class="icon-btn" title="Rename" aria-label="Rename agent" onClick={startEdit}>
                  <I.Pencil />
                </button>
              </>
            }
          >
            <input
              class="input"
              style={{ "max-width": "420px", height: "28px" }}
              value={draft()}
              onInput={(e) => setDraft(e.currentTarget.value)}
              onBlur={commit}
              onKeyDown={(e) => {
                if (e.key === "Enter") commit();
                if (e.key === "Escape") setEditing(false);
              }}
              ref={(el) => queueMicrotask(() => el.focus())}
              aria-label="Agent title"
            />
          </Show>
        </div>
        <div class="view-actions">
          <Show when={isBusy(a().status)}>
            <button class="btn ghost danger" onClick={() => void actions.stop(a().id)} title="Kill the running turn">
              <I.Stop /> Stop
            </button>
          </Show>
          <button class="btn ghost" title="Remove this agent and its transcript" onClick={() => actions.confirmDeleteAgent(a())}>
            <I.Trash /> Remove
          </button>
        </div>
        <div class="subhead">
          <span class="tag">{a().harness === "claude-code" ? "Claude Code" : "Codex"}</span>
          <span class="in-box">
            <Show when={machine()}>{(m) => <Dot status={m().status} />}</Show>
            <button class="link" onClick={() => actions.select({ kind: "box", id: a().machineId })} title="Open the box">
              {a().machineName}
            </button>
          </span>
          <span class="stats mono" style={{ "font-size": "var(--text-2xs)" }}>
            <span>{a().status.kind === "done" ? "Ready for you" : agentStatusLabel(a().status)}</span>
            <span>
              <b>{a().turns}</b> turns
            </span>
            <span>
              <b>{a().totalCostUsd > 0 ? usd(a().totalCostUsd) : "$0"}</b>
            </span>
            <Show when={a().inputTokens + a().outputTokens > 0}>
              <span>
                {tokens(a().inputTokens)} in · {tokens(a().outputTokens)} out
              </span>
            </Show>
          </span>
        </div>
      </header>
      <nav class="subtabs" role="tablist" aria-label="Agent sections">
        <button class="subtab" role="tab" aria-selected={state.ui.agentTab === "chat"} onClick={() => actions.setAgentTab("chat")}>
          Chat
        </button>
        <button class="subtab" role="tab" aria-selected={state.ui.agentTab === "changes"} onClick={() => actions.setAgentTab("changes")} title="The repository in the agent's directory">
          Changes
        </button>
      </nav>
      <Show
        when={state.ui.agentTab === "changes"}
        fallback={
          <>
            <Show when={a().status.kind === "failed"}>
              <div class="msg-note" style={{ margin: "10px 22px 0" }}>
                {(a().status as { message: string }).message}
                <Show when={machine() && machine()!.status !== "running"}>
                  {" "}
                  · the box is {statusLabel[machine()!.status].toLowerCase()}
                </Show>
              </div>
            </Show>
            <Transcript agent={a()} />
            <Composer agent={a()} />
          </>
        }
      >
        <div class="view-body scroll">
          {/* Keyed on the agent so another agent starts from its own directory. */}
          <Show when={a().id} keyed>
            <Changes machine={machine()} cwd={a().cwd} agentBusy={isBusy(a().status)} />
          </Show>
        </div>
      </Show>
    </section>
  );
}
