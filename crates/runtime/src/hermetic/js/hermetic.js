// RT-006 (A4) -- hermetic global shadows. MUST be 7-bit ASCII (deno_core
// requires extension code to be ASCII).
//
// Rebinds the JS-visible nondeterminism sources to the hermetic ops so default
// determinism is structural. This is the ONLY place a program reads them through
// the ambient globals. Honest boundary: this closes the AMBIENT path only -- it
// is hermetic-by-default + defense-in-depth, NOT a sandbox against adversarial
// code that stashes a pre-shadow reference / reaches an unshadowed intrinsic /
// uses eval/Function/FFI (CANON 24.3).
//
// Evaluated AFTER RT-001's bootstrap and RT-004's web globals (extension order),
// so `crypto`/`performance` already exist when present and get rebound here. The
// ops are called LAZILY (at first use, during the run) -- never at module-eval
// time -- so the hermetic state may be installed after construction.

import { core } from "ext:core/mod.js";

const {
  op_hermetic_now_ms,
  op_hermetic_mono_ms,
  op_hermetic_random_fill,
} = core.ops;

// --- Date: virtualize ONLY the current-time read --------------------------
// `new Date(args)` / `Date.parse` / `Date.UTC` keep native semantics; only the
// zero-arg constructor and `Date.now` read the (virtual) clock.
const OriginalDate = globalThis.Date;
function Date(...args) {
  if (!new.target) {
    // `Date()` called as a function -> the current-time string.
    return new OriginalDate(op_hermetic_now_ms()).toString();
  }
  return args.length === 0
    ? new OriginalDate(op_hermetic_now_ms())
    : new OriginalDate(...args);
}
Date.now = () => op_hermetic_now_ms();
Date.parse = OriginalDate.parse;
Date.UTC = OriginalDate.UTC;
Date.prototype = OriginalDate.prototype;
Object.defineProperty(Date, Symbol.hasInstance, {
  value: (x) => x instanceof OriginalDate,
});
globalThis.Date = Date;

// --- Math.random: seed a JS PRNG ONCE from one op draw, advance in JS ------
// `Math.random` is hot; we do not cross the FFI boundary per call. Under the
// default Seeded source the one bootstrap draw is identical every run, so the
// whole sequence is reproducible. sfc32 (128-bit state) seeded from 16 bytes.
let _rand = null;
function _seedMathRandom() {
  const seed = new Uint8Array(16);
  op_hermetic_random_fill(seed);
  const dv = new DataView(seed.buffer);
  let a = dv.getUint32(0, true);
  let b = dv.getUint32(4, true);
  let c = dv.getUint32(8, true);
  let d = dv.getUint32(12, true);
  _rand = function () {
    a |= 0;
    b |= 0;
    c |= 0;
    d |= 0;
    const t = ((a + b) | 0) + d | 0;
    d = (d + 1) | 0;
    a = b ^ (b >>> 9);
    b = (c + (c << 3)) | 0;
    c = (c << 21) | (c >>> 11);
    c = (c + t) | 0;
    return (t >>> 0) / 4294967296;
  };
}
Math.random = () => {
  if (_rand === null) _seedMathRandom();
  return _rand();
};

// --- crypto.getRandomValues / randomUUID: route through the op -------------
// RT-004 owns the `crypto` global and is not a dependency, so guard on presence.
const cryptoObj = globalThis.crypto;
if (cryptoObj && typeof cryptoObj.getRandomValues === "function") {
  cryptoObj.getRandomValues = (view) => {
    // Fill the underlying bytes regardless of the typed-array element type, so a
    // Uint32Array (etc.) is fully randomized just like getRandomValues requires.
    const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
    op_hermetic_random_fill(bytes);
    return view;
  };
  if (typeof cryptoObj.randomUUID === "function") {
    cryptoObj.randomUUID = () => {
      const b = new Uint8Array(16);
      op_hermetic_random_fill(b);
      b[6] = (b[6] & 0x0f) | 0x40; // version 4
      b[8] = (b[8] & 0x3f) | 0x80; // variant 1
      const h = [];
      for (let i = 0; i < 256; i++) h.push((i + 0x100).toString(16).slice(1));
      return (
        h[b[0]] + h[b[1]] + h[b[2]] + h[b[3]] + "-" +
        h[b[4]] + h[b[5]] + "-" +
        h[b[6]] + h[b[7]] + "-" +
        h[b[8]] + h[b[9]] + "-" +
        h[b[10]] + h[b[11]] + h[b[12]] + h[b[13]] + h[b[14]] + h[b[15]]
      );
    };
  }
}

// --- performance.now: route through the monotonic op ----------------------
const perf = globalThis.performance;
if (perf && typeof perf.now === "function") {
  perf.now = () => op_hermetic_mono_ms();
}
