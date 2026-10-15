// The judge as a WebAssembly host runs it, and the checks that the built module is what the crate
// says it is: no imports, a memory ceiling, and verdicts that agree with the native tests.
//
//   node --permission --allow-fs-read=<the .wasm> judge/host.mjs <the .wasm>
//
// Run under Node's permission model, so a module that tried to reach the host would find this
// process unable to give it anything either.

import { readFileSync } from "node:fs";

const wasm = process.argv[2];
const module = await WebAssembly.compile(readFileSync(wasm));

let failures = 0;
function check(what, ok, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? `: ${detail}` : ""}`);
  if (!ok) failures += 1;
}

check("the module imports nothing", WebAssembly.Module.imports(module).length === 0,
  JSON.stringify(WebAssembly.Module.imports(module)));

function encode({ steps, heap, seed, clock, source, cases }) {
  const bytes = [];
  const u32 = (n) => { const b = new Uint8Array(4); new DataView(b.buffer).setUint32(0, n, true); bytes.push(...b); };
  const u64 = (n) => { const b = new Uint8Array(8); new DataView(b.buffer).setBigUint64(0, BigInt(n), true); bytes.push(...b); };
  const f64 = (n) => { const b = new Uint8Array(8); new DataView(b.buffer).setFloat64(0, n, true); bytes.push(...b); };
  const str = (s) => { const b = new TextEncoder().encode(s); u32(b.length); bytes.push(...b); };
  const value = (v) => {
    if (v === undefined) bytes.push(0);
    else if (v === null) bytes.push(1);
    else if (v === false) bytes.push(2);
    else if (v === true) bytes.push(3);
    else if (typeof v === "number") { bytes.push(4); f64(v); }
    else { bytes.push(5); str(v); }
  };
  u32(1); u64(steps); u64(heap); u64(seed); f64(clock); str(source); u32(cases.length);
  for (const [args, expected] of cases) { u32(args.length); args.forEach(value); value(expected); }
  return new Uint8Array(bytes);
}

function decode(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let at = 0;
  const u8 = () => view.getUint8(at++);
  const u32 = () => { const n = view.getUint32(at, true); at += 4; return n; };
  const str = () => { const n = u32(); const s = new TextDecoder().decode(bytes.subarray(at, at + n)); at += n; return s; };
  const value = () => {
    const tag = u8();
    if (tag === 4) { const n = view.getFloat64(at, true); at += 8; return n; }
    if (tag === 5) return str();
    return [undefined, null, false, true][tag];
  };
  const version = u32();
  const refused = u8();
  const verdicts = [];
  for (let n = u32(); n > 0; n -= 1) {
    const code = u8();
    const detail = code === 1 ? value() : code === 3 || code === 5 ? str() : undefined;
    verdicts.push([["pass", "wrong", "not-primitive", "threw", "stopped", "no-module", "engine"][code], detail]);
  }
  return { version, refused, verdicts };
}

async function judge(input) {
  const { exports } = await WebAssembly.instantiate(module, {});
  const bytes = encode(input);
  const at = exports.input(bytes.length);
  new Uint8Array(exports.memory.buffer, at, bytes.length).set(bytes);
  const length = exports.run();
  const out = new Uint8Array(exports.memory.buffer, exports.output(), length).slice();
  return { ...decode(out), memory: exports.memory };
}

const base = { steps: 2_000_000, heap: 16 << 20, seed: 7, clock: 1_700_000_000_000 };
const wordCount = "export default function count(text) { const words = text.match(/\\S+/g); return words === null ? 0 : words.length; }";
const cases = [[[""], 0], [["one"], 1], [["one\ttwo　three"], 3], [["  a  b  "], 2]];

const correct = await judge({ ...base, source: wordCount, cases });
check("a correct delivery passes every case", correct.verdicts.every(([v]) => v === "pass"), JSON.stringify(correct.verdicts));

// The ceiling is 512 MiB, 8192 pages: growing to exactly that succeeds and one page more does not.
// Growing by four gigabytes would fail on any 32-bit module, so the edge is what is asked.
const { memory } = correct;
const ceiling = 8192;
const grow = (pages) => { try { memory.grow(pages); return true; } catch { return false; } };
const pages = () => memory.buffer.byteLength / 65536;
check("memory grows to its ceiling", grow(ceiling - pages()), `${pages()} pages`);
check("and not one page past it", !grow(1), `${pages()} pages`);

const attacks = [
  ["a wrong answer", "export default (t) => t.split(' ').length;", "wrong"],
  ["a rewritten serializer", "JSON.stringify = () => '0'; export default () => 42;", "wrong"],
  ["an object that converts to the answer", "export default () => ({ valueOf() { return 0; } });", "not-primitive"],
  ["a loop at load", "while (true) {} export default () => 0;", "stopped"],
  ["a loop in the call", "export default () => { for (;;) {} };", "stopped"],
  ["a memory bomb", "export default () => { const a = []; for (;;) a.push(new Array(1e5).fill(1)); };", ["threw", "stopped"]],
  ["unbounded recursion", "export default function f() { return f() + 1; }", "threw"],
  ["a static import", "import fs from 'node:fs'; export default () => 0;", "no-module"],
  ["a host global", "export default () => typeof process + typeof require + typeof fetch;", "wrong"],
];
for (const [what, source, wanted] of attacks) {
  const { verdicts } = await judge({ ...base, source, cases: [[["a b"], 0]] });
  const [verdict, detail] = verdicts[0] ?? [];
  const ok = Array.isArray(wanted) ? wanted.includes(verdict) : verdict === wanted;
  check(what, ok, `${verdict} ${JSON.stringify(detail ?? "").slice(0, 80)}`);
}

const twice = [await judge({ ...base, source: "export default () => Math.random() + '|' + Date.now();", cases: [[[], null]] }),
  await judge({ ...base, source: "export default () => Math.random() + '|' + Date.now();", cases: [[[], null]] })];
check("the same input gives the same verdict twice", JSON.stringify(twice[0].verdicts) === JSON.stringify(twice[1].verdicts),
  JSON.stringify(twice[0].verdicts));

const garbage = await judge({ ...base, source: "", cases: [] });
check("a well-formed empty input is read", garbage.refused === 0 && garbage.verdicts.length === 0);

process.exitCode = failures === 0 ? 0 : 1;
