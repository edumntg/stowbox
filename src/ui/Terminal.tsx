import { createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { api } from "../bridge/api";
import type { TermTarget } from "../bridge/types";
import { isWindows } from "./common";

/** Writes into an open terminal as if typed. */
export type TermInput = (text: string) => void;

const opening: Record<TermTarget["kind"], string> = {
  shell: "Opening a login shell in the box…",
  agent: "Opening the agent's session in the box…",
  "claude-login": "Starting Claude Code's sign-in in the box…",
};

/** One interactive session inside a box, streamed over the provider's exec
 * channel: a login shell by default, or what `target` names. Nothing has to
 * be installed locally: no ssh, no CLI. */
export default function Terminal(p: {
  machine: string;
  key?: string;
  target?: TermTarget;
  /** The bar's text while connected. */
  hint?: JSX.Element;
  onInput?: (write: TermInput | null) => void;
  onEnd?: () => void;
}) {
  const target = p.target ?? { kind: "shell" };
  let host!: HTMLDivElement;
  const [status, setStatus] = createSignal<"connecting" | "open" | "ended" | "error">("connecting");
  const [detail, setDetail] = createSignal("");
  const [bad, setBad] = createSignal(false);
  let cleanup: (() => void) | null = null;
  let disposed = false;
  onCleanup(() => {
    disposed = true;
    cleanup?.();
  });

  onMount(async () => {
    const [{ Terminal: XTerm }, { FitAddon }] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit")]);
    await import("@xterm/xterm/css/xterm.css");
    if (disposed) return;

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
    term.open(host);
    fit.fit();

    const enc = new TextEncoder();
    const dec = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    let id: string | null = null;

    const onData = term.onData((s) => {
      if (id) void api.terminalWrite(id, enc.encode(s)).catch(() => undefined);
    });
    const ro = new ResizeObserver(() => {
      fit.fit();
      if (id) void api.terminalResize(id, term.cols, term.rows).catch(() => undefined);
    });
    ro.observe(host);

    cleanup = () => {
      p.onInput?.(null);
      onData.dispose();
      ro.disconnect();
      if (id) void api.terminalClose(id).catch(() => undefined);
      term.dispose();
    };

    try {
      id = await api.terminalOpen(p.machine, term.cols, term.rows, target, (ev) => {
        if (ev.type === "data") term.write(dec(ev.b64));
        else if (ev.type === "exit") {
          setStatus("ended");
          setBad(ev.code !== 0);
          setDetail(target.kind === "shell" ? `The shell exited with code ${ev.code}.` : ev.code === 0 ? "Exited." : `Exited with code ${ev.code}.`);
          p.onInput?.(null);
          p.onEnd?.();
        } else {
          setStatus("error");
          setBad(true);
          setDetail(ev.message);
          p.onInput?.(null);
          p.onEnd?.();
        }
      });
      if (disposed) {
        cleanup();
        return;
      }
      setStatus("open");
      const open = id;
      p.onInput?.((text) => void api.terminalWrite(open, enc.encode(text)).catch(() => undefined));
      term.focus();
    } catch (e) {
      setStatus("error");
      setBad(true);
      setDetail(String(e));
    }
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
