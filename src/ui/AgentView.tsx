import { Show, createEffect, createMemo, createSignal, lazy, on, untrack } from "solid-js";
import type { AgentMeta } from "../bridge/types";
import { actions, agentBoxGone as boxGone, agentWorking, machineById, phaseOf, state } from "../state/store";
import * as I from "../icons";
import { AgentDot, Dot } from "./common";
import { agentPhaseLabel, fileSize } from "./format";
import type { TermInput } from "./Terminal";
import { agentTerminalKey, closeTerminal } from "./termSessions";

const Changes = lazy(() => import("./Changes"));
const Terminal = lazy(() => import("./Terminal"));

const UPLOAD_MAX = 20 * 1024 * 1024;

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Typed apart from Enter, so the harness takes it as a key press. */
function submit(w: TermInput, text: string) {
  w(text);
  window.setTimeout(() => w("\r"), 120);
}

/** One agent: its harness's own interface in a terminal, on its own session,
 * and the repository it works in. Every agent has its own terminal; in a box
 * with tmux the harness keeps running when the terminal closes, and the next
 * one attaches to it. */
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
  const halted = () => {
    const m = machine();
    return !m || m.status === "stopped" || m.status === "failed";
  };
  const harnessName = () => (a().harness === "codex" ? "Codex" : "Claude Code");
  const tmux = () => state.tmux[a().machineId];
  const running = () => !!state.live[a().id]?.running;

  // The view's terminal. It is a view of a session that outlives it: leaving
  // the agent keeps the session, and coming back shows the same one.
  const [term, setTerm] = createSignal<{ agent: string; input: string | null; key: number } | null>(null);
  const [termEnded, setTermEnded] = createSignal(false);
  /** Set by End session: the terminal stays closed until asked for again. */
  const [ended, setEnded] = createSignal(false);
  let termKey = 0;
  let write: TermInput | null = null;
  let handled = 0;
  const openTerm = (input: string | null) => {
    write = null;
    setTermEnded(false);
    setEnded(false);
    setTerm({ agent: a().id, input, key: ++termKey });
  };
  const agentId = createMemo(() => a().id);
  createEffect(
    on(agentId, () => {
      write = null;
      setTerm(null);
      setEnded(false);
      setTermEnded(false);
    }),
  );
  createEffect(() => {
    const req = state.ui.agentTerminal;
    const id = agentId();
    const ready = state.ui.agentTab === "terminal" && !gone() && !halted();
    const fresh = req && req.agentId === id && req.seq !== handled ? req : null;
    if (fresh) handled = fresh.seq;
    if (!ready) return;
    untrack(() => {
      if (fresh?.input && write && !termEnded()) submit(write, fresh.input);
      else if (fresh || (!term() && !ended())) openTerm(fresh?.input ?? null);
    });
  });

  const endSession = async () => {
    write = null;
    setTerm(null);
    setEnded(true);
    await actions.stop(a().id);
  };

  // Files dropped on the terminal are copied into the box and their paths
  // typed at the prompt, where the harness reads them.
  const [dragging, setDragging] = createSignal(false);
  const [copying, setCopying] = createSignal<{ name: string; error?: string } | null>(null);
  let picker!: HTMLInputElement;
  const addFiles = async (files: File[]) => {
    const id = a().id;
    for (const file of files) {
      const name = file.name || "pasted-image.png";
      if (file.size === 0) return setCopying({ name, error: "the file is empty" });
      if (file.size > UPLOAD_MAX) return setCopying({ name, error: `${fileSize(file.size)}; files are limited to 20 MB each` });
      setCopying({ name });
      try {
        const up = await actions.upload(id, name, new Uint8Array(await file.arrayBuffer()));
        if (write && agentId() === id) write(up.path + " ");
        setCopying(null);
      } catch (e) {
        return setCopying({ name, error: errText(e) });
      }
    }
  };

  return (
    <section class="view agent-view">
      <header class="view-head">
        <div class="view-title">
          <Show when={!gone()} fallback={<Dot status="stopped" />}>
            <AgentDot agent={a()} />
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
          <Show when={!gone() && !ended() && (running() || (term() && !termEnded()))}>
            <button class="btn ghost danger" onClick={() => void endSession()} title={`Quit ${harnessName()} in the box. The conversation stays; opening the agent again resumes it.`}>
              <I.Stop /> End session
            </button>
          </Show>
          <button class="btn ghost" title="End its session and remove it from Stowbox" onClick={() => actions.confirmDeleteAgent(a())}>
            <I.Trash /> Remove
          </button>
        </div>
        <div class="subhead">
          <span class="tag">{harnessName()}</span>
          <span class="in-box">
            <Show when={machine()}>{(m) => <Dot status={m().status} />}</Show>
            <Show when={!gone()} fallback={<span>{a().machineName}</span>}>
              <button class="link" onClick={() => actions.select({ kind: "box", id: a().machineId })} title="Open the box">
                {a().machineName}
              </button>
            </Show>
          </span>
          <span class="stats mono" style={{ "font-size": "var(--text-2xs)" }}>
            <span>{gone() ? "Box deleted" : agentPhaseLabel[phaseOf(a())]}</span>
            <Show when={a().sessionId}>
              {(sid) => (
                <span title={`Session ${sid()}`}>
                  session <b>{sid().slice(0, 8)}</b>
                </span>
              )}
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
        <button class="subtab" role="tab" aria-selected={state.ui.agentTab === "terminal" || gone()} onClick={() => actions.setAgentTab("terminal")}>
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
        <div class="view-body">
          <div class="empty-state">
            <span>
              <strong>{a().machineName} was deleted.</strong> Nothing more can run in this agent. Its conversation went with the box.
            </span>
            <button class="btn" onClick={() => actions.confirmDeleteAgent(a())}>
              <I.Trash /> Remove the agent
            </button>
          </div>
        </div>
      </Show>
      <Show when={state.ui.agentTab === "changes" && !gone()}>
        <div class="view-body scroll">
          {/* Keyed on the agent so another agent starts from its own directory. */}
          <Show when={a().id} keyed>
            <Changes machine={machine()} cwd={a().cwd} agentBusy={agentWorking(a())} />
          </Show>
        </div>
      </Show>
      <Show when={!gone() && (state.ui.agentTab === "terminal" || term())}>
        <div class="view-body flush" classList={{ hidden: state.ui.agentTab !== "terminal" }}>
          <Show
            when={!halted() && term()?.agent === a().id && term()}
            fallback={
              <div class="view-body">
                <div class="empty-state">
                  <Show
                    when={halted()}
                    fallback={
                      <Show when={ended()} fallback={<span>Opening {harnessName()}…</span>}>
                        <span>
                          <strong>The session ended.</strong> The conversation is kept; opening it again resumes where it stopped.
                        </span>
                        <button class="btn" onClick={() => openTerm(null)}>
                          <I.Terminal /> Open {harnessName()} again
                        </button>
                      </Show>
                    }
                  >
                    <span>
                      <strong>{machine()?.name ?? a().machineName} is not running.</strong> Start it to open {harnessName()} here.
                    </span>
                  </Show>
                </div>
              </div>
            }
          >
            {(t) => (
              <div
                class="term-pane"
                classList={{ dragging: dragging() }}
                onDragOver={(e) => {
                  if (!e.dataTransfer?.types.includes("Files")) return;
                  e.preventDefault();
                  setDragging(true);
                }}
                onDragLeave={(e) => e.currentTarget === e.target && setDragging(false)}
                onDrop={(e) => {
                  setDragging(false);
                  const dropped = Array.from(e.dataTransfer?.files ?? []);
                  if (!dropped.length) return;
                  e.preventDefault();
                  void addFiles(dropped);
                }}
              >
                <div class="term-tools">
                  <span>
                    <Show
                      when={copying()}
                      fallback={
                        <>
                          {harnessName()} in {machine()?.name ?? a().machineName}.{" "}
                          {tmux() === false
                            ? "This box has no tmux, so the agent stops when this terminal closes."
                            : "It keeps running in the box when you close the tab; opening the agent again attaches to it."}
                        </>
                      }
                    >
                      {(c) => (
                        <Show when={c().error} fallback={<>Copying {c().name} into the box…</>}>
                          <span class="danger-text">
                            Could not copy {c().name}: {c().error}
                          </span>
                        </Show>
                      )}
                    </Show>
                  </span>
                  <input
                    ref={picker}
                    type="file"
                    multiple
                    hidden
                    onChange={(e) => {
                      void addFiles(Array.from(e.currentTarget.files ?? []));
                      e.currentTarget.value = "";
                    }}
                  />
                  <button class="btn sm ghost" onClick={() => picker.click()} title="Copy files into the box and type their paths here (or drop them on the terminal)">
                    <I.Paperclip /> Add files
                  </button>
                  <Show when={termEnded()}>
                    <button
                      class="btn sm"
                      onClick={() => {
                        closeTerminal(agentTerminalKey(a().id));
                        openTerm(null);
                      }}
                    >
                      <I.Refresh /> Open again
                    </button>
                  </Show>
                </div>
                <Show when={t().key} keyed>
                  <Terminal
                    machine={a().machineId}
                    target={{ kind: "agent", id: a().id, input: t().input }}
                    persist={agentTerminalKey(a().id)}
                    hint="Connected. Drop files here to type their paths. Closing the agent's tab closes this terminal."
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
