// Takes the README screenshots from the browser build and its mock bridge,
// over the DevTools protocol, so they never need a boxd account and always
// show the same data. Start the two sides first:
//
//   pnpm dev
//   chromium --headless=new --hide-scrollbars --force-device-scale-factor=2 --window-size=1280,800 \
//     --remote-debugging-port=9222 --remote-allow-origins=* http://localhost:1420/
//   node .github/scripts/screenshots.mjs docs/screenshots

import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const out = resolve(process.argv[2] ?? "docs/screenshots");
mkdirSync(out, { recursive: true });
const devtools = `http://127.0.0.1:${process.env.STOWBOX_DEVTOOLS_PORT ?? 9222}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const targets = await (await fetch(`${devtools}/json/list`)).json();
const target = targets.find((t) => t.type === "page" && t.url.includes("localhost:1420"));
if (!target) throw new Error(`no page on localhost:1420 among ${JSON.stringify(targets.map((t) => t.url))}`);
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve, { once: true });
  ws.addEventListener("error", reject, { once: true });
});
let seq = 0;
const pending = new Map();
ws.addEventListener("message", (m) => {
  const msg = JSON.parse(m.data);
  const p = pending.get(msg.id);
  if (!p) return;
  pending.delete(msg.id);
  msg.error ? p.reject(new Error(`${p.method}: ${msg.error.message}`)) : p.resolve(msg.result);
});
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { method, resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
const run = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
};
const click = (css) => run(`(() => { const el = document.querySelector(${JSON.stringify(css)}); if (!el) throw new Error('no ' + ${JSON.stringify(css)}); el.click(); })()`);
const key = (text, modifiers = 0) =>
  Promise.all(["keyDown", "keyUp"].map((type) => send("Input.dispatchKeyEvent", { type, key: text, code: `Key${text.toUpperCase()}`, modifiers })));
async function shot(name) {
  await sleep(600);
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  const file = join(out, `${name}.png`);
  writeFileSync(file, Buffer.from(data, "base64"));
  console.log(file);
}

await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 2, mobile: false });
await run("localStorage.setItem('stowbox.theme', 'dark'); location.reload(); true");
await sleep(1500);
await run("document.querySelector('.topbar') ? true : (() => { throw new Error('not ready') })()");

await shot("fleet");

await click('button.row[title="Add a /health endpoint and a test for it"]');
await sleep(800);
await shot("agent");
await click(".subtab:nth-of-type(2)"); // the agent's Changes tab, which reads its repository
await sleep(1500);
await shot("changes");

await click('.row-name[title="mock-worker"]');
await sleep(800);
await click(".subtab:nth-of-type(4)"); // Ports
await sleep(1200);
await shot("ports");

await run("Array.from(document.querySelectorAll('.row-name')).find((e) => e.textContent === 'Setup presets')?.closest('button')?.click()");
await sleep(800);
await shot("presets");

await key("k", 2); // Ctrl+K
await sleep(400);
await send("Input.insertText", { text: "nightly" });
await sleep(900);
await shot("palette");
await key("Escape");

ws.close();
