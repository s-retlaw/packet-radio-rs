#!/bin/bash
# Generate 300 baud HF AFSK test WAV files using Dire Wolf gen_packets
# and record atest baselines
#
# 300 baud AFSK uses 1600 Hz mark / 1800 Hz space (200 Hz shift)
# This is the HF SSB packet mode used on 30m and other HF bands
set -e
DIR="$(dirname "$0")/300"
mkdir -p "$DIR"

echo "=== Generating 300 baud test WAV files ==="

# Tier 1: Clean signals at multiple sample rates
echo "Tier 1: Clean signals..."
gen_packets -B 300 -r 11025 -o "$DIR/300_clean_11025.wav"
gen_packets -B 300 -r 22050 -o "$DIR/300_clean_22050.wav"
gen_packets -B 300 -r 44100 -o "$DIR/300_clean_44100.wav"
gen_packets -B 300 -r 48000 -o "$DIR/300_clean_48000.wav"

# Tier 2: Noise sweep
echo "Tier 2: Noise sweep..."
gen_packets -B 300 -n 100 -r 11025 -o "$DIR/300_noise100_11025.wav"
gen_packets -B 300 -n 100 -r 44100 -o "$DIR/300_noise100_44100.wav"

# Tier 3: Frequency offset (nominal: mark=1600, space=1800)
echo "Tier 3: Frequency offsets..."
gen_packets -m 1570 -s 1770 -b 300 -r 11025 -o "$DIR/300_offset_m30_11025.wav"
gen_packets -m 1630 -s 1830 -b 300 -r 11025 -o "$DIR/300_offset_p30_11025.wav"
gen_packets -m 1550 -s 1750 -b 300 -r 11025 -o "$DIR/300_offset_m50_11025.wav"
gen_packets -m 1650 -s 1850 -b 300 -r 11025 -o "$DIR/300_offset_p50_11025.wav"
gen_packets -m 1700 -s 1900 -b 300 -r 11025 -o "$DIR/300_offset_p100_11025.wav"

# Tier 4: Clock drift
echo "Tier 4: Variable speed..."
gen_packets -B 300 -v 3,0.2 -r 11025 -o "$DIR/300_varspeed_3pct_11025.wav"
gen_packets -B 300 -v 5,0.5 -r 11025 -o "$DIR/300_varspeed_5pct_11025.wav"

# Tier 5: Amplitude variation
echo "Tier 5: Amplitude..."
gen_packets -B 300 -a 10 -n 50 -r 11025 -o "$DIR/300_weak_11025.wav"
gen_packets -B 300 -a 100 -n 50 -r 11025 -o "$DIR/300_strong_11025.wav"

# Record Dire Wolf baselines
echo ""
echo "=== Recording Dire Wolf 300 Baud Baselines ==="
echo "=== Dire Wolf 300 Baud Baselines ===" > "$DIR/direwolf_baselines.txt"
echo "Generated: $(date)" >> "$DIR/direwolf_baselines.txt"
echo "" >> "$DIR/direwolf_baselines.txt"
for wav in "$DIR"/*.wav; do
    echo "--- $(basename "$wav") ---" | tee -a "$DIR/direwolf_baselines.txt"
    atest -B 300 "$wav" 2>&1 | tail -5 >> "$DIR/direwolf_baselines.txt"
    echo "" >> "$DIR/direwolf_baselines.txt"
done
echo ""
echo "Done. Baselines saved to $DIR/direwolf_baselines.txt"
