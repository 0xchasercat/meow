//! Virtual cache URL scheme `meow-cache:<sri>` (deterministic, host-path-free).
//!
//! A cached dependency's module identity is its content hash, encoded as the SRI
//! string (`sha256-<base64>`) in the URL's opaque path. Deterministic across
//! machines (depends only on content), never leaks `$HOME` into module identity
//! (I-6). The authority-less (`scheme:opaque-path`) form is used deliberately: the
//! SRI's standard-base64 alphabet (`+`, `/`, `=`) is preserved verbatim in an
//! opaque path, so `encode`/`decode` round-trip with no percent-encoding. The
//! `meow:` namespace is reserved for native APIs, hence the distinct `meow-cache`.

use deno_core::url::Url;
use meow_pkg::ContentHash;

use crate::resolver::ResolveError;

/// The virtual scheme for content-addressed (cached) modules.
pub const SCHEME: &str = "meow-cache";

/// Encode a content hash as the stable virtual module URL `meow-cache:<sri>`.
///
/// Infallible: the SRI alphabet is URL-path-safe in an opaque path, so the parse
/// of a value this function itself produces never fails (asserted, not hoped — a
/// failure here is a logic bug in this module, not reachable user input).
pub fn encode(hash: &ContentHash) -> Url {
    let text = format!("{SCHEME}:{}", hash.to_sri());
    Url::parse(&text).expect("a meow-cache: URL built from an SRI is always valid")
}

/// Decode a `meow-cache:<sri>` URL back to its content hash.
///
/// Requires the EXACT opaque form: any authority, query (`?`), or fragment (`#`)
/// is rejected with [`ResolveError::InvalidVirtualUrl`]. Those components do not
/// reach the content hash ([`Url::path`] drops query/fragment), so accepting them
/// would let two distinct specifiers (`…#a`, `…#b`) collapse onto one hash and
/// instantiate the same blob twice. A wrong scheme or unparseable SRI body is the
/// same typed error (never panics).
pub fn decode(url: &Url) -> Result<ContentHash, ResolveError> {
    if url.scheme() != SCHEME {
        return Err(ResolveError::InvalidVirtualUrl(url.clone()));
    }
    // The canonical form `encode` emits has no authority/query/fragment; anything
    // decorated is not a content identity we minted, so refuse it.
    if url.has_authority() || url.query().is_some() || url.fragment().is_some() {
        return Err(ResolveError::InvalidVirtualUrl(url.clone()));
    }
    // Opaque path holds the SRI verbatim (no leading `/`, no authority).
    ContentHash::from_sri(url.path()).map_err(|_| ResolveError::InvalidVirtualUrl(url.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_content_hash() {
        // Bytes whose sha256 digest exercises the full base64 alphabet in the SRI.
        let hash = ContentHash::of(b"the quick brown fox jumps over the lazy dog");
        let url = encode(&hash);
        assert_eq!(url.scheme(), SCHEME);
        // `.as_str()` is stable: re-parsing the encoded form yields the same hash.
        let reparsed = Url::parse(url.as_str()).expect("encoded URL re-parses");
        assert_eq!(decode(&reparsed).expect("decodes"), hash);
        assert_eq!(decode(&url).expect("decodes"), hash);
    }

    #[test]
    fn rejects_wrong_scheme() {
        let url = Url::parse("file:///proj/main.ts").unwrap();
        assert!(matches!(
            decode(&url),
            Err(ResolveError::InvalidVirtualUrl(_))
        ));
    }

    #[test]
    fn rejects_malformed_sri_body() {
        let url = Url::parse("meow-cache:not-a-real-sri").unwrap();
        assert!(matches!(
            decode(&url),
            Err(ResolveError::InvalidVirtualUrl(_))
        ));
    }

    #[test]
    fn rejects_decorated_meow_cache_urls() {
        // A real SRI body, then the SAME body decorated three ways. `path()` drops
        // the query/fragment, so without the guard these would all decode to one
        // hash under distinct specifiers — the duplicate-instantiation bug. Each
        // decorated form (and the authority form) must be refused.
        let sri = ContentHash::of(b"decorated url payload").to_sri();
        let plain = format!("{SCHEME}:{sri}");

        // The plain opaque form still round-trips (SRI special chars intact).
        let ok = Url::parse(&plain).expect("plain opaque parses");
        assert!(decode(&ok).is_ok(), "plain opaque form must still decode");

        for decorated in [
            format!("{plain}#x"),                  // fragment
            format!("{plain}?q=1"),                // query
            format!("{SCHEME}://authority/{sri}"), // authority
        ] {
            let url = Url::parse(&decorated).expect("decorated form parses");
            assert!(
                matches!(decode(&url), Err(ResolveError::InvalidVirtualUrl(_))),
                "decorated meow-cache URL must be rejected: {decorated}"
            );
        }
    }
}
