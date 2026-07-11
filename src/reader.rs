use crate::models::{ReaderFlow, ReaderFont, ReaderPreferences, ReaderTheme};

/// Colori (bg, fg, muted, link) dei temi di lettura.
/// Il bg è riusato dal guscio dell'app per tingere il "foglio" attorno all'iframe.
pub fn theme_colors(theme: ReaderTheme) -> (&'static str, &'static str, &'static str, &'static str)
{
    match theme {
        ReaderTheme::Light => ("#fbf7ef", "#1f1c18", "#6f675d", "#1d4ed8"),
        ReaderTheme::Sepia => ("#f4ead8", "#282015", "#7a6545", "#8a4f16"),
        ReaderTheme::Dark => ("#121417", "#e8e6df", "#a9a398", "#9db8e8"),
    }
}

/// I font sono caricati anche dentro l'iframe `srcdoc`: gli @font-face del
/// documento padre non si propagano, ma la base URL sì, quindi i percorsi
/// relativi `fonts/…` risolvono sugli stessi file.
const READER_FONT_FACES: &str = r#"
@font-face { font-family: "Literata"; font-style: normal; font-weight: 400; font-display: swap; src: url("fonts/literata-regular.woff2") format("woff2"); }
@font-face { font-family: "Literata"; font-style: italic; font-weight: 400; font-display: swap; src: url("fonts/literata-italic.woff2") format("woff2"); }
@font-face { font-family: "Literata"; font-style: normal; font-weight: 600; font-display: swap; src: url("fonts/literata-600.woff2") format("woff2"); }
@font-face { font-family: "IBM Plex Sans"; font-style: normal; font-weight: 400; font-display: swap; src: url("fonts/plex-sans-regular.woff2") format("woff2"); }
@font-face { font-family: "IBM Plex Sans"; font-style: normal; font-weight: 500; font-display: swap; src: url("fonts/plex-sans-500.woff2") format("woff2"); }
@font-face { font-family: "IBM Plex Sans"; font-style: normal; font-weight: 600; font-display: swap; src: url("fonts/plex-sans-600.woff2") format("woff2"); }
"#;

