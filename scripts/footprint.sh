#!/usr/bin/env sh
# scripts/footprint.sh — owned by DIST-001
#
# The `footprint` gate's measurement tool (GATES.md → I-10). Builds the release
# binary if absent, reports on-disk + gzip-compressed size, checks the budget.
# Budget is the COMPRESSED download (CANON §26.1: "< 60 MB refers to the compressed
# download; on-disk is larger"). Exit non-zero iff the compressed size is over budget.
#
#   env:  FOOTPRINT_BUDGET_BYTES   default 62914560 (60 * 1024 * 1024)
#   out (one line, stable/greppable):
#         footprint: meow on-disk=<bytes> gzip=<bytes> budget=<bytes> status=<ok|over>
#   exit: 0 ok, 1 over budget, 2 binary missing / build failed
#
# For the V8-less skeleton gzip is a few MB → status=ok. The number becomes the
# real watch point at RT-001, when V8 is actually linked in.
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/meow"
BUDGET="${FOOTPRINT_BUDGET_BYTES:-62914560}"

if [ ! -x "$BIN" ]; then
    cargo build --release -p meow-cli --manifest-path "$ROOT/Cargo.toml" 1>&2 \
        || { echo "footprint: release build failed" >&2; exit 2; }
fi
[ -x "$BIN" ] || { echo "footprint: binary missing at $BIN" >&2; exit 2; }

ondisk="$(wc -c < "$BIN" | tr -d ' ')"
gzip="$(gzip -c "$BIN" | wc -c | tr -d ' ')"

if [ "$gzip" -gt "$BUDGET" ]; then status=over; else status=ok; fi
echo "footprint: meow on-disk=$ondisk gzip=$gzip budget=$BUDGET status=$status"
[ "$status" = ok ] || exit 1
