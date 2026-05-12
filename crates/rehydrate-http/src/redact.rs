//! Strip bearer / basic-auth tokens out of strings that might end up
//! in error bodies surfaced to the renderer or to user-facing logs.
//!
//! Some servers (Ghost included, in error paths) echo the incoming
//! `Authorization` header back in JSON error responses. Without
//! redaction those tokens would land in the UI's recent-logs panel
//! and on disk under the user's log directory. The redactor is
//! intentionally aggressive — it errs on the side of over-redacting
//! because a false positive in a debug string is far cheaper than a
//! leaked Admin API key.

/// Apply the redaction. Two pattern classes, each with its own
/// terminator rule:
///
/// - **JSON-shaped header**: `"Authorization":"<token>"`. The value
///   runs until the next unescaped `"`. Used by every JSON error
///   body that includes request headers.
/// - **Bare scheme prefix**: `Bearer <token>`, `Basic …`, `Ghost …`.
///   These can appear inline anywhere — header logs, Rust Debug
///   strings, error messages. The token runs until whitespace or
///   one of `",}]` (so JSON-quoted forms also terminate cleanly).
///
/// Both prefix matches are case-insensitive.
pub fn redact_credentials(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(prefix_len) = match_json_authorization(&bytes[i..]) {
            out.push_str(&s[i..i + prefix_len]);
            i += prefix_len;
            let mut j = i;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            if j > i {
                out.push_str("[REDACTED]");
                i = j;
            }
            continue;
        }
        if let Some(prefix_len) = match_scheme_prefix(&bytes[i..]) {
            out.push_str(&s[i..i + prefix_len]);
            i += prefix_len;
            let mut j = i;
            while j < bytes.len() {
                let c = bytes[j];
                if c == b' '
                    || c == b'\t'
                    || c == b'\n'
                    || c == b'\r'
                    || c == b'"'
                    || c == b','
                    || c == b'}'
                    || c == b']'
                {
                    break;
                }
                j += 1;
            }
            if j > i {
                out.push_str("[REDACTED]");
                i = j;
            }
            continue;
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// `"Authorization":"` (case-insensitive prefix; spaces around the
/// `":"` separator aren't supported because servers don't actually
/// emit them).
fn match_json_authorization(b: &[u8]) -> Option<usize> {
    let mut i = 0;
    if i >= b.len() || b[i] != b'"' {
        return None;
    }
    i += 1;
    let key = b"authorization";
    if b.len() < i + key.len() {
        return None;
    }
    for (k, &kb) in key.iter().enumerate() {
        if !b[i + k].eq_ignore_ascii_case(&kb) {
            return None;
        }
    }
    i += key.len();
    if b.len() < i + 3 {
        return None;
    }
    if &b[i..i + 3] != b"\":\"" {
        return None;
    }
    Some(i + 3)
}

/// Match a bare scheme prefix at the start of `b`, case-insensitively.
/// Returns the consumed length including the trailing space.
fn match_scheme_prefix(b: &[u8]) -> Option<usize> {
    for prefix in [b"Bearer " as &[u8], b"Basic ", b"Ghost "] {
        if b.len() >= prefix.len()
            && b[..prefix.len()]
                .iter()
                .zip(prefix.iter())
                .all(|(a, p)| a.eq_ignore_ascii_case(p))
        {
            return Some(prefix.len());
        }
    }
    None
}

fn utf8_char_len(b: u8) -> usize {
    // `b < 0xc0` covers both ASCII and stray continuation bytes; the
    // latter shouldn't happen at a `&str` boundary but treating them
    // as single-byte advances keeps the redactor from panicking on
    // truncated UTF-8 (e.g. log lines clipped mid-codepoint).
    match b {
        0x00..=0xbf => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_strips_bearer_tokens() {
        let s = r#"{"error":"invalid","Authorization":"Bearer eyJhbGciOi.x.y"}"#;
        let r = redact_credentials(s);
        assert!(!r.contains("eyJhbGciOi"), "{r}");
        assert!(r.contains("[REDACTED]"), "{r}");
    }

    #[test]
    fn redact_strips_basic_auth() {
        let s = "Authorization: Basic dXNlcjpwYXNz\nother stuff";
        let r = redact_credentials(s);
        assert!(!r.contains("dXNlcjpwYXNz"), "{r}");
    }

    #[test]
    fn redact_strips_ghost_token() {
        let s = "Authorization: Ghost eyJhbGciOiJIUzI1NiIs.xx.yy";
        let r = redact_credentials(s);
        assert!(!r.contains("eyJhbGciOiJIUzI1NiIs"), "{r}");
    }

    #[test]
    fn redact_preserves_unrelated_strings() {
        assert_eq!(
            redact_credentials("hello world, no tokens here"),
            "hello world, no tokens here"
        );
    }
}
