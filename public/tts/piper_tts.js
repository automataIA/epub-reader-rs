// Piper TTS glue — the single bridge between the Rust/Leptos app and the
// vendored Piper engine (vits-web + onnxruntime-web, all WASM, no GPU).
//
// The heavy inference runs inside vits-web's own worker, so the UI thread stays
// responsive. This module: downloads the voice model (from HF, cached in OPFS),
// splits a chapter into sentences, generates + plays them back-to-back via Web
// Audio, and exposes a small polled `status` so Rust can drive the UI without
// callback plumbing. Exposed as `globalThis.PiperTTS`.

let piperMod = null;
async function getPiper() {
  if (!piperMod) piperMod = (await import("/tts/piper.bundle.js")).piper;
  return piperMod;
}

let ctx = null; // AudioContext (created on Play, i.e. after a user gesture)
let queueTime = 0; // absolute time of the end of the currently queued audio
let sources = []; // scheduled buffer sources (so Stop can kill them)
let genId = 0; // bumps on Stop / new Start to cancel stale async runs

// Polled by Rust each tick. `state`: idle | downloading | speaking | paused | error
let status = { state: "idle", sentence: 0, total: 0, download: 0, error: null };

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// vits-web stores the model in OPFS but never validates it: an interrupted or
// partial write leaves a truncated .onnx that stored() still reports as present,
// so the download is skipped and predict() then fails at session creation with
// "No graph was found in the protobuf". Detect that, purge the cached voice, and
// re-download once.
function isCorruptModelError(e) {
  const m = String((e && e.message) || e).toLowerCase();
  return (
    m.includes("protobuf") ||
    m.includes("no graph") ||
    m.includes("create a session") ||
    m.includes("failed to load model")
  );
}

async function downloadVoice(piper, voiceId, myId) {
  status.state = "downloading";
  status.download = 0;
  await piper.download(voiceId, (p) => {
    if (genId === myId && p.total) status.download = p.loaded / p.total;
  });
}

async function predictHealed(piper, voiceId, text, lengthScale, myId) {
  try {
    return await piper.predict({ text, voiceId, lengthScale });
  } catch (e) {
    if (!isCorruptModelError(e)) throw e;
    // Corrupt cached model — drop the cached ORT session, purge the OPFS copy,
    // and re-download once.
    globalThis.__ortSess?.delete(voiceId);
    await piper.remove(voiceId);
    await downloadVoice(piper, voiceId, myId);
    if (genId !== myId) throw new Error("stopped");
    status.state = "speaking";
    try {
      return await piper.predict({ text, voiceId, lengthScale });
    } catch (e2) {
      if (isCorruptModelError(e2))
        throw new Error("Model still corrupt after re-download (OPFS storage full?)");
      throw e2;
    }
  }
}

function splitSentences(text) {
  const parts = text
    .replace(/\s+/g, " ")
    .match(/[^.!?…\n]+[.!?…]*/g);
  return (parts || [text]).map((s) => s.trim()).filter(Boolean);
}

function scheduleBuffer(buf) {
  const src = ctx.createBufferSource();
  src.buffer = buf;
  src.connect(ctx.destination);
  const now = ctx.currentTime;
  if (queueTime < now + 0.05) queueTime = now + 0.05;
  src.start(queueTime);
  queueTime += buf.duration;
  sources.push(src);
  // Drop references to sources that have finished playing.
  src.onended = () => {
    sources = sources.filter((s) => s !== src);
  };
}

async function closeCtx() {
  for (const s of sources) {
    try {
      s.stop();
    } catch {}
  }
  sources = [];
  if (ctx) {
    try {
      await ctx.close();
    } catch {}
    ctx = null;
  }
  queueTime = 0;
}

// Under Tauri (desktop) we synthesize on the native GPU engine (onnxruntime via
// the `tts_synthesize` command); in the browser we use the JS Piper engine. Both
// share ALL the playback machinery below (Web Audio queue, backpressure, status,
// pause/resume/stop) — only the per-sentence synthesis step differs.
const IS_TAURI = !!globalThis.__TAURI__;

async function startAsync(voiceId, text, speed) {
  const myId = ++genId;
  const lengthScale = speed > 0 ? 1 / speed : 1;
  status = { state: "downloading", sentence: 0, total: 0, download: 0, error: null };
  try {
    const piper = await getPiper();
    // Browser: the JS engine caches the voice model in OPFS. Under Tauri the
    // native backend owns model download/cache (WebKitGTK has no OPFS), so skip.
    if (!IS_TAURI && !(await piper.stored()).includes(voiceId)) {
      await downloadVoice(piper, voiceId, myId);
    }
    if (genId !== myId) return;

    status.state = "speaking";
    ctx = new AudioContext();
    queueTime = 0;

    const sentences = splitSentences(text);
    status.total = sentences.length;

    for (let i = 0; i < sentences.length; i++) {
      if (genId !== myId) return; // stopped
      status.sentence = i + 1;
      let buf;
      if (IS_TAURI) {
        // Phonemize here (reuses the espeak path), infer on the native GPU.
        const ids = await piper.phonemize({ voiceId, text: sentences[i] });
        if (genId !== myId) return;
        const { samples, sampleRate } = await globalThis.__TAURI__.core.invoke(
          "tts_synthesize",
          { voiceId, phonemeIds: Array.from(ids), speed },
        );
        if (genId !== myId) return;
        const f32 = Float32Array.from(samples);
        buf = ctx.createBuffer(1, f32.length, sampleRate);
        buf.copyToChannel(f32, 0);
      } else {
        const wav = await predictHealed(piper, voiceId, sentences[i], lengthScale, myId);
        if (genId !== myId) return;
        buf = await ctx.decodeAudioData(await wav.arrayBuffer());
      }
      scheduleBuffer(buf);
      // Backpressure: keep at most ~12s of audio queued ahead of playback.
      while (genId === myId && ctx && queueTime - ctx.currentTime > 12) {
        await sleep(200);
      }
    }
    // Let the queued audio finish playing.
    while (genId === myId && ctx && ctx.currentTime < queueTime) {
      await sleep(200);
    }
    if (genId === myId) {
      status.state = "idle";
      await closeCtx();
    }
  } catch (e) {
    if (genId === myId) {
      status.state = "error";
      status.error = String((e && e.message) || e);
      await closeCtx();
    }
  }
}

globalThis.PiperTTS = {
  // Fire-and-forget; Rust polls status(). Must be called from a user gesture.
  // `speed` (default 1) scales the reading rate (length_scale = 1/speed).
  start(voiceId, text, speed) {
    startAsync(voiceId, text, speed);
  },
  pause() {
    if (ctx && status.state === "speaking") {
      ctx.suspend();
      status.state = "paused";
    }
  },
  resume() {
    if (ctx && status.state === "paused") {
      ctx.resume();
      status.state = "speaking";
    }
  },
  stop() {
    genId++;
    status.state = "idle";
    closeCtx();
  },
  status() {
    return JSON.stringify(status);
  },
};
