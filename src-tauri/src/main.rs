// Nasconde la finestra console su Windows nelle build release (in debug resta
// per vedere i log). Senza questo il binario parte come console app → terminale.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "gpu-tts")]
mod tts;

fn main() {
    let builder = tauri::Builder::default();

    #[cfg(feature = "gpu-tts")]
    let builder = builder.invoke_handler(tauri::generate_handler![tts::tts_synthesize]);

    builder
        .run(tauri::generate_context!())
        .expect("failed to run EPUB Reader RS");
}
