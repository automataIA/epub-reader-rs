//! Native GPU TTS for the desktop (Tauri) build only — TEMPORARY "TTS Lab".
//!
//! The web/gh-pages build keeps the JS Piper engine (onnxruntime-web, WASM CPU).
//! Here we run the same Piper `.onnx` VITS models natively via `ort`, using the
//! platform GPU execution provider (DirectML/CoreML/CUDA) and falling back to CPU.
//! Phonemization stays in the webview (the existing JS Piper phonemizer) and the
//! phoneme ids are passed in; playback is the webview's Web Audio queue. So this
//! module is only the native inference core.
//!
//! Exposed command: `tts_synthesize` -> returns raw PCM for a phoneme sequence.
//! Sessions are cached per (voice, provider).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use ort::{ep, session::Session, value::Tensor};
use serde::Serialize;

const HF_BASE: &str = "https://huggingface.co/diffusionstudio/piper-voices/resolve/main";

/// Piper voice id -> relative path under the HF repo.
fn voice_rel_path(voice_id: &str) -> Option<&'static str> {
    match voice_id {
        "en_US-ryan-high" => Some("en/en_US/ryan/high/en_US-ryan-high.onnx"),
        "it_IT-paola-medium" => Some("it/it_IT/paola/medium/it_IT-paola-medium.onnx"),
        _ => None,
    }
}

/// Cached sessions, keyed by `"{voice_id}|{provider}"`. A session holds a GPU
/// context, so we build each voice once and reuse it (the JS engine already does
/// this via `globalThis.__ortSess`).
static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Synth {
    /// Mono f32 PCM in [-1, 1].
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Execution provider that actually got used ("CUDA" or "CPU").
    pub provider: String,
    /// Inference wall-clock in milliseconds.
    pub ms: u128,
}


/// Synthesize `phoneme_ids` with the given voice and speed, returning PCM.
#[tauri::command]
pub async fn tts_synthesize(
    voice_id: String,
    phoneme_ids: Vec<i64>,
    speed: f32,
) -> Result<Synth, String> {
    tokio::task::spawn_blocking(move || synthesize(&voice_id, phoneme_ids, speed))
        .await
        .map_err(|e| e.to_string())?
}

/// Download (if needed) the model, run inference, return PCM + which EP was used.
fn synthesize(voice_id: &str, ids: Vec<i64>, speed: f32) -> Result<Synth, String> {
    let (onnx, json) = ensure_model(voice_id)?;
    let cfg: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(&json).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let sample_rate = cfg["audio"]["sample_rate"].as_u64().unwrap_or(22050) as u32;
    let noise = cfg["inference"]["noise_scale"].as_f64().unwrap_or(0.667) as f32;
    let base_len = cfg["inference"]["length_scale"].as_f64().unwrap_or(1.0) as f32;
    let noise_w = cfg["inference"]["noise_w"].as_f64().unwrap_or(0.8) as f32;
    // Higher length_scale = slower; speed 2.0 halves it.
    let length = base_len / speed.max(0.1);

    let n = ids.len() as i64;
    let input = Tensor::from_array((vec![1_i64, n], ids)).map_err(|e| e.to_string())?;
    let input_lengths = Tensor::from_array((vec![1_i64], vec![n])).map_err(|e| e.to_string())?;
    let scales =
        Tensor::from_array((vec![3_i64], vec![noise, length, noise_w])).map_err(|e| e.to_string())?;

    let map = SESSIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().map_err(|e| e.to_string())?;

    // Find a cached session for this voice (either provider), else build one.
    let provider = if guard.contains_key(&format!("{voice_id}|CUDA")) {
        "CUDA"
    } else if guard.contains_key(&format!("{voice_id}|CPU")) {
        "CPU"
    } else {
        let (session, prov) = build_session(&onnx)?;
        guard.insert(format!("{voice_id}|{prov}"), session);
        prov
    };
    let session = guard
        .get_mut(&format!("{voice_id}|{provider}"))
        .expect("session just inserted");

    let t0 = Instant::now();
    let outputs = session
        .run(ort::inputs![input, input_lengths, scales])
        .map_err(|e| e.to_string())?;
    let (_shape, audio) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| e.to_string())?;
    let samples = audio.to_vec();
    let ms = t0.elapsed().as_millis();

    Ok(Synth {
        samples,
        sample_rate,
        provider: provider.to_string(),
        ms,
    })
}

/// Build a session, trying the platform GPU execution provider first (hard error
/// if it can't register) and falling back to CPU. Returns the session and the
/// provider name used.
///
/// EP per-OS (chosen by the target-specific `ort` features in Cargo.toml):
/// Windows → DirectML, macOS → CoreML, Linux → CUDA. `ort`'s default `copy-dylibs`
/// places the onnxruntime + provider libs next to the binary, so a Tauri bundle
/// ships them; DirectML/CoreML use OS-built-in runtimes, while Linux CUDA still
/// needs the CUDA runtime present (else it falls back to CPU here).
fn build_session(onnx: &Path) -> Result<(Session, &'static str), String> {
    let gpu = (|| -> Result<Session, String> {
        let mut builder = Session::builder().map_err(|e| e.to_string())?;
        #[cfg(target_os = "windows")]
        {
            builder = builder
                .with_execution_providers([ep::DirectML::default().build().error_on_failure()])
                .map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "macos")]
        {
            builder = builder
                .with_execution_providers([ep::CoreML::default().build().error_on_failure()])
                .map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "linux")]
        {
            builder = builder
                .with_execution_providers([ep::CUDA::default().build().error_on_failure()])
                .map_err(|e| e.to_string())?;
        }
        builder.commit_from_file(onnx).map_err(|e| e.to_string())
    })();
    match gpu {
        Ok(session) => Ok((session, GPU_PROVIDER)),
        Err(_) => {
            let session = Session::builder()
                .map_err(|e| e.to_string())?
                .with_execution_providers([ep::CPU::default().build()])
                .map_err(|e| e.to_string())?
                .commit_from_file(onnx)
                .map_err(|e| e.to_string())?;
            Ok((session, "CPU"))
        }
    }
}

#[cfg(target_os = "windows")]
const GPU_PROVIDER: &str = "DirectML";
#[cfg(target_os = "macos")]
const GPU_PROVIDER: &str = "CoreML";
#[cfg(target_os = "linux")]
const GPU_PROVIDER: &str = "CUDA";

/// Ensure `<cache>/epub-reader-rs/tts/<voice>.onnx(.json)` exist, downloading
/// from Hugging Face on first use. Mirrors the JS engine's OPFS cache, but the
/// native process can't see OPFS so it keeps its own copy on disk.
fn ensure_model(voice_id: &str) -> Result<(PathBuf, PathBuf), String> {
    let rel = voice_rel_path(voice_id).ok_or_else(|| format!("unknown voice {voice_id}"))?;
    let dir = dirs::cache_dir()
        .ok_or("no cache dir")?
        .join("epub-reader-rs")
        .join("tts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let onnx = dir.join(format!("{voice_id}.onnx"));
    let json = dir.join(format!("{voice_id}.onnx.json"));
    if !onnx.exists() {
        download(&format!("{HF_BASE}/{rel}"), &onnx)?;
    }
    if !json.exists() {
        download(&format!("{HF_BASE}/{rel}.json"), &json)?;
    }
    Ok((onnx, json))
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let bytes = reqwest::blocking::get(url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("download {url}: {e}"))?;
    // Write to a temp file then rename, so an interrupted download never leaves a
    // truncated model that later fails at session creation.
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}
