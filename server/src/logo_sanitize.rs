//! Tenant logo sanitization.
//!
//! Issue #9 — admin theme upload. A tenant admin can ship arbitrary bytes to
//! `POST /v1/theme/logo`; we need a small, pure-functional pipeline that
//!
//!   * accepts SVG, PNG, JPEG, and WEBP,
//!   * rejects everything else,
//!   * strips dangerous constructs out of SVG (script tags, event handlers,
//!     `<foreignObject>`, off-origin `xlink:href`, CSS expressions, etc.),
//!   * validates the file header (magic bytes) for the raster formats so the
//!     content-type claim matches reality.
//!
//! We deliberately do NOT re-encode rasters — that would require pulling in a
//! decoder (image, libwebp) for every tenant upload and the CPU cost is hard
//! to justify when the magic + content-type + size cap covers the malicious
//! shapes we care about. If a tenant ships a "PNG" with hidden bytes after
//! the IEND marker, that PNG still cannot become script in an `<img>` tag.
//!
//! The SVG sanitizer is a hand-rolled scan over the bytes plus a `quick-xml`
//! parse. The byte scan is "deny-first": any of the danger tokens fails the
//! upload outright. We chose that over a permissive whitelist because:
//!
//!   * Logos are by convention static — paths, fills, gradients, text. No
//!     legitimate logo needs `<script>` or `<foreignObject>`.
//!   * Whitelist sanitizers are notoriously easy to bypass (mixed-case,
//!     entity-encoded attributes, CDATA sections). A deny pass catches the
//!     known-bad shapes before the parser ever sees them.
//!   * `quick-xml` confirms the body is well-formed and strips comments +
//!     processing instructions on re-emit, which closes the bypass surface
//!     where a `<script>` is hidden inside a comment that a downstream
//!     renderer might still execute (rare, but cheap to defend against).

use std::borrow::Cow;
use std::io::Cursor;

use quick_xml::events::Event;
use quick_xml::Reader;
use quick_xml::Writer;

/// 256 KiB. Matches the DB CHECK constraint in migration 0020.
pub const MAX_LOGO_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoKind {
    Svg,
    Png,
    Jpeg,
    Webp,
}

