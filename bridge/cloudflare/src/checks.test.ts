import assert from "node:assert/strict";
import { test } from "node:test";
import { BOX_ID, DEFAULT_IDLE_SECS, bearer, frame, newBoxId, parseControl, parseCreate, parseIdle, parseStart } from "./checks.ts";

test("box ids are 20 even letters and digits", () => {
  const id = newBoxId(crypto.getRandomValues(new Uint8Array(32)));
  assert.match(id, BOX_ID);
  // Bytes past 252 are skipped rather than skewing the alphabet.
  assert.equal(newBoxId(new Uint8Array([255, 253, ...Array(20).fill(0)])), "box-aaaaaaaaaaaaaaaaaaaa");
  for (const bad of ["box-", "box-AAAAAAAAAAAAAAAAAAAA", "box-aaaaaaaaaaaaaaaaaaa/", "../registry", "box-aaaaaaaaaaaaaaaaaaaaa"]) {
    assert.doesNotMatch(bad, BOX_ID, bad);
  }
});

test("only a plain bearer token is read", () => {
  assert.equal(bearer("Bearer abc"), "abc");
  for (const bad of [null, "", "Bearer ", "bearer abc", "Basic abc", "Bearer a b", "Bearer a\tb", `Bearer ${"x".repeat(513)}`]) {
    assert.equal(bearer(bad), null, String(bad));
  }
});

test("a create body is checked field by field", () => {
  assert.deepEqual(parseCreate({ name: "api-work" }), { name: "api-work", size: "standard-2", idleSecs: DEFAULT_IDLE_SECS, env: {} });
  assert.deepEqual(parseCreate({ name: "a", size: "standard-4", idle_secs: 60, env: { A_1: "x" } }), { name: "a", size: "standard-4", idleSecs: 60, env: { A_1: "x" } });
  for (const bad of [
    null,
    [],
    "x",
    {},
    { name: "-x" },
    { name: "A" },
    { name: "a".repeat(64) },
    { name: "a", size: "lite" },
    { name: "a", idle_secs: 59 },
    { name: "a", idle_secs: 1.5 },
    { name: "a", env: { "A-B": "x" } },
    { name: "a", env: { A: 1 } },
    { name: "a", env: { A: "x\0" } },
    { name: "a", env: [] },
  ]) {
    assert.equal(typeof parseCreate(bad), "string", JSON.stringify(bad));
  }
  assert.equal(typeof parseCreate({ name: "a", env: Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`V${i}`, ""])) }), "string");
  assert.equal(typeof parseCreate({ name: "a", env: { A: "x".repeat(70_000) } }), "string");
});

test("idle times stay between a minute and a day", () => {
  assert.equal(parseIdle(60), 60);
  assert.equal(parseIdle(86_400), 86_400);
  for (const bad of [0, 59, 86_401, "600", null, Number.NaN]) assert.equal(typeof parseIdle(bad), "string", String(bad));
});

test("a start message names a command and a sane size", () => {
  assert.deepEqual(parseStart('{"type":"start","argv":["sh","-c","echo hi"]}'), { argv: ["sh", "-c", "echo hi"], tty: false, cols: 80, rows: 24 });
  assert.deepEqual(parseStart('{"type":"start","argv":["bash"],"tty":true,"cols":0,"rows":99999}'), { argv: ["bash"], tty: true, cols: 1, rows: 1000 });
  for (const bad of [
    "",
    "nope",
    "null",
    '{"argv":["sh"]}',
    '{"type":"start"}',
    '{"type":"start","argv":[]}',
    '{"type":"start","argv":[""]}',
    '{"type":"start","argv":[1]}',
    '{"type":"start","argv":["a\\u0000b"]}',
    JSON.stringify({ type: "start", argv: Array(65).fill("x") }),
    JSON.stringify({ type: "start", argv: ["x".repeat(300_000)] }),
  ]) {
    assert.equal(typeof parseStart(bad), "string", bad);
  }
});

test("control messages are a resize or the end of input", () => {
  assert.deepEqual(parseControl('{"type":"resize","cols":132,"rows":40}'), { type: "resize", cols: 132, rows: 40 });
  assert.deepEqual(parseControl('{"type":"eof"}'), { type: "eof" });
  for (const bad of ["", "x", '{"type":"kill"}', "[]", `{"type":"eof","x":"${"a".repeat(2000)}"}`]) assert.equal(parseControl(bad), null, bad);
});

test("output frames lead with the stream", () => {
  assert.deepEqual(Array.from(frame(2, new Uint8Array([7, 8]))), [2, 7, 8]);
  assert.deepEqual(Array.from(frame(1, new Uint8Array())), [1]);
});
