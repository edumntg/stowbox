import { createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { api } from "../bridge/api";
import type { TermTarget } from "../bridge/types";
import { isWindows } from "./common";
import { sessions } from "./termSessions";

/** Writes into an open terminal as if typed. */
export type TermInput = (text: string) => void;

const opening: Record<TermTarget["kind"], string> = {
  shell: "Opening a login shell in the box…",
  agent: "Opening the agent's session in the box…",
  "claude-login": "Starting Claude Code's sign-in in the box…",
};

type Status = "connecting" | "open" | "ended" | "error";

/** An xterm and the box process behind it. Its element moves between
 * views; the process lives until `close`. */
interface Session {
  el: HTMLDivElement;
  term: import("@xterm/xterm").Terminal;
  fit: import("@xterm/addon-fit").FitAddon;
  id: string | null;
  status: Status;
  detail: string;
  bad: boolean;
  /** Set while open. */
  write: TermInput | null;
  /** The view showing it, if any. */
  view: { refresh(): void; onInput?: (write: TermInput | null) => void; onEnd?: () => void } | null;
  close(): void;
}

const over = (s: Session) => s.status === "ended" || s.status === "error";

async function createSession(machine: string, target: TermTarget, host: HTMLElement, key: string | undefined): Promise<Session> {
  const [{ Terminal: XTerm }, { FitAddon }] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit")]);
  await import("@xterm/xterm/css/xterm.css");

  const css = getComputedStyle(document.documentElement);
  const term = new XTerm({
    fontFamily: css.getPropertyValue("--font-mono") || "monospace",
    fontSize: 13,
    lineHeight: 1.2,
    cursorBlink: true,
    scrollback: 5000,
    theme: {
      background: css.getPropertyValue("--terminal").trim() || "#0a0a0a",
      foreground: "#e5e5e5",
      cursor: "#fafafa",
      selectionBackground: "rgba(255, 255, 255, 0.22)",
    },
  });
  // Windows terminals copy with Ctrl+C when text is selected (and send
  // Ctrl+C, the interrupt, when nothing is) and paste with Ctrl+V. xterm
  // turns both into control characters, so hand them back here: copy the
  // selection, and let Ctrl+V reach the browser's paste, which xterm reads.
  if (isWindows) {
    term.attachCustomKeyEventHandler((e) => {
      if (!e.ctrlKey || e.shiftKey || e.altKey || e.metaKey) return true;
      const k = e.key.toLowerCase();
      if (k === "c" && term.hasSelection()) {
        if (e.type === "keydown") {
          void api.copyText(term.getSelection()).catch(() => undefined);
          term.clearSelection();
        }
        e.preventDefault();
        return false;
      }
      return k !== "v";
    });
  }
  const fit = new FitAddon();
  term.loadAddon(fit);
  const el = document.createElement("div");
  el.className = "term-el";
  host.appendChild(el);
  term.open(el);
  fit.fit();

  const enc = new TextEncoder();
  const dec = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  let closed = false;
  const onData = term.onData((d) => {
    if (s.id) void api.terminalWrite(s.id, enc.encode(d)).catch(() => undefined);
  });
  const s: Session = {
    el,
    term,
    fit,
    id: null,
    status: "connecting",
    detail: "",
    bad: false,
    write: null,
    view: null,
    close() {
      if (closed) return;
      closed = true;
      if (key && sessions.get(key) === s) sessions.delete(key);
      s.view?.onInput?.(null);
      onData.dispose();
      if (s.id) void api.terminalClose(s.id).catch(() => undefined);
      term.dispose();
      el.remove();
    },
  };
  if (key) sessions.set(key, s);
  const end = (status: Status, detail: string, bad: boolean) => {
    Object.assign(s, { status, detail, bad, write: null });
    s.view?.onInput?.(null);
    s.view?.refresh();
    s.view?.onEnd?.();
  };

  void api
    .terminalOpen(machine, term.cols, term.rows, target, (ev) => {
      if (ev.type === "data") term.write(dec(ev.b64));
      else if (ev.type === "exit")
        end("ended", target.kind === "shell" ? `The shell exited with code ${ev.code}.` : ev.code === 0 ? "Exited." : `Exited with code ${ev.code}.`, ev.code !== 0);
      else end("error", ev.message, true);
    })
    .then((id) => {
      if (closed) {
        void api.terminalClose(id).catch(() => undefined);
        return;
      }
      s.id = id;
      if (over(s)) return;
      s.status = "open";
      s.write = (text) => void api.terminalWrite(id, enc.encode(text)).catch(() => undefined);
      s.view?.onInput?.(s.write);
      s.view?.refresh();
      if (s.view) term.focus();
    })
    .catch((e) => end("error", String(e), true));
  return s;
}

/** One interactive session inside a box, streamed over the provider's exec
 * channel: a login shell by default, or what `target` names. Nothing has to
 * be installed locally: no ssh, no CLI. With `persist`, the session outlives
 * the view: leaving it keeps the process running, and the next view with
 * the same key shows it again, scrollback and all. */
export default function Terminal(p: {
  machine: string;
  key?: string;
  target?: TermTarget;
  /** Keeps the session under this key when the view goes away. */
  persist?: string;
  /** The bar's text while connected. */
  hint?: JSX.Element;
  onInput?: (write: TermInput | null) => void;
  onEnd?: () => void;
}) {
  // Read once: props can be getters over a view that is already gone by
  // the time this one cleans up.
  const target = p.target ?? { kind: "shell" };
  const persist = p.persist;
  const onInput = p.onInput;
  let host!: HTMLDivElement;
  const [status, setStatus] = createSignal<Status>("connecting");
  const [detail, setDetail] = createSignal("");
  const [bad, setBad] = createSignal(false);
  let session: Session | null = null;
  let ro: ResizeObserver | null = null;
  let disposed = false;

  const detach = () => {
    ro?.disconnect();
    const s = session;
    if (!s) return;
    s.view = null;
    onInput?.(null);
    if (persist && !over(s)) s.el.remove();
    else s.close();
  };
  onCleanup(() => {
    disposed = true;
    detach();
  });

  onMount(async () => {
    const kept = persist ? (sessions.get(persist) as Session | undefined) : undefined;
    let s: Session;
    let reused = false;
    if (kept && !over(kept)) {
      s = kept;
      reused = true;
      host.appendChild(s.el);
    } else {
      kept?.close();
      s = await createSession(p.machine, target, host, persist);
    }
    session = s;
    s.view = {
      refresh: () => {
        setStatus(s.status);
        setDetail(s.detail);
        setBad(s.bad);
      },
      onInput: onInput,
      onEnd: p.onEnd,
    };
    if (disposed) {
      detach();
      return;
    }
    s.view.refresh();
    const resize = () => {
      s.fit.fit();
      if (s.id) void api.terminalResize(s.id, s.term.cols, s.term.rows).catch(() => undefined);
    };
    ro = new ResizeObserver(resize);
    ro.observe(host);
    resize();
    if (s.write) {
      onInput?.(s.write);
      // Asked for with a first input while the session already ran: type it.
      const input = target.kind === "agent" ? target.input?.trim() : "";
      if (reused && input) {
        const w = s.write;
        w(input);
        window.setTimeout(() => w("\r"), 120);
      }
    }
    if (s.status !== "connecting") s.term.focus();
  });

  return (
    <div class="term-wrap">
      <div class="term-host" ref={host} />
      <div class="term-bar" classList={{ ended: bad() }}>
        {status() === "connecting" && <span>{opening[target.kind]}</span>}
        {status() === "open" && <span>{p.hint ?? "Connected. Close the tab to end the session."}</span>}
        {(status() === "ended" || status() === "error") && <span>{detail() || "The session ended."}</span>}
      </div>
    </div>
  );
}
