// Builds the vendored, self-contained Piper TTS bundle.
//
// Piper (VITS via onnxruntime-web, WASM) is our only TTS engine. This script:
//   1. Patches the installed @diffusionstudio/vits-web so its runtime assets
//      (onnxruntime-web + piper_phonemize wasm) load from LOCAL /tts/ paths
//      instead of CDNs, and registers the it_IT-paola-medium voice (it exists
//      in the diffusionstudio HF repo but isn't in the lib's built-in map).
//   2. Bundles it into public/tts/piper.bundle.js (self-contained ESM, incl.
//      onnxruntime-web 1.18 JS + the piper_phonemize glue).
//
// The patch is idempotent, so it works both on a pristine `npm ci` node_modules
// and on a re-run. Voice models are still fetched from Hugging Face at runtime
// (they are hundreds of MB) and cached by vits-web in OPFS.

import * as esbuild from "esbuild";
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "fs";
import { fileURLToPath } from "url";
import { dirname, resolve } from "path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const vitsPath = resolve(root, "node_modules/@diffusionstudio/vits-web/dist/vits-web.js");

// --- 0. fetch the runtime wasm assets we serve locally (once) ---
// These are downloaded from CDNs at BUILD time and then served from /tts/ at
// runtime, so the deployed app makes no external CDN request for its engine.
const ORT = "https://cdnjs.cloudflare.com/ajax/libs/onnxruntime-web/1.18.0/";
const PIPER = "https://cdn.jsdelivr.net/npm/@diffusionstudio/piper-wasm@1.0.0/build/";
const assets = [
  // onnxruntime-web picks a variant by browser capabilities (threaded needs
  // cross-origin isolation, which GitHub Pages lacks → simd non-threaded).
  [ORT + "ort-wasm.wasm", "public/tts/ort/ort-wasm.wasm"],
  [ORT + "ort-wasm-simd.wasm", "public/tts/ort/ort-wasm-simd.wasm"],
  [ORT + "ort-wasm-threaded.wasm", "public/tts/ort/ort-wasm-threaded.wasm"],
  [ORT + "ort-wasm-simd-threaded.wasm", "public/tts/ort/ort-wasm-simd-threaded.wasm"],
  [PIPER + "piper_phonemize.wasm", "public/tts/piper/piper_phonemize.wasm"],
  [PIPER + "piper_phonemize.data", "public/tts/piper/piper_phonemize.data"],
];
for (const [url, rel] of assets) {
  const dest = resolve(root, rel);
  if (existsSync(dest)) continue;
  mkdirSync(dirname(dest), { recursive: true });
  process.stdout.write(`fetching ${rel} … `);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`failed to fetch ${url}: ${res.status}`);
  writeFileSync(dest, Buffer.from(await res.arrayBuffer()));
  console.log("ok");
}

// --- 1. patch vits-web (idempotent) ---
let src = readFileSync(vitsPath, "utf8");

// Point ORT + phonemize wasm at local vendored paths (served at /tts/...).
src = src.replaceAll(
  "https://cdnjs.cloudflare.com/ajax/libs/onnxruntime-web/1.18.0/",
  "/tts/ort/",
);
src = src.replaceAll(
  "https://cdn.jsdelivr.net/npm/@diffusionstudio/piper-wasm@1.0.0/build/piper_phonemize",
  "/tts/piper/piper_phonemize",
);

// Register paola-medium (Italian) in the voice path map, next to riccardo.
const riccardo = '"it_IT-riccardo-x_low": "it/it_IT/riccardo/x_low/it_IT-riccardo-x_low.onnx",';
if (!src.includes("it_IT-paola-medium")) {
  src = src.replace(
    riccardo,
    riccardo + ' "it_IT-paola-medium": "it/it_IT/paola/medium/it_IT-paola-medium.onnx",',
  );
}

// Speed control: expose length_scale so predict({..., lengthScale}) can override
// the model's baked-in default (higher = slower). The glue passes 1/speed.
src = src.replaceAll(
  "g = i.inference.length_scale",
  "g = e.lengthScale ?? i.inference.length_scale",
);

