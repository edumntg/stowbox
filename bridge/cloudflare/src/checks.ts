// What the bridge accepts, as pure functions, so they test under plain Node.
// Everything here runs on input from the network before it is used.

/** Ids the bridge mints: `box-` and 20 lowercase letters or digits. */
export const BOX_ID = /^box-[a-z0-9]{20}$/;
/** A name a person gave a box. It is only stored and shown, never run. */
export const BOX_NAME = /^[a-z0-9][a-z0-9-]{0,62}$/;
/** An environment variable name for a box. */
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

/** The sizes this Worker deploys, one Durable Object class each. */
export const SIZES = ["standard-2", "standard-3", "standard-4"] as const;
export type Size = (typeof SIZES)[number];
export const DEFAULT_SIZE: Size = "standard-2";

/** How long a box keeps running with nothing connected, in seconds. */
export const DEFAULT_IDLE_SECS = 30 * 60;
export const MIN_IDLE_SECS = 60;
export const MAX_IDLE_SECS = 24 * 60 * 60;

/** Most boxes one bridge keeps. */
export const MAX_BOXES = 100;
/** Largest JSON body read from a request. */
export const MAX_BODY = 256 << 10;
/** Largest start message, and most arguments in it. */
export const MAX_START = 256 << 10;
const MAX_ARGS = 64;
/** Most box environment variables, and their total size. */
const MAX_ENV = 64;
const MAX_ENV_BYTES = 64 << 10;
/** The shortest token the bridge will run with. */
export const MIN_TOKEN = 32;

export function isSize(s: unknown): s is Size {
  return typeof s === "string" && (SIZES as readonly string[]).includes(s);
}

/** A new box id from 20 random letters and digits. */
export function newBoxId(random: Uint8Array): string {
  const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789";
  let out = "box-";
  // 36 does not divide 256, so bytes past 252 are skipped to keep it even.
  for (const b of random) {
    if (out.length === 24) break;
    if (b < 252) out += alphabet[b % 36];
  }
  return out;
}

/** The token from `Authorization: Bearer <token>`, or null. */
export function bearer(header: string | null): string | null {
  if (header === null) return null;
  const m = /^Bearer ([\x21-\x7e]{1,512})$/.exec(header);
  return m === null ? null : m[1];
}

export interface CreateRequest {
  name: string;
  size: Size;
  idleSecs: number;
  env: Record<string, string>;
}

/** A create body: `{ name, size?, idle_secs?, env? }`, or why it is refused. */
export function parseCreate(body: unknown): CreateRequest | string {
  if (typeof body !== "object" || body === null || Array.isArray(body)) return "the body must be a JSON object";
  const b = body as Record<string, unknown>;
  if (typeof b.name !== "string" || !BOX_NAME.test(b.name)) {
    return "name must be 1-63 lowercase letters, digits or hyphens, starting with a letter or digit";
  }
  const size = b.size ?? DEFAULT_SIZE;
  if (!isSize(size)) return `size must be one of ${SIZES.join(", ")}`;
  const idle = parseIdle(b.idle_secs ?? DEFAULT_IDLE_SECS);
  if (typeof idle === "string") return idle;
  const env = parseEnv(b.env ?? {});
  if (typeof env === "string") return env;
  return { name: b.name, size, idleSecs: idle, env };
}

export function parseIdle(v: unknown): number | string {
  if (typeof v !== "number" || !Number.isInteger(v) || v < MIN_IDLE_SECS || v > MAX_IDLE_SECS) {
    return `idle_secs must be a whole number from ${MIN_IDLE_SECS} to ${MAX_IDLE_SECS}`;
  }
  return v;
}

function parseEnv(v: unknown): Record<string, string> | string {
  if (typeof v !== "object" || v === null || Array.isArray(v)) return "env must be an object of strings";
  const entries = Object.entries(v as Record<string, unknown>);
  if (entries.length > MAX_ENV) return `env may hold at most ${MAX_ENV} variables`;
  let bytes = 0;
  const out: Record<string, string> = {};
  for (const [k, val] of entries) {
    if (!ENV_NAME.test(k) || typeof val !== "string" || val.includes("\0")) return "env names must be shell names and values strings without NUL";
    bytes += k.length + val.length;
    out[k] = val;
  }
  return bytes > MAX_ENV_BYTES ? `env may hold at most ${MAX_ENV_BYTES} bytes` : out;
}

export interface Start {
  argv: string[];
  tty: boolean;
  cols: number;
  rows: number;
}

/** A terminal size, clamped to what a PTY can take. */
function dim(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isInteger(v) ? Math.min(Math.max(v, 1), 1000) : fallback;
}

/**
 * The first message on an exec socket:
 * `{ type: "start", argv: [..], tty?, cols?, rows? }`, or why it is refused.
 * The argv runs in the box as given; the bridge's token is what makes that
 * the box owner's choice.
 */
export function parseStart(text: string): Start | string {
  if (text.length > MAX_START) return "the start message is too large";
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return "the start message is not JSON";
  }
  if (typeof v !== "object" || v === null) return "the start message must be an object";
  const m = v as Record<string, unknown>;
  if (m.type !== "start") return "the first message must be a start";
  const argv = m.argv;
  if (!Array.isArray(argv) || argv.length === 0 || argv.length > MAX_ARGS) return `argv must hold 1 to ${MAX_ARGS} strings`;
  if (!argv.every((a): a is string => typeof a === "string" && !a.includes("\0")) || argv[0] === "") {
    return "argv must hold strings without NUL, the first not empty";
  }
  return { argv, tty: m.tty === true, cols: dim(m.cols, 80), rows: dim(m.rows, 24) };
}

export type Control = { type: "resize"; cols: number; rows: number } | { type: "eof" };

/** A text message after the start: a resize or the end of input. */
export function parseControl(text: string): Control | null {
  if (text.length > 1024) return null;
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof v !== "object" || v === null) return null;
  const m = v as Record<string, unknown>;
  if (m.type === "eof") return { type: "eof" };
  if (m.type === "resize") return { type: "resize", cols: dim(m.cols, 80), rows: dim(m.rows, 24) };
  return null;
}

/** Output for the socket: one byte for the stream (1 stdout, 2 stderr), then the bytes. */
export function frame(stream: 1 | 2, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(data.length + 1);
  out[0] = stream;
  out.set(data, 1);
  return out;
}
