#!/usr/bin/env bash
# 0xos · session-start.sh  (SessionStart hook)
# Orients a fresh session from disk (CONSTITUTION A1: disk is memory).
# Reads harness/state.json + harness/decisions.json and emits additionalContext:
#   phase · active_task + blockers · top drift_flags · resume_pointer ·
#   any OPEN bounded/needs-input decisions (the ones that actually pause) · the spine reminder.
# Robust + non-blocking: if files or python3 are missing, emits a minimal context and exits 0.
set -uo pipefail

# --- locate the harness -------------------------------------------------
ROOT="${CLAUDE_PROJECT_DIR:-$PWD}"
HARNESS="$ROOT/harness"
[ -d "$HARNESS" ] || HARNESS="$ROOT"   # fall back to root if not split out
STATE="$HARNESS/state.json"
DECISIONS="$HARNESS/decisions.json"

SPINE="0xos spine: CONSTITUTION → PLAN → spec → commit → gate → surface. Disk is memory; state.json is pure data. No human gates in dev — only mechanical bounds."

emit() { # emit <additionalContext-string>  → valid SessionStart JSON
  python3 - "$1" <<'PY' 2>/dev/null || printf '%s' "$1"
import json,sys
print(json.dumps({"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":sys.argv[1]}}))
PY
}

# --- degrade gracefully if python3 missing ------------------------------
if ! command -v python3 >/dev/null 2>&1; then
  emit "$SPINE (state.json not parsed: python3 unavailable)"
  exit 0
fi

CTX="$(python3 - "$STATE" "$DECISIONS" "$SPINE" <<'PY' 2>/dev/null
import json, sys
state_p, dec_p, spine = sys.argv[1], sys.argv[2], sys.argv[3]
out = []

def load(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception:
        return None

s = load(state_p)
if s is None:
    print(spine + " (no readable state.json — fresh or first run.)")
    sys.exit(0)

ph = s.get("phase") or {}
out.append(f"phase: {ph.get('id','?')} {ph.get('name','')}".rstrip())
out.append(f"mode: {s.get('mode','?')}")

at = s.get("active_task")
if at:
    line = f"active_task: {at.get('id','?')} [{at.get('status','?')}]"
    bl = at.get("blockers") or []
    if bl: line += " blockers=" + ",".join(bl)
    out.append(line)
else:
    out.append("active_task: none")

dfs = s.get("drift_flags") or []
if dfs:
    order = {"high":0,"medium":1,"low":2}
    top = sorted(dfs, key=lambda d: order.get(d.get("severity","low"),9))[:5]
    out.append("drift_flags (top):")
    for d in top:
        out.append(f"  - [{d.get('severity','?')}] {d.get('id','?')} {d.get('tag','')}: {d.get('summary','')}".rstrip())

rp = s.get("resume_pointer") or {}
read = rp.get("read") or []
if read: out.append("resume.read: " + ", ".join(read))
if rp.get("active"): out.append("resume.active: " + str(rp["active"]))
if rp.get("next_pull_hint"): out.append("resume.next: " + str(rp["next_pull_hint"]))

# OPEN decisions that actually pause an agent: bounded + needs-input
d = load(dec_p)
pausing = []
if d:
    for r in (d.get("open") or []):
        if r.get("kind") in ("bounded","needs-input") and r.get("status","open") != "resolved":
            pausing.append(r)
if pausing:
    out.append("OPEN pausing decisions (resolve to unblock):")
    for r in pausing:
        if r.get("kind") == "bounded":
            detail = f"bound={r.get('bound','?')} needed={r.get('needed','?')} cap={r.get('current_cap','?')}"
        else:
            detail = f"needs={r.get('needs','?')} blocks={','.join(r.get('blocks') or []) or 'none'}"
        out.append(f"  - {r.get('id','?')} [{r.get('kind')}] {r.get('title','')}: {detail}".rstrip())
else:
    out.append("OPEN pausing decisions: none")

print("\n".join(out) + "\n\n" + spine)
PY
)"

[ -n "$CTX" ] || CTX="$SPINE (state.json present but unparsed.)"
emit "$CTX"
exit 0
