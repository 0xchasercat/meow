#!/usr/bin/env bash
# 0xos · guard-irreversible.sh  (PreToolUse hook)
# The ONLY hard stops in dev (A6). Three mechanical bounds — never a human review:
#   1. SCOPE JAIL    — deny file edits/writes & Bash that resolve OUTSIDE the project root.
#   2. PUBLIC-LEAK   — deny `git push` (& friends) to a PUBLIC remote when staged/committed
#                      content matches secret patterns. (Secrets on disk / private git = fine.)
#   3. SPEND CAP     — deny tool calls matching blocked paid-command patterns from bounds.json,
#                      and log a `bounded` decision record.
# Philosophy: FAIL-OPEN. We optimize for velocity — if we can't be sure it's a violation,
# we ALLOW (and leave a note). Only a clear, mechanical breach blocks.
# Emits the official PreToolUse deny JSON when blocking; silent allow (exit 0) otherwise.
set -uo pipefail

ROOT="${CLAUDE_PROJECT_DIR:-$PWD}"
HARNESS="$ROOT/harness"
[ -d "$HARNESS" ] || HARNESS="$ROOT"
BOUNDS="$HARNESS/bounds.json"
DECISIONS="$HARNESS/decisions.json"

INPUT="$(cat 2>/dev/null || true)"

deny() { # deny <reason>  → official PreToolUse deny JSON, then exit 0 (the JSON does the blocking)
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$1" <<'PY'
import json,sys
print(json.dumps({"hookSpecificOutput":{
  "hookEventName":"PreToolUse",
  "permissionDecision":"deny",
  "permissionDecisionReason":sys.argv[1]}}))
PY
  else
    # Minimal hand-rolled JSON (reason kept simple; escape double quotes).
    r="${1//\"/\\\"}"
    printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"%s"}}\n' "$r"
  fi
  exit 0
}

allow() { exit 0; }   # silent allow — no decision = proceed

# No payload or no python3 → we can't reason about it. Fail-OPEN.
[ -n "$INPUT" ] || allow
command -v python3 >/dev/null 2>&1 || allow

# --- delegate the analysis to python3 (robust parsing). It prints one of:
#       ALLOW
#       DENY\t<reason>
#       BOUNDED\t<reason>\t<cmd>      (deny + we log a bounded record below)
RESULT="$(python3 - "$INPUT" "$ROOT" "$BOUNDS" <<'PY' 2>/dev/null || echo "ALLOW"
import json, os, re, sys

raw, root, bounds_p = sys.argv[1], sys.argv[2], sys.argv[3]
root = os.path.realpath(root)

try:
    h = json.loads(raw)
except Exception:
    print("ALLOW"); sys.exit(0)   # unparsable → fail-open

tool = h.get("tool_name") or ""
ti   = h.get("tool_input") or {}
if not isinstance(ti, dict):
    print("ALLOW"); sys.exit(0)

def emit(kind, reason, cmd=""):
    print("\t".join([kind, reason, cmd])); sys.exit(0)

# ---------- helpers ----------
def inside(path):
    """True if absolute-resolved path is within the project root."""
    if not path:
        return True  # nothing to check
    p = path
    if not os.path.isabs(p):
        p = os.path.join(root, p)
    p = os.path.realpath(p)
    return p == root or p.startswith(root + os.sep)

# ---------- 1. SCOPE JAIL (file tools) ----------
# Edit/Write/NotebookEdit/MultiEdit etc. expose the target via file_path / notebook_path.
fp = ti.get("file_path") or ti.get("notebook_path") or ti.get("path")
if fp and tool not in ("Read",):   # reads outside scope are harmless; writes are not
    if not inside(fp):
        emit("DENY", f"scope jail: '{fp}' resolves outside the project root ({root}). "
                     f"Agents may not touch paths outside this project (CONSTITUTION A6).")

# ---------- bounds.json (spend cap + blocked patterns) ----------
blocked = []
spend_cap = None
try:
    with open(bounds_p) as f:
        b = json.load(f)
    blocked = b.get("blocked_cmd_patterns") or []
    spend_cap = b.get("spend_cap_usd")
except Exception:
    pass

cmd = ""
if tool == "Bash":
    cmd = ti.get("command") or ""

# ---------- 3. SPEND CAP / blocked paid commands ----------
if cmd and blocked:
    for pat in blocked:
        try:
            if re.search(pat, cmd):
                emit("BOUNDED",
                     f"spend cap: command matches blocked paid pattern /{pat}/ "
                     f"(cap={spend_cap}). Raise the cap in harness/bounds.json to proceed.",
                     cmd)
        except re.error:
            # bad pattern in config → ignore that pattern (fail-open), keep checking others
            continue

