import { For, Show, createEffect, createMemo, createSignal, lazy, on, untrack } from "solid-js";
import type { AgentMeta, Attachment } from "../bridge/types";
import { actions, agentBoxGone as boxGone, machineById, state } from "../state/store";
import * as I from "../icons";
import { AgentDot, Dot } from "./common";
import { diffFor } from "./diff";
import { agentStatusLabel, duration, fileSize, isBusy, statusLabel, tokens, usd } from "./format";
import { renderMarkdown } from "./markdown";
import { needsLogin, refusedCommand, runsInTerminal, slashCommand } from "./slash";
import type { TermInput } from "./Terminal";
import { itemIndexForEvent, toItems, toolSummary, type Item } from "./transcript";

const Changes = lazy(() => import("./Changes"));
const Terminal = lazy(() => import("./Terminal"));

/** A way into the agent's terminal under a chat item that needs it: a
 * command the headless turn refused, or a missing login. */
const TerminalLink = (p: { agent: AgentMeta; text: string }) => {
  const refused = () => refusedCommand(p.text);
  const login = () => p.agent.harness === "claude-code" && needsLogin(p.text);
  const input = () => refused() ?? (login() ? "/login" : null);
  return (
    <Show when={input() && !boxGone(p.agent)}>
      <button class="btn sm term-link" onClick={() => actions.openAgentTerminal(p.agent.id, input())}>
        <I.Terminal /> {login() && !refused() ? "Sign in from the terminal" : `Run ${input()} in the terminal`}
      </button>
    </Show>
  );
};

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

  // An event picked in the palette's transcript search: bring its item into
  // view once it is rendered, mark it for a moment, open a folded tool card.
  createEffect(
    on(
      () => [state.ui.focusEvent, events()] as const,
      ([f, evs]) => {
        if (!f || f.agentId !== p.agent.id || !evs) return;
        const k = itemIndexForEvent(evs, f.event);
        actions.clearFocusEvent();
        if (k < 0) return;
        requestAnimationFrame(() => {
          const el = scroller.querySelector(`.chat-item[data-item="${k}"]`)?.firstElementChild as HTMLElement | null | undefined;
          if (!el) return;
          if (el instanceof HTMLDetailsElement) el.open = true;
          el.scrollIntoView({ block: "center" });
          el.classList.remove("found");
          void el.offsetWidth;
          el.classList.add("found");
          window.setTimeout(() => el.classList.remove("found"), 1600);
        });
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
              {(it, i) => (
                <div class="chat-item" data-item={i()}>
                  {(() => {
                    switch (it.kind) {
                      case "user":
                        return (
                          <div class="msg-user">
                            <Show when={it.text}>{it.text}</Show>
                            <Show when={it.attachments.length}>
                              <FileList files={it.attachments} />
                            </Show>
                          </div>
                        );
                      case "text":
                        return (
                          <>
                            <div class="msg-text" innerHTML={renderMarkdown(it.text)} />
                            <TerminalLink agent={p.agent} text={it.text} />
                          </>
                        );
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
                          <>
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
                            <Show when={!it.ok && it.message}>{(m) => <TerminalLink agent={p.agent} text={m()} />}</Show>
                          </>
                        );
                      case "note":
                        return (
                          <>
                            <div class="msg-note">{it.text}</div>
                            <TerminalLink agent={p.agent} text={it.text} />
                          </>
                        );
                      case "exit":
                        return (
                          <Show when={it.code !== 0}>
                            <div class="msg-note">{it.code === -1 ? "Stopped." : `Process exited with code ${it.code}.`}</div>
                          </Show>
                        );
                    }
                  })()}
                </div>
              )}
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

/** Files on a sent or queued message: name and size, the path in the box on hover. */
const FileList = (p: { files: Attachment[] }) => (
  <ul class="msg-files">
    <For each={p.files}>
      {(f) => (
        <li title={f.path}>
          <I.Paperclip />
          <span class="mono">{f.name}</span>
          <span class="file-size">{fileSize(f.size)}</span>
        </li>
      )}
    </For>
  </ul>
);

/** Messages waiting for the running turn to end, editable until they go out. */
const QueuePanel = (p: { agent: AgentMeta }) => {
  const queue = () => p.agent.queue ?? [];
  const paused = () => !!p.agent.queuePaused;
  const working = () => isBusy(p.agent.status);
  const [editing, setEditing] = createSignal<string | null>(null);
  const [draft, setDraft] = createSignal("");
  const why = () => {
    if (paused()) return p.agent.status.kind === "stopped" ? "Paused after Stop. Nothing goes out until you send it." : "Paused: the first message could not be sent.";
    return working() ? "Each one goes out after the current turn ends." : "Sending the next one…";
  };
  const save = (id: string) => {
    setEditing(null);
    void actions.editQueued(p.agent.id, id, draft());
  };

  return (
    <Show when={queue().length > 0}>
      <section class="queue" aria-label="Queued messages">
        <div class="queue-head">
          <span class="section-title">Queued · {queue().length}</span>
          <span class="queue-why">{why()}</span>
          <Show when={paused()}>
            <button class="btn sm" disabled={working()} onClick={() => void actions.resumeQueue(p.agent.id)} title="Send the first message now; the rest follow turn by turn">
              <I.Send /> Send next
            </button>
          </Show>
          <button class="btn sm ghost" onClick={() => void actions.clearQueue(p.agent.id)} title="Remove every queued message">
            Clear
          </button>
        </div>
        <ol class="queue-list scroll">
          <For each={queue()}>
            {(m) => (
              <li class="queue-row">
                <span class="sq" data-state={paused() ? "idle" : "pending"} />
                <div class="queue-body">
                  <Show
                    when={editing() === m.id}
                    fallback={
                      <div class="queue-text" classList={{ muted: !m.text }}>
                        {m.text || "Files only"}
                      </div>
                    }
                  >
                    <textarea
                      class="input queue-edit"
                      rows={2}
                      value={draft()}
                      onInput={(e) => setDraft(e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.shiftKey) {
                          e.preventDefault();
                          save(m.id);
                        }
                        if (e.key === "Escape") setEditing(null);
                      }}
                      ref={(el) => queueMicrotask(() => el.focus())}
                      aria-label="Queued message"
                    />
                  </Show>
                  <Show when={m.attachments.length}>
                    <FileList files={m.attachments} />
                  </Show>
                </div>
                <div class="queue-actions">
                  <Show
                    when={editing() === m.id}
                    fallback={
                      <>
                        <button
                          class="icon-btn"
                          title="Edit"
                          aria-label="Edit queued message"
                          onClick={() => {
                            setDraft(m.text);
                            setEditing(m.id);
                          }}
                        >
                          <I.Pencil />
                        </button>
                        <button class="icon-btn" title="Remove" aria-label="Remove queued message" onClick={() => void actions.removeQueued(p.agent.id, m.id)}>
                          <I.Close />
                        </button>
                      </>
                    }
                  >
                    <button class="btn sm" onClick={() => save(m.id)}>
                      Save
                    </button>
                    <button class="btn sm ghost" onClick={() => setEditing(null)}>
                      Cancel
                    </button>
                  </Show>
                </div>
              </li>
            )}
          </For>
        </ol>
      </section>
    </Show>
  );
};

/** Mirrors stowbox_core::upload: MAX_BYTES and MAX_FILES. */
const UPLOAD_MAX = 20 * 1024 * 1024;
const FILES_MAX = 10;

type PendingFile = { key: number; name: string; size: number; state: "uploading" | "ready" | "failed"; attachment?: Attachment; error?: string };
let fileSeq = 0;

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

const Composer = (p: { agent: AgentMeta; beforeSend: () => void }) => {
  let box!: HTMLTextAreaElement;
  let picker!: HTMLInputElement;
  const [text, setText] = createSignal("");
  const [files, setFiles] = createSignal<PendingFile[]>([]);
  const [sending, setSending] = createSignal(false);
  const [dragging, setDragging] = createSignal(false);
  const machine = () => machineById(p.agent.machineId);
  const working = () => isBusy(p.agent.status);
  const uploading = () => files().some((f) => f.state === "uploading");
  const failed = () => files().some((f) => f.state === "failed");
  const ready = () => files().flatMap((f) => (f.state === "ready" && f.attachment ? [f.attachment] : []));
  const toTerminal = () => runsInTerminal(p.agent.harness, text());
  const canSend = () => (toTerminal() ? !blocked() : (!!text().trim() || ready().length > 0) && !uploading() && !failed() && !sending() && !blocked());
  const blocked = () => {
    const m = machine();
    if (!m) return "This box was deleted.";
    if (m.status === "stopped") return `${m.name} is stopped. Start it to run the agent.`;
    if (m.status === "failed") return `${m.name} is in a failed state.`;
    return "";
  };

  // Uploads belong to one agent's box: another agent starts with none.
  // A memo, so a push that replaces the agent's record does not count as a change.
  const agentId = createMemo(() => p.agent.id);
  createEffect(on(agentId, () => setFiles([]), { defer: true }));

  const update = (key: number, patch: Partial<PendingFile>) => setFiles((fs) => fs.map((f) => (f.key === key ? { ...f, ...patch } : f)));

  const addFiles = (list: File[]) => {
    const agentId = p.agent.id;
    for (const file of list) {
      const key = ++fileSeq;
      const name = file.name || "pasted-image.png";
      if (files().length >= FILES_MAX) {
        setFiles((fs) => [...fs, { key, name, size: file.size, state: "failed", error: `up to ${FILES_MAX} files per message` }]);
        continue;
      }
      if (file.size > UPLOAD_MAX) {
        const error = `${fileSize(file.size)}; attachments are limited to 20 MB each`;
        setFiles((fs) => [...fs, { key, name, size: file.size, state: "failed", error }]);
        continue;
      }
      if (file.size === 0) {
        setFiles((fs) => [...fs, { key, name, size: 0, state: "failed", error: "the file is empty" }]);
        continue;
      }
      setFiles((fs) => [...fs, { key, name, size: file.size, state: "uploading" }]);
      void file
        .arrayBuffer()
        .then((buf) => actions.upload(agentId, name, new Uint8Array(buf)))
        .then((attachment) => update(key, { state: "ready", attachment }))
        .catch((e) => update(key, { state: "failed", error: errText(e) }));
    }
  };

  const grow = () => {
    box.style.height = "auto";
    box.style.height = Math.min(box.scrollHeight, 200) + "px";
  };

  const submit = async () => {
    if (!canSend()) return;
    const t = text().trim();
    if (toTerminal()) {
      actions.openAgentTerminal(p.agent.id, t);
      setText("");
      queueMicrotask(grow);
      return;
    }
    // One process at a time on the session: the terminal closes first.
    p.beforeSend();
    const attached = ready();
    setSending(true);
    try {
      if (working()) await actions.queue(p.agent.id, t, attached);
      else await actions.send(p.agent.id, t, attached);
      setText("");
      setFiles([]);
      queueMicrotask(grow);
    } catch {
      /* the store already showed the error */
    } finally {
      setSending(false);
      box.focus();
    }
  };

  const foot = () => {
    if (blocked()) return blocked();
    if (failed()) return "Remove the files that could not be attached to send.";
    if (uploading()) return "Copying files into the box…";
    if (toTerminal())
      return `/${slashCommand(text())} needs ${p.agent.harness === "codex" ? "Codex" : "Claude Code"}'s own interface: Enter runs it in the Terminal tab, on this session.`;
    if (working()) return "Enter queues the message; it goes out when this turn ends.";
    return "Enter sends, Shift+Enter adds a line. Paste or drop files to attach them. /login, /status and the like run in the Terminal tab.";
  };

  return (
    <div class="composer">
      <div
        class="composer-box"
        classList={{ dragging: dragging() }}
        onDragOver={(e) => {
          if (!e.dataTransfer?.types.includes("Files") || blocked()) return;
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          setDragging(false);
          const dropped = Array.from(e.dataTransfer?.files ?? []);
          if (!dropped.length || blocked()) return;
          e.preventDefault();
          addFiles(dropped);
        }}
      >
        <Show when={files().length}>
          <ul class="attach-list" aria-label="Attached files">
            <For each={files()}>
              {(f) => (
                <li class="attach" data-state={f.state} title={f.attachment?.path ?? f.error ?? "Copying into the box"}>
                  <Show when={f.state === "failed"} fallback={<I.Paperclip />}>
                    <I.Warning />
                  </Show>
                  <span class="mono attach-name">{f.name}</span>
                  <span class="attach-note">{f.state === "failed" ? f.error : f.state === "uploading" ? "copying" : fileSize(f.size)}</span>
                  <button class="icon-btn" aria-label={`Remove ${f.name}`} title="Remove" onClick={() => setFiles((fs) => fs.filter((x) => x.key !== f.key))}>
                    <I.Close />
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <textarea
          ref={box}
          rows={1}
          placeholder={working() ? "Queue a follow-up for when this turn ends" : p.agent.turns === 0 ? "What should the agent do?" : "Reply, or give the next task"}
          value={text()}
          disabled={!!blocked()}
          onInput={(e) => {
            setText(e.currentTarget.value);
            grow();
          }}
          onPaste={(e) => {
            const pasted = Array.from(e.clipboardData?.files ?? []);
            if (!pasted.length) return;
            e.preventDefault();
            addFiles(pasted);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void submit();
            }
          }}
          aria-label="Message to the agent"
        />
        <div class="composer-actions">
          <input
            ref={picker}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              addFiles(Array.from(e.currentTarget.files ?? []));
              e.currentTarget.value = "";
            }}
          />
          <button class="icon-btn" disabled={!!blocked()} onClick={() => picker.click()} title="Attach files (copied into the box, up to 20 MB each)" aria-label="Attach files">
            <I.Paperclip />
          </button>
          <button
            class="icon-btn"
            disabled={!!blocked()}
            onClick={() => actions.openAgentTerminal(p.agent.id)}
            title={`Switch to ${p.agent.harness === "codex" ? "Codex" : "Claude Code"}'s own interface on this session (/login, /status, /config)`}
            aria-label="Open the agent's terminal"
          >
            <I.Terminal />
          </button>
          <Show
            when={toTerminal()}
            fallback={
              <Show
                when={working()}
                fallback={
                  <button class="btn primary" disabled={!canSend()} onClick={() => void submit()} aria-label="Send">
                    <I.Send /> Send
                  </button>
                }
              >
                <button class="btn" disabled={!canSend()} onClick={() => void submit()} title="Send when the current turn ends" aria-label="Queue">
                  <I.ListChecks /> Queue
                </button>
                <button class="btn danger" onClick={() => void actions.stop(p.agent.id)} title="Kill the running turn; queued messages wait">
                  <I.Stop /> Stop
                </button>
              </Show>
            }
          >
            <button class="btn primary" disabled={!canSend()} onClick={() => void submit()} aria-label="Run in the terminal">
              <I.Terminal /> Run in terminal
            </button>
          </Show>
        </div>
      </div>
      <div class="composer-foot">
        <span>{foot()}</span>
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

  const gone = () => boxGone(a());
  const busy = () => isBusy(a().status);
  const halted = () => {
    const m = machine();
    return !m || m.status === "stopped" || m.status === "failed";
  };
  const harnessName = () => (a().harness === "codex" ? "Codex" : "Claude Code");
  // The Terminal tab: the harness's own interface on the agent's session.
  // It stays open while the Chat tab shows, and closes before a turn runs,
  // so only one process works on the session at a time.
  const [term, setTerm] = createSignal<{ input: string | null; key: number } | null>(null);
  const [termEnded, setTermEnded] = createSignal(false);
  let termKey = 0;
  let write: TermInput | null = null;
  let handled = 0;
  const openTerm = (input: string | null) => {
    write = null;
    setTermEnded(false);
    setTerm({ input, key: ++termKey });
  };
  const closeTerm = () => {
    write = null;
    setTerm(null);
  };
  const agentId = createMemo(() => a().id);
  createEffect(on(agentId, closeTerm, { defer: true }));
  createEffect(on(busy, (b) => b && closeTerm(), { defer: true }));
  createEffect(() => {
    const req = state.ui.agentTerminal;
    if (state.ui.agentTab !== "terminal") return;
    const fresh = req && req.agentId === agentId() && req.seq !== handled ? req : null;
    if (fresh) handled = fresh.seq;
    untrack(() => {
      if (gone() || busy() || halted()) return;
      if (fresh?.input && write && !termEnded()) {
        const w = write;
        w(fresh.input);
        // Typed apart from Enter, so the TUI takes it as a key press.
        window.setTimeout(() => w("\r"), 120);
      } else if (!term() || termEnded() || fresh?.input) openTerm(fresh?.input ?? null);
    });
  });

  return (
    <section class="view agent-view">
      <header class="view-head">
        <div class="view-title">
          <Show when={!gone()} fallback={<Dot status="stopped" />}>
            <AgentDot status={a().status} />
          </Show>
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
            <Show when={!gone()} fallback={<span>{a().machineName}</span>}>
              <button class="link" onClick={() => actions.select({ kind: "box", id: a().machineId })} title="Open the box">
                {a().machineName}
              </button>
            </Show>
          </span>
          <span class="stats mono" style={{ "font-size": "var(--text-2xs)" }}>
            <span>{gone() ? "Box deleted" : a().status.kind === "done" ? "Ready for you" : agentStatusLabel(a().status)}</span>
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
          <Show when={a().worktree}>
            {(wt) => (
              <span class="in-box branch mono" title={`Its own worktree: ${wt().path}`}>
                <I.Fork />
                <span>{wt().branch}</span>
              </span>
            )}
          </Show>
        </div>
      </header>
      <nav class="subtabs" role="tablist" aria-label="Agent sections">
        <button class="subtab" role="tab" aria-selected={state.ui.agentTab === "chat" || gone()} onClick={() => actions.setAgentTab("chat")}>
          Chat
        </button>
        <button
          class="subtab"
          role="tab"
          aria-selected={state.ui.agentTab === "terminal" && !gone()}
          disabled={gone()}
          onClick={() => actions.setAgentTab("terminal")}
          title={`${harnessName()}'s own interface on this session: /login, /status, /config`}
        >
          <Show when={term() && !termEnded()}>
            <span class="sq" data-state="running" />
          </Show>
          Terminal
        </button>
        <button
          class="subtab"
          role="tab"
          aria-selected={state.ui.agentTab === "changes" && !gone()}
          disabled={gone()}
          onClick={() => actions.setAgentTab("changes")}
          title="The repository in the agent's directory"
        >
          Changes
        </button>
      </nav>
      <Show when={gone()}>
        <div class="box-gone" role="status">
          <span class="sq" data-state="idle" />
          <span>
            <strong>{a().machineName} was deleted.</strong> This agent's transcript stays as history; nothing more can run in it.
          </span>
          <button class="btn sm ghost" onClick={() => actions.confirmDeleteAgent(a())}>
            <I.Trash /> Remove the agent
          </button>
        </div>
      </Show>
      <Show when={state.ui.agentTab === "chat" || gone()}>
        <Show when={a().status.kind === "failed" && !gone()}>
          <div class="msg-note" style={{ margin: "10px 22px 0" }}>
            {(a().status as { message: string }).message}
            <Show when={machine() && machine()!.status !== "running"}>
              {" "}
              · the box is {statusLabel[machine()!.status].toLowerCase()}
            </Show>
          </div>
        </Show>
        <Transcript agent={a()} />
        <Show when={!gone()}>
          <QueuePanel agent={a()} />
          <Composer agent={a()} beforeSend={closeTerm} />
        </Show>
      </Show>
      <Show when={state.ui.agentTab === "changes" && !gone()}>
        <div class="view-body scroll">
          {/* Keyed on the agent so another agent starts from its own directory. */}
          <Show when={a().id} keyed>
            <Changes machine={machine()} cwd={a().cwd} agentBusy={isBusy(a().status)} />
          </Show>
        </div>
      </Show>
      <Show when={!gone() && (state.ui.agentTab === "terminal" || term())}>
        <div class="view-body flush" classList={{ hidden: state.ui.agentTab !== "terminal" }}>
          <Show
            when={!busy() && term()}
            fallback={
              <div class="view-body">
                <div class="empty-state">
                  <Show
                    when={busy()}
                    fallback={
                      <Show when={halted()} fallback={<span>Opening {harnessName()}…</span>}>
                        <span>
                          <strong>{machine()?.name ?? a().machineName} is not running.</strong> Start it to open {harnessName()} here.
                        </span>
                      </Show>
                    }
                  >
                    <span>
                      <strong>A turn is running in the chat.</strong> The terminal opens on the same session once it ends, so two processes never write to it at
                      once.
                    </span>
                    <button class="btn danger" onClick={() => void actions.stop(a().id)}>
                      <I.Stop /> Stop the turn
                    </button>
                  </Show>
                </div>
              </div>
            }
          >
            {(t) => (
              <div class="term-pane">
                <div class="term-tools">
                  <span>
                    {harnessName()} in {machine()?.name ?? a().machineName}
                    {a().sessionId ? (
                      <>
                        {" "}
                        on session <span class="mono">{a().sessionId!.slice(0, 8)}</span>
                      </>
                    ) : (
                      ", new session"
                    )}
                    . What happens here does not show in the chat transcript; the next chat message continues from it.
                  </span>
                  <Show when={termEnded()}>
                    <button class="btn sm" onClick={() => openTerm(null)}>
                      <I.Refresh /> Open again
                    </button>
                  </Show>
                  <button class="btn sm ghost" onClick={() => actions.setAgentTab("chat")}>
                    <I.Chat /> Back to chat
                  </button>
                </div>
                <Show when={t().key} keyed>
                  <Terminal
                    machine={a().machineId}
                    target={{ kind: "agent", id: a().id, input: t().input }}
                    hint="Connected. /exit or Back to chat; the session stays open until you send from the chat."
                    onInput={(w) => (write = w)}
                    onEnd={() => setTermEnded(true)}
                  />
                </Show>
              </div>
            )}
          </Show>
        </div>
      </Show>
    </section>
  );
}
