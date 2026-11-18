import { Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { api } from "../bridge/api";
import type { TermTarget } from "../bridge/types";
import { isMac } from "./common";
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
  /** The last web address on screen, for the bar's Open and Copy. */
  url: string;
  /** Set while open. */
  write: TermInput | null;
  /** The view showing it, if any. */
  view: { refresh(): void; onInput?: (write: TermInput | null) => void; onEnd?: () => void } | null;
  close(): void;
}

const over = (s: Session) => s.status === "ended" || s.status === "error";

const URL_RE = /https?:\/\/[^\s"'<>`]+/g;

/** The last http(s) address in the visible rows. A long address runs over
 * several rows: soft-wrapped ones are marked as such, and a full row that
 * ends inside the address carries on into the next too, since some TUIs
 * break lines themselves. */
function lastUrlOnScreen(term: import("@xterm/xterm").Terminal): string {
  const buf = term.buffer.active;
  const text: string[] = [];
  let line = "";
  for (let y = buf.viewportY; y < buf.viewportY + term.rows; y++) {
    const row = buf.getLine(y);
    if (!row) break;
    const s = row.translateToString(true);
    const next = buf.getLine(y + 1);
    line += s;
    const nextText = next?.translateToString(true) ?? "";
    const runsOn = next?.isWrapped || (s.length >= term.cols && /[^\s]$/.test(s) && /^[^\s]/.test(nextText) && /https?:\/\/[^\s]*$/.test(line));
    if (!runsOn) {
      text.push(line);
      line = "";
    }
  }
  if (line) text.push(line);
  const found = text.join("\n").match(URL_RE);
  // Closing punctuation of the sentence around it is not part of it.
  return found ? found[found.length - 1].replace(/[).,;:!?\]]+$/, "") : "";
}

async function createSession(machine: string, target: TermTarget, host: HTMLElement, key: string | undefined): Promise<Session> {
  const [{ Terminal: XTerm }, { FitAddon }, { WebLinksAddon }] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit"), import("@xterm/addon-web-links")]);
  await import("@xterm/xterm/css/xterm.css");

  const css = getComputedStyle(document.documentElement);
  const term = new XTerm({
    fontFamily: css.getPropertyValue("--font-mono") || "monospace",
    fontSize: 13,
    lineHeight: 1.2,
    cursorBlink: true,
    scrollback: 5000,
    // Option-drag selects on a Mac while the app tracks the mouse, as
    // Shift-drag does elsewhere.
    macOptionClickForcesSelection: true,
    // Links the app marks itself (OSC 8) open in the browser too.
    linkHandler: { activate: (_e, uri) => void api.openUrl(uri).catch(() => undefined) },
    theme: {
      background: css.getPropertyValue("--terminal").trim() || "#0a0a0a",
      foreground: "#e5e5e5",
      cursor: "#fafafa",
      selectionBackground: "rgba(255, 255, 255, 0.22)",
    },
  });
  // Full-screen harnesses such as Claude Code track the mouse. A plain drag
  // then goes to the app (Shift-drag still selects), and every mouse move
  // the app is told about counts as input, on which xterm drops the
  // selection: it vanishes before it can be copied. So the last selection
  // is kept here until a key is pressed or the terminal is clicked, and
  // copying takes it when xterm has already let go.
  let kept = "";
  term.onSelectionChange(() => {
    if (term.hasSelection()) kept = term.getSelection();
  });
  const selection = () => term.getSelection() || kept;
  // Ctrl+C copies when there is a selection (and is the interrupt when
  // there is none), Ctrl+Shift+C always copies, Cmd+C on a Mac. Ctrl+V and
  // Ctrl+Shift+V reach the browser's paste, which xterm reads; xterm would
  // otherwise send them as control characters.
  term.attachCustomKeyEventHandler((e) => {
    const k = e.key.toLowerCase();
    const ctrlOnly = e.ctrlKey && !e.altKey && !e.metaKey;
    const copyKey = k === "c" && (isMac ? e.metaKey && !e.ctrlKey && !e.altKey : ctrlOnly && (e.shiftKey || !!selection()));
    if (copyKey) {
      if (e.type === "keydown") {
        const text = selection();
        if (text) void api.copyText(text).catch(() => undefined);
        kept = "";
        term.clearSelection();
      }
      e.preventDefault();
      return false;
    }
    if (!isMac && ctrlOnly && k === "v") return false;
    if (e.type === "keydown" && !["Shift", "Control", "Alt", "Meta"].includes(e.key)) kept = "";
    return true;
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new WebLinksAddon((_e, uri) => void api.openUrl(uri).catch(() => undefined)));
  const el = document.createElement("div");
  el.className = "term-el";
  host.appendChild(el);
  // Capture: xterm's own handlers must not see the click first.
  el.addEventListener("mousedown", () => (kept = ""), true);
  // The menu's Copy, or the browser's, while xterm holds no selection.
  el.addEventListener("copy", (e) => {
    if (term.hasSelection() || !kept) return;
    e.clipboardData?.setData("text/plain", kept);
    e.preventDefault();
  });
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
    url: "",
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
  // Look for an address once output settles, not on every chunk.
  let urlTimer = 0;
  const findUrl = () => {
    window.clearTimeout(urlTimer);
    urlTimer = window.setTimeout(() => {
      if (closed) return;
      const url = lastUrlOnScreen(term);
      if (url === s.url) return;
      s.url = url;
      s.view?.refresh();
    }, 150);
  };
  term.onWriteParsed(findUrl);
  term.onScroll(findUrl);
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
  const [url, setUrl] = createSignal("");
  const [copied, setCopied] = createSignal(false);
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
        if (s.url !== url()) setCopied(false);
        setUrl(s.url);
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
        <Show when={url()}>
          {(u) => (
            <span class="term-link">
              <span class="term-link-url mono" title={u()}>
                {u()}
              </span>
              <button class="btn sm ghost" onClick={() => void api.openUrl(u()).catch(() => undefined)}>
                Open link
              </button>
              <button
                class="btn sm ghost"
                onClick={() =>
                  void api
                    .copyText(u())
                    .then(() => setCopied(true))
                    .catch(() => undefined)
                }
              >
                {copied() ? "Copied" : "Copy link"}
              </button>
            </span>
          )}
        </Show>
      </div>
    </div>
  );
}
