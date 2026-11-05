// `{{name}}` placeholders in setup preset commands. Mirrors
// stowbox_core::provision (placeholders, variables_in, substitute): only
// `{{` + a valid variable name + `}}` counts, spaces inside the braces are
// allowed, and a value goes in as one single-quoted shell word.

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

function placeholders(cmd: string): { start: number; end: number; name: string }[] {
  const out: { start: number; end: number; name: string }[] = [];
  let i = 0;
  for (;;) {
    const start = cmd.indexOf("{{", i);
    if (start < 0) break;
    const close = cmd.indexOf("}}", start + 2);
    if (close < 0) break;
    const name = cmd.slice(start + 2, close).replace(/^ +| +$/g, "");
    if (NAME.test(name)) {
      out.push({ start, end: close + 2, name });
      i = close + 2;
    } else {
      i = start + 1;
    }
  }
  return out;
}

/** Variable names used in `commands`, once each, in order of first use. */
export function variablesIn(commands: string[]): string[] {
  const names: string[] = [];
  for (const c of commands) for (const p of placeholders(c)) if (!names.includes(p.name)) names.push(p.name);
  return names;
}

const singleQuote = (s: string) => `'${s.replaceAll("'", `'\\''`)}'`;

/** Fills every placeholder; throws on one without a value. */
export function substitute(cmd: string, values: Record<string, string>): string {
  let out = "";
  let last = 0;
  for (const p of placeholders(cmd)) {
    const v = values[p.name];
    if (v === undefined) throw new Error(`{{${p.name}}} has no value`);
    out += cmd.slice(last, p.start) + singleQuote(v);
    last = p.end;
  }
  return out + cmd.slice(last);
}

/** A preset's variables as its commands use them now, with stored defaults.
 * Reading the commands also covers presets saved before variables existed. */
export function presetVariables(p: { commands: string[]; variables?: { name: string; default: string }[] }) {
  return variablesIn(p.commands).map((name) => ({ name, default: p.variables?.find((v) => v.name === name)?.default ?? "" }));
}
