// stowbox-bridge: the Worker Stowbox talks to for Cloudflare Containers.
//
// Cloudflare has no hosted sandbox API: a container is started by a Durable
// Object in the person's own account. This Worker is that Durable Object,
// plus a small authenticated API over it (see README.md for the protocol).
//
// Every request carries the bridge token as a bearer token. Nothing here
// logs a request body or a command line: Stowbox puts a box's environment,
// which may hold keys, into the command it runs.

import { DurableObject } from "cloudflare:workers";
import {
  BOX_ID,
  MAX_BODY,
  MAX_BOXES,
  MIN_TOKEN,
  bearer,
  frame,
  newBoxId,
  parseControl,
  parseCreate,
  parseIdle,
  parseStart,
  type Size,
} from "./checks.js";

const VERSION = 1;
/** How long a new exec socket may wait for its start message. */
const START_WAIT_MS = 30_000;
/** Input that arrives before the command is running, held at most. */
const MAX_EARLY_INPUT = 1 << 20;
/** Output queued on a socket before the bridge stops reading the command. */
const MAX_QUEUED_OUTPUT = 1 << 20;
/** How often an open exec tells the registry the box is in use. */
const HEARTBEAT_MS = 60_000;
/** Process 1 in a box: waits forever, and exits on the stop signal. */
const INIT = 'trap "exit 0" TERM; while :; do sleep 86400 & wait $!; done';

interface Env {
  STOWBOX_TOKEN?: string;
  REGISTRY: DurableObjectNamespace<Registry>;
  BOX_STANDARD_2: DurableObjectNamespace<Box>;
  BOX_STANDARD_3: DurableObjectNamespace<Box>;
  BOX_STANDARD_4: DurableObjectNamespace<Box>;
}

/**
 * What the registry keeps of a box. `started_at` is 0 while stopped, and
 * `seen_at` is the last time anything used the box: the container stops
 * `idle_secs` after that, so the list can say so without waking the box,
 * which would count as use and keep it running.
 */
interface Record_ {
  id: string;
  name: string;
  size: Size;
  created_at: number;
  idle_secs: number;
  started_at: number;
  seen_at: number;
}

interface Meta {
  id: string;
  idleSecs: number;
  env: Record<string, string>;
}

function now(): number {
  return Math.floor(Date.now() / 1000);
}

function namespace(env: Env, size: Size): DurableObjectNamespace<Box> {
  switch (size) {
    case "standard-2":
      return env.BOX_STANDARD_2;
    case "standard-3":
      return env.BOX_STANDARD_3;
    case "standard-4":
      return env.BOX_STANDARD_4;
  }
}

/** The list of boxes, kept apart because Durable Objects cannot be listed. */
export class Registry extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.storage.sql.exec(
      `CREATE TABLE IF NOT EXISTS box (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, size TEXT NOT NULL, created_at INTEGER NOT NULL,
        idle_secs INTEGER NOT NULL, started_at INTEGER NOT NULL DEFAULT 0, seen_at INTEGER NOT NULL DEFAULT 0)`,
    );
  }

  /** Adds a box, or returns false when the bridge already holds the most. */
  add(r: Record_): boolean {
    const count = this.ctx.storage.sql.exec<{ n: number }>("SELECT COUNT(*) AS n FROM box").one().n;
    if (count >= MAX_BOXES) return false;
    this.ctx.storage.sql.exec(
      "INSERT INTO box (id, name, size, created_at, idle_secs, started_at, seen_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      r.id,
      r.name,
      r.size,
      r.created_at,
      r.idle_secs,
      r.started_at,
      r.seen_at,
    );
    return true;
  }

  get(id: string): Record_ | null {
    const rows = this.ctx.storage.sql.exec<Record<string, string | number>>("SELECT * FROM box WHERE id = ?", id).toArray();
    return rows.length === 0 ? null : (rows[0] as unknown as Record_);
  }

  list(): Record_[] {
    return this.ctx.storage.sql.exec<Record<string, string | number>>("SELECT * FROM box ORDER BY created_at").toArray() as unknown as Record_[];
  }

  /** The box was used just now. */
  seen(id: string): void {
    this.ctx.storage.sql.exec("UPDATE box SET seen_at = ? WHERE id = ?", now(), id);
  }

  /** The container started (`at` > 0) or stopped (0). */
  started(id: string, at: number): void {
    this.ctx.storage.sql.exec("UPDATE box SET started_at = ?, seen_at = ? WHERE id = ?", at, now(), id);
  }

  setIdle(id: string, secs: number): void {
    this.ctx.storage.sql.exec("UPDATE box SET idle_secs = ?, seen_at = ? WHERE id = ?", secs, now(), id);
  }

  remove(id: string): void {
    this.ctx.storage.sql.exec("DELETE FROM box WHERE id = ?", id);
  }
}

