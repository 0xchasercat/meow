// strict-web Stateless-Edge bootstrap (RT-004, CANON 8.1).
//
// Installs EXACTLY the committed Stateless-Edge global subset on `globalThis`
// from the wired, UNMODIFIED upstream Deno extensions (deno_webidl / deno_web /
// deno_crypto / deno_fetch). The extension JS ships as `lazy_loaded_js` IIFE
// polyfills that are NOT auto-evaluated; `Deno.core.loadExtScript` evaluates one
// (recursively pulling its own deps) and returns its exports. We then bind only
// the 8.1 names -- NO DOM / window / localStorage / document / navigator (those
// live in Deno's runtime bootstrap, which RT-004 deliberately does not pull), and
// NOT `console` (RT-001 already installs the determinism-friendly `op_print` one;
// we must not double-install it).
//
// Honest boundary (Operator notes): the wired runtime surface is a SUPERSET of
// this committed set (deno_web also brings streams, EventTarget, structuredClone,
// atob/btoa, performance; deno_crypto brings getRandomValues/randomUUID). We do
// not strip those (that would mean patching upstream, an I-10 violation); we
// scope the committed/typed promise to 8.1 and bind nothing else here.

const core = Deno.core;
const load = core.loadExtScript;

function def(name, value) {
  // WHATWG global classes/functions are non-enumerable, writable, configurable.
  Object.defineProperty(globalThis, name, {
    value,
    writable: true,
    enumerable: false,
    configurable: true,
  });
}

// --- WebIDL brand (cross-module interface branding) ---
const webidl = load("ext:deno_webidl/00_webidl.js");
Object.defineProperty(globalThis, webidl.brand, {
  value: webidl.brand,
  writable: true,
  enumerable: false,
  configurable: true,
});

// --- deno_web: encoding, abort, blob/file, url, urlpattern, timers ---
// DOMException underpins the platform error model (AbortSignal.abort() throws an
// `AbortError` DOMException; Response/Request validation throws DOMExceptions too).
// deno_web's abort/fetch JS references it as a global, so it must be installed
// BEFORE those -- bind it first.
const domException = load("ext:deno_web/01_dom_exception.js");
def("DOMException", domException.DOMException);

const encoding = load("ext:deno_web/08_text_encoding.js");
def("TextEncoder", encoding.TextEncoder);
def("TextDecoder", encoding.TextDecoder);

const abortSignal = load("ext:deno_web/03_abort_signal.js");
def("AbortController", abortSignal.AbortController);
def("AbortSignal", abortSignal.AbortSignal);

// `09_file.js` exports both `Blob` and `File`; we bind ONLY `Blob`. `File` is a
// transitive deno_web global but NOT in the committed 8.1 set (which commits only
// `Blob`/`FormData`), so it must not ship as committed API (I-11 superset honesty).
const file = load("ext:deno_web/09_file.js");
def("Blob", file.Blob);

const url = load("ext:deno_web/00_url.js");
def("URL", url.URL);
def("URLSearchParams", url.URLSearchParams);

const urlPattern = load("ext:deno_web/01_urlpattern.js");
def("URLPattern", urlPattern.URLPattern);

const timers = load("ext:deno_web/02_timers.js");
// `setTimeout`/`clearTimeout` are writable + enumerable per WHATWG.
Object.defineProperty(globalThis, "setTimeout", {
  value: timers.setTimeout,
  writable: true,
  enumerable: true,
  configurable: true,
});
Object.defineProperty(globalThis, "clearTimeout", {
  value: timers.clearTimeout,
  writable: true,
  enumerable: true,
  configurable: true,
});

// --- deno_crypto: crypto.subtle (+ Crypto/CryptoKey/SubtleCrypto) ---
const crypto = load("ext:deno_crypto/00_crypto.js");
Object.defineProperty(globalThis, "crypto", {
  value: crypto.crypto,
  writable: false,
  enumerable: true,
  configurable: true,
});
def("Crypto", crypto.Crypto);
def("CryptoKey", crypto.CryptoKey);
def("SubtleCrypto", crypto.SubtleCrypto);

// --- deno_fetch (default-on `web-fetch` feature): fetch + Request/Response/
// Headers/FormData. Present iff the deno_fetch extension was wired in (its
// `op_fetch` op is registered); when the footprint contingency drops the
// feature these globals are absent and this block is skipped. ---
if (typeof core.ops.op_fetch === "function") {
  const headers = load("ext:deno_fetch/20_headers.js");
  def("Headers", headers.Headers);

  // deno_fetch's `26_fetch.js` destructures `internals.__telemetry` /
  // `__telemetryUtil` (Deno's OpenTelemetry integration, supplied by deno_runtime
  // which RT-004 does not pull). Every use is guarded by `TRACING_ENABLED`, so a
  // disabled stub is sufficient and honest (no tracing claim). `internals` is the
  // shared object the lazy scripts see via the captured `__bootstrap`.
  const internals = globalThis.__bootstrap.internals;
  if (!internals.__telemetry) {
    internals.__telemetry = { TRACING_ENABLED: false, PROPAGATORS: [] };
    internals.__telemetryUtil = {};
  }

  const formData = load("ext:deno_fetch/21_formdata.js");
  def("FormData", formData.FormData);

  const request = load("ext:deno_fetch/23_request.js");
  def("Request", request.Request);

  const response = load("ext:deno_fetch/23_response.js");
  def("Response", response.Response);

  // `fetch` is the one host-touching global, so the committed `fetch` is a thin
  // wrapper that authorizes the connect target through meow's single network seam
  // BEFORE the request op runs (op_meow_fetch_check -> RT-002 CapabilityCheck). For
  // http(s) it passes the full host:port (parity with op_tcp_connect); IP-literal
  // hosts bypass on the Rust side (SEC-001/P6). A denial rejects the returned
  // promise with a TypeError; non-http(s) and unparseable inputs fall through so
  // deno_fetch produces its own standard error (we add no behavior).
  const fetchImpl = load("ext:deno_fetch/26_fetch.js").fetch;
  function authorizeTarget(input) {
    let urlStr;
    if (typeof input === "string") urlStr = input;
    else if (input instanceof URL) urlStr = input.href;
    else if (input && typeof input.url === "string") urlStr = input.url;
    if (urlStr === undefined) return;
    let u;
    try {
      u = new URL(urlStr);
    } catch {
      return; // let deno_fetch reject with its own URL error
    }
    if (u.protocol !== "http:" && u.protocol !== "https:") return;
    const port = u.port || (u.protocol === "https:" ? "443" : "80");
    core.ops.op_meow_fetch_check(u.hostname, Number(port));
  }
  function meowFetch(input, init) {
    try {
      authorizeTarget(input);
    } catch (e) {
      return Promise.reject(e); // WHATWG: network failures reject, never throw sync
    }
    return fetchImpl(input, init);
  }
  Object.defineProperty(globalThis, "fetch", {
    value: meowFetch,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}
