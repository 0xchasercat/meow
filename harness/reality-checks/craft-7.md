# Craft gate — wave-2 (P1/MVP): RT-004 · RT-006 · CFG-002 · LOAD-003 · RT-005

- **Gate:** craft (CRAFT.md — "is this the right way a principled senior owner would build it?")
- **Scope:** commit range c9dd25e..HEAD — LOAD-003 (full Node resolution), RT-005 (meow:http serve + typegen + native registry), RT-004 (strict-web globals), RT-006 (determinism harness), CFG-002 (root package.json projection).
- **At:** 2026-06-08T01:15:00Z
- **Verdict:** **PASS (green)** — no `block` findings. 3 `major` (surfaced; two need a Main decision / honest-constraint record, one needs a residual named), 4 `minor`, 3 `nit`. Floor taken as green per task; this is a craft-only review.

The honest-boundary discipline (the meow cardinal sin, I-11) is held *consistently and well* across RT-004/005/006 — this is the strongest part of the wave. The blocking-class defect this gate most fears (prose that sells trust the code does not back) is absent. The majors below are deeper-invariant gaps (Node fidelity, single seam, full determinism) that work and match their specs but a principled owner would want surfaced/decided before the gates that depend on them.

---

## Findings

### MAJOR

**M1 — Condition resolution discards package.json key order; fixed-priority iteration diverges from Node.**
- where: `crates/loader/src/package.rs:41` (`ExportsTarget::Conditions(BTreeMap<String, ExportsTarget>)`), `crates/loader/src/resolver.rs:25` (`CONDITIONS`), `:636-653` (`condition_target_resolve`).
- why-it's-the-wrong-decision: Node's exports algorithm selects the **first object key that is in the active condition set** — precedence is the *author's* key order in package.json. This impl stores conditions in a `BTreeMap` (key order is destroyed → alphabetical) and iterates a fixed runtime priority `["meow","import","node","default"]`, returning the highest-priority *active* condition regardless of authoring order. For `{"node":"./n.js","import":"./e.js"}` Node resolves `./n.js`; meow resolves `./e.js` — a *different file*. The chosen data structure makes faithful insertion-order matching structurally impossible, and LOAD-003's contract (CANON §12.2 "implement the full Node resolution algorithm faithfully") is the I-1/I-5 chokepoint the LSP will share. The corpus test (`conditional_exports_choose_import_and_require_only_errors`) happens to use an `import`-first object, so it does not exercise the divergence.
- the-better-decision: this is a real policy fork (Node-faithful insertion-order vs a deliberate meow runtime priority), so it should be **decided and recorded**, not left implicit. If Node fidelity is the commitment: preserve key order (`Vec<(String, ExportsTarget)>` or an order-preserving map) and pick the first key ∈ active set. If a fixed runtime priority is the commitment: keep it but **name the divergence from Node as an honest constraint** (CANON §24 / Operator notes) — the spec's "faithful"/"insertion order" language currently contradicts the code. The spec did list a priority array, so the implementer had cover; the defect is the unrecorded fidelity decision, not negligence.

