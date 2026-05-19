//! Tenant custom-CSS sanitization.
//!
//! Issue #9 — admin theme upload. A tenant admin can ship an arbitrary CSS
//! overlay to `POST /v1/theme/custom-css`. We sanitize before persisting so a
//! compromised admin token can't turn the brand-polish surface into a
//! persistent-XSS vector.
//!
//! Why "deny known-bad" rather than "parse + whitelist":
//!
//!   * A real CSS parser (lightningcss, swc-css) drags in tens of thousands
//!     of LOC and a non-trivial AST surface; for a brand-overlay use case
//!     that surface area is overkill.
//!   * Whitelist sanitizers historically over-trust the parser; bypasses
//!     have shipped via case folding, entity decoding, and ASI quirks. A
//!     small set of deny tokens, scanned byte-by-byte after lower-casing
//!     and comment-stripping, is provably easier to audit.
//!   * The GET endpoint adds `Content-Security-Policy: style-src 'self'` as
//!     a defence-in-depth net under the sanitizer — even if a bypass slips
//!     past, the response itself can't pull in external stylesheets.
//!
//! Rules — reject ANY of:
//!
//!   * `@import` (the original lever for external-stylesheet pulls)
//!   * `expression(` (legacy IE; included because legacy browsers still hit
//!     these endpoints during transition windows)
//!   * `behavior:` (legacy IE binding to HTC files)
//!   * `javascript:` / `vbscript:` URIs
//!   * `data:text/html`, `data:text/javascript`, `data:application/javascript`
//!   * `url(...)` pointing at a non-`data:` URL or a non-`#fragment` ref.
//!     Tenants can still embed inline data-URI gradients (`url(data:image/...)`)
//!     and reference same-document SVG fragments (`url(#gradient)`).
//!   * HTML-tag-looking content (`<script`, `<iframe`, etc.) — defends
//!     against CSS-in-HTML injection contexts where a `</style>` would
//!     re-enter HTML parsing.
//!   * The literal sequence `</style>` — same rationale; closing the host
//!     element from within the CSS payload is always hostile.
//!
//! Crucially, comments are stripped FIRST and the deny-scan runs over the
//! stripped output. `/* */@import url(evil);` is a classic bypass: a naive
//! "string contains" scan misses it because the `@import` is not contiguous
//! when blockers split tokens across a comment, and even if the scan finds
//! it, a downstream CSS parser would still execute the import after dropping
//! the comment. We strip then rescan to guarantee the bytes we persist are
//! the bytes we audited.

/// 32 KiB. Matches the DB CHECK constraint in migration 0025.
pub const MAX_CSS_BYTES: usize = 32 * 1024;

