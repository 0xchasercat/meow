//! strict-web Stateless-Edge globals (RT-004 · CANON §8.1).
//!
//! Assembles the ordered, UNMODIFIED upstream Deno web extensions onto the
//! default runtime so `meow run` sees the §8.1 subset — `fetch`, `Request`/
//! `Response`/`Headers`, `URL`/`URLPattern`, `crypto.subtle`, `TextEncoder`/
//! `TextDecoder`, `AbortController`, `Blob`, `FormData`, `setTimeout` (+ fetch
//! streaming bodies). `console` stays RT-001's `op_print`-backed global (we never
//! double-install it). NO DOM / `window` / `localStorage` (those live only in
//! Deno's runtime bootstrap, which RT-004 does not pull).
//!
//! At this `deno_core 0.403` generation `deno_url` and `deno_console` were merged
//! into `deno_web`, so URL/URLPattern/console infra ship there; the extension
//! `init()` (the macro's full constructor — there is no `init_ops_and_esm` in this
//! pin) registers ops + the lazy-loaded polyfill JS. The actual global wiring is
//! done by [`mod@meow_web`]'s `bootstrap.js` entry point (it `loadExtScript`s each
//! polyfill and binds exactly the committed names).
//!
//! `fetch` and its `Request`/`Response`/`Headers`/`FormData` companions live in
//! the heavy `deno_fetch` crate (hyper/rustls); they are behind the default-on
//! `web-fetch` feature so the footprint gate (I-10) can drop them as a unit.

use std::sync::Arc;

use deno_core::Extension;

#[cfg(feature = "web-fetch")]
pub mod perms;

/// The curated strict-web ambient global declarations (CANON §8.1), embedded in
/// the binary. `meow sync` writes this verbatim into `.meow/types/strict-web.d.ts`
/// so editors + `meow check` resolve the §8.1 globals with nothing installed (I-9).
/// Curated from the upstream WHATWG/Deno `.d.ts`, scoped to exactly the committed
/// set; the shadow tsconfig pins `lib: ["esnext"]` so no DOM leaks in.
pub const STRICT_WEB_DTS: &str = include_str!("lib/strict-web.d.ts");

/// The `fetch` network capability handle. `Send + Sync` because deno_fetch's DNS
/// resolver runs in `tokio::spawn`; it reuses RT-002's [`CapabilityCheck`] seam
/// (and its [`CapRequest::NetConnect`](crate::io::CapRequest::NetConnect) variant)
/// behind an `Arc`. At P1 the default is RT-002's `AllowAll` (seam, not
/// enforcement — SEC-001/P6).
pub type NetCaps = Arc<dyn crate::io::CapabilityCheck + Send + Sync>;

/// Inputs needed to stand up the Web globals.
pub struct WebOptions {
    /// The network gate `fetch` consults before resolving a host (A3).
    pub caps: NetCaps,
    /// `User-Agent` the fetch client sends — a fixed build-time string (no host
    /// read, I-6), e.g. `"meow/<version>"`.
    pub user_agent: String,
}

deno_core::extension!(
    meow_web,
    esm_entry_point = "ext:meow_web/bootstrap.js",
    esm = [dir "src/web/js", "bootstrap.js"],
);

/// Build the ordered Stateless-Edge extension list (CANON §8.1) to append to
/// RT-001's `RuntimeOptions.extensions`. Order: `deno_webidl` → `deno_web` →
/// `deno_crypto` → (`deno_fetch` when `web-fetch`) → `meow_web` (the global-wiring
/// bootstrap, last, so every polyfill is registered before it runs).
pub fn extensions(opts: WebOptions) -> Vec<Extension> {
    let WebOptions { caps, user_agent } = opts;

    let mut exts = vec![
        deno_webidl::deno_webidl::init(),
        deno_web::deno_web::init(
            Arc::new(deno_web::BlobStore::default()),
            None,  // maybe_location: no document base URL (no window) — I-6
            false, // enable_css_parser_features: off (DOMMatrix CSS parsing not in §8.1)
            deno_web::InMemoryBroadcastChannel::default(),
        ),
        deno_crypto::deno_crypto::init(None), // seed=None at P1; RT-006 wires the hermetic seed
    ];

    #[cfg(feature = "web-fetch")]
    exts.extend(fetch_extensions(caps, user_agent));
    #[cfg(not(feature = "web-fetch"))]
    let _ = (caps, user_agent);

    exts.push(meow_web::init());
    exts
}

/// The `deno_fetch` extension plus the `OpState` seed for the concrete
/// `PermissionsContainer` it reads (set to `allow_all` — meow's own capability
/// gate is the DNS resolver, see [`perms`]).
#[cfg(feature = "web-fetch")]
fn fetch_extensions(caps: NetCaps, user_agent: String) -> Vec<Extension> {
    use deno_permissions::PermissionsContainer;

    let parser = Arc::new(perms::NoopDescriptorParser);
    let perms_container = PermissionsContainer::allow_all(parser);

    let options = deno_fetch::Options {
        user_agent,
        // meow's capability seam runs ahead of the connector (A3, I-6).
        resolver: deno_fetch::dns::Resolver::custom(Arc::new(perms::CapResolver::new(caps))),
        // Default `file:` handler errors for every request — `file:` fetch is out
        // of scope at P1, denied without touching the disk (no FS authority, I-6).
        ..Default::default()
    };
    vec![
        deno_net::deno_net::init(None, None),
        deno_fetch::deno_fetch::init(options),
        Extension {
            name: "meow_web_fetch_perms",
            op_state_fn: Some(Box::new(move |state| {
                state.put::<PermissionsContainer>(perms_container.clone());
            })),
            ..Default::default()
        },
    ]
}
