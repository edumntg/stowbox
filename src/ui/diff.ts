// Turns an Edit/Write tool call into diff lines for the transcript. Only the
// two colours the product allows: added green, removed red.

export type DiffLine = { kind: "add" | "del" | "ctx" | "file"; text: string };

const MAX_LINES = 80;

function cap(lines: DiffLine[]): DiffLine[] {
  if (lines.length <= MAX_LINES) return lines;
  const hidden = lines.length - MAX_LINES;
  return [...lines.slice(0, MAX_LINES), { kind: "ctx", text: `… ${hidden} more line${hidden === 1 ? "" : "s"}` }];
}

function str(v: unknown): string | null {
  return typeof v === "string" ? v : null;
}

export function diffFor(name: string, input: unknown): DiffLine[] | null {
  if (!input || typeof input !== "object") return null;
  const i = input as Record<string, unknown>;
  const file = str(i.file_path) ?? str(i.notebook_path) ?? "";
  const out: DiffLine[] = [];
  if (name === "Edit") {
    const oldS = str(i.old_string);
    const newS = str(i.new_string);
    if (oldS === null || newS === null) return null;
    if (file) out.push({ kind: "file", text: file });
    for (const l of oldS.split("\n")) out.push({ kind: "del", text: "- " + l });
    for (const l of newS.split("\n")) out.push({ kind: "add", text: "+ " + l });
    return cap(out);
  }
  if (name === "MultiEdit" && Array.isArray(i.edits)) {
    if (file) out.push({ kind: "file", text: file });
    for (const e of i.edits as Record<string, unknown>[]) {
      const oldS = str(e.old_string) ?? "";
      const newS = str(e.new_string) ?? "";
      for (const l of oldS.split("\n")) out.push({ kind: "del", text: "- " + l });
      for (const l of newS.split("\n")) out.push({ kind: "add", text: "+ " + l });
      out.push({ kind: "ctx", text: "" });
    }
    return cap(out);
  }
  if (name === "Write") {
    const content = str(i.content);
    if (content === null) return null;
    if (file) out.push({ kind: "file", text: file });
    for (const l of content.split("\n")) out.push({ kind: "add", text: "+ " + l });
    return cap(out);
  }
  return null;
}
