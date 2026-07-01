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
// CONDITIONAL APPLICATION (perf): shadows are applied at RUNTIME via
// `globalThis.__meowApplyHermeticShadows`, NOT at module-eval time. This is
// critical for V8 snapshots: if shadows were applied during snapshot creation
// (module-eval), the rebinding would be baked into the V8 heap and could never
// be undone at runtime -- even under `--trust`, `Math.random` would remain the
// JS sfc32 polyfill and `Date.now` would remain an FFI op call. By deferring the
// call to runtime init, the snapshot keeps V8's native intrinsics intact, and
// shadows are applied only when the config actually requires them (virtual clock
// / seeded RNG). Under `--trust` the conditionals short-circuit and V8's
// highly-optimized native `Math.random` / `Date.now` / `performance.now` run
// directly -- no FFI boundary crossing in hot loops.
//
// The host (Runtime::apply_hermetic_shadows / the CLI) calls
// `globalThis.__meowApplyHermeticShadows()` once after the isolate is created
// (from snapshot or fresh). The ops are destructured at module-eval time so the
// closure captures valid op references that survive across snapshot restore.

import { core } from "ext:core/mod.js";

const {
  op_hermetic_now_ms,
  op_hermetic_mono_ms,
  op_hermetic_random_fill,
  op_hermetic_status,
} = core.ops;

// --- sfc32 PRNG state (lazily seeded on first Math.random call) ------------
// Seeded ONCE from one op draw; successive calls advance the JS-side stream.
// Under the default Seeded source the bootstrap draw is identical every run, so
// the whole sequence is reproducible. sfc32 (128-bit state) seeded from 16 bytes.
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

// --- Runtime shadow application ----------------------------------------------
// Called ONCE by the host after isolate creation. When a real source is granted,
// the corresponding shadows are skipped so V8's native intrinsics stay in place.
globalThis.__meowApplyHermeticShadows = function () {
  const [isVirtualClock, isSeededRng] = op_hermetic_status();

  // --- Date + performance: shadow ONLY under the virtual clock ------------
  if (isVirtualClock) {
    // `new Date(args)` / `Date.parse` / `Date.UTC` keep native semantics; only
    // the zero-arg constructor and `Date.now` read the (virtual) clock.
    const OriginalDate = globalThis.Date;
    function Date(...args) {
      if (!new.target) {
        return new OriginalDate(op_hermetic_now_ms()).toString();
      }
      const construct_args = args.length === 0 ? [op_hermetic_now_ms()] : args;
      return Reflect.construct(OriginalDate, construct_args, new.target);
    }
    Date.now = () => op_hermetic_now_ms();
    Date.parse = OriginalDate.parse;
    Date.UTC = OriginalDate.UTC;
    Date.prototype = OriginalDate.prototype;
    Object.defineProperty(Date, Symbol.hasInstance, {
      value: (x) => x instanceof OriginalDate,
    });
    globalThis.Date = Date;

    const perf = globalThis.performance;
    if (perf && typeof perf.now === "function") {
      perf.now = () => op_hermetic_mono_ms();
    }
  }

  // --- Math.random + crypto: shadow ONLY under the seeded RNG -------------
  if (isSeededRng) {
    Math.random = () => {
      if (_rand === null) _seedMathRandom();
      return _rand();
    };

    const cryptoObj = globalThis.crypto;
    if (cryptoObj && typeof cryptoObj.getRandomValues === "function") {
      cryptoObj.getRandomValues = (view) => {
        const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
        op_hermetic_random_fill(bytes);
        return view;
      };
      if (typeof cryptoObj.randomUUID === "function") {
        cryptoObj.randomUUID = () => {
          const b = new Uint8Array(16);
          op_hermetic_random_fill(b);
          b[6] = (b[6] & 0x0f) | 0x40;
          b[8] = (b[8] & 0x3f) | 0x80;
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
  }
};