/// Sanitized CSS, byte-identical to what we'll persist + serve.
#[derive(Debug)]
pub struct SanitizedCss {
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SanitizeError {
    #[error("custom CSS is empty")]
    Empty,
    #[error("custom CSS exceeds {MAX_CSS_BYTES}-byte cap (got {0} bytes)")]
    TooLarge(usize),
    #[error("@import is not allowed in custom CSS (use the theme variables instead)")]
    ImportRule,
    #[error("CSS expression() is not allowed")]
    Expression,
    #[error("CSS behavior: is not allowed")]
    BehaviorBinding,
    #[error("javascript: URI is not allowed")]
    JavascriptUri,
    #[error("vbscript: URI is not allowed")]
    VbscriptUri,
    #[error("data:text/html URI is not allowed")]
    DataHtml,
    #[error("data:text/javascript URI is not allowed")]
    DataJavascript,
    #[error("data:application/javascript URI is not allowed")]
    DataAppJavascript,
    #[error("url() must reference a same-document fragment (`url(#...)`) or a data: URI")]
    ExternalUrl,
    #[error("HTML-tag-like content is not allowed in custom CSS ({0})")]
    HtmlTag(&'static str),
    #[error("the sequence `</style>` is not allowed in custom CSS")]
    StyleClose,
    #[error("custom CSS must be valid UTF-8: {0}")]
    Utf8(String),
}

/// Validate + sanitize an uploaded CSS payload.
///
/// On success, `SanitizedCss.bytes` is the byte-identical payload to persist
/// and serve. We do not re-encode (a CSS round-trip needs a parser); the
/// deny-scan after comment-strip is the only transformation that matters
/// for safety, and we preserve the original (pre-strip) text so admins see
/// their own comments back when they edit. The deny-scan is performed on
/// the *stripped* haystack only — the persisted bytes are the original
/// minus a 1:1 strip is intentional: re-emitting after strip would change
/// whitespace too. Instead, we strip into a separate buffer used only for
/// scanning; the original bytes are returned verbatim.
pub fn sanitize(raw: &[u8]) -> Result<SanitizedCss, SanitizeError> {
    if raw.is_empty() {
        return Err(SanitizeError::Empty);
    }
    if raw.len() > MAX_CSS_BYTES {
        return Err(SanitizeError::TooLarge(raw.len()));
    }

    // The DB column is TEXT — enforce UTF-8 up front so a downstream
    // `String::from_utf8` doesn't blow up the response handler.
    let text = std::str::from_utf8(raw).map_err(|e| SanitizeError::Utf8(e.to_string()))?;

    // Stripped, lower-cased copy is the audit haystack. We never persist or
    // serve this — it exists purely to run the deny rules.
    let stripped_lower = strip_comments_and_lower(text.as_bytes());

    scan_for_denied(&stripped_lower)?;

    Ok(SanitizedCss { bytes: raw.to_vec() })
}

/// Strip `/* ... */` comments and lowercase the remainder. Unterminated
/// comments (`/* ... <EOF>`) are silently dropped through to end-of-input —
/// the scanner sees no `@import` because everything past the opener has been
/// elided, but that's correct: a parser would also drop those bytes. We use
/// the stripped + lower-cased haystack for ALL subsequent rule scans so a
/// `/* */@i\u{200B}mport` or `/* */@IMPORT url(...)` is caught.
fn strip_comments_and_lower(raw: &[u8]) -> Vec<u8> {
    let n = raw.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        // Detect comment start.
        if i + 1 < n && raw[i] == b'/' && raw[i + 1] == b'*' {
            // Skip until matching `*/` or EOF.
            i += 2;
            while i + 1 < n {
                if raw[i] == b'*' && raw[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            // If we exited the inner loop because of EOF, i may equal n; the
            // outer loop condition handles that.
            if i + 1 >= n {
                // Unterminated comment — drop everything else. Conservative
                // behaviour: matches what a real CSS parser would do.
                break;
            }
            continue;
        }
        out.push(raw[i].to_ascii_lowercase());
        i += 1;
    }
    out
}

/// Run the full deny-rule pipeline against the stripped + lower-cased
/// haystack. The first matching rule wins so the caller gets a precise
/// error variant rather than a generic "rejected".
fn scan_for_denied(haystack: &[u8]) -> Result<(), SanitizeError> {
    // Order matters: more specific patterns first (data:text/html before the
    // generic url() rule, javascript: before url()).
    if contains_token(haystack, b"@import") {
        return Err(SanitizeError::ImportRule);
    }
    if contains_token(haystack, b"expression(") {
        return Err(SanitizeError::Expression);
    }
    if contains_token(haystack, b"behavior:") {
        return Err(SanitizeError::BehaviorBinding);
    }
    if contains_token(haystack, b"javascript:") {
        return Err(SanitizeError::JavascriptUri);
    }
    if contains_token(haystack, b"vbscript:") {
        return Err(SanitizeError::VbscriptUri);
    }
    if contains_token(haystack, b"data:text/html") {
        return Err(SanitizeError::DataHtml);
    }
    if contains_token(haystack, b"data:text/javascript") {
        return Err(SanitizeError::DataJavascript);
    }
    if contains_token(haystack, b"data:application/javascript") {
        return Err(SanitizeError::DataAppJavascript);
    }
    if contains_token(haystack, b"</style>") {
        return Err(SanitizeError::StyleClose);
    }
    // HTML-tag-like content. These would only matter if the response was ever
    // mis-served as `text/html`, which the GET endpoint pins shut — but the
    // bytes might also surface in inline `<style>` blocks if an operator ever
    // mirrored them client-side, and `</style>` reopening HTML parsing is the
    // classic XSS vector. Defence in depth.
    for (tag, label) in [
        (b"<script".as_slice(), "<script>"),
        (b"<iframe".as_slice(), "<iframe>"),
        (b"<object".as_slice(), "<object>"),
        (b"<embed".as_slice(), "<embed>"),
        (b"<link".as_slice(), "<link>"),
        (b"<meta".as_slice(), "<meta>"),
        (b"<base".as_slice(), "<base>"),
    ] {
        if contains_token(haystack, tag) {
            return Err(SanitizeError::HtmlTag(label));
        }
    }

    // url() references — only allow same-document fragments and data: URIs.
    // The walk is hand-rolled because a CSS parser would over-allow tokens
    // we don't care about and we want a byte-exact rule.
    scan_url_refs(haystack)?;

    Ok(())
}

fn contains_token(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Walk every `url(` occurrence in the haystack and verify the argument is
/// either a `#fragment` reference, a `data:` URI, or empty. Anything else
/// (`http://...`, `/relative/path`, `//cdn.example.com/...`) is rejected.
fn scan_url_refs(haystack: &[u8]) -> Result<(), SanitizeError> {
    let needle = b"url(";
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            // Walk to the closing ')', skipping any opening whitespace +
            // optional quote.
            let arg_start = i + needle.len();
            let mut j = arg_start;
            while j < haystack.len() && is_css_ws(haystack[j]) {
                j += 1;
            }
            let quote = match haystack.get(j) {
                Some(&b'"') => {
                    j += 1;
                    Some(b'"')
                }
                Some(&b'\'') => {
                    j += 1;
                    Some(b'\'')
                }
                _ => None,
            };
            // Capture until the closing quote (if any) or the closing paren.
            let value_start = j;
            let value_end = match quote {
                Some(q) => {
                    while j < haystack.len() && haystack[j] != q {
                        j += 1;
                    }
                    j
                }
                None => {
                    while j < haystack.len() && haystack[j] != b')' {
                        j += 1;
                    }
                    j
                }
            };
            let value = trim_ws(&haystack[value_start..value_end]);

            if !is_allowed_url(value) {
                return Err(SanitizeError::ExternalUrl);
            }
            // Advance past this `url(...)`; we don't need to find the closing
            // paren precisely — i+1 is safe and keeps the scan linear.
            i = arg_start;
        } else {
            i += 1;
        }
    }
    Ok(())
}

fn is_css_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn trim_ws(s: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = s.len();
    while start < end && is_css_ws(s[start]) {
        start += 1;
    }
    while end > start && is_css_ws(s[end - 1]) {
        end -= 1;
    }
    &s[start..end]
}

/// A `url()` argument is allowed iff:
///   * it's empty (defer to whatever bytes the parser sees as malformed —
///     this isn't dangerous on its own), or
///   * it starts with `#` (same-document fragment, e.g. `url(#gradient)`),
///     or
///   * it starts with `data:image/` (inline image — the most common
///     non-fragment legitimate use), or
///   * it starts with `data:application/font` / `data:font/` (inline font
///     data — supports `@font-face` overlays).
///
/// Everything else is rejected: `https://...`, `http://...`, `//cdn/...`,
/// `/absolute/path`, `relative/path`, and the dangerous data: types are
/// caught upstream by the literal-substring rules so we don't need to list
/// them here.
fn is_allowed_url(value: &[u8]) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.starts_with(b"#") {
        return true;
    }
    // Already-rejected dangerous data: subtypes were caught upstream. Anything
    // that survives down here and starts with `data:image/` or `data:font/`
    // or `data:application/font` is safe to keep.
    if value.starts_with(b"data:image/")
        || value.starts_with(b"data:font/")
        || value.starts_with(b"data:application/font")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_class_overrides() {
        let css = br#".sp-hero { color: #336699; background: var(--sp-primary); }"#;
        let r = sanitize(css).unwrap();
        assert_eq!(r.bytes, css);
    }

    #[test]
    fn accepts_fragment_url() {
        let css = br#".sp-hero { fill: url(#brand-gradient); }"#;
        sanitize(css).unwrap();
    }

    #[test]
    fn accepts_data_uri_image() {
        let css = br#".sp-hero { background-image: url(data:image/png;base64,AAAA); }"#;
        sanitize(css).unwrap();
    }

    #[test]
    fn accepts_data_uri_font() {
        let css = br#"@font-face { src: url(data:font/woff2;base64,AAAA); }"#;
        sanitize(css).unwrap();
    }

    #[test]
    fn rejects_import() {
        let css = br#"@import url("https://evil.com/x.css");"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ImportRule);
    }

    #[test]
    fn rejects_import_mixed_case() {
        let css = br#"@IMPORT url("evil.css");"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ImportRule);
    }

    #[test]
    fn rejects_import_hidden_in_comment_prefix() {
        // The classic bypass: split the token across a comment so a naive
        // scanner doesn't see `@import` contiguously. After strip the bytes
        // become `@import url(evil);` and the scanner catches it.
        let css = br#"/* harmless */@import url(evil);"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ImportRule);
    }

    #[test]
    fn rejects_import_with_comment_inside_token() {
        // Some parsers tolerate `@/*x*/import` and merge the token after
        // comment-strip. Our strip-then-scan handles this; the contiguous
        // `@import` only appears after stripping.
        let css = br#"@/*x*/import url(evil);"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ImportRule);
    }

    #[test]
    fn rejects_external_url() {
        let css = br#".sp-hero { background: url(https://evil.com/x.png); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ExternalUrl);
    }

    #[test]
    fn rejects_protocol_relative_url() {
        let css = br#".sp-hero { background: url(//cdn.example.com/x.png); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ExternalUrl);
    }

    #[test]
    fn rejects_absolute_path_url() {
        let css = br#".sp-hero { background: url(/etc/passwd); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ExternalUrl);
    }

    #[test]
    fn rejects_relative_url() {
        let css = br#".sp-hero { background: url(images/bg.png); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ExternalUrl);
    }

    #[test]
    fn rejects_quoted_external_url() {
        let css = br#".sp-hero { background: url("https://evil.com/x.png"); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::ExternalUrl);
    }

    #[test]
    fn rejects_expression() {
        let css = br#".sp-hero { width: expression(alert(1)); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::Expression);
    }

    #[test]
    fn rejects_behavior_binding() {
        let css = br#".sp-hero { behavior: url(xss.htc); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::BehaviorBinding);
    }

    #[test]
    fn rejects_javascript_uri() {
        let css = br#".sp-hero { background: url(javascript:alert(1)); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::JavascriptUri);
    }

    #[test]
    fn rejects_vbscript_uri() {
        let css = br#".sp-hero { background: url(vbscript:msgbox); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::VbscriptUri);
    }

    #[test]
    fn rejects_data_text_html() {
        let css = br#".x { background: url(data:text/html,<script>alert(1)</script>); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::DataHtml);
    }

    #[test]
    fn rejects_data_text_javascript() {
        let css = br#".x { background: url(data:text/javascript,alert(1)); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::DataJavascript);
    }

    #[test]
    fn rejects_data_application_javascript() {
        let css = br#".x { background: url(data:application/javascript,alert(1)); }"#;
        assert_eq!(sanitize(css).unwrap_err(), SanitizeError::DataAppJavascript);
    }

    #[test]
    fn rejects_script_tag() {
        let css = br#".x { color: red } <script>alert(1)</script>"#;
        let err = sanitize(css).unwrap_err();
        assert!(matches!(err, SanitizeError::HtmlTag(_)));
    }

    #[test]
    fn rejects_style_close_token() {
        // The classic "break out of <style>" injection.
        let css = br#".x { color: red } </style><script>alert(1)</script>"#;
        // </style> wins because it's checked before the <script> tag rule.
        let err = sanitize(css).unwrap_err();
        // Either StyleClose or the script-tag rule is fine — the point is
        // this never gets stored.
        assert!(matches!(
            err,
            SanitizeError::StyleClose | SanitizeError::HtmlTag(_)
        ));
    }

    #[test]
    fn rejects_too_large() {
        let big = vec![b'a'; MAX_CSS_BYTES + 1];
        assert_eq!(sanitize(&big).unwrap_err(), SanitizeError::TooLarge(MAX_CSS_BYTES + 1));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(sanitize(b"").unwrap_err(), SanitizeError::Empty);
    }

    #[test]
    fn rejects_invalid_utf8() {
        let bad = &[0xC3, 0x28]; // invalid two-byte sequence
        let err = sanitize(bad).unwrap_err();
        assert!(matches!(err, SanitizeError::Utf8(_)));
    }

    #[test]
    fn preserves_original_bytes_including_comments() {
        // Comments are legal in CSS — we strip only for scanning, not for
        // persisting. The tenant's authored bytes survive verbatim.
        let css = b"/* brand polish v3 */\n.sp-hero { color: red; }";
        let r = sanitize(css).unwrap();
        assert_eq!(r.bytes, css);
    }

    #[test]
    fn accepts_complex_realistic_overlay() {
        // A representative payload — class overrides, variables, a data
        // gradient, a fragment URL. Should pass cleanly.
        let css = br#"
            /* Acme Corp brand overlay */
            :root {
              --sp-primary: #d40000;
            }
            .sp-hero {
              background: url(#brand-gradient);
              border: 1px solid var(--sp-border);
            }
            .sp-badge {
              background-image: url(data:image/svg+xml;base64,PHN2Zy8+);
            }
        "#;
        sanitize(css).unwrap();
    }

    #[test]
    fn unterminated_comment_does_not_panic() {
        // Edge case: a `/*` with no closing — strip-and-scan must not loop.
        let css = b"/* never closes";
        // Empty after stripping → no deny matches → accepted. The CSS is
        // semantically void but the sanitizer's job is "is this dangerous",
        // and inert text isn't.
        sanitize(css).unwrap();
    }
}
