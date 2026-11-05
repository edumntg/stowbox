// Drives the real Stowbox build on Windows over WebDriver (msedgedriver,
// which starts the exe and attaches to its WebView2), with no API
// key configured: the onboarding must render, a malformed key must get the
// app's own validation message, and a well-formed but invalid key must come
// back with boxd's refusal while the app stays on onboarding. Screenshots of
// the webview and of the whole screen (to see the native window frame) go to
// the output directory with msedgedriver's verbose log. No dependencies:
// WebDriver is plain HTTP. tauri-driver is not needed on Windows: all it
// does there is turn `tauri:options` into the `ms:edgeOptions` sent below.
//
//   node windows-smoke.mjs <path to stowbox.exe> <path to msedgedriver.exe> <output dir>

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const [app, nativeDriver, outArg] = process.argv.slice(2);
if (!app || !nativeDriver) {
  console.error("usage: node windows-smoke.mjs <stowbox.exe> <msedgedriver.exe> [out dir]");
  process.exit(2);
}
const out = resolve(outArg ?? "smoke");
mkdirSync(out, { recursive: true });
const here = dirname(fileURLToPath(import.meta.url));
const base = "http://127.0.0.1:4444";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
};

async function wd(method, path, body) {
  const r = await fetch(base + path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${JSON.stringify(j.value ?? j)}`);
  return j.value;
}

async function until(what, fn, ms = 30000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${what}${last ? `: ${last.message}` : ""}`);
}

const ELEMENT = "element-6066-11e4-a6c6-4ef7f9e68fda";
let session;
const find = async (css) => (await wd("POST", `/session/${session}/element`, { using: "css selector", value: css }))[ELEMENT];
const findAll = async (css) => (await wd("POST", `/session/${session}/elements`, { using: "css selector", value: css })).map((e) => e[ELEMENT]);
const text = async (el) => wd("GET", `/session/${session}/element/${el}/text`);
const run = (script, args = []) => wd("POST", `/session/${session}/execute/sync`, { script, args });

async function shot(name) {
  if (session) {
    const png = await wd("GET", `/session/${session}/screenshot`);
    writeFileSync(join(out, `${name}-webview.png`), Buffer.from(png, "base64"));
  }
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

const driver = spawn(nativeDriver, ["--port=4444", "--verbose", `--log-path=${join(out, "msedgedriver.log")}`], { stdio: "inherit" });
let failed = false;
try {
  await until("msedgedriver", async () => (await fetch(base + "/status")).ok, 30000);
  session = (
    await wd("POST", "/session", {
      capabilities: { alwaysMatch: { browserName: "webview2", "ms:edgeChromium": true, "ms:edgeOptions": { binary: resolve(app), args: [] } } },
    })
  ).sessionId;
  console.log(`session ${session}`);

  const key = await until("the onboarding key field", () => find("#ob-key"), 60000);
  const heading = await text(await find(".onboard-card h1"));
  check("onboarding renders with no API key", heading === "Connect your boxd account", heading);
  check("no boot error", (await findAll(".boot-error")).length === 0);
  const win = windowInfo();
  check("process alive with a window titled Stowbox", win.some((w) => w.MainWindowTitle === "Stowbox" && w.Responding), JSON.stringify(win));
  const ua = await run("return navigator.userAgent + ' | ' + navigator.platform + ' | ' + window.innerWidth + 'x' + window.innerHeight + ' | theme ' + document.documentElement.dataset.theme");
  console.log(`webview: ${ua}`);
  const mono = await run("return getComputedStyle(document.documentElement).getPropertyValue('--font-mono')");
  check("monospace stack has Windows fonts", /Cascadia Mono/.test(mono) && /Consolas/.test(mono), mono.trim());
  await shot("1-onboarding");

  // A key that is not even shaped like one: the app's own message.
  await wd("POST", `/session/${session}/element/${key}/value`, { text: "bxd_short" });
  await wd("POST", `/session/${session}/element/${await find("button[type=submit]")}/click`, {});
  const local = await until("the validation error", async () => text(await find(".form-error")), 15000);
  check("malformed key gets the validation error", local.includes("does not look like a boxd API key"), local);

  // Shaped like a key, but not one: boxd answers and the app shows it.
  await wd("POST", `/session/${session}/element/${key}/clear`, {});
  await wd("POST", `/session/${session}/element/${key}/value`, { text: "bxd_stowboxsmoke_not_a_real_key_0000000000" });
  await wd("POST", `/session/${session}/element/${await find("button[type=submit]")}/click`, {});
  const remote = await until(
    "boxd's answer",
    async () => {
      const t = await text(await find(".form-error"));
      const busy = (await text(await find("button[type=submit]"))).includes("Checking");
      return !busy && t && !t.includes("does not look like") ? t : null;
    },
    45000,
  );
  check("invalid key gets boxd's refusal", remote.length > 0, remote);
  check("still on onboarding after the refusal", (await findAll("#ob-key")).length === 1);
  await shot("2-refused");

  // The dark theme is the default; the light one through the same path as
  // the Settings switch, so the native title bar can be compared.
  await run("localStorage.setItem('stowbox.theme', 'light'); location.reload();");
  await until("the onboarding after reload", () => find("#ob-key"), 30000);
  await sleep(1000);
  check("light theme applied", (await run("return document.documentElement.dataset.theme")) === "light");
  await shot("3-light");
} catch (e) {
  failed = true;
  console.error(e);
  await shot("error").catch(() => undefined);
} finally {
  if (session) await wd("DELETE", `/session/${session}`).catch(() => undefined);
  driver.kill();
}

writeFileSync(join(out, "results.json"), JSON.stringify(results, null, 2));
const bad = results.filter((r) => !r.ok);
if (failed || bad.length) {
  console.error(`${bad.length} check(s) failed${failed ? ", and the run stopped on an error" : ""}`);
  process.exit(1);
}
console.log(`all ${results.length} checks passed`);
