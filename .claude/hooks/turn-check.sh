#!/usr/bin/env bash
# 0xos · turn-check.sh  (Stop hook)
# Two jobs, both NON-BLOCKING (we never hard-stop the agent — A5/A6: only mechanical
# bounds block, and those live in PreToolUse, not here):
#   1. Emit a terse turn-end self-check reminder.
#   2. Validate harness/state.json against harness/schemas/state.schema.json.
#      Invalid → WARN loudly in the context; do NOT block (A3 says invalid is a bug to
#      fix, not to ignore — but blocking the turn would just strand the agent).
# Uses python3 + jsonschema if importable; else a light structural check; else skips.
set -uo pipefail

ROOT="${CLAUDE_PROJECT_DIR:-$PWD}"
HARNESS="$ROOT/harness"
[ -d "$HARNESS" ] || HARNESS="$ROOT"
STATE="$HARNESS/state.json"
SCHEMA="$HARNESS/schemas/state.schema.json"

CHECK="turn-end self-check: (1) does state.json reflect reality? (2) does this turn's work trace the spine (spec→plan→constitution)? (3) is anything green only on unit tests but not reality-gated? (4) was anything silently dropped? Fix state.json before ending if it drifted — disk is memory."

# additionalContext is the non-blocking surface for Stop; degrade to plain stdout.
emit() {
  python3 - "$1" <<'PY' 2>/dev/null || printf '%s\n' "$1"
import json,sys
print(json.dumps({"hookSpecificOutput":{"hookEventName":"Stop","additionalContext":sys.argv[1]}}))
PY
}

if ! command -v python3 >/dev/null 2>&1; then
  emit "$CHECK  (state.json not validated: python3 unavailable.)"
  exit 0
fi

VALID="$(python3 - "$STATE" "$SCHEMA" <<'PY' 2>/dev/null
import json, sys
state_p, schema_p = sys.argv[1], sys.argv[2]

def load(p):
    with open(p) as f: return json.load(f)

try:
    s = load(state_p)
except FileNotFoundError:
    print("SKIP no state.json"); sys.exit(0)
except Exception as e:
    print(f"INVALID state.json is not valid JSON: {e}"); sys.exit(0)

# Preferred path: real schema validation if jsonschema + schema file present.
try:
    import jsonschema  # type: ignore
    try:
        schema = load(schema_p)
    except Exception:
        schema = None
    if schema is not None:
        try:
            jsonschema.validate(instance=s, schema=schema)
            print("OK schema-validated"); sys.exit(0)
        except jsonschema.ValidationError as e:
            path = "/".join(str(x) for x in e.absolute_path) or "(root)"
            print(f"INVALID at {path}: {e.message}"); sys.exit(0)
except ImportError:
    pass

# Fallback: light structural check (no jsonschema installed).
required = ["version","updated_at","mode","phase","subsystems","gates","decisions","resume_pointer"]
missing = [k for k in required if k not in s]
if missing:
    print("INVALID (structural) missing keys: " + ", ".join(missing)); sys.exit(0)
if s.get("version") != 3:
    print(f"INVALID (structural) version must be 3, got {s.get('version')!r}"); sys.exit(0)
if s.get("mode") not in ("incubating","graduated"):
    print(f"INVALID (structural) mode must be incubating|graduated, got {s.get('mode')!r}"); sys.exit(0)
print("OK structural (jsonschema not installed — light check only)")
PY
)"

case "$VALID" in
  OK*|SKIP*) emit "$CHECK  [state.json: ${VALID}]" ;;
  INVALID*)  emit "⚠️  STATE.JSON INVALID — $VALID. Per A3 this is a bug: fix state.json so it validates against schemas/state.schema.json before continuing.

$CHECK" ;;
  *)         emit "$CHECK  [state.json: not checked]" ;;
esac
exit 0
