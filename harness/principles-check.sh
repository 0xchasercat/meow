#!/usr/bin/env bash
# 0xos · principles-check.sh — the fast grep/validation gate (local floor).
# Run from the project root (or anywhere — it finds harness/). Exit NONZERO on any violation.
#
#   ./harness/principles-check.sh        # from project root
#   cd harness && ./principles-check.sh  # from harness/
#
# v1 enforces CONSTITUTION A2 (the spine is traceable): every spec under harness/specs/*.md
# (excluding _TEMPLATE.md) MUST carry a non-empty `plan_ref:` AND a non-empty
# `constitution_ref:` in its YAML frontmatter. Untraceable work is scope creep.
set -uo pipefail

# --- locate the harness -------------------------------------------------
SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -d "$SELF_DIR/specs" ]; then
  HARNESS="$SELF_DIR"                  # invoked from inside harness/
elif [ -d "$SELF_DIR/harness/specs" ]; then
  HARNESS="$SELF_DIR/harness"
elif [ -d "${PWD}/harness/specs" ]; then
  HARNESS="${PWD}/harness"
else
  HARNESS="$SELF_DIR"
fi
SPECS="$HARNESS/specs"

FAIL=0
note() { printf '  ✗ %s\n' "$1" >&2; FAIL=1; }
ok()   { printf '  ✓ %s\n' "$1"; }

echo "0xos principles-check · harness=$HARNESS"

# =======================================================================
# A2 · spine traceability — plan_ref + constitution_ref non-empty per spec
# =======================================================================
echo "[A2] spec spine refs (plan_ref + constitution_ref)…"
if [ ! -d "$SPECS" ]; then
  note "no specs/ directory at $SPECS"
else
  shopt -s nullglob 2>/dev/null || true
  found=0
  for f in "$SPECS"/*.md; do
    base="$(basename "$f")"
    [ "$base" = "_TEMPLATE.md" ] && continue
    found=$((found+1))

    # Extract the frontmatter block (between the first two `---` lines) and grep within it.
    fm="$(awk 'NR==1&&/^---[[:space:]]*$/{f=1;next} f&&/^---[[:space:]]*$/{exit} f{print}' "$f")"

    # plan_ref: must exist with a non-empty, non-comment value.
    plan="$(printf '%s\n' "$fm" | grep -E '^[[:space:]]*plan_ref:' | head -1 \
            | sed -E 's/^[[:space:]]*plan_ref:[[:space:]]*//; s/[[:space:]]*#.*$//; s/[[:space:]]*$//')"
    if [ -z "$plan" ]; then
      note "$base — missing/empty plan_ref"
    fi

    # constitution_ref: value after the colon, strip a trailing comment; reject empty / [] .
    cref="$(printf '%s\n' "$fm" | grep -E '^[[:space:]]*constitution_ref:' | head -1 \
            | sed -E 's/^[[:space:]]*constitution_ref:[[:space:]]*//; s/[[:space:]]*#.*$//; s/[[:space:]]*$//')"
    if [ -z "$cref" ] || [ "$cref" = "[]" ]; then
      note "$base — missing/empty constitution_ref"
    fi

    [ -n "$plan" ] && [ -n "$cref" ] && [ "$cref" != "[]" ] && ok "$base — plan_ref=$plan constitution_ref=$cref"
  done
  [ "$found" -eq 0 ] && echo "  (no specs to check yet)"
fi

# =======================================================================
# PROJECT-SPECIFIC CHECKS — add greps below for Part B invariants.
# -----------------------------------------------------------------------
# Each invariant in CONSTITUTION Part B should have a cheap grep here that fails
# loudly when violated. Examples (delete / replace with your own):
#
#   echo "[I-1] money never stored as float…"
#   if grep -REn 'amount.*: *(f32|f64|float|Number)' "$HARNESS/.." --include='*.rs' --include='*.ts' >/dev/null 2>&1; then
#     note "I-1 — found a float money type"
#   else
#     ok "I-1 — no float money types"
#   fi
#
#   echo "[I-2] no TODO/FIXME in shipped specs…"
#   ...
# =======================================================================
# =======================================================================
# PROJECT CHECKS · meow Part B invariants (floor tripwires; no-op until code lands).
# Each backs a CONSTITUTION Part B invariant; the adversarial proof is the matching
# Tier-2 gate (GATES.md). Scoped to first-party source under the repo root — never
# harness/ docs, which legitimately quote the banned phrases.
# =======================================================================
ROOT="$(cd "$HARNESS/.." 2>/dev/null && pwd || echo "$HARNESS")"
EXCL='/(node_modules|\.meow|harness|target|\.git)/'