/** One box: a Durable Object and the container it starts. */
export class Box extends DurableObject<Env> {
  readonly #container: Container;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    const container = ctx.container;
    if (container === undefined) throw new Error("this Durable Object has no container attached");
    this.#container = container;
    // The timeout belongs to this instance, not the container, so a new
    // instance sets it again.
    if (container.running) {
      void ctx.blockConcurrencyWhile(async () => {
        const meta = await this.#meta();
        if (meta !== undefined) await container.setInactivityTimeout(meta.idleSecs * 1000);
      });
    }
  }

  get #registry() {
    return this.env.REGISTRY.getByName("registry");
  }

  async #meta(): Promise<Meta | undefined> {
    return this.ctx.storage.get<Meta>("meta");
  }

  async #ensureRunning(): Promise<Meta> {
    const meta = await this.#meta();
    if (meta === undefined) throw new Error("this box was never set up");
    if (!this.#container.running) {
      // The image's own command is a REPL that exits without a terminal.
      // Bash as process 1 reaps what ended sessions leave behind, which
      // `sleep` would leave as zombies.
      this.#container.start({ entrypoint: ["bash", "-c", INIT], enableInternet: true, env: meta.env, labels: { box: meta.id } });
      await this.#registry.started(meta.id, now());
    } else {
      await this.#registry.seen(meta.id);
    }
    await this.#container.setInactivityTimeout(meta.idleSecs * 1000);
    return meta;
  }

  async init(meta: Meta): Promise<void> {
    await this.ctx.storage.put("meta", meta);
    await this.#ensureRunning();
  }

  async start(): Promise<void> {
    await this.#ensureRunning();
  }

  async stop(): Promise<void> {
    if (this.#container.running) await this.#container.destroy();
    const meta = await this.#meta();
    if (meta !== undefined) await this.#registry.started(meta.id, 0);
  }

  async setIdle(secs: number): Promise<void> {
    const meta = await this.#meta();
    if (meta === undefined) throw new Error("this box was never set up");
    await this.ctx.storage.put("meta", { ...meta, idleSecs: secs });
    if (this.#container.running) await this.#container.setInactivityTimeout(secs * 1000);
    await this.#registry.setIdle(meta.id, secs);
  }

  /** Stops the container and forgets the box. */
  async remove(): Promise<void> {
    if (this.#container.running) await this.#container.destroy();
    await this.ctx.storage.deleteAll();
  }

  /**
   * Hangs up the session `pid` leads, so a shell passes it to its jobs, then
   * kills whatever is left of it a second later. This works after the
   * leader has exited, too. The pid is the runtime's number, never a string
   * from the socket.
   */
  async #endSession(pid: number): Promise<void> {
    const script = 'pkill -HUP -s "$1"; sleep 1; pkill -KILL -s "$1"; true';
    try {
      const p = await this.#container.exec(["sh", "-c", script, "stop", String(Math.trunc(pid))]);
      await Promise.race([p.exitCode, new Promise((r) => setTimeout(r, 5_000))]);
    } catch (cause) {
      console.error({ event: "exec.stop.failed", error: describe(cause) });
    }
  }

  /** An exec socket. The Worker has checked the token and the box. */
  override async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      return json({ error: "exec needs a WebSocket upgrade" }, 426);
    }
    const [client, server] = Object.values(new WebSocketPair());
    server.binaryType = "arraybuffer";
    // Accepted, not hibernated: hibernation would drop the process handle.
    server.accept();
    void this.#exec(server);
    return new Response(null, { status: 101, webSocket: client });
  }

  async #exec(ws: WebSocket): Promise<void> {
    const abort = new AbortController();
    let proc: ExecProcess | null = null;
    let input: WritableStreamDefaultWriter<Uint8Array> | null = null;
    let early: Uint8Array[] = [];
    let earlyBytes = 0;
    let earlyEof = false;
    let earlySize: { cols: number; rows: number } | null = null;
    let exited = false;
    // The exit went to the client: what the command left running is its own.
    let finished = false;
    let closed = false;
    let ended = false;
    let started: ((text: string) => void) | null = null;
    const firstText = new Promise<string>((resolve, reject) => {
      started = resolve;
      setTimeout(() => reject(new Error("no start message")), START_WAIT_MS);
    });

    // Each exec leads a session of its own, so ending the session ends what
    // the command left running too, whether the socket closed or broke, and
    // even when the command itself has already exited.
    const end = () => {
      if (ended || finished || proc === null) return;
      ended = true;
      const pid = proc.pid;
      this.ctx.waitUntil(
        this.#endSession(pid).finally(() => {
          // Killing a command that already exited records an error.
          if (!exited) abort.abort();
        }),
      );
    };
    const stop = () => {
      closed = true;
      if (proc === null) abort.abort();
      else end();
    };
    const fail = (message: string) => {
      stop();
      try {
        ws.send(JSON.stringify({ type: "error", message }));
        ws.close(1011, "exec failed");
      } catch {
        // The socket is already gone.
      }
    };
    const write = (data: Uint8Array) => {
      if (input !== null) {
        input.write(data).catch(stop);
      } else if (earlyBytes + data.length <= MAX_EARLY_INPUT) {
        early.push(data);
        earlyBytes += data.length;
      } else {
        fail("too much input before the command started");
      }
    };

    // A listener that throws closes the socket without "close" firing, which
    // would leave the command running, so neither throws.
    ws.addEventListener("message", (event) => {
      if (event.data instanceof ArrayBuffer) {
        write(new Uint8Array(event.data));
        return;
      }
      if (started !== null) {
        const resolve = started;
        started = null;
        resolve(event.data);
        return;
      }
      const control = parseControl(event.data);
      if (control?.type === "resize") {
        if (proc === null) earlySize = control;
        else resize(proc, control);
      } else if (control?.type === "eof") {
        if (input !== null) input.close().catch(() => {});
        else earlyEof = true;
      }
    });
    ws.addEventListener("close", stop);
    ws.addEventListener("error", stop);

    let start;
    try {
      start = parseStart(await firstText);
    } catch {
      fail("no start message within 30 s");
      return;
    }
    if (typeof start === "string") {
      fail(start);
      return;
    }

    let meta: Meta;
    try {
      meta = await this.#ensureRunning();
      proc = await this.#container.exec(start.argv, {
        pty: start.tty ? { cols: start.cols, rows: start.rows } : undefined,
        env: start.tty ? { TERM: "xterm-256color" } : undefined,
        stdin: "pipe",
        stdout: "pipe",
        stderr: start.tty ? undefined : "pipe",
        signal: abort.signal,
      });
    } catch (cause) {
      if (closed) return;
      console.error({ event: "exec.start.failed", error: describe(cause) });
      fail("the box could not start the command");
      return;
    }
    const running = proc;
    running.exitCode.then(
      () => (exited = true),
      () => (exited = true),
    );
    // The socket went while the command was starting.
    if (closed) {
      end();
      return;
    }
    // A stream the command does not have is null or, on a PTY, absent.
    if (running.stdin) {
      input = running.stdin.getWriter();
      for (const chunk of early) input.write(chunk).catch(stop);
      if (earlyEof) input.close().catch(() => {});
    }
    early = [];
    if (earlySize !== null) resize(running, earlySize);

    // An open socket keeps the container up; the registry is told so.
    const id = meta.id;
    const heartbeat = setInterval(() => void this.#registry.seen(id).catch(() => {}), HEARTBEAT_MS);

    const pump = async (stream: ReadableStream<Uint8Array> | null | undefined, which: 1 | 2) => {
      if (!stream) return;
      for await (const chunk of stream) {
        // Nobody is reading; stop() is ending the command.
        if (ws.readyState !== WebSocket.OPEN) return;
        ws.send(frame(which, chunk));
        // A slow client: the command waits on its pipe rather than the
        // queue growing in this object's memory.
        while (ws.bufferedAmount > MAX_QUEUED_OUTPUT && ws.readyState === WebSocket.OPEN) {
          await new Promise((r) => setTimeout(r, 20));
        }
      }
    };
    try {
      await Promise.all([pump(running.stdout, 1), pump(running.stderr, 2)]);
      const code = await running.exitCode;
      if (ws.readyState !== WebSocket.OPEN) return;
      finished = true;
      ws.send(JSON.stringify({ type: "exit", code }));
      ws.close(1000, "exited");
    } catch (cause) {
      if (abort.signal.aborted) return;
      console.error({ event: "exec.output.failed", error: describe(cause) });
      fail("the command's output broke");
    } finally {
      clearInterval(heartbeat);
      void this.#registry.seen(id).catch(() => {});
    }
  }
}

