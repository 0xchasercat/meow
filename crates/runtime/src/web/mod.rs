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
/// the binary. `meow sync` writes this verbatim into `.meow/strict-web.d.ts` so
/// editors + `meow check` resolve the §8.1 globals with nothing installed (I-9).
/// Curated from the upstream WHATWG/Deno `.d.ts`, scoped to exactly the committed
/// set; the shadow tsconfig pins `lib: ["esnext"]` so no DOM leaks in.
///
/// FEATURE-CORRECT: the `fetch` group (`fetch`/`Headers`/`Request`/`Response`/
/// `FormData`) lives in `strict-web.fetch.d.ts` and is appended ONLY under the
/// default-on `web-fetch` feature — the same feature that wires `deno_fetch`. A
/// `--no-default-features` build omits the extension AND these types together, so
/// the typed surface never promises a global that would throw `ReferenceError`
/// (I-9/I-11). The CLI links this crate with its real features, so the bytes it
/// threads into `meow_config`'s shadow-gen already track the build.
#[cfg(feature = "web-fetch")]
pub const STRICT_WEB_DTS: &str = concat!(
    include_str!("lib/strict-web.base.d.ts"),
    include_str!("lib/strict-web.fetch.d.ts"),
);
/// See the `web-fetch` variant above. No-fetch build: base globals only.
#[cfg(not(feature = "web-fetch"))]
pub const STRICT_WEB_DTS: &str = include_str!("lib/strict-web.base.d.ts");

/// The `fetch` network capability handle, seeded into `OpState` for the
/// `op_meow_fetch_check` gate. It reuses RT-002's [`CapabilityCheck`] seam (and its
/// [`CapRequest::NetConnect`](crate::io::CapRequest::NetConnect) variant) behind an
/// `Arc`. At P1 the default is RT-002's `AllowAll` (seam, not enforcement —
/// SEC-001/P6). `Send + Sync` so the same handle composes with deno_fetch's
/// `Send`-bound machinery; the gate itself runs synchronously on the op thread.
pub type NetCaps = Arc<dyn crate::io::CapabilityCheck + Send + Sync>;

/// Inputs needed to stand up the Web globals.
pub struct WebOptions {
    /// The network gate `fetch` consults before connecting (full host:port, A3).
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

// The `fetch` capability gate op. Registered only with `web-fetch` (it borrows the
// fetch-only `NetCaps` seed); the committed `fetch` wrapper in `bootstrap.js` calls
// it before the request op runs.
#[cfg(feature = "web-fetch")]
deno_core::extension!(meow_web_fetch, ops = [perms::op_meow_fetch_check],);

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

/// The `deno_fetch` extension, the `op_meow_fetch_check` capability gate, and the
/// `OpState` seeds both consume: the concrete `PermissionsContainer` deno_fetch
/// reads (set to `allow_all` — meow's gate is the single authority) and the
/// [`NetCaps`] the gate checks (see [`perms`]).
#[cfg(feature = "web-fetch")]
fn fetch_extensions(caps: NetCaps, user_agent: String) -> Vec<Extension> {
    use deno_permissions::PermissionsContainer;

    // Ensure rustls has a process-default crypto provider before any TLS use.
    // PKG-002's `ureq` pulled rustls' `ring` provider, so with deno_fetch's
    // aws-lc-rs ALSO linked, rustls' default is ambiguous and deno_fetch's
    // get_default() panics — install aws-lc-rs (its expected provider) once.
    ensure_crypto_provider();

    let parser = Arc::new(perms::NoopDescriptorParser);
    let perms_container = PermissionsContainer::allow_all(parser);

    let options = deno_fetch::Options {
        user_agent,
        // Default GAI resolver: meow's gate runs at the `fetch` entry (the bootstrap
        // wrapper → op_meow_fetch_check), not at DNS, so the seam sees the full
        // host:port connect target (A3, I-6). deno_fetch's own permission container
        // is allow_all, so meow's seam is the sole authority.
        // Default `file:` handler errors for every request — `file:` fetch is out
        // of scope at P1, denied without touching the disk (no FS authority, I-6).
        ..Default::default()
    };
    vec![
        deno_net::deno_net::init(None, None),
        deno_fetch::deno_fetch::init(options),
        meow_web_fetch::init(),
        Extension {
            name: "meow_web_fetch_perms",
            op_state_fn: Some(Box::new(move |state| {
                state.put::<PermissionsContainer>(perms_container.clone());
                // The network seam the gate consults (op_meow_fetch_check).
                state.put::<NetCaps>(caps.clone());
            })),
            ..Default::default()
        },
    ]
}

/// Install the process-wide rustls crypto-provider default once (aws-lc-rs, the
/// provider deno_fetch expects). Needed because PKG-002's `ureq` HTTPS client adds
/// the `ring` provider, leaving rustls with two providers and no auto-default. The
/// `Once` makes it idempotent + thread-safe; an Err means a default is already set
/// (fine — any installed default stops the panic). web-fetch-gated.
#[cfg(feature = "web-fetch")]
fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