# P12 (I-2) · first-party is ESM-only; CJS is dependency-only.
echo "[P12·I-2] no first-party CommonJS…"
if find "$ROOT" -name '*.cjs' 2>/dev/null | grep -Ev "$EXCL" | grep -q .; then
  note "P12 — first-party .cjs present; CJS is dependency-only (ADR-3)"
else ok "P12 — no first-party .cjs"; fi

# P13 (I-3) · erasable TypeScript only — no enum / namespace / import = in first-party TS.
echo "[P13·I-3] erasable TypeScript only…"
if grep -REn --include='*.ts' --include='*.mts' \
     -e '^[[:space:]]*(export[[:space:]]+)?(const[[:space:]]+)?enum[[:space:]]' \
     -e '^[[:space:]]*(export[[:space:]]+)?namespace[[:space:]]' \
     -e '[[:alnum:]_$][[:space:]]*=[[:space:]]*require[[:space:]]*\(' \
     "$ROOT" 2>/dev/null | grep -Ev "$EXCL" | grep -q .; then
  note "P13 — non-erasable TS (enum/namespace/import=) in first-party code (I-3)"
else ok "P13 — no non-erasable TS constructs"; fi

# P15 (I-1) · one parser, one resolver — Oxc parser constructed only in the graph/parse crate.
echo "[P15·I-1] single parser entrypoint…"
if grep -REn --include='*.rs' -e 'Parser::new' "$ROOT" 2>/dev/null \
     | grep -Ev "$EXCL" | grep -Ev '/(graph|parse)/' | grep -q .; then
  note "P15 — parser constructed outside the graph/parse crate (I-1)"
else ok "P15 — no stray parser construction"; fi

# P16 (I-6) · no ambient host reads outside the sanctioned host/hermetic seam.
echo "[P16·I-6] no ambient host reads…"
if grep -REn --include='*.rs' \
     -e 'std::env::var' -e 'SystemTime::now' -e 'Instant::now' -e 'rand::' \
     "$ROOT" 2>/dev/null | grep -Ev "$EXCL" | grep -Ev '/(host|hermetic)/' | grep -q .; then
  note "P16 — ambient host read (env/clock/rand) outside the host seam (I-6)"
else ok "P16 — no ambient host reads"; fi

# P17 (I-9) · no @types/meow — runtime types are generated from the implementation.
echo "[P17·I-9] no @types/meow…"
if grep -REn --include='package.json' -e '@types/meow' "$ROOT" 2>/dev/null \
     | grep -Ev "$EXCL" | grep -q .; then
  note "P17 — @types/meow present; meow:* types are generated (I-9)"
else ok "P17 — no @types/meow"; fi

# P17 (I-11) · honest claims — no over-claim phrasing in product-facing surfaces.
echo "[P17·I-11] honest claims in product surfaces…"
if grep -REn --include='*.rs' --include='*.md' \
     -e 'mathematically blocked' -e '100% secure' \
     "$ROOT/README.md" "$ROOT/docs" "$ROOT/crates" "$ROOT/src" 2>/dev/null \
     | grep -Ev "$EXCL" | grep -q .; then
  note "P17 — over-claim phrasing (\"mathematically blocked\"/\"100% secure\") in a product surface (I-11)"
else ok "P17 — no over-claim phrasings"; fi

# I-10 · upstream V8, no embedded toolchains (checked once a Cargo manifest exists).
echo "[I-10] upstream V8 / no embedded toolchains…"
if find "$ROOT" -name 'Cargo.toml' 2>/dev/null | grep -Ev "$EXCL" | grep -q .; then
  if grep -REn --include='Cargo.toml' -e 'rusty_v8[^=]*=[^,}]*git' -e 'deno_core[^=]*=[^,}]*git' \
       "$ROOT" 2>/dev/null | grep -Ev "$EXCL" | grep -q .; then
    note "I-10 — V8/deno_core pulled from a git fork; consume unmodified upstream (ADR-1)"
  else ok "I-10 — no forked V8 source"; fi
else ok "I-10 — (no Cargo manifest yet)"; fi

echo
if [ "$FAIL" -ne 0 ]; then
  echo "principles-check: FAILED" >&2
  exit 1
fi
echo "principles-check: PASSED"
exit 0
