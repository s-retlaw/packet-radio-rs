#!/bin/bash
# Generate 9600 baud G3RUH test WAV files using DireWolf's gen_packets.
#
# Usage: ./generate_9600_tests.sh [output_dir]
#
# Requires gen_packets and atest from DireWolf to be installed.
# Files are saved in the specified directory (default: tests/wav/).

set -euo pipefail

OUTDIR="${1:-tests/wav}"
mkdir -p "$OUTDIR"

# Check for DireWolf tools
if ! command -v gen_packets &> /dev/null; then
    echo "Error: gen_packets not found. Install DireWolf first."
    echo "  https://github.com/wb2osz/direwolf"
    exit 1
fi

echo "Generating 9600 baud test WAV files in $OUTDIR/"

# Clean 9600 @ 48000 Hz — 100 frames with increasing noise
echo "  9600_noise100_48k.wav ..."
gen_packets -B 9600 -r 48000 -n 100 -o "$OUTDIR/9600_noise100_48k.wav"

# Clean 9600 @ 44100 Hz — 100 frames with increasing noise
echo "  9600_noise100_44k.wav ..."
gen_packets -B 9600 -r 44100 -n 100 -o "$OUTDIR/9600_noise100_44k.wav"

# Clean 9600 @ 38400 Hz — 100 frames (MCU-optimized rate, 4 sps exact)
echo "  9600_noise100_38k.wav ..."
gen_packets -B 9600 -r 38400 -n 100 -o "$OUTDIR/9600_noise100_38k.wav"

# Single clean frame @ 48000 Hz (for sanity check)
echo "  9600_clean_48k.wav ..."
gen_packets -B 9600 -r 48000 -n 1 -o "$OUTDIR/9600_clean_48k.wav"

echo ""
echo "DireWolf baselines:"
echo ""

# Run atest on each file to get DireWolf baseline
for wav in "$OUTDIR"/9600_*.wav; do
    if [ -f "$wav" ]; then
        name=$(basename "$wav")
        count=$(atest -B 9600 "$wav" 2>&1 | grep -c "^[0-9]" || true)
        echo "  $name: $count frames"
    fi
done

echo ""
echo "Done. Test files are in $OUTDIR/"