pub fn reader_css(preferences: &ReaderPreferences) -> String {
    let (bg, fg, muted, link) = theme_colors(preferences.theme);

    let family = match preferences.font_family {
        ReaderFont::Serif => "'Literata', Georgia, 'Times New Roman', serif",
        ReaderFont::Sans => "'IBM Plex Sans', ui-sans-serif, system-ui, sans-serif",
    };

    // Il layout va su <body>: i capitoli reali hanno un proprio <body>, quindi
    // regole su <main> non si applicherebbero mai (il wrapper <main> esiste
    // solo per i frammenti senza body).
    let layout = match preferences.flow {
        ReaderFlow::Scroll => format!(
            "body{{box-sizing:border-box!important;width:auto!important;max-width:{width}px!important;margin:0 auto!important;padding:{m}px {m}px {mb}px!important;}}",
            width = preferences.page_width,
            m = preferences.page_margin,
            mb = preferences.page_margin + 24,
        ),
        ReaderFlow::Paged => format!(
            "body{{box-sizing:border-box!important;width:auto!important;max-width:none!important;height:100vh;margin:0!important;padding:{m}px!important;column-width:{width}px;column-gap:56px;overflow-x:auto;overflow-y:hidden;}}",
            width = preferences.page_width,
            m = preferences.page_margin,
        ),
    };

    let publisher_reset = if preferences.publisher_styles {
        ""
    } else {
        "body *{font-family:inherit!important;color:inherit!important;background:transparent!important;}"
    };

    format!(
        r#"
        {font_faces}
        :root {{
            color-scheme: light dark;
            --reader-bg: {bg};
            --reader-fg: {fg};
            --reader-muted: {muted};
        }}
        html, body {{
            margin: 0;
            min-height: 100%;
            background: var(--reader-bg);
            color: var(--reader-fg);
        }}
        /* La tipografia scelta dall'utente vince sempre sui CSS del publisher:
           font, dimensione e interlinea sono i controlli del pannello Style. */
        body {{
            font-family: {family} !important;
            font-size: {font_size}px !important;
            line-height: {line_height} !important;
        }}
        p, li, blockquote, dd, dt {{
            font-family: inherit !important;
            font-size: inherit !important;
            line-height: {line_height} !important;
        }}
        img, svg, video {{
            max-width: 100%;
            height: auto;
        }}
        a {{ color: {link}; }}
        p {{ margin: 0 0 1em; }}
        h1, h2, h3 {{
            line-height: 1.2;
            margin: 1.4em 0 .65em;
        }}
        ::selection {{
            background: color-mix(in srgb, #facc15 55%, transparent);
        }}
        mark[data-reader-highlight] {{
            background: #fde68a;
            color: inherit;
            border-radius: 0.16em;
            padding: 0.02em 0.08em;
        }}
        {layout}
        {publisher_reset}
        "#,
        font_faces = READER_FONT_FACES,
        font_size = preferences.font_size,
        line_height = preferences.line_height,
    )
}

/// Testo per la lettura ad alta voce: come `plain_text_from_html` ma
/// - salta sempre `<script>`/`<style>`,
/// - separa i blocchi con un punto (così il TTS fa pausa tra paragrafi/titoli),
/// - i blocchi `<pre>` vengono letti "a voce" (simboli espansi in parole) o
///   sostituiti da una frase segnaposto, secondo `read_code_blocks`,
/// - il `<code>` inline viene espanso in parole pronunciabili.
pub fn tts_text_from_html(html: &str, lang: &str, read_code_blocks: bool) -> String {
    const BLOCK_TAGS: [&str; 13] = [
        "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "blockquote", "br", "tr", "section",
    ];

    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    let mut in_entity = false;
    let mut entity = String::new();
    let mut skip_depth = 0usize; // <script>/<style>
    let mut pre_depth = 0usize;
    let mut code_depth = 0usize;
    let mut pre_buf = String::new();

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let raw = tag.trim().trim_end_matches('/');
                let closing = raw.starts_with('/');
                let name = raw
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                match name.as_str() {
                    "script" | "style" => {
                        if closing {
                            skip_depth = skip_depth.saturating_sub(1);
                        } else {
                            skip_depth += 1;
                        }
                    }
                    "pre" => {
                        if closing {
                            pre_depth = pre_depth.saturating_sub(1);
                            if pre_depth == 0 {
                                if read_code_blocks {
                                    out.push_str(&spoken_code(&pre_buf, lang));
                                } else if lang == "italian" {
                                    out.push_str(" Blocco di codice saltato. ");
                                } else {
                                    out.push_str(" Code block skipped. ");
                                }
                                pre_buf.clear();
                            }
                        } else {
                            pre_depth += 1;
                        }
                    }
                    "code" => {
                        if closing {
                            code_depth = code_depth.saturating_sub(1);
                        } else {
                            code_depth += 1;
                        }
                    }
                    _ => {}
                }
                if closing && BLOCK_TAGS.contains(&name.as_str()) && pre_depth == 0 {
                    out.push_str(". ");
                } else {
                    out.push(' ');
                }
            }
            _ if in_tag => tag.push(ch),
            _ if skip_depth > 0 => {}
            '&' => {
                in_entity = true;
                entity.clear();
            }
            ';' if in_entity => {
                in_entity = false;
                let decoded = match entity.as_str() {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" => '\'',
                    "nbsp" => ' ',
                    _ => ' ',
                };
                if pre_depth > 0 {
                    pre_buf.push(decoded);
                } else {
                    out.push(decoded);
                }
            }
            _ if in_entity => entity.push(ch),
            _ if pre_depth > 0 => pre_buf.push(ch),
            _ if code_depth > 0 => out.push_str(&spoken_code(&ch.to_string(), lang)),
            _ => out.push(ch),
        }
    }

    collapse_speech_text(&out)
}

