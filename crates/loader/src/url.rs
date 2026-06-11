//! Virtual cache URL scheme `meow-cache://<algo>-<lowerhex>/<member>`.
//!
//! The authority is the cached package's integrity hash in a host-legal form, and
//! the path is the package-relative member path. The URL is deterministic across
//! machines (content-addressed, never host-path-addressed) and stable under relative
//! joins inside one cached package.

use deno_core::url::Url;
use meow_pkg::ContentHash;

use crate::resolver::ResolveError;

/// The virtual scheme for cached package members.
pub const SCHEME: &str = "meow-cache";

/// Encode a cached package member as `meow-cache://<algo>-<lowerhex>/<member>`.
pub fn encode(pkg: &ContentHash, member: &str) -> Url {
    let mut url = Url::parse(&format!("{SCHEME}://{}/", pkg.to_url_host()))
        .expect("a meow-cache URL built from a validated hash is always valid");
    url.set_path(member);
    url
}

/// Decode a cached package member URL back to `(package-integrity, member-path)`.
pub fn decode(url: &Url) -> Result<(ContentHash, String), ResolveError> {
    if url.scheme() != SCHEME
        || !url.has_authority()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ResolveError::InvalidVirtualUrl(url.clone()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ResolveError::InvalidVirtualUrl(url.clone()))?;
    let package = ContentHash::from_url_host(host)
        .map_err(|_| ResolveError::InvalidVirtualUrl(url.clone()))?;
    let member = url
        .path()
        .strip_prefix('/')
        .ok_or_else(|| ResolveError::InvalidVirtualUrl(url.clone()))?;
    Ok((package, member.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_package_member() {
        let hash = ContentHash::of(b"the quick brown fox jumps over the lazy dog");
        let url = encode(&hash, "dist/index.js");
        assert_eq!(url.scheme(), SCHEME);
        assert_eq!(url.host_str(), Some(hash.to_url_host().as_str()));
        assert_eq!(
            decode(&Url::parse(url.as_str()).expect("encoded URL re-parses")).expect("decodes"),
            (hash.clone(), "dist/index.js".to_owned())
        );
        assert_eq!(
            decode(&url).expect("decodes"),
            (hash, "dist/index.js".to_owned())
        );
    }

    #[test]
    fn decodes_package_root() {
        let hash = ContentHash::of(b"package root");
        let url = Url::parse(&format!("{SCHEME}://{}/", hash.to_url_host())).unwrap();
        assert_eq!(decode(&url).expect("decodes root"), (hash, String::new()));
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
    fn rejects_malformed_host() {
        let url = Url::parse("meow-cache://not-a-real-hash/dist/index.js").unwrap();
        assert!(matches!(
            decode(&url),
            Err(ResolveError::InvalidVirtualUrl(_))
        ));
    }

    #[test]
    fn rejects_decorated_meow_cache_urls() {
        let hash = ContentHash::of(b"decorated url payload");
        let plain = format!("{SCHEME}://{}/dist/index.js", hash.to_url_host());
        let ok = Url::parse(&plain).expect("plain parses");
        assert!(decode(&ok).is_ok(), "plain authority form must decode");

        for decorated in [
            format!("{plain}#x"),
            format!("{plain}?q=1"),
            format!("{SCHEME}:{}", hash.to_sri()),
            format!("{SCHEME}://user@{}/dist/index.js", hash.to_url_host()),
        ] {
            let url = Url::parse(&decorated).expect("decorated form parses");
            assert!(
                matches!(decode(&url), Err(ResolveError::InvalidVirtualUrl(_))),
                "decorated meow-cache URL must be rejected: {decorated}"
            );
        }
    }
}