**M2 — The network capability seam is fragmented across two incompatible handle types.**
- where: `crates/runtime/src/web/mod.rs:57` (`NetCaps = Arc<dyn CapabilityCheck + Send + Sync>`, seeded `:137`) vs `crates/runtime/src/io/mod.rs:37` + `crates/runtime/src/ext/http/ops.rs:196` (`Rc<dyn CapabilityCheck>`).
- why-it's-the-wrong-decision: `fetch` authorizes through the `Arc<…+Send+Sync>` seam; `serve` authorizes through a separate `Rc<dyn CapabilityCheck>` slot (and falls back to its own `AllowAll` when absent — `ops.rs:198`, `ext/http/mod.rs:20-22`). They are two distinct OpState values under two type keys. At P1 both are `AllowAll`, so it is invisible *today* — but the spec's whole point ("reuse the value RT-002 installs — do not store a second copy"; I-6 "one governed network entry") is structurally violated: SEC-001 will have to discover and populate two incompatible slots, and a grant wired for `fetch` will silently not govern `serve` (and vice-versa). This is the "design that fights the next requirement" symptom — the next requirement (capability enforcement) is the named follow-on.
- the-better-decision: choose **one** canonical capability handle type — `Arc<dyn CapabilityCheck + Send + Sync>` (the strictest, required by deno_fetch's spawned-task path; it works single-threaded too) — seed it once, and have io/fetch/serve all read that single type. The `Rc`-vs-`Arc` split is the path of least resistance, not the single-seam the invariant requires.

**M3 — Timezone/locale is an unclosed and unnamed determinism leak in the hermetic harness.**
- where: `crates/runtime/src/hermetic/js/hermetic.js:27-47` (Date wrapper); residual register in `crates/runtime/src/hermetic/mod.rs:12-16` and `state.rs:1-10`.
- why-it's-the-wrong-decision: the wrapper virtualizes the clock *value* (`Date.now`, zero-arg `new Date()`), but `Date.prototype.{toString,getHours,getTimezoneOffset,toLocaleString,…}` and `Intl` still render in the **host-local timezone/locale**, which V8 reads from the OS. So `new Date(x).toString()` (or any locale-rendered date) differs across two machines under the *default* config — contradicting the I-6 promise ("the host environment [and] clock … cannot influence a run unless explicitly granted") and threatening the P1 two-machine `determinism` exit gate when the host env (incl. `TZ`) is scrambled. The honest-boundary register here is otherwise exemplary but names only the *adversarial-code* residuals (pre-shadow refs, eval/FFI), not TZ/locale — so an ambient host influence is left both unclosed and unwritten, which is itself the craft defect (CANON §24: "every limit written down rather than implied"). The committed test surface sidesteps it (`toISOString`/`getTime`), masking the gap.
- the-better-decision: minimum — **name TZ/locale as a hermetic residual** in the module prose + CANON §24 and scope the determinism claim accordingly. Better — pin V8's default timezone to UTC (and consider a fixed locale) under the virtual clock so default `Date`/`Intl` rendering is reproducible, the way the clock value already is. Either is small; leaving it silent is the issue.

### MINOR

**m4 — Dead/redundant `meow:` scheme branch.** `crates/loader/src/resolver.rs:220-224`: `specifier.strip_prefix("meow:")` is unreachable for every real specifier — `Url::parse("meow:<name>")` always succeeds and is already dispatched at `:199-203`. Two code paths for one scheme is a legibility/maintenance smell. Remove the strip_prefix arm (keep the absolute-URL arm).

**m5 — `longest_pattern_match` scores by total key length, not Node's `PATTERN_KEY_COMPARE`.** `crates/loader/src/resolver.rs:852` uses `key.len()`. Node ranks pattern specificity by the **base length before `*`** first, then total length. They diverge for overlapping patterns whose `*` sits at different offsets (e.g. `"./a/*"` vs `"./*/long"` on `"./a/long"`: Node picks `"./a/*"`, this picks `"./*/long"`). The spec explicitly names `PATTERN_KEY_COMPARE`. Better: compare `find('*')` prefix length first, then total length.

**m6 — Capability defaulting for http is double-sited.** `crates/runtime/src/ext/http/mod.rs:19-23` (extension `state` closure seeds `AllowAll` if absent) AND `crates/runtime/src/ext/http/ops.rs:195-198` (op falls back to `AllowAll`). The op-level fallback is dead given the closure seed; more importantly a missing seam should be a wiring invariant surfaced loudly, not silently allowed in two places. Pick one site (the extension seed) and let the op read it directly. (Note `web/perms.rs:76` uses `borrow::<NetCaps>()` — panics-if-absent — for the *same* class of invariant; the two subsystems disagree on the convention.)

**m7 — `native.rs` keeps three hand-synced sites.** `crates/runtime/src/native.rs:11` (`NATIVE_MODULES`), `:37-42` (`native_module_source` match), `:44-49` (`native_module_declaration` match) must all be edited together to add a module — a classic drift source. `include_str!` needs literal paths, but a single `const &[(&str, &str, &str)]` table of `(name, source, decl)` unifies all three behind one source of truth.

### NIT

**n8 — Non-ASCII member paths don't round-trip through the virtual URL.** `crates/loader/src/url.rs:20` `set_path(member)` percent-encodes; `:41-44` `decode` returns the encoded form, so a member like `dist/föö.js` resolves at `locate` time but fails `fs.contains` at `load` time. Rare (non-ASCII npm members) but a latent correctness gap; decode could percent-decode the path, or members could be matched decoded.

**n9 — `Command::Doctor` landing table is stale.** `crates/cli/src/cli.rs:186` maps `doctor → ("doctor","P6")`, but CFG-002 wired the package.json check now; the "single source of truth" table claims unimplemented-in-P6. Harmless (Doctor dispatches to `cmd_doctor`, never the stub), but the table no longer tells the truth for that verb.

**n10 — `root_deps_from_lockfile` flattens the whole closure to roots.** `crates/cli/src/cli.rs:618-632` makes every lockfile entry a first-party-importable bare specifier (transitive-only deps become importable). Honestly flagged in the doc comment as a P1 stand-in until `meow install`; acceptable, noted for when PKG-002 lands.

---

## Top observations

1. **Honest-boundary prose is consistently excellent.** Across RT-004/005/006 the seams that are `AllowAll` at P1 are named as seams-not-enforcement; `fetch` is documented as authorization-not-transport *and* the IP-literal/raw-op bypasses are named (`web/perms.rs:23-26`); typegen is honestly tsc-delegated with no native-emitter claim; types are "curated-not-generated"; the hermetic shadow is "hermetic-by-default / defense-in-depth," never "sandboxed." The cardinal sin is well avoided — the one residual it *missed* is TZ/locale (M3).

2. **RT-005's http op layer and `http.ts` are genuinely well-built.** Sync-bind so `Server.addr` carries the resolved port; capability checked before any socket; handler throw / non-`Response` → 500 with the loop still serving; idempotent shutdown; drain-then-resolve-`finished`; bind failure rejects via `finished` (not a throw from `serve`). The tests (`http.rs:200-373`) are real end-to-end behavior — real hyper client, asserts body/status/headers, capability denial recorded + port-stays-free, fault-isolation — not plumbing.

3. **LOAD-003 encapsulation and robustness are correct.** Unexported subpath → `SubpathNotExported` with no file probe; legacy extension/index probing applies only to no-`exports` packages and relative joins, never to `exports`/`imports` targets; malformed archive/manifest/integrity-mismatch all surface typed `ResolveError`s (`package.rs:55-102`) with no panic on user-reachable paths. The corpus tests assert exact resolved member URLs and the encapsulation/typed-error cases.

4. **CFG-002's byte-stability via struct field order is a thoughtful, correct call.** `RootPackageJson`'s declaration order gives deterministic key order *without* `serde_json/preserve_order` (which would have reordered PKG's lockfile) — `package.rs:11-13,29-47`. Tri-state classify + unparseable-on-disk → `HandEdited` (warn, never abort) + idempotent skip-write are the right shapes; owned/overwrite honesty is stated, not implied.

5. **typegen is honest and disciplined.** Delegates to `tsc --emitDeclarationOnly`, normalizes for byte-determinism (CRLF→LF, BOM strip, fixed header, single trailing newline), scratch under the *system* temp with an RAII `ScratchDir` cleanup on every exit path, errors that name the cause and the fix. Byte-stability is correctly contingent on a fixed tsc version (the documented delegation boundary).
