#!/bin/bash
# Generate 1200 baud AFSK test WAV files using Dire Wolf gen_packets
# and record atest baselines
#
# 1200 baud AFSK uses 1200 Hz mark / 2200 Hz space (1000 Hz shift)
# This is the standard VHF APRS/packet radio mode
set -e
DIR="$(dirname "$0")/1200"
mkdir -p "$DIR"

echo "=== Generating 1200 baud test WAV files ==="

# Tier 1: Clean signals at multiple sample rates
echo "Tier 1: Clean signals..."
gen_packets -B 1200 -r 11025 -o "$DIR/1200_clean_11025.wav"
gen_packets -B 1200 -r 22050 -o "$DIR/1200_clean_22050.wav"
gen_packets -B 1200 -r 44100 -o "$DIR/1200_clean_44100.wav"
gen_packets -B 1200 -r 48000 -o "$DIR/1200_clean_48000.wav"

# Tier 2: Noise sweep
echo "Tier 2: Noise sweep..."
gen_packets -B 1200 -n 100 -r 11025 -o "$DIR/1200_noise100_11025.wav"
gen_packets -B 1200 -n 100 -r 22050 -o "$DIR/1200_noise100_22050.wav"
gen_packets -B 1200 -n 100 -r 44100 -o "$DIR/1200_noise100_44100.wav"
gen_packets -B 1200 -n 100 -r 48000 -o "$DIR/1200_noise100_48000.wav"

# Tier 3: Frequency offset (nominal: mark=1200, space=2200, 1000 Hz shift)
echo "Tier 3: Frequency offsets..."
gen_packets -m 1150 -s 2150 -b 1200 -r 11025 -o "$DIR/1200_offset_m50_11025.wav"
gen_packets -m 1250 -s 2250 -b 1200 -r 11025 -o "$DIR/1200_offset_p50_11025.wav"
gen_packets -m 1100 -s 2100 -b 1200 -r 11025 -o "$DIR/1200_offset_m100_11025.wav"
gen_packets -m 1300 -s 2300 -b 1200 -r 11025 -o "$DIR/1200_offset_p100_11025.wav"
gen_packets -m 1000 -s 2000 -b 1200 -r 11025 -o "$DIR/1200_offset_m200_11025.wav"
gen_packets -m 1400 -s 2400 -b 1200 -r 11025 -o "$DIR/1200_offset_p200_11025.wav"

# Tier 4: Clock drift
echo "Tier 4: Variable speed..."
gen_packets -B 1200 -v 3,0.2 -r 11025 -o "$DIR/1200_varspeed_3pct_11025.wav"
gen_packets -B 1200 -v 5,0.5 -r 11025 -o "$DIR/1200_varspeed_5pct_11025.wav"

# Tier 5: Amplitude variation
echo "Tier 5: Amplitude..."
gen_packets -B 1200 -a 10 -n 50 -r 11025 -o "$DIR/1200_weak_11025.wav"
gen_packets -B 1200 -a 100 -n 50 -r 11025 -o "$DIR/1200_strong_11025.wav"

# Record Dire Wolf baselines
echo ""
echo "=== Recording Dire Wolf 1200 Baud Baselines ==="
echo "=== Dire Wolf 1200 Baud Baselines ===" > "$DIR/direwolf_baselines.txt"
echo "Generated: $(date)" >> "$DIR/direwolf_baselines.txt"
echo "" >> "$DIR/direwolf_baselines.txt"
for wav in "$DIR"/*.wav; do
    echo "--- $(basename "$wav") ---" | tee -a "$DIR/direwolf_baselines.txt"
    atest -B 1200 "$wav" 2>&1 | tail -5 >> "$DIR/direwolf_baselines.txt"
    echo "" >> "$DIR/direwolf_baselines.txt"
done
echo ""
echo "Done. Baselines saved to $DIR/direwolf_baselines.txt"
