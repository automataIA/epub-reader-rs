use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BookRecord {
    pub id: String,
    pub title: String,
    pub creators: Vec<String>,
    pub language: Option<String>,
    pub cover_href: Option<String>,
    pub added_at: f64,
    pub last_opened_at: Option<f64>,
    pub progress: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EpubMetadata {
    pub title: String,
    pub creators: Vec<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpineItem {
    pub idref: String,
    pub href: String,
    pub linear: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TocItem {
    pub label: String,
    pub href: String,
    pub children: Vec<TocItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EpubPackage {
    pub metadata: EpubMetadata,
    pub manifest: HashMap<String, ManifestItem>,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<TocItem>,
    pub opf_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReaderLocation {
    pub book_id: String,
    pub spine_index: usize,
    pub scroll_ratio: f32,
    pub element_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub book_id: String,
    pub spine_href: String,
    pub selector: TextQuoteSelector,
    pub color: HighlightColor,
    pub note: String,
    #[serde(default)]
    pub kind: AnnotationKind,
    pub created_at: f64,
    pub updated_at: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnotationKind {
    #[default]
    Note,
    Bookmark,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextQuoteSelector {
    pub exact: String,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightColor {
    #[default]
    Yellow,
    Green,
    Blue,
    Pink,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReaderPreferences {
    pub theme: ReaderTheme,
    pub font_family: ReaderFont,
    pub font_size: u8,
    pub line_height: f32,
    pub page_width: u16,
    #[serde(default = "default_page_margin")]
    pub page_margin: u16,
    pub flow: ReaderFlow,
    pub publisher_styles: bool,
    #[serde(default)]
    pub tts: TtsPreferences,
}

fn default_page_margin() -> u16 {
    48
}

/// Preferenze della lettura ad alta voce (modello locale, scaricato on demand).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TtsPreferences {
    /// Id lingua del modello ("english" | "italian").
    pub language: String,
    /// Nome della voce predefinita (es. "alba", "giovanni").
    pub voice: String,
    /// Se leggere i blocchi di codice (espansi in parole) o saltarli.
    pub read_code_blocks: bool,
    /// Velocità di lettura (1.0 = normale, 2.0 = doppia). Range UI 0.5–2.0.
    #[serde(default = "default_tts_speed")]
    pub speed: f32,
}

fn default_tts_speed() -> f32 {
    1.0
}

impl Default for TtsPreferences {
    fn default() -> Self {
        Self {
            language: "english".to_string(),
            voice: "alba".to_string(),
            read_code_blocks: false,
            speed: default_tts_speed(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReaderTheme {
    #[default]
    Light,
    Sepia,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReaderFont {
    #[default]
    Serif,
    Sans,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReaderFlow {
    #[default]
    Scroll,
    Paged,
}

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            theme: ReaderTheme::Light,
            font_family: ReaderFont::Serif,
            font_size: 18,
            line_height: 1.65,
            page_width: 760,
            page_margin: 48,
            flow: ReaderFlow::Scroll,
            publisher_styles: true,
            tts: TtsPreferences::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredBook {
    pub record: BookRecord,
    pub package: EpubPackage,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub spine_index: usize,
    pub href: String,
    pub title: String,
    pub snippet: String,
}
