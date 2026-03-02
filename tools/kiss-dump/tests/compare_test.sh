#!/bin/bash
# compare_test.sh — Live comparison: our TNC vs Direwolf atest
#
# Usage: ./compare_test.sh [wav-file]
# Default: iso/direwolf_review/input_wav/03_100-mic-e-bursts-flat.wav

set -euo pipefail

WAV="${1:-iso/direwolf_review/input_wav/03_100-mic-e-bursts-flat.wav}"
PORT=18731  # high port to avoid conflicts
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

# ── 1. Run Direwolf atest ────────────────────────────────────────────
echo "=== Running Direwolf atest on $WAV ==="
if ! command -v atest &>/dev/null; then
    echo "SKIP: atest not found on PATH (install Direwolf to enable comparison)"
    exit 0
fi

atest "$WAV" 2>&1 \
  | sed 's/\x1b\[[^m]*m//g' \
  | grep '^\[0\]' \
  | sed 's/^\[0\] //' \
  | sed 's/<0x[0-9a-f][0-9a-f]>//gI' \
  | sort > "$TMPDIR/dw.txt"
DW_COUNT=$(wc -l < "$TMPDIR/dw.txt")
echo "Direwolf: $DW_COUNT frames"

if [ "$DW_COUNT" -eq 0 ]; then
    echo "FAIL: Direwolf decoded 0 frames"
    exit 1
fi

# ── 2. Run our TNC + kiss-dump ───────────────────────────────────────
echo "=== Running our TNC (multi mode) on $WAV ==="
cargo run --release -p packet-radio-desktop -- \
  --wav "$WAV" --kiss-port $PORT --multi --no-tui &
TNC_PID=$!
sleep 1  # let TNC start listening

cargo run --release -p kiss-dump -- --count --quiet localhost:$PORT \
  | sort > "$TMPDIR/ours.txt" 2>"$TMPDIR/count.txt"
wait $TNC_PID 2>/dev/null || true
OUR_COUNT=$(wc -l < "$TMPDIR/ours.txt")
echo "Our TNC:  $OUR_COUNT frames"

# ── 3. Compare ──────────────────────────────────────────────────────
COMMON=$(comm -12 "$TMPDIR/dw.txt" "$TMPDIR/ours.txt" | wc -l)
DW_ONLY=$(comm -23 "$TMPDIR/dw.txt" "$TMPDIR/ours.txt" | wc -l)
US_ONLY=$(comm -13 "$TMPDIR/dw.txt" "$TMPDIR/ours.txt" | wc -l)

echo ""
echo "=== Results ==="
echo "Common:    $COMMON"
echo "DW-only:   $DW_ONLY"
echo "Us-only:   $US_ONLY"
echo "Overlap:   $(( COMMON * 100 / DW_COUNT ))%"

# Show differences if any
if [ "$DW_ONLY" -gt 0 ]; then
    echo ""
    echo "--- Frames only in Direwolf ---"
    comm -23 "$TMPDIR/dw.txt" "$TMPDIR/ours.txt" | head -10
fi
if [ "$US_ONLY" -gt 0 ]; then
    echo ""
    echo "--- Frames only in our TNC ---"
    comm -13 "$TMPDIR/dw.txt" "$TMPDIR/ours.txt" | head -10
fi

# Exit with failure if overlap < 95%
if [ $(( COMMON * 100 / DW_COUNT )) -lt 95 ]; then
    echo "FAIL: overlap < 95%"
    exit 1
fi
echo "PASS"
