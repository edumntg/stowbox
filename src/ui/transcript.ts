import type { AgentEvent, Attachment } from "../bridge/types";

export type Item =
  | { kind: "user"; text: string; attachments: Attachment[] }
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool"; id: string; name: string; input: unknown; result?: string; isError?: boolean }
  | { kind: "result"; ok: boolean; message: string | null; costUsd: number | null; durationMs: number | null; turns: number | null; subtype: string }
  | { kind: "note"; text: string }
  | { kind: "exit"; code: number };

/** Folds raw events into renderable items: tool results attach to their call,
 * consecutive stderr lines become one note. */
export function toItems(events: AgentEvent[]): Item[] {
  const items: Item[] = [];
  for (const ev of events) {
    switch (ev.type) {
      case "user_message":
        items.push({ kind: "user", text: ev.text, attachments: ev.attachments ?? [] });
        break;
      case "assistant":
        for (const b of ev.blocks) {
          if (b.type === "text") items.push({ kind: "text", text: b.text });
          else if (b.type === "thinking") items.push({ kind: "thinking", text: b.text });
          else items.push({ kind: "tool", id: b.id, name: b.name, input: b.input });
        }
        break;
      case "tool_result": {
        let attached = false;
        for (let i = items.length - 1; i >= 0; i--) {
          const it = items[i];
          if (it.kind === "tool" && it.id === ev.tool_use_id) {
            it.result = ev.content;
            it.isError = ev.is_error;
            attached = true;
            break;
          }
        }
        if (!attached) items.push({ kind: "tool", id: ev.tool_use_id, name: "tool", input: null, result: ev.content, isError: ev.is_error });
        break;
      }
      case "result":
        items.push({ kind: "result", ok: ev.ok, message: ev.message, costUsd: ev.cost_usd, durationMs: ev.duration_ms, turns: ev.turns, subtype: ev.subtype });
        break;
      case "stderr": {
        const last = items[items.length - 1];
        if (last && last.kind === "note") last.text += "\n" + ev.text;
        else items.push({ kind: "note", text: ev.text });
        break;
      }
      case "exit":
        items.push({ kind: "exit", code: ev.code });
        break;
      case "init":
      case "text_delta":
        break;
    }
  }
  return items;
}

/** The item that shows event `ev` of `events` (as `toItems` folds them), or
 * -1 when the event shows nothing. A tool result is its tool call's card; a
 * stderr line folded into the note before it is that note. */
export function itemIndexForEvent(events: AgentEvent[], ev: number): number {
  const e = events[ev];
  if (!e) return -1;
  const before = toItems(events.slice(0, ev));
  const after = toItems(events.slice(0, ev + 1));
  if (e.type === "tool_result") {
    for (let i = after.length - 1; i >= 0; i--) {
      const it = after[i];
      if (it.kind === "tool" && it.id === e.tool_use_id) return i;
    }
    return -1;
  }
  if (after.length > before.length) return before.length;
  // Folded into the last item (a stderr line joining a note).
  return after.length && e.type === "stderr" ? after.length - 1 : -1;
}

/** One line that says what a tool call is doing, for the collapsed card. */
export function toolSummary(name: string, input: unknown): string {
  if (!input || typeof input !== "object") return "";
  const i = input as Record<string, unknown>;
  const pick = (k: string) => (typeof i[k] === "string" ? (i[k] as string) : "");
  switch (name) {
    case "Bash":
    case "command":
      return pick("command");
    case "Read":
    case "Write":
    case "Edit":
    case "MultiEdit":
    case "NotebookEdit":
      return pick("file_path") || pick("notebook_path");
    case "Glob":
    case "Grep":
      return [pick("pattern"), pick("path")].filter(Boolean).join("  in  ");
    case "WebFetch":
    case "WebSearch":
      return pick("url") || pick("query");
    case "Task":
      return pick("description");
    default: {
      const s = JSON.stringify(input);
      return s.length > 120 ? s.slice(0, 117) + "…" : s;
    }
  }
}
