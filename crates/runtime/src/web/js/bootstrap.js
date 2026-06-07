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

const file = load("ext:deno_web/09_file.js");
def("Blob", file.Blob);
def("File", file.File);

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

  const fetch = load("ext:deno_fetch/26_fetch.js");
  Object.defineProperty(globalThis, "fetch", {
    value: fetch.fetch,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}