/// Espande un frammento di codice in parole pronunciabili: simboli con nome,
/// identificatori camelCase/snake_case spezzati in parole.
fn spoken_code(code: &str, lang: &str) -> String {
    let it = lang == "italian";
    let mut out = String::with_capacity(code.len() * 2);

    let mut chars = code.chars().peekable();
    while let Some(ch) = chars.next() {
        let two = chars.peek().map(|next| (ch, *next));
        let spoken_pair = match two {
            Some(('-', '>')) => Some(if it { "freccia" } else { "arrow" }),
            Some(('=', '>')) => Some(if it { "freccia doppia" } else { "fat arrow" }),
            Some(('=', '=')) => Some(if it { "uguale uguale" } else { "double equals" }),
            Some(('!', '=')) => Some(if it { "diverso da" } else { "not equals" }),
            Some(('&', '&')) => Some("and"),
            Some(('|', '|')) => Some("or"),
            Some((':', ':')) => Some(if it { "due punti due punti" } else { "colon colon" }),
            _ => None,
        };
        if let Some(word) = spoken_pair {
            chars.next();
            out.push(' ');
            out.push_str(word);
            out.push(' ');
            continue;
        }
        let spoken = match ch {
            '_' => Some(" "),
            '=' => Some(if it { " uguale " } else { " equals " }),
            '{' => Some(if it { " graffa aperta " } else { " open brace " }),
            '}' => Some(if it { " graffa chiusa " } else { " close brace " }),
            '(' => Some(if it { " parentesi aperta " } else { " open paren " }),
            ')' => Some(if it { " parentesi chiusa " } else { " close paren " }),
            '[' => Some(if it { " quadra aperta " } else { " open bracket " }),
            ']' => Some(if it { " quadra chiusa " } else { " close bracket " }),
            '<' => Some(if it { " minore di " } else { " less than " }),
            '>' => Some(if it { " maggiore di " } else { " greater than " }),
            '&' => Some(" ampersand "),
            '|' => Some(" pipe "),
            '!' => Some(if it { " punto esclamativo " } else { " bang " }),
            '*' => Some(if it { " asterisco " } else { " star " }),
            '/' => Some(" slash "),
            '\\' => Some(" backslash "),
            '#' => Some(if it { " cancelletto " } else { " hash " }),
            '@' => Some(" at "),
            '~' => Some(" tilde "),
            '^' => Some(if it { " circonflesso " } else { " caret " }),
            ';' => Some(if it { " punto e virgola. " } else { " semicolon. " }),
            '\n' => Some(". "),
            _ => None,
        };
        if let Some(word) = spoken {
            out.push_str(word);
            continue;
        }
        // camelCase → "camel Case": spazio prima delle maiuscole dopo minuscole.
        if ch.is_ascii_uppercase()
            && out
                .chars()
                .last()
                .is_some_and(|prev| prev.is_ascii_lowercase())
        {
            out.push(' ');
        }
        out.push(ch);
    }

    out
}

/// Comprime spazi e sequenze di punteggiatura prodotte dalla conversione
/// (". . ." → ".") per non far "balbettare" il TTS.
fn collapse_speech_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_meaningful = ' ';
    for word in text.split_whitespace() {
        // Scarta i segmenti fatti di sola punteggiatura ripetuta dai tag di blocco.
        if word.chars().all(|c| c == '.' || c == ',') {
            if last_meaningful != '.' && !out.is_empty() {
                out.push('.');
                last_meaningful = '.';
            }
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        last_meaningful = word.chars().last().unwrap_or(' ');
    }
    out
}

pub fn plain_text_from_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_entity = false;
    let mut entity = String::new();

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            '&' if !in_tag => {
                in_entity = true;
                entity.clear();
            }
            ';' if in_entity => {
                in_entity = false;
                out.push(match entity.as_str() {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" => '\'',
                    "nbsp" => ' ',
                    _ => ' ',
                });
            }
            _ if in_tag => {}
            _ if in_entity => entity.push(ch),
            _ => out.push(ch),
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_text_skips_code_blocks_when_disabled() {
        let html = "<p>Before.</p><pre><code>let x = 1;</code></pre><p>After.</p>";
        let text = tts_text_from_html(html, "english", false);
        assert!(text.contains("Before"));
        assert!(text.contains("Code block skipped"));
        assert!(text.contains("After"));
        assert!(!text.contains("let x"));
    }

    #[test]
    fn tts_text_reads_code_blocks_with_spoken_symbols() {
        let html = "<pre>fn get_user() -> UserName {}</pre>";
        let text = tts_text_from_html(html, "english", true);
        assert!(text.contains("get user"));
        assert!(text.contains("arrow"));
        assert!(text.contains("User Name"));
        assert!(text.contains("open brace"));
    }

    #[test]
    fn tts_text_skips_scripts_and_breaks_paragraphs() {
        let html = "<style>p{color:red}</style><p>One</p><p>Two</p>";
        let text = tts_text_from_html(html, "english", false);
        assert!(!text.contains("color"));
        assert_eq!(text, "One. Two.");
    }

    #[test]
    fn tts_text_italian_placeholder() {
        let html = "<pre>x</pre>";
        let text = tts_text_from_html(html, "italian", false);
        assert!(text.contains("Blocco di codice saltato"));
    }
}
