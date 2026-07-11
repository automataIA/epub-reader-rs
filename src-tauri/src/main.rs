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
