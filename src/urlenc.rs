//! Percent-encoding, percent-decoding, and query-string splitting.
//!
//! Deliberately dependency-free and deliberately *here* rather than pulled from
//! `url`: the whole need is three small functions, and one of them
//! ([`decode`]) sits directly on top of the single nastiest trap in Schwab's
//! OAuth flow.
//!
//! # The trap
//!
//! Schwab's authorization redirect carries the code in the query string, and
//! the code Schwab issues **contains characters that must be percent-encoded**
//! — an `@` in the trailing segment being the one everyone hits. A caller who
//! slices the raw redirect URL and posts the substring verbatim sends `%40`
//! where Schwab expects `@`, and the token endpoint answers with an opaque
//! `unsupported_token_type` / 400 that reads like a malformed request rather
//! than a mis-decoded one. So the code must be percent-*decoded* exactly once
//! on the way out of the URL, and then form-encoded exactly once on the way
//! into the token request body. Doing neither works by accident for codes that
//! happen to contain no special characters, which is why this is a bug people
//! ship.
//!
//! See `docs/api.md` ("The authorization code must be URL-decoded").

/// Percent-encode a string for use as a query-string or form value.
///
/// Unreserved characters (RFC 3986 §2.3) pass through; everything else becomes
/// `%XX`. Space becomes `%20`, not `+` — `+` is only valid in
/// `application/x-www-form-urlencoded` and Schwab's token endpoint reads the
/// body with a parser that accepts `%20` in both places, so one rule covers
/// both uses.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-decode a string, treating `+` as a space.
///
/// Total: a stray `%` or a truncated escape is passed through literally rather
/// than dropped or panicked on. A half-decoded credential is worth more to the
/// user than an error, because they can see what went wrong in the output.
pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match hex_pair(bytes[i + 1], bytes[i + 2]) {
                Some(byte) => {
                    out.push(byte);
                    i += 3;
                }
                None => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(hi: u8, lo: u8) -> Option<u8> {
    Some(hex_digit(hi)? << 4 | hex_digit(lo)?)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Pull one **decoded** parameter out of a query string or a full URL.
///
/// Accepts either `code=abc&state=xyz` or
/// `https://127.0.0.1/?code=abc&state=xyz`, because the user pastes whichever
/// their browser gave them and the difference should not be their problem.
pub fn query_param(raw: &str, name: &str) -> Option<String> {
    let qs = raw.split_once('?').map(|(_, q)| q).unwrap_or(raw);
    // A fragment is never part of the query.
    let qs = qs.split('#').next().unwrap_or(qs);
    qs.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| decode(v))
    })
}

/// Build a query string from already-*unencoded* pairs.
pub fn query_string(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreserved_characters_pass_through() {
        assert_eq!(encode("abcXYZ123-_.~"), "abcXYZ123-_.~");
    }

    #[test]
    fn reserved_characters_are_escaped() {
        assert_eq!(encode("https://127.0.0.1"), "https%3A%2F%2F127.0.0.1");
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("x@y"), "x%40y");
    }

    /// The headline case. A Schwab authorization code ends in an `@` segment,
    /// which arrives from the browser as `%40`; posting it still-encoded is
    /// the classic "my token request 400s and I don't know why".
    #[test]
    fn an_encoded_at_sign_round_trips_back_to_an_at_sign() {
        let from_browser = "C0.b2F1dGgyLmJkYy5zY2h3YWIuY29t.exampleCode%40";
        let decoded = decode(from_browser);
        assert!(decoded.ends_with('@'), "{decoded}");
        assert!(!decoded.contains("%40"), "decoded twice or not at all");
        // And the round trip is exact: re-encoding yields what the browser
        // showed, so neither direction is lossy.
        assert_eq!(encode(&decoded), from_browser);
    }

    #[test]
    fn decode_is_total_on_malformed_escapes() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("50%z9"), "50%z9");
        assert_eq!(decode("a%2"), "a%2");
        assert_eq!(decode("a+b"), "a b");
        assert_eq!(decode(""), "");
    }

    #[test]
    fn a_parameter_is_read_from_a_bare_query_string_or_a_full_url() {
        assert_eq!(
            query_param("code=abc%40&session=1", "code").as_deref(),
            Some("abc@")
        );
        assert_eq!(
            query_param("https://127.0.0.1:8182/?code=abc%40&session=1", "code").as_deref(),
            Some("abc@")
        );
    }

    #[test]
    fn a_missing_parameter_is_none_rather_than_empty() {
        assert_eq!(query_param("state=xyz", "code"), None);
        assert_eq!(query_param("", "code"), None);
        // A prefix match must not count: `codeword` is not `code`.
        assert_eq!(query_param("codeword=abc", "code"), None);
    }

    #[test]
    fn a_fragment_is_not_part_of_the_query() {
        assert_eq!(query_param("https://x/?code=abc#frag=nope", "frag"), None);
        assert_eq!(
            query_param("https://x/?code=abc#frag", "code").as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn query_strings_are_built_with_encoded_values() {
        assert_eq!(
            query_string(&[("redirect_uri", "https://127.0.0.1"), ("client_id", "K")]),
            "redirect_uri=https%3A%2F%2F127.0.0.1&client_id=K"
        );
    }
}
