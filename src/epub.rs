use crate::models::{EpubMetadata, EpubPackage, ManifestItem, SearchResult, SpineItem, TocItem};
use crate::reader::plain_text_from_html;
use base64::Engine;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use thiserror::Error;
use zip::ZipArchive;

#[derive(Clone, Debug)]
pub struct EpubBook {
    pub package: EpubPackage,
    files: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Error)]
pub enum EpubError {
    #[error("Invalid EPUB ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("Invalid EPUB I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid EPUB XML: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("Incomplete EPUB: {0}")]
    Invalid(String),
}

impl EpubBook {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EpubError> {
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        let mut files = HashMap::new();

        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            if file.is_dir() {
                continue;
            }

            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            files.insert(normalize_path(file.name()), data);
        }

        let container = read_utf8(&files, "META-INF/container.xml")?;
        let opf_path = parse_container(&container)?;
        let opf = read_utf8(&files, &opf_path)?;
        let mut package = parse_opf(&opf, &opf_path)?;

        if package.toc.is_empty() {
            package.toc = parse_toc_from_manifest(&files, &package)?;
        }

        Ok(Self { package, files })
    }

    pub fn render_spine_item(
        &self,
        spine_index: usize,
        reader_css: &str,
    ) -> Result<String, EpubError> {
        let item = self
            .package
            .spine
            .get(spine_index)
            .ok_or_else(|| EpubError::Invalid("chapter out of range".into()))?;
        let html = read_utf8(&self.files, &item.href)?;
        let rewritten = self.rewrite_resources(&html, &item.href);
        Ok(wrap_reader_document(
            &sanitize_xhtml(&rewritten),
            reader_css,
        ))
    }

    /// HTML grezzo di un capitolo (senza CSS reader né risorse riscritte):
    /// usato per estrarre il testo da leggere ad alta voce.
    pub fn spine_item_html(&self, spine_index: usize) -> Result<String, EpubError> {
        let item = self
            .package
            .spine
            .get(spine_index)
            .ok_or_else(|| EpubError::Invalid("chapter out of range".into()))?;
        read_utf8(&self.files, &item.href)
    }

    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let needle = query.trim().to_lowercase();
        if needle.len() < 2 {
            return Vec::new();
        }

        self.package
            .spine
            .iter()
            .enumerate()
            .filter_map(|(spine_index, item)| {
                let html = read_utf8(&self.files, &item.href).ok()?;
                let text = plain_text_from_html(&html);
                let haystack = text.to_lowercase();
                let offset = haystack.find(&needle)?;
                let mut start = offset.saturating_sub(70);
                while !text.is_char_boundary(start) {
                    start -= 1;
                }
                let mut end = (offset + needle.len() + 110).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                Some(SearchResult {
                    spine_index,
                    href: item.href.clone(),
                    title: title_for_href(&self.package.toc, &item.href)
                        .unwrap_or_else(|| format!("Chapter {}", spine_index + 1)),
                    snippet: text[start..end].trim().to_string(),
                })
            })
            .take(40)
            .collect()
    }

    fn rewrite_resources(&self, html: &str, current_href: &str) -> String {
        let mut out = html.to_string();
        for attr in ["src", "href"] {
            out = rewrite_attr_values(&out, attr, |value| {
                if is_external_url(value) || value.starts_with('#') || value.starts_with("data:") {
                    return value.to_string();
                }

                let path = resolve_relative(current_href, value);
                let clean_path = path.split('#').next().unwrap_or(&path);
                match self.files.get(clean_path) {
                    Some(bytes) => {
                        let mime = mime_for_path(clean_path);
                        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                        format!("data:{mime};base64,{encoded}")
                    }
                    None => value.to_string(),
                }
            });
        }
        out
    }
}

/// Estrae una risorsa (es. la copertina) da un EPUB come data URI,
/// senza dover tenere in memoria l'intero archivio decompresso.
pub fn cover_data_uri(bytes: &[u8], cover_href: &str) -> Option<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    let clean = normalize_path(cover_href);
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).ok()?;
        if file.is_dir() || normalize_path(file.name()) != clean {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).ok()?;
        let mime = mime_for_path(&clean);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
        return Some(format!("data:{mime};base64,{encoded}"));
    }
    None
}

