import { For, Show, createEffect, createMemo, createSignal, on } from "solid-js";
import type { AgentMeta } from "../bridge/types";
import { actions, machineById, state } from "../state/store";
import * as I from "../icons";
import { AgentSquare, StatusSquare } from "./common";
import { agentStatusLabel, duration, isBusy, statusLabel, tokens, usd } from "./format";
import { renderMarkdown } from "./markdown";
import { toItems, toolSummary, type Item } from "./transcript";

const ToolCard = (p: { item: Extract<Item, { kind: "tool" }> }) => {
  const summary = () => toolSummary(p.item.name, p.item.input);
  const stateLabel = () => (p.item.result === undefined ? "running" : p.item.isError ? "error" : "done");
  return (
    <details class="t-tool" classList={{ error: !!p.item.isError }}>
      <summary>
        <span class="name">{p.item.name}</span>
        <span class="cmd mono">{summary()}</span>
        <span class="state">{stateLabel()}</span>
      </summary>
      <Show when={p.item.input && !summary()}>
        <pre class="mono">{JSON.stringify(p.item.input, null, 2)}</pre>
      </Show>
      <Show when={p.item.result !== undefined}>
        <pre class="mono">{p.item.result || "(no output)"}</pre>
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
    <div class="transcript" ref={scroller}>
      <Show when={events()} fallback={<div class="t-empty">Loading the transcript…</div>}>
        <Show
          when={items().length > 0 || live()}
          fallback={
            <div class="t-empty">
              <strong>Describe the task.</strong>
              <span>
                {p.agent.harness === "claude-code" ? "Claude Code" : "Codex"} will run inside {p.agent.machineName}
                {p.agent.cwd ? ` from ${p.agent.cwd}` : ""}. Each message is one turn; the session id is kept so the next message continues the same
                conversation.
              </span>
            </div>
          }
        >
          <div class="t-inner">
            <For each={items()}>
              {(it) => {
                switch (it.kind) {
                  case "user":
                    return <div class="t-user">{it.text}</div>;
                  case "text":
                    return <div class="t-text" innerHTML={renderMarkdown(it.text)} />;
                  case "thinking":
                    return (
                      <details class="t-thinking">
                        <summary>Reasoning</summary>
                        {it.text}
                      </details>
                    );
                  case "tool":
                    return <ToolCard item={it} />;
                  case "result":
                    return (
                      <div class="t-result" classList={{ failed: !it.ok }}>
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
                    return <div class="t-note mono">{it.text}</div>;
                  case "exit":
                    return (
                      <Show when={it.code !== 0}>
                        <div class="t-note">{it.code === -1 ? "Stopped." : `Process exited with code ${it.code}.`}</div>
                      </Show>
                    );
                }
              }}
            </For>
            <Show when={live()}>
              <div class="t-text t-live" innerHTML={renderMarkdown(live())} />
            </Show>
            <Show when={!live() && p.agent.status.kind === "starting"}>
              <div class="t-note">Starting {p.agent.harness === "claude-code" ? "claude" : "codex"} in {machine()?.name ?? p.agent.machineName}…</div>
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
          <button class="btn primary" disabled={!text().trim() || busy() || !!blocked()} onClick={() => void submit()}>
            <I.Send /> Send
          </button>
        </Show>
      </div>
      <div class="composer-hint">
        <Show when={blocked()} fallback={<span>Enter sends, Shift+Enter adds a line.</span>}>
          <span>{blocked()}</span>
        </Show>
        <span>{p.agent.skipPermissions ? "Tools run without approval prompts." : "Tools that need approval are denied."}</span>
        <Show when={p.agent.cwd}>
          <span class="mono">cwd {p.agent.cwd}</span>
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
          <AgentSquare status={a().status} />
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
              style={{ "max-width": "420px" }}
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
        <div class="actions">
          <button class="icon-btn" title={state.ui.inspector ? "Hide the box panel" : "Show the box panel"} aria-label="Toggle box panel" onClick={() => actions.toggleInspector()}>
            <I.Sidebar />
          </button>
          <button class="icon-btn" title="Remove this agent and its transcript" aria-label="Remove agent" onClick={() => actions.confirmDeleteAgent(a())}>
            <I.Trash />
          </button>
        </div>
        <div class="subhead">
          <span class="pill">{a().harness === "claude-code" ? "Claude Code" : "Codex"}</span>
          <span class="in-box">
            <Show when={machine()}>{(m) => <StatusSquare status={m().status} />}</Show>
            <button class="link" onClick={() => actions.select({ kind: "box", id: a().machineId })} title="Open the box">
              {a().machineName}
            </button>
          </span>
          <span class="stats mono">
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
      <Show when={a().status.kind === "failed"}>
        <div class="t-note" style={{ margin: "10px 24px 0" }}>
          {(a().status as { message: string }).message}
          <Show when={machine() && machine()!.status !== "running"}>
            {" "}
            · the box is {statusLabel[machine()!.status].toLowerCase()}
          </Show>
        </div>
      </Show>
      <Transcript agent={a()} />
      <Composer agent={a()} />
    </section>
  );
}
