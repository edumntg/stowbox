// Diff lines for the Changes view. Only the two colours the product allows:
// added green, removed red.

export type DiffLine = { kind: "add" | "del" | "ctx" | "file" | "hunk"; text: string };

/** Lines of a unified diff's hunks (as git prints them) with their kind. */
export function patchLines(patch: string): DiffLine[] {
  const lines = patch.endsWith("\n") ? patch.slice(0, -1).split("\n") : patch.split("\n");
  return lines.map((text) => ({
    kind: text.startsWith("@@") ? "hunk" : text.startsWith("+") ? "add" : text.startsWith("-") ? "del" : "ctx",
    text,
  }));
}
