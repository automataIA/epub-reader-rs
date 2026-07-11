//! Lettura ad alta voce con **Piper** (VITS via onnxruntime-web, tutto WASM,
//! nessuna GPU), 100% locale.
//!
//! L'inferenza e la riproduzione girano in JavaScript: l'engine è vendorizzato in
//! `public/tts/` (bundle + wasm di ORT ed espeak) e la logica di orchestrazione sta
//! in `public/tts/piper_tts.js`, esposta come oggetto globale `PiperTTS`. Qui ci
//! sono solo i binding e il catalogo voci. L'inferenza ORT gira in un worker
//! (`env.wasm.proxy`), quindi il thread della UI resta reattivo. I modelli voce (decine di
//! MB) sono scaricati da Hugging Face alla prima riproduzione e messi in cache (OPFS).

use serde::Deserialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = PiperTTS, js_name = start)]
    fn piper_start(voice_id: &str, text: &str, speed: f32);
    #[wasm_bindgen(js_namespace = PiperTTS, js_name = pause)]
    fn piper_pause();
    #[wasm_bindgen(js_namespace = PiperTTS, js_name = resume)]
    fn piper_resume();
    #[wasm_bindgen(js_namespace = PiperTTS, js_name = stop)]
    fn piper_stop();
    #[wasm_bindgen(js_namespace = PiperTTS, js_name = status)]
    fn piper_status() -> String;
}

/// Voce Piper associata a una lingua di lettura.
pub struct Voice {
    /// Id lingua usato nelle preferenze ("english" | "italian").
    pub language: &'static str,
    pub label: &'static str,
    /// Id voce nel repo Piper (scaricato da HF a runtime).
    pub voice_id: &'static str,
}

/// Voci scelte: inglese → Ryan (high), italiano → Paola (medium).
pub const VOICES: [Voice; 2] = [
    Voice {
        language: "english",
        label: "English — Ryan",
        voice_id: "en_US-ryan-high",
    },
    Voice {
        language: "italian",
        label: "Italiano — Paola",
        voice_id: "it_IT-paola-medium",
    },
];

/// Voce per una lingua (fallback: la prima, inglese).
pub fn voice_for_language(language: &str) -> &'static Voice {
    VOICES
        .iter()
        .find(|voice| voice.language == language)
        .unwrap_or(&VOICES[0])
}

/// Stato della lettura, letto (in polling) dal glue JS.
#[derive(Clone, Debug, PartialEq)]
pub enum PiperState {
    Idle,
    /// Download del modello in corso: frazione 0..=1.
    Downloading(f64),
    Speaking { sentence: u32, total: u32 },
    Paused,
    Error(String),
}

#[derive(Deserialize)]
struct RawStatus {
    state: String,
    #[serde(default)]
    sentence: u32,
    #[serde(default)]
    total: u32,
    #[serde(default)]
    download: f64,
    #[serde(default)]
    error: Option<String>,
}

/// Legge lo stato corrente dal glue JS (`PiperTTS.status()`).
pub fn read_status() -> PiperState {
    let Ok(raw) = serde_json::from_str::<RawStatus>(&piper_status()) else {
        return PiperState::Idle;
    };
    match raw.state.as_str() {
        "downloading" => PiperState::Downloading(raw.download),
        "speaking" => PiperState::Speaking {
            sentence: raw.sentence,
            total: raw.total,
        },
        "paused" => PiperState::Paused,
        "error" => PiperState::Error(raw.error.unwrap_or_default()),
        _ => PiperState::Idle,
    }
}

/// Avvia la lettura di `text` con la voce `voice_id` a velocità `speed` (1.0 =
/// normale, 2.0 = doppia). Va chiamata da un gesto utente (crea l'AudioContext).
/// Fire-and-forget: lo stato si legge con [`read_status`].
pub fn start(voice_id: &str, text: &str, speed: f32) {
    piper_start(voice_id, text, speed);
}

pub fn pause() {
    piper_pause();
}

pub fn resume() {
    piper_resume();
}

pub fn stop() {
    piper_stop();
}
