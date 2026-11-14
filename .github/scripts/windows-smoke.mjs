// Drives the real Stowbox build on Windows with no API key configured: the
// onboarding must render, a malformed key must get the app's own validation
// message, and a well-formed but invalid key must come back with boxd's
// refusal while the app stays on onboarding. Screenshots of the webview and
// of the whole screen (to see the native window frame) go to the output
// directory.
//
// The app is started here and driven over the DevTools protocol, on the port
// the smoke build opens in its WebView2 (see windows-smoke.yml): plain HTTP
// to list the pages, then one WebSocket. No dependencies, nothing to
// download, no driver whose version has to match the runner's WebView2.
// WebDriver was tried first (tauri-driver, then msedgedriver attached to
// that same port) and failed in ways the protocol did not explain; see the
// history of this file.
//
//   node windows-smoke.mjs <path to stowbox.exe> [out dir]
//   node windows-smoke.mjs --attach [out dir]      # a page already open on the port, for trying this script out
//
// To try it here against the browser build and its mock bridge:
//   pnpm dev   (then)   chromium --headless --remote-debugging-port=9222 --remote-allow-origins=* "http://localhost:1420/?onboarding"
// The mock accepts any bxd_ key, so the two boxd checks fail there by design.

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const [app, outArg] = process.argv.slice(2);
if (!app) {
  console.error("usage: node windows-smoke.mjs <stowbox.exe | --attach> [out dir]");
  process.exit(2);
}
const attachOnly = app === "--attach";
const out = resolve(outArg ?? "smoke");
mkdirSync(out, { recursive: true });
const here = dirname(fileURLToPath(import.meta.url));
const DEVTOOLS = Number(process.env.STOWBOX_DEVTOOLS_PORT ?? 9222);
const devtools = `http://127.0.0.1:${DEVTOOLS}`;
const windows = process.platform === "win32";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
};

