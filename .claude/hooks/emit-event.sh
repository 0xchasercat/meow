#!/usr/bin/env bash
# 0xos · emit-event.sh  (telemetry feed for Mission Control's Fleet view)
# Wired to many lifecycle events: SubagentStart, SubagentStop, PreToolUse,
# PostToolUseFailure, PreCompact, Stop. Reads the hook's JSON on stdin and POSTs it
# verbatim (plus a few envelope fields) to the local Mission Control ingest endpoint.
# FAIL-SILENT and fast: if MC isn't running or curl is missing, this is a no-op.
# It NEVER blocks a tool call or a turn — it only observes. Always exits 0.
set -uo pipefail

PORT="${OXOS_PORT:-7878}"
URL="http://127.0.0.1:${PORT}/ingest"

# Read stdin (the hook payload). If none, nothing to emit.
PAYLOAD="$(cat 2>/dev/null || true)"
[ -n "$PAYLOAD" ] || exit 0

# curl is the only hard dependency; without it we can't emit — no-op.
command -v curl >/dev/null 2>&1 || exit 0

ROOT="${CLAUDE_PROJECT_DIR:-$PWD}"
TS="$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo '')"

# Wrap the raw payload in a thin 0xos envelope so MC can route by project without
# re-deriving cwd. If python3 is present, build clean JSON; else POST the raw payload.
BODY="$PAYLOAD"
if command -v python3 >/dev/null 2>&1; then
  BODY="$(python3 - "$PAYLOAD" "$ROOT" "$TS" <<'PY' 2>/dev/null || printf '%s' "$PAYLOAD"
import json, sys
raw, root, ts = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    hook = json.loads(raw)
except Exception:
    hook = {"_unparsed": raw}
env = {
    "source": "0xos-hook",
    "project_root": root,
    "emitted_at": ts,
    "event": hook.get("hook_event_name") if isinstance(hook, dict) else None,
    "hook": hook,
}
print(json.dumps(env))
PY
)"
fi

# Fire-and-forget. --max-time 2 keeps the lifecycle snappy even when MC is down.
curl --max-time 2 -s -o /dev/null \
     -X POST "$URL" \
     -H 'Content-Type: application/json' \
     --data-binary "$BODY" >/dev/null 2>&1 || true

exit 0
