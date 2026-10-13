import { createSignal, onCleanup, onMount } from "solid-js";
import { api } from "../bridge/api";

/** One interactive shell inside a box, streamed over the provider's exec
 * channel. Nothing has to be installed locally: no ssh, no CLI. */
export default function Terminal(p: { machine: string; key?: string }) {
  let host!: HTMLDivElement;
  const [status, setStatus] = createSignal<"connecting" | "open" | "ended" | "error">("connecting");
  const [detail, setDetail] = createSignal("");
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
        background: css.getPropertyValue("--term-bg").trim() || "#121212",
        foreground: "#e6e6e6",
        cursor: "#f2b43c",
        selectionBackground: "rgba(242, 180, 60, 0.3)",
      },
    });
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
      onData.dispose();
      ro.disconnect();
      if (id) void api.terminalClose(id).catch(() => undefined);
      term.dispose();
    };

    try {
      id = await api.terminalOpen(p.machine, term.cols, term.rows, (ev) => {
        if (ev.type === "data") term.write(dec(ev.b64));
        else if (ev.type === "exit") {
          setStatus("ended");
          setDetail(`The shell exited with code ${ev.code}.`);
        } else {
          setStatus("error");
          setDetail(ev.message);
        }
      });
      if (disposed) {
        cleanup();
        return;
      }
      setStatus("open");
      term.focus();
    } catch (e) {
      setStatus("error");
      setDetail(String(e));
    }
  });

  return (
    <div class="term-wrap">
      <div class="term-host" ref={host} />
      <div class="term-bar" classList={{ ended: status() === "ended" || status() === "error" }}>
        {status() === "connecting" && <span>Opening a login shell in the box…</span>}
        {status() === "open" && <span>Connected. Close the tab to end the session.</span>}
        {(status() === "ended" || status() === "error") && <span>{detail() || "The session ended."}</span>}
      </div>
    </div>
  );
}