fn parse_container(xml: &str) -> Result<String, EpubError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if local_name(e.name().as_ref()) == "rootfile" => {
                if let Some(path) = attr(&e, "full-path") {
                    return Ok(normalize_path(&path));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Err(EpubError::Invalid(
        "META-INF/container.xml has no rootfile".into(),
    ))
}

fn parse_opf(xml: &str, opf_path: &str) -> Result<EpubPackage, EpubError> {
    let base = parent_dir(opf_path);
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut metadata = EpubMetadata::default();
    let mut manifest = HashMap::<String, ManifestItem>::new();
    let mut spine_refs = Vec::<(String, bool)>::new();
    let mut current_text_tag: Option<String> = None;

    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let tag = local_name(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "title" | "creator" | "language" | "identifier" => {
                        current_text_tag = Some(tag);
                    }
                    "item" => {
                        if let Some(item) = manifest_item(&e, &base) {
                            manifest.insert(item.id.clone(), item);
                        }
                    }
                    "itemref" => {
                        if let Some(idref) = attr(&e, "idref") {
                            let linear = attr(&e, "linear").is_none_or(|v| v != "no");
                            spine_refs.push((idref, linear));
                        }
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => match local_name(e.name().as_ref()) {
                "item" => {
                    if let Some(item) = manifest_item(&e, &base) {
                        manifest.insert(item.id.clone(), item);
                    }
                }
                "itemref" => {
                    if let Some(idref) = attr(&e, "idref") {
                        let linear = attr(&e, "linear").is_none_or(|v| v != "no");
                        spine_refs.push((idref, linear));
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(tag) = &current_text_tag {
                    let value = text
                        .decode()
                        .map_err(|err| EpubError::Invalid(err.to_string()))?
                        .trim()
                        .to_string();
                    if !value.is_empty() {
                        match tag.as_str() {
                            "title" if metadata.title.is_empty() => metadata.title = value,
                            "creator" => metadata.creators.push(value),
                            "language" if metadata.language.is_none() => {
                                metadata.language = Some(value)
                            }
                            "identifier" if metadata.identifier.is_none() => {
                                metadata.identifier = Some(value)
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::End(e) => {
                let tag = local_name(e.name().as_ref()).to_string();
                if matches!(
                    tag.as_str(),
                    "title" | "creator" | "language" | "identifier"
                ) {
                    current_text_tag = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if metadata.title.is_empty() {
        metadata.title = "Untitled EPUB".into();
    }

    let spine = spine_refs
        .into_iter()
        .filter_map(|(idref, linear)| {
            let href = manifest.get(&idref)?.href.clone();
            Some(SpineItem {
                idref,
                href,
                linear,
            })
        })
        .collect::<Vec<_>>();

    if spine.is_empty() {
        return Err(EpubError::Invalid("EPUB spine is empty".into()));
    }

    // La TOC reale (nav/ncx) viene estratta da parse_toc_from_manifest al
    // caricamento del pacchetto: qui resta vuota, niente segnaposto.
    let toc = Vec::new();

    Ok(EpubPackage {
        metadata,
        manifest,
        spine,
        toc,
        opf_path: normalize_path(opf_path),
    })
}

fn parse_toc_from_manifest(
    files: &HashMap<String, Vec<u8>>,
    package: &EpubPackage,
) -> Result<Vec<TocItem>, EpubError> {
    for item in package.manifest.values() {
        let is_nav = item.properties.iter().any(|p| p == "nav");
        let is_ncx = item.media_type.contains("ncx");
        if !is_nav && !is_ncx {
            continue;
        }

        let xml = read_utf8(files, &item.href)?;
        let toc = if is_ncx {
            parse_ncx(&xml)
        } else {
            parse_nav(&xml)
        }?;

        if !toc.is_empty() {
            return Ok(toc);
        }
    }

    Ok(package
        .spine
        .iter()
        .enumerate()
        .map(|(index, item)| TocItem {
            label: format!("Chapter {}", index + 1),
            href: item.href.clone(),
            children: Vec::new(),
        })
        .collect())
}

fn parse_nav(xml: &str) -> Result<Vec<TocItem>, EpubError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut in_nav = false;
    let mut pending_href = None::<String>;
    let mut toc = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Start(e) => match local_name(e.name().as_ref()) {
                "nav" => in_nav = true,
                "a" if in_nav => pending_href = attr(&e, "href").map(|href| normalize_path(&href)),
                _ => {}
            },
            Event::Text(text) if in_nav => {
                if let Some(href) = pending_href.take() {
                    let label = text
                        .decode()
                        .map_err(|err| EpubError::Invalid(err.to_string()))?
                        .trim()
                        .to_string();
                    if !label.is_empty() {
                        toc.push(TocItem {
                            label,
                            href,
                            children: Vec::new(),
                        });
                    }
                }
            }
            Event::End(e) if local_name(e.name().as_ref()) == "nav" => in_nav = false,
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(toc)
}

fn parse_ncx(xml: &str) -> Result<Vec<TocItem>, EpubError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut toc = Vec::new();
    let mut in_text = false;
    let mut current_label = String::new();
    let mut current_href = None::<String>;

    loop {
        match reader.read_event()? {
            Event::Start(e) if local_name(e.name().as_ref()) == "text" => in_text = true,
            Event::Empty(e) if local_name(e.name().as_ref()) == "content" => {
                current_href = attr(&e, "src").map(|src| normalize_path(&src));
            }
            Event::Text(text) if in_text => {
                current_label = text
                    .decode()
                    .map_err(|err| EpubError::Invalid(err.to_string()))?
                    .trim()
                    .to_string()
            }
            Event::End(e) => match local_name(e.name().as_ref()) {
                "text" => in_text = false,
                "navPoint" => {
                    if let Some(href) = current_href.take() {
                        toc.push(TocItem {
                            label: if current_label.is_empty() {
                                "Section".into()
                            } else {
                                current_label.clone()
                            },
                            href,
                            children: Vec::new(),
                        });
                        current_label.clear();
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(toc)
}

fn manifest_item(e: &BytesStart<'_>, base: &str) -> Option<ManifestItem> {
    let id = attr(e, "id")?;
    let href = resolve_base(base, &attr(e, "href")?);
    let media_type = attr(e, "media-type").unwrap_or_default();
    let properties = attr(e, "properties")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    Some(ManifestItem {
        id,
        href,
        media_type,
        properties,
    })
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .filter_map(Result::ok)
        .find(|a| local_name(a.key.as_ref()) == name)
        .map(|a| String::from_utf8_lossy(a.value.as_ref()).to_string())
}

fn read_utf8(files: &HashMap<String, Vec<u8>>, path: &str) -> Result<String, EpubError> {
    let bytes = files
        .get(&normalize_path(path))
        .ok_or_else(|| EpubError::Invalid(format!("risorsa mancante: {path}")))?;
    Ok(String::from_utf8_lossy(bytes).to_string())
}

fn wrap_reader_document(body: &str, reader_css: &str) -> String {
    let content = if body.contains("<body") {
        body.to_string()
    } else {
        format!("<body><main>{body}</main></body>")
    };

    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>{reader_css}</style></head>{content}</html>"#
    )
}

fn sanitize_xhtml(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    loop {
        let Some(start) = rest.to_lowercase().find("<script") else {
            out.push_str(rest);
            break;
        };

        out.push_str(&rest[..start]);
        let after_start = &rest[start..];
        let Some(end) = after_start.to_lowercase().find("</script>") else {
            break;
        };
        rest = &after_start[end + "</script>".len()..];
    }

    strip_event_attrs(&out)
}

fn strip_event_attrs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    while let Some(ch) = chars.next() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ => {}
        }
        if in_tag && ch.is_whitespace() && matches!(chars.peek(), Some('o' | 'O')) {
            let mut probe = String::new();
            while let Some(next) = chars.peek().copied() {
                if next == '=' || next.is_whitespace() || probe.len() > 24 {
                    break;
                }
                probe.push(next);
                chars.next();
            }

            if probe.to_lowercase().starts_with("on") {
                while let Some(next) = chars.next() {
                    if next == '"' || next == '\'' {
                        let quote = next;
                        for q in chars.by_ref() {
                            if q == quote {
                                break;
                            }
                        }
                        break;
                    }
                    if next == '>' {
                        out.push('>');
                        in_tag = false;
                        break;
                    }
                }
            } else {
                out.push(ch);
                out.push_str(&probe);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn rewrite_attr_values<F>(html: &str, attr_name: &str, mut rewrite: F) -> String
where
    F: FnMut(&str) -> String,
{
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let needle = format!("{attr_name}=");

    while let Some(index) = rest.find(&needle) {
        out.push_str(&rest[..index + needle.len()]);
        rest = &rest[index + needle.len()..];
        let Some(quote) = rest.chars().next() else {
            break;
        };
        if quote != '"' && quote != '\'' {
            continue;
        }
        out.push(quote);
        rest = &rest[quote.len_utf8()..];
        if let Some(end) = rest.find(quote) {
            let value = &rest[..end];
            out.push_str(&rewrite(value));
            out.push(quote);
            rest = &rest[end + quote.len_utf8()..];
        } else {
            out.push_str(rest);
            rest = "";
            break;
        }
    }

    out.push_str(rest);
    out
}

fn title_for_href(toc: &[TocItem], href: &str) -> Option<String> {
    let href_no_fragment = href.split('#').next().unwrap_or(href);
    toc.iter().find_map(|item| {
        let item_href = item.href.split('#').next().unwrap_or(&item.href);
        // Le voci nav possono essere relative alla cartella del nav stesso,
        // quindi il confronto esatto viene integrato da un suffix match.
        if item_href == href_no_fragment
            || href_no_fragment.ends_with(item_href)
            || item_href.ends_with(href_no_fragment)
        {
            Some(item.label.clone())
        } else {
            title_for_href(&item.children, href)
        }
    })
}

fn is_external_url(value: &str) -> bool {
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("mailto:")
        || value.starts_with("javascript:")
}

fn resolve_relative(current_href: &str, value: &str) -> String {
    let base = parent_dir(current_href);
    resolve_base(&base, value)
}

fn resolve_base(base: &str, value: &str) -> String {
    let path = if base.is_empty() {
        value.to_string()
    } else {
        format!("{base}/{value}")
    };
    normalize_path(&path)
}

fn parent_dir(path: &str) -> String {
    normalize_path(path)
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

fn normalize_path(path: &str) -> String {
    let without_fragment = path.trim_start_matches('/').replace('\\', "/");
    let mut parts = Vec::<&str>::new();
    for part in without_fragment.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

fn local_name(name: &[u8]) -> &str {
    let raw = std::str::from_utf8(name).unwrap_or_default();
    raw.rsplit_once(':').map(|(_, local)| local).unwrap_or(raw)
}

fn mime_for_path(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "css" => "text/css",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "otf" => "font/otf",
        "ttf" => "font/ttf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "xhtml" | "html" => "application/xhtml+xml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_paths_without_escaping_root() {
        assert_eq!(
            resolve_relative("OPS/Text/chapter.xhtml", "../Images/cover.png"),
            "OPS/Images/cover.png"
        );
        assert_eq!(normalize_path("/OPS/./Text/../nav.xhtml"), "OPS/nav.xhtml");
    }

    #[test]
    fn parses_minimal_container_and_opf() {
        let container = r#"
        <container>
          <rootfiles>
            <rootfile full-path="OPS/content.opf"/>
          </rootfiles>
        </container>"#;
        assert_eq!(parse_container(container).unwrap(), "OPS/content.opf");

        let opf = r#"
        <package>
          <metadata>
            <dc:title>Test Book</dc:title>
            <dc:creator>Ada</dc:creator>
            <dc:language>it</dc:language>
          </metadata>
          <manifest>
            <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
            <item id="c1" href="Text/c1.xhtml" media-type="application/xhtml+xml"/>
          </manifest>
          <spine>
            <itemref idref="c1"/>
          </spine>
        </package>"#;
        let package = parse_opf(opf, "OPS/content.opf").unwrap();
        assert_eq!(package.metadata.title, "Test Book");
        assert_eq!(package.spine[0].href, "OPS/Text/c1.xhtml");
    }

    #[test]
    fn strips_event_attrs_only_inside_tags() {
        assert_eq!(
            strip_event_attrs("<p>i lauti onorarii che ricaverò</p>"),
            "<p>i lauti onorarii che ricaverò</p>"
        );
        assert_eq!(
            strip_event_attrs(r#"<a href="x" onclick="evil()">clicca</a>"#),
            r#"<a href="x">clicca</a>"#
        );
    }
}