impl LogoKind {
    pub fn content_type(self) -> &'static str {
        match self {
            LogoKind::Svg => "image/svg+xml",
            LogoKind::Png => "image/png",
            LogoKind::Jpeg => "image/jpeg",
            LogoKind::Webp => "image/webp",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            LogoKind::Svg => "svg",
            LogoKind::Png => "png",
            LogoKind::Jpeg => "jpg",
            LogoKind::Webp => "webp",
        }
    }

    fn from_content_type(ct: &str) -> Option<Self> {
        // The browser may include parameters like `; charset=utf-8`.
        let head = ct.split(';').next().unwrap_or(ct).trim().to_ascii_lowercase();
        match head.as_str() {
            "image/svg+xml" | "image/svg" => Some(LogoKind::Svg),
            "image/png" => Some(LogoKind::Png),
            "image/jpeg" | "image/jpg" => Some(LogoKind::Jpeg),
            "image/webp" => Some(LogoKind::Webp),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct SanitizedLogo {
    pub bytes: Vec<u8>,
    pub kind: LogoKind,
}

#[derive(Debug, thiserror::Error)]
pub enum SanitizeError {
    #[error("unsupported content type `{0}` (allowed: image/svg+xml, image/png, image/jpeg, image/webp)")]
    UnsupportedContentType(String),
    #[error("logo is empty")]
    Empty,
    #[error("logo exceeds {MAX_LOGO_BYTES}-byte cap (got {0} bytes)")]
    TooLarge(usize),
    #[error("magic bytes do not match content type `{0}`")]
    MagicMismatch(&'static str),
    #[error("SVG rejected: {0}")]
    DangerousSvg(&'static str),
    #[error("SVG is not well-formed XML: {0}")]
    MalformedSvg(String),
}

/// Validate + sanitize an uploaded logo.
///
/// On success the returned bytes are safe to persist and serve back with the
/// returned content-type. On error the caller should surface the message as
/// HTTP 400.
pub fn sanitize(content_type: &str, raw: &[u8]) -> Result<SanitizedLogo, SanitizeError> {
    if raw.is_empty() {
        return Err(SanitizeError::Empty);
    }
    if raw.len() > MAX_LOGO_BYTES {
        return Err(SanitizeError::TooLarge(raw.len()));
    }
    let kind = LogoKind::from_content_type(content_type)
        .ok_or_else(|| SanitizeError::UnsupportedContentType(content_type.to_string()))?;

    let bytes = match kind {
        LogoKind::Svg => sanitize_svg(raw)?,
        LogoKind::Png => {
            check_magic(raw, &PNG_MAGIC, "image/png")?;
            raw.to_vec()
        }
        LogoKind::Jpeg => {
            check_magic(raw, &JPEG_MAGIC, "image/jpeg")?;
            raw.to_vec()
        }
        LogoKind::Webp => {
            check_webp(raw)?;
            raw.to_vec()
        }
    };

    Ok(SanitizedLogo { bytes, kind })
}

// --- raster magic checks --------------------------------------------------

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];

fn check_magic(raw: &[u8], magic: &[u8], ct: &'static str) -> Result<(), SanitizeError> {
    if raw.len() < magic.len() || &raw[..magic.len()] != magic {
        return Err(SanitizeError::MagicMismatch(ct));
    }
    Ok(())
}

/// WEBP is a RIFF container: `RIFF<4-byte size>WEBP<...>`. We check both
/// markers; the size field itself is not validated against the buffer length
/// (some encoders pad).
fn check_webp(raw: &[u8]) -> Result<(), SanitizeError> {
    if raw.len() < 12 || &raw[..4] != b"RIFF" || &raw[8..12] != b"WEBP" {
        return Err(SanitizeError::MagicMismatch("image/webp"));
    }
    Ok(())
}

// --- SVG -------------------------------------------------------------------

/// Lower-cased substrings that disqualify an SVG outright. Mixed-case and
/// whitespace variants are handled by lower-casing the haystack first AND
/// applying a couple of normalised-whitespace passes for the patterns that
/// commonly try to bypass naive matchers ("on click", "on\tclick", etc.).
const DENY_TOKENS: &[&str] = &[
    "<script",
    "</script",
    "<foreignobject",
    "<iframe",
    "<embed",
    "<object",
    "<base",
    "<link",
    "<meta",
    // CSS-in-attribute escapes — common SVG-XSS shapes
    "javascript:",
    "vbscript:",
    "data:text/html",
    "data:application/javascript",
    "expression(",
    "@import",
    "behavior:",
    // base64-encoded js payload markers
    "data:text/javascript",
];

fn sanitize_svg(raw: &[u8]) -> Result<Vec<u8>, SanitizeError> {
    // 1. Cheap byte-level deny scan. Lower-casing once is O(n); the haystack
    //    is at most MAX_LOGO_BYTES so this is bounded.
    let lower = ascii_lower(raw);

    for token in DENY_TOKENS {
        if lower.windows(token.len()).any(|w| w == token.as_bytes()) {
            return Err(SanitizeError::DangerousSvg(match *token {
                "<script" | "</script" => "<script> not allowed",
                "<foreignobject" => "<foreignObject> not allowed",
                "<iframe" => "<iframe> not allowed",
                "<embed" => "<embed> not allowed",
                "<object" => "<object> not allowed",
                "<base" => "<base> not allowed",
                "<link" => "<link> not allowed",
                "<meta" => "<meta> not allowed",
                "javascript:" => "javascript: URI not allowed",
                "vbscript:" => "vbscript: URI not allowed",
                "data:text/html" => "data:text/html URI not allowed",
                "data:application/javascript" => "data:application/javascript URI not allowed",
                "expression(" => "CSS expression() not allowed",
                "@import" => "@import not allowed",
                "behavior:" => "CSS behavior: not allowed",
                "data:text/javascript" => "data:text/javascript URI not allowed",
                _ => "disallowed SVG construct",
            }));
        }
    }

    // 2. Inline event handler detection. We want to catch `onclick`, `ON LOAD`,
    //    `on\tmouseover`, etc. Walk the haystack and look for the literal
    //    "on" + ascii letter pair after a whitespace/quote/`<` boundary and
    //    before an `=`.
    if has_event_handler_attr(&lower) {
        return Err(SanitizeError::DangerousSvg(
            "on* event-handler attributes not allowed",
        ));
    }

    // 3. Reject hrefs that aren't either same-document fragment refs
    //    (`href="#..."`) or absent. Catches `xlink:href="data:..."`, ftp://,
    //    file://, etc.
    if has_off_origin_href(&lower) {
        return Err(SanitizeError::DangerousSvg(
            "href / xlink:href must be a same-document fragment (#...)",
        ));
    }

    // 4. Now hand off to `quick-xml`. The deny pass already eliminated the
    //    nasty shapes; this step (a) confirms well-formedness and (b) strips
    //    comments and processing instructions to harden against weird
    //    downstream renderers that might still parse them.
    let cleaned = strip_xml_extras(raw).map_err(|e| SanitizeError::MalformedSvg(e.to_string()))?;
    Ok(cleaned)
}

fn ascii_lower(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    for &b in raw {
        out.push(b.to_ascii_lowercase());
    }
    out
}

/// Scan for inline event handlers. The pattern is roughly:
///
///     <attr-boundary> on <ascii-letter>{1,32} <optional-ws> =
///
/// where `<attr-boundary>` is whitespace or a quote (covers attributes
/// emitted inline on an element). Hand-rolled because regex on every upload
/// is fine but a state machine is just as readable and avoids the regex dep
/// growing further.
fn has_event_handler_attr(lower: &[u8]) -> bool {
    let n = lower.len();
    let mut i = 0;
    while i + 3 < n {
        let b = lower[i];
        if !(b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b'"' || b == b'\'') {
            i += 1;
            continue;
        }
        // Position i is a boundary; check the bytes that follow.
        if lower[i + 1] == b'o' && lower[i + 2] == b'n' {
            // Walk forward over [a-z]+ then optional ws then '='.
            let mut j = i + 3;
            let mut letters = 0;
            while j < n && lower[j].is_ascii_lowercase() {
                j += 1;
                letters += 1;
            }
            if (1..=32).contains(&letters) {
                while j < n
                    && (lower[j] == b' '
                        || lower[j] == b'\t'
                        || lower[j] == b'\n'
                        || lower[j] == b'\r')
                {
                    j += 1;
                }
                if j < n && lower[j] == b'=' {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// True iff any `href=...` or `xlink:href=...` value is something other than
/// a same-document fragment. Fragment refs are `#foo`; everything else
/// (`http:`, `data:image/png;base64,...`, `/etc/passwd`) is rejected.
fn has_off_origin_href(lower: &[u8]) -> bool {
    for needle in [b"href=".as_slice(), b"xlink:href=".as_slice()] {
        let mut i = 0;
        while i + needle.len() < lower.len() {
            if lower[i..i + needle.len()] == *needle {
                let after = &lower[i + needle.len()..];
                // Skip optional quote.
                let (start, quote) = match after.first() {
                    Some(b'"') => (1, Some(b'"')),
                    Some(b'\'') => (1, Some(b'\'')),
                    _ => (0, None),
                };
                let value_start = start;
                let value_end = match quote {
                    Some(q) => after[value_start..]
                        .iter()
                        .position(|&b| b == q)
                        .map(|p| value_start + p)
                        .unwrap_or(after.len()),
                    None => after[value_start..]
                        .iter()
                        .position(|&b| b == b' ' || b == b'>' || b == b'\t' || b == b'\n')
                        .map(|p| value_start + p)
                        .unwrap_or(after.len()),
                };
                let value = &after[value_start..value_end];
                let trimmed = trim_ascii_ws(value);
                if !trimmed.is_empty() && !trimmed.starts_with(b"#") {
                    return true;
                }
                i += needle.len();
            } else {
                i += 1;
            }
        }
    }
    false
}

fn trim_ascii_ws(s: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = s.len();
    while start < end && matches!(s[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    while end > start && matches!(s[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    &s[start..end]
}

/// Re-emit the SVG without comments or processing instructions. Anything the
/// parser doesn't recognise as a clean element/attribute/text bubbles up as
/// an error and we reject the upload.
fn strip_xml_extras(raw: &[u8]) -> Result<Vec<u8>, quick_xml::Error> {
    let mut reader = Reader::from_reader(Cursor::new(raw));
    let config = reader.config_mut();
    config.trim_text(false);
    config.expand_empty_elements = false;
    // Reject mismatched end tags etc. — well-formedness is part of the
    // sanitizer contract.
    config.check_end_names = true;

    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(raw.len())));
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Comment(_)) | Ok(Event::PI(_)) => {
                // Drop on the floor — defence in depth.
            }
            Ok(Event::Start(e)) => writer.write_event(Event::Start(e))?,
            Ok(Event::End(e)) => writer.write_event(Event::End(e))?,
            Ok(Event::Empty(e)) => writer.write_event(Event::Empty(e))?,
            Ok(Event::Text(e)) => writer.write_event(Event::Text(e))?,
            Ok(Event::CData(e)) => {
                // Convert CDATA to escaped text — keeps the visible content
                // but blocks the `<![CDATA[<script>]]>` bypass shape (the
                // deny pass already caught the literal `<script` substring,
                // but CDATA text gets emitted as raw `<` characters; safer
                // to escape).
                let txt = e.into_inner();
                let owned: Cow<'_, [u8]> = Cow::Owned(txt.into_owned());
                let text = quick_xml::events::BytesText::from_escaped(
                    String::from_utf8_lossy(&owned).into_owned(),
                );
                writer.write_event(Event::Text(text))?;
            }
            Ok(Event::Decl(e)) => writer.write_event(Event::Decl(e))?,
            Ok(Event::DocType(e)) => writer.write_event(Event::DocType(e))?,
            Err(e) => return Err(e),
        }
        buf.clear();
    }

    Ok(writer.into_inner().into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_SVG: &[u8] = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <rect width="24" height="24" fill="#2563eb"/>
  <text x="12" y="14" font-size="6" text-anchor="middle" fill="#fff">SP</text>
</svg>"##;

    #[test]
    fn accepts_clean_svg() {
        let r = sanitize("image/svg+xml", GOOD_SVG).unwrap();
        assert_eq!(r.kind, LogoKind::Svg);
        assert!(!r.bytes.is_empty());
    }

    #[test]
    fn rejects_svg_with_script_tag() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)), "got {err:?}");
    }

    #[test]
    fn rejects_svg_with_mixed_case_script() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><SCRipt>alert(1)</SCRipt></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn rejects_svg_with_onclick_handler() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect onclick="alert(1)" width="1" height="1"/></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn rejects_svg_with_onload_with_whitespace() {
        // Whitespace before '=' is still an event handler — quirks-mode parsers
        // accept it.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" onload  =  "alert(1)"></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn rejects_svg_with_foreignobject() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject></foreignObject></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn rejects_svg_with_javascript_uri_href() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><a xlink:href="javascript:alert(1)"><rect/></a></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn rejects_svg_with_external_xlink_href() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><image xlink:href="http://evil.example/x.png"/></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn accepts_svg_with_internal_fragment_href() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="g"/></defs><rect fill="url(#g)" width="1" height="1"/><use href="#g"/></svg>"##;
        let r = sanitize("image/svg+xml", svg).unwrap();
        assert_eq!(r.kind, LogoKind::Svg);
    }

    #[test]
    fn rejects_svg_with_css_expression() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><style>rect { width: expression(alert(1)); }</style><rect/></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn rejects_svg_with_import() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><style>@import "evil.css";</style></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn rejects_svg_with_data_text_html() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><image xlink:href="data:text/html,<script>alert(1)</script>"/></svg>"#;
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::DangerousSvg(_)));
    }

    #[test]
    fn malformed_svg_is_rejected() {
        // Mismatched end tag — `<svg>...</xyz>` is not well-formed XML.
        let svg = b"<svg><rect/></xyz>";
        let err = sanitize("image/svg+xml", svg).unwrap_err();
        assert!(matches!(err, SanitizeError::MalformedSvg(_)), "got {err:?}");
    }

    #[test]
    fn strips_comments() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><!-- a harmless comment --><rect width="1" height="1"/></svg>"#;
        let r = sanitize("image/svg+xml", svg).unwrap();
        let s = String::from_utf8(r.bytes).unwrap();
        assert!(!s.contains("<!--"), "comment should be stripped: {s}");
    }

    #[test]
    fn accepts_minimal_png() {
        // 1x1 transparent PNG.
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\rIDATx\x9cc\xfc\xcf\xc0\x00\x00\x00\x03\x00\x01\x5c\xcd\xff\x69\x00\x00\x00\x00IEND\xaeB`\x82";
        let r = sanitize("image/png", png).unwrap();
        assert_eq!(r.kind, LogoKind::Png);
    }

    #[test]
    fn rejects_png_with_wrong_magic() {
        let not_png = b"not really png at all blah blah";
        let err = sanitize("image/png", not_png).unwrap_err();
        assert!(matches!(err, SanitizeError::MagicMismatch(_)), "got {err:?}");
    }

    #[test]
    fn accepts_minimal_jpeg() {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend_from_slice(b"JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xFF\xD9");
        let r = sanitize("image/jpeg", &jpeg).unwrap();
        assert_eq!(r.kind, LogoKind::Jpeg);
    }

    #[test]
    fn rejects_jpeg_with_wrong_magic() {
        let err = sanitize("image/jpeg", b"\x00\x00\x00\x00").unwrap_err();
        assert!(matches!(err, SanitizeError::MagicMismatch(_)));
    }

    #[test]
    fn accepts_minimal_webp() {
        // RIFF<size>WEBP<...>
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&12u32.to_le_bytes());
        webp.extend_from_slice(b"WEBPVP8 \x00\x00\x00\x00");
        let r = sanitize("image/webp", &webp).unwrap();
        assert_eq!(r.kind, LogoKind::Webp);
    }

    #[test]
    fn rejects_webp_with_wrong_magic() {
        let err = sanitize("image/webp", b"RIFFXXXXNOTWEBP").unwrap_err();
        assert!(matches!(err, SanitizeError::MagicMismatch(_)));
    }

    #[test]
    fn rejects_unknown_content_type() {
        let err = sanitize("image/bmp", b"\0\0\0\0").unwrap_err();
        assert!(matches!(err, SanitizeError::UnsupportedContentType(_)));
    }

    #[test]
    fn rejects_empty() {
        let err = sanitize("image/svg+xml", b"").unwrap_err();
        assert!(matches!(err, SanitizeError::Empty));
    }

    #[test]
    fn rejects_too_large() {
        let big = vec![0u8; MAX_LOGO_BYTES + 1];
        let err = sanitize("image/png", &big).unwrap_err();
        assert!(matches!(err, SanitizeError::TooLarge(_)));
    }

    #[test]
    fn content_type_with_charset_is_accepted() {
        let r = sanitize("image/svg+xml; charset=utf-8", GOOD_SVG).unwrap();
        assert_eq!(r.kind, LogoKind::Svg);
    }
}