/** The three classes differ only in the instance type wrangler gives them. */
export class BoxStandard2 extends Box {}
export class BoxStandard3 extends Box {}
export class BoxStandard4 extends Box {}

function resize(proc: ExecProcess, size: { cols: number; rows: number }): void {
  if (!proc.isPty) return;
  try {
    proc.resize(size.cols, size.rows);
  } catch {
    // The command may have just exited.
  }
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/** Structured logs drop an Error's message unless it is turned into text. */
function describe(cause: unknown): string {
  return cause instanceof Error ? `${cause.name}: ${cause.message}` : String(cause);
}

async function sha256(s: string): Promise<ArrayBuffer> {
  return crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
}

/** Compares digests, so the time taken says nothing about the token. */
async function authorized(request: Request, token: string): Promise<boolean> {
  const given = bearer(request.headers.get("Authorization"));
  if (given === null) return false;
  const [a, b] = await Promise.all([sha256(given), sha256(token)]);
  return crypto.subtle.timingSafeEqual(a, b);
}

/** The body as JSON, refused past MAX_BODY bytes before more is read. */
async function readJson(request: Request): Promise<unknown> {
  if (Number(request.headers.get("content-length") ?? "0") > MAX_BODY) throw new RangeError("body too large");
  if (request.body === null) return {};
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  for await (const chunk of request.body) {
    bytes += chunk.byteLength;
    if (bytes > MAX_BODY) throw new RangeError("body too large");
    chunks.push(chunk);
  }
  const all = new Uint8Array(bytes);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  const text = new TextDecoder().decode(all);
  return text === "" ? {} : JSON.parse(text);
}

function boxJson(r: Record_) {
  // The container stops idle_secs after its last use; a minute of slack
  // covers the heartbeat.
  const running = r.started_at > 0 && now() < r.seen_at + r.idle_secs + HEARTBEAT_MS / 1000;
  return {
    id: r.id,
    name: r.name,
    size: r.size,
    created_at: r.created_at,
    status: running ? "running" : "stopped",
    started_at: running ? r.started_at : 0,
    idle_secs: r.idle_secs,
  };
}

async function route(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const registry = env.REGISTRY.getByName("registry");
  const path = url.pathname;
  const method = request.method;

  if (path === "/v1/info" && method === "GET") {
    return json({ bridge: "stowbox-bridge", version: VERSION });
  }

  // Listing reads the registry only: waking each box would keep it up.
  if (path === "/v1/boxes" && method === "GET") {
    return json({ boxes: (await registry.list()).map(boxJson) });
  }

  if (path === "/v1/boxes" && method === "POST") {
    let body: unknown;
    try {
      body = await readJson(request);
    } catch {
      return json({ error: "the body must be JSON of at most 256 KiB" }, 400);
    }
    const req = parseCreate(body);
    if (typeof req === "string") return json({ error: req }, 400);
    const at = now();
    const record: Record_ = {
      id: newBoxId(crypto.getRandomValues(new Uint8Array(32))),
      name: req.name,
      size: req.size,
      created_at: at,
      idle_secs: req.idleSecs,
      started_at: 0,
      seen_at: at,
    };
    if (!(await registry.add(record))) return json({ error: `this bridge already holds ${MAX_BOXES} boxes` }, 409);
    const box = namespace(env, req.size).getByName(record.id);
    try {
      await box.init({ id: record.id, idleSecs: req.idleSecs, env: req.env });
    } catch (cause) {
      // A container that started before the failure must not be left behind.
      await box.remove().catch(() => {});
      await registry.remove(record.id);
      throw cause;
    }
    return json(boxJson((await registry.get(record.id)) ?? record), 201);
  }

  const m = /^\/v1\/boxes\/([^/]+)(?:\/(start|stop|idle|exec))?$/.exec(path);
  if (m === null) return json({ error: "not found" }, 404);
  const [, id, action] = m;
  if (!BOX_ID.test(id)) return json({ error: "no such box" }, 404);
  const record = await registry.get(id);
  if (record === null) return json({ error: "no such box" }, 404);
  const box = namespace(env, record.size).getByName(id);
  const fresh = async () => json(boxJson((await registry.get(id)) ?? record));

  if (action === undefined && method === "GET") return json(boxJson(record));
  if (action === undefined && method === "DELETE") {
    await box.remove();
    await registry.remove(id);
    return new Response(null, { status: 204 });
  }
  if (action === "start" && method === "POST") {
    await box.start();
    return fresh();
  }
  if (action === "stop" && method === "POST") {
    await box.stop();
    return fresh();
  }
  if (action === "idle" && method === "PUT") {
    let body: unknown;
    try {
      body = await readJson(request);
    } catch {
      return json({ error: "the body must be JSON" }, 400);
    }
    const secs = parseIdle((body as Record<string, unknown> | null)?.secs);
    if (typeof secs === "string") return json({ error: secs }, 400);
    await box.setIdle(secs);
    return fresh();
  }
  if (action === "exec" && method === "GET") return box.fetch(new Request(url, request));
  return json({ error: "method not allowed" }, 405);
}

export default {
  async fetch(request, env): Promise<Response> {
    const token = env.STOWBOX_TOKEN ?? "";
    if (token.length < MIN_TOKEN) {
      return json({ error: `the bridge has no STOWBOX_TOKEN secret of at least ${MIN_TOKEN} characters` }, 503);
    }
    if (!(await authorized(request, token))) return json({ error: "the bridge token was refused" }, 401);
    try {
      return await route(request, env);
    } catch (cause) {
      console.error({ event: "request.failed", path: new URL(request.url).pathname, error: describe(cause) });
      return json({ error: "the bridge failed; its Workers logs say why" }, 500);
    }
  },
} satisfies ExportedHandler<Env>;