# ---------- scope jail for Bash (best-effort, fail-open) ----------
# Block only CLEAR escapes: cd/rm/cp/mv/etc. targeting an absolute path outside root,
# or writes to the home dir / fs root. Heuristic — must not over-block, so fail-open on doubt.
if cmd:
    # explicit absolute paths in the command
    for m in re.finditer(r'(^|\s)(/[^\s"\';|&]+)', cmd):
        cand = m.group(2)
        # skip obvious read-only/system reads that don't mutate
        if not inside(cand):
            # only treat as a violation if a mutating verb is present anywhere in the cmd
            if re.search(r'\b(rm|rmdir|mv|cp|chmod|chown|tee|truncate|dd|ln|mkdir|touch|>>?|git\s+-C)\b', cmd) \
               or re.search(r'>\s*' + re.escape(cand), cmd):
                emit("DENY",
                     f"scope jail: Bash command targets '{cand}' outside the project root "
                     f"({root}). Mutating paths outside this project is blocked (A6).")
    # destructive sweeps at fs root / home
    if re.search(r'\brm\s+-[rfRions]*[rf][rfRions]*\s+(/|~|\$HOME)(\s|/|$)', cmd):
        emit("DENY", "scope jail: refusing rm -rf against / or $HOME.")

# ---------- 2. PUBLIC-LEAK GUARD (git push to public remote w/ secrets) ----------
# Secrets on disk and in PRIVATE git are fine by design. Only block a PUSH to a PUBLIC
# remote that carries secret-looking content.
if cmd and re.search(r'\bgit\b', cmd) and re.search(r'\bpush\b', cmd):
    SECRET_RE = re.compile(
        r'(AKIA[0-9A-Z]{16})'                                        # AWS access key id
        r'|(aws_secret_access_key\s*=\s*\S+)'
        r'|(ghp_[A-Za-z0-9]{30,})|(github_pat_[A-Za-z0-9_]{30,})'    # GitHub tokens
        r'|(sk-[A-Za-z0-9]{20,})'                                    # OpenAI-style
        r'|(xox[baprs]-[A-Za-z0-9-]{10,})'                           # Slack
        r'|(-----BEGIN (?:RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----)'
        r'|((api[_-]?key|secret|password|token)\s*[:=]\s*["\']?[A-Za-z0-9_\-]{16,})',
        re.IGNORECASE,
    )
    def is_public_remote(url):
        if not url: return False
        u = url.lower()
        # SSH form git@github.com:... or https://github.com/...
        return any(host in u for host in ("github.com","gitlab.com","bitbucket.org")) \
               and "ghe." not in u  # crude: enterprise hosts treated as private
    try:
        import subprocess
        # determine the remote being pushed to (default: origin)
        mr = re.search(r'git\s+push\s+(?:--\S+\s+)*([A-Za-z0-9._\-]+)', cmd)
        remote = mr.group(1) if mr and not mr.group(1).startswith("-") else "origin"
        rurl = subprocess.run(["git","-C",root,"remote","get-url",remote],
                              capture_output=True, text=True, timeout=2).stdout.strip()
        if is_public_remote(rurl):
            # scan staged + last-commit diff for secrets (what would actually go out)
            diff = subprocess.run(["git","-C",root,"diff","--cached"],
                                  capture_output=True, text=True, timeout=3).stdout
            diff += subprocess.run(["git","-C",root,"log","-1","-p","--no-color"],
                                  capture_output=True, text=True, timeout=3).stdout
            if SECRET_RE.search(diff):
                emit("DENY",
                     f"public-leak guard: '{remote}' ({rurl}) is a PUBLIC remote and the "
                     f"content to push matches a secret pattern. Secrets on disk / in private "
                     f"git are fine — pushing them public is not (A6). Use a private remote or "
                     f"strip the secret.")
    except Exception:
        # any uncertainty about remote/diff → FAIL-OPEN (velocity over paranoia)
        pass

print("ALLOW")
PY
)"

# --- act on the verdict --------------------------------------------------
KIND="${RESULT%%$'\t'*}"
case "$KIND" in
  DENY)
    REASON="$(printf '%s' "$RESULT" | cut -f2)"
    deny "$REASON"
    ;;
  BOUNDED)
    REASON="$(printf '%s' "$RESULT" | cut -f2)"
    CMD="$(printf '%s' "$RESULT" | cut -f3)"
    # best-effort: append a `bounded` record to decisions.json (non-fatal if it fails)
    if command -v python3 >/dev/null 2>&1; then
      python3 - "$DECISIONS" "$REASON" "$CMD" <<'PY' 2>/dev/null || true
import json, os, sys, datetime
p, reason, cmd = sys.argv[1], sys.argv[2], sys.argv[3]
now = datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")
try:
    d = json.load(open(p))
except Exception:
    d = {"version":1,"updated_at":now,"open":[],"resolved":[]}
d.setdefault("open",[]); d.setdefault("resolved",[])
# de-dupe: don't pile identical bounded records for the same command
if not any(r.get("kind")=="bounded" and r.get("summary")==cmd and r.get("status")=="open"
           for r in d["open"]):
    n = sum(1 for r in d["open"]+d["resolved"] if str(r.get("id","")).startswith("B-")) + 1
    d["open"].append({
        "id": f"B-{n:03d}", "kind": "bounded",
        "title": "spend cap hit", "opened_at": now, "status": "open",
        "bound": "spend_cap", "summary": cmd, "needed": "raise spend cap",
    })
    d["updated_at"] = now
    json.dump(d, open(p,"w"), indent=2); open(p,"a").write("\n")
PY
    fi
    deny "$REASON"
    ;;
  *)
    allow
    ;;
esac