async function until(what, fn, ms = 30000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
      last = undefined;
    } catch (e) {
      last = e;
    }
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${what}${last ? `: ${last.message}` : ""}`);
}

// One DevTools session on one page: request ids match replies, events are
// ignored. Node 22 has WebSocket built in.
class Page {
  constructor(ws) {
    this.ws = ws;
    this.seq = 0;
    this.pending = new Map();
    ws.addEventListener("message", (m) => {
      const msg = JSON.parse(m.data);
      if (msg.id === undefined) return;
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.reject(new Error(`${p.method}: ${msg.error.message}`));
      else p.resolve(msg.result);
    });
    ws.addEventListener("close", () => {
      for (const p of this.pending.values()) p.reject(new Error(`${p.method}: the page closed`));
      this.pending.clear();
    });
  }

  static async open(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error(`could not open ${url}`)), { once: true });
    });
    return new Page(ws);
  }

  send(method, params = {}) {
    const id = ++this.seq;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { method, resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  /** Runs an expression in the page and returns its value (promises awaited). */
  async eval(expression) {
    const r = await this.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }

  close() {
    this.ws.close();
  }
}

/** What the page is showing, for the log when a check cannot be made. */
const STATE = `(() => ({
  ready: document.readyState,
  url: location.href,
  title: document.title,
  boot: document.querySelector('.boot')?.textContent ?? null,
  body: (document.body?.innerText ?? '').replace(/\\s+/g, ' ').slice(0, 600),
}))()`;

let page;
const exists = (css) => page.eval(`!!document.querySelector(${JSON.stringify(css)})`);
const count = (css) => page.eval(`document.querySelectorAll(${JSON.stringify(css)}).length`);
const text = (css) => page.eval(`document.querySelector(${JSON.stringify(css)})?.textContent ?? null`);
const click = (css) => page.eval(`(() => { const el = document.querySelector(${JSON.stringify(css)}); if (!el) throw new Error('no ' + ${JSON.stringify(css)}); el.click(); })()`);

/** Replaces the field's content the way typing does, so the UI's input
 * handlers run. */
async function type(css, value) {
  await page.eval(`(() => { const el = document.querySelector(${JSON.stringify(css)}); if (!el) throw new Error('no ' + ${JSON.stringify(css)}); el.focus(); el.select(); })()`);
  await page.send("Input.insertText", { text: value });
}

async function shot(name) {
  if (page) {
    try {
      const { data } = await page.send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(out, `${name}-webview.png`), Buffer.from(data, "base64"));
    } catch (e) {
      console.log(`webview capture failed: ${e.message}`);
    }
  }
  if (!windows) return;
  try {
    execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(here, "screen.ps1"), join(out, `${name}-screen.png`)], { stdio: "inherit" });
  } catch (e) {
    console.log(`screen capture failed: ${e.message}`);
  }
}

function windowInfo() {
  const ps = "Get-Process stowbox -ErrorAction SilentlyContinue | Select-Object Id, MainWindowTitle, Responding | ConvertTo-Json -Compress";
  const raw = execFileSync("powershell", ["-NoProfile", "-Command", ps]).toString().trim();
  if (!raw) return [];
  const v = JSON.parse(raw);
  return Array.isArray(v) ? v : [v];
}

async function describe(label) {
  try {
    console.log(`${label}: ${JSON.stringify(await page.eval(STATE))}`);
  } catch (e) {
    console.log(`${label}: page state unavailable: ${e.message}`);
  }
}

const appProc = attachOnly ? null : spawn(resolve(app), [], { stdio: "inherit" });
appProc?.on("exit", (code, signal) => console.log(`the app exited: code ${code}, signal ${signal}`));
let failed = false;
try {
  const version = await until("WebView2's DevTools port", async () => (await fetch(`${devtools}/json/version`)).json(), 60000);
  console.log(`attached to ${version.Browser}`);
  // The app's one window, by its Tauri origin; any other page is logged so
  // a wrong pick is visible.
  const target = await until(
    "the app's page",
    async () => {
      const targets = await (await fetch(`${devtools}/json/list`)).json();
      const pages = targets.filter((t) => t.type === "page");
      return pages.find((t) => /^https?:\/\/tauri\.localhost\//.test(t.url)) ?? pages.find((t) => !/^(about:blank|devtools:|edge:|chrome:)/.test(t.url)) ?? null;
    },
    60000,
  );
  console.log(`pages: ${JSON.stringify((await (await fetch(`${devtools}/json/list`)).json()).map((t) => ({ type: t.type, url: t.url, title: t.title })))}`);
  console.log(`driving ${target.url}`);
  page = await Page.open(target.webSocketDebuggerUrl);
  await page.send("Page.enable");
  await page.send("Runtime.enable");

  try {
    await until("the onboarding key field", () => exists("#ob-key"), 60000);
  } catch (e) {
    await describe("page while waiting for the onboarding");
    throw e;
  }
  const heading = await text(".onboard-card h1");
  check("onboarding renders with no API key", heading === "Connect your boxd account", heading);
  check("no boot error", (await count(".boot-error")) === 0);
  if (windows) {
    const win = windowInfo();
    check("process alive with a window titled Stowbox", win.some((w) => w.MainWindowTitle === "Stowbox" && w.Responding), JSON.stringify(win));
  }
  const ua = await page.eval("navigator.userAgent + ' | ' + navigator.platform + ' | ' + window.innerWidth + 'x' + window.innerHeight + ' | theme ' + document.documentElement.dataset.theme");
  console.log(`webview: ${ua}`);
  const mono = await page.eval("getComputedStyle(document.documentElement).getPropertyValue('--font-mono')");
  check("monospace stack has Windows fonts", /Cascadia Mono/.test(mono) && /Consolas/.test(mono), mono.trim());
  await shot("1-onboarding");

  // A key that is not even shaped like one: the app's own message.
  await type("#ob-key", "bxd_short");
  await click("button[type=submit]");
  const local = await until("the validation error", () => text(".form-error"), 15000);
  check("malformed key gets the validation error", local.includes("does not look like a boxd API key"), local);

  // Shaped like a key, but not one: boxd answers and the app shows it.
  await type("#ob-key", "bxd_stowboxsmoke_not_a_real_key_0000000000");
  await click("button[type=submit]");
  let remote;
  try {
    remote = await until(
      "boxd's answer",
      async () => {
        const t = await text(".form-error");
        const busy = ((await text("button[type=submit]")) ?? "").includes("Checking");
        return !busy && t && !t.includes("does not look like") ? t : null;
      },
      45000,
    );
  } catch (e) {
    await describe("page while waiting for boxd's answer");
    throw e;
  }
  check("invalid key gets boxd's refusal", remote.length > 0, remote);
  check("still on onboarding after the refusal", (await count("#ob-key")) === 1);
  await shot("2-refused");

  // The dark theme is the default; the light one through the same path as
  // the Settings switch, so the native title bar can be compared.
  await page.eval("localStorage.setItem('stowbox.theme', 'light'); location.reload(); true");
  await sleep(1000);
  await until("the onboarding after reload", () => exists("#ob-key"), 30000);
  await sleep(1000);
  check("light theme applied", (await page.eval("document.documentElement.dataset.theme")) === "light");
  await shot("3-light");
} catch (e) {
  failed = true;
  console.error(e);
  await shot("error").catch(() => undefined);
} finally {
  page?.close();
  appProc?.kill();
}

writeFileSync(join(out, "results.json"), JSON.stringify(results, null, 2));
const bad = results.filter((r) => !r.ok);
if (failed || bad.length) {
  console.error(`${bad.length} check(s) failed${failed ? ", and the run stopped on an error" : ""}`);
  process.exit(1);
}
console.log(`all ${results.length} checks passed`);