// Perf: vits-web recreates the whole InferenceSession (re-parsing the ~120MB model
// and re-reading it from OPFS) on EVERY predict() call — the main cause of the UI
// stutter. Cache the session per voiceId. The glue evicts the entry on cache-heal.
src = src.replaceAll(
  "k = await f(`${u}/${n}`, m), y = await _.InferenceSession.create(await k.arrayBuffer())",
  "y = await (async () => { const M = (globalThis.__ortSess ??= new Map()); let S = M.get(e.voiceId); if (!S) { const k = await f(u + '/' + n, m); S = await _.InferenceSession.create(await k.arrayBuffer()); M.set(e.voiceId, S); } return S; })()",
);

// Perf: run ORT inference in a worker (proxy) so run() never blocks the UI thread,
// and pin numThreads=1 (this app is never cross-origin-isolated, so threaded ORT
// falls back to 1 anyway and only spams a console warning).
src = src.replaceAll(
  "_.env.wasm.numThreads = navigator.hardwareConcurrency",
  "_.env.wasm.numThreads = 1, _.env.wasm.proxy = !0",
);

// With proxy=true the ORT worker resolves wasmPaths in its own (blob:) context,
// where a root-relative "/tts/ort/" is not a valid URL. Absolutize it against the
// document URL (handles both local "/" and gh-pages "/epub-reader-rs/" base paths).
src = src.replaceAll(
  "_.env.wasm.wasmPaths = B",
  "_.env.wasm.wasmPaths = new URL(B, location.href).href",
);

// The bundled phonemizer maps phonemes with its own (256-slot) id table, ignoring
// the model's phoneme_id_map. Small models like en_US-ryan-high have only 130
// symbols, so IPA diacritics espeak emits for English (e.g. the syllabic marker
// U+0329, id 144, in "bottle"/"rhythm") land out of the embedding range and crash
// OrtRun at the Gather node. Real Piper drops phonemes absent from the model; do
// the same by filtering ids >= num_symbols (i is the parsed model config).
src = src.replaceAll(
  "v(JSON.parse(l).phoneme_ids)",
  "v(JSON.parse(l).phoneme_ids.filter((x) => x < i.num_symbols))",
);

// Expose a phonemize-only export so the native (Tauri) TTS path can reuse the
// exact same espeak phonemization as the web path, then run inference natively.
// Mirrors the phonemize half of predict() (`N`), returning the filtered id array.
// (TEMPORARY: supports the native GPU-TTS "lab".) Uses module internals h,_,c,u,f,x,B.
if (!src.includes("async function Nph(")) {
  const pm = src.match(/import\("(\.\/piper-[^"]+\.js)"\)/);
  if (!pm) throw new Error("build-tts: could not find piper import path to reuse");
  const piperImport = JSON.stringify(pm[1]);
  const fn = `
async function Nph(e) {
  h = h ?? await import(${piperImport});
  _ = _ ?? await import("onnxruntime-web");
  const n = c[e.voiceId], o = JSON.stringify([{ text: e.text.trim() }]);
  _.env.allowLocalModels = !1;
  _.env.wasm.wasmPaths = new URL(B, location.href).href;
  const a = await fetch(u + "/" + n + ".json"), i = JSON.parse(await a.text());
  return await new Promise(async (v) => {
    (await h.createPiperPhonemize({
      print: (l) => { v(JSON.parse(l).phoneme_ids.filter((z) => z < i.num_symbols)); },
      printErr: (l) => { throw new Error(l); },
      locateFile: (l) => l.endsWith(".wasm") ? x + ".wasm" : l.endsWith(".data") ? x + ".data" : l
    })).callMain(["-l", i.espeak.voice, "--input", o, "--espeak_data", "/espeak-ng-data"]);
  });
}
`;
  src = src.replace("export {", fn + "\nexport {\n  Nph as phonemize,");
}

writeFileSync(vitsPath, src);

// --- 2. bundle ---
const stub = resolve(root, "scripts/empty.js");
await esbuild.build({
  stdin: {
    contents: "export * as piper from '@diffusionstudio/vits-web';",
    resolveDir: root,
    loader: "js",
  },
  bundle: true,
  format: "esm",
  platform: "browser",
  // The piper_phonemize Emscripten glue references Node built-ins behind an
  // ENVIRONMENT_IS_NODE guard (never taken in the browser); stub them out.
  alias: { fs: stub, path: stub, module: stub, crypto: stub, os: stub },
  outfile: resolve(root, "public/tts/piper.bundle.js"),
  logLevel: "info",
});
