# Optimization Log — 1200 Baud AFSK (Bell 202)

This document captures every optimization approach tried during development of
the packet-radio-rs 1200-baud AFSK demodulator, organized by technique. It
records what worked, what didn't, and why — so that future contributors don't
repeat dead-end experiments.

All results reference the **WA8LMF TNC Test CD** benchmark suite:
- **Track 1** (`01_Track_1.wav`): 1002 packets, mixed signal conditions
- **Track 2** (`02_100-mic-e-bursts-de-emphasized.wav`): 974 packets, de-emphasized Mic-E bursts — hardest track
- **Track 3** (`03_Track_3.wav`): 100 packets, clean signals
- **Track 4** (`04_Track_4.wav`): 98 packets, weak/noisy signals

Reference decoder: **Dire Wolf** (DW) scores 2174 total.

---

## Current Best Results (2026-02-24)

| Track | DireWolf | Fast | Quality | DM+PLL | Multi (38×) | CorrSlicer×3ph | Combined |
|-------|----------|------|---------|--------|-------------|----------------|----------|
| 01    | 1002     | 581  | 582     | 744    | 998         | 965            | 1002     |
| 02    | 974      | 563  | 564     | 386    | 935         | 929            | 946      |
| 03    | 100      | 90   | 90      | 85     | 100         | 100            | 100      |
| 04    | 98       | 60   | 60      | 63     | 104         | 104            | 105      |
| **Total** | **2174** | **1294** | **1296** | **1278** | **2137** | **2098** | **2153** |
| **%DW** | — | 59.5% | 59.6% | 58.8% | 98.3% | 96.5% | **99.0%** |

### Frame Overlap Analysis (Track 2, Multi vs CorrSlicer)

Frame-level diff using `--attribution` and `--diff` tools:

| Metric | Count |
|--------|-------|
| Shared (both decode) | 921 |
| Multi-only | 14 |
| CorrSlicer-only | 9 |

Different algorithms genuinely find different frames — the 23 exclusive frames
justify running both architectures in Combined mode.

### Frame-Level Diff vs Dire Wolf (Track 2)

| | DW Unique | Us Unique | Overlap | DW-only | Us-only |
|---|-----------|-----------|---------|---------|---------|
| T1 | 837 | 829 | 784 | 53 | 45 |
| T2 | 814 | 777 | 733 | 81 | 44 |
| T3 | 1 | 1 | 1 | 0 | 0 |
| T4 | 98 | 103 | 96 | 2 | 7 |

DW-only frames on T2: 65% moderate signal (level 60-79), 22% strong (80+), 12%
weak (40-59). 57% flat mark/space ratio. We mostly miss moderate-strength
flat-response signals, not weak ones — suggesting a frequency/timing alignment
issue rather than sensitivity.

---

## Demodulator Architectures

### 1. Goertzel + Bresenham (Primary)

**How it works:** Compute mark and space tone energy using Goertzel filters over
each symbol period. Compare energies for bit decisions. Bresenham algorithm
provides fixed symbol timing.

- Q14 coefficients: `2·cos(2π·f/Fs)` → mark=25328, space=10126 at 11025 Hz
- Energy: `|X|² = s1² + s2² - coeff·s1·s2`
- State reset at each symbol boundary
- Bresenham timing: integer counter, no floating point

**Why it's fast:** One multiply-accumulate per sample per tone, no transcendentals.
Integer-only. Ideal for MCUs.

**Single-decoder results:** T1=581, T2=563, T3=90, T4=60 → 1294 (59.5% DW)

**History:** Goertzel was adopted after delay-multiply failed on the initial
loopback test. The DM discriminator's inherent asymmetry (3:1 mark/space ratio
at d=4) caused 28% BER even on clean signals, while Goertzel's energy comparison
is inherently balanced.

### 2. Delay-Multiply Discriminator

**Pipeline:** `Audio → BPF → Delay(d) → Multiply → LPF → Accumulate → Timing → NRZI → HDLC`

The delay-multiply discriminator multiplies the signal by a delayed copy of
itself. The sign of the product indicates which tone is present (higher or lower
than the center frequency).

**Key findings:**
- **BPF+LPF are essential**: Without them, d=8 produces only 39 frames. With
  BPF+LPF: 386 frames on T2. Filters provide frequency selectivity that the
  raw discriminator lacks.
- **Optimal delay:** d=8 at 11025 Hz (~87% of symbol period). Mark produces
  positive output, enabling direct threshold at zero.
- **d=5 variant:** Mark produces negative output — used for polarity diversity
  in multi-decoder.
- **Short delays (d=2-4) fail on real signals:** Only 18 packets. Works for
  clean synthetic signals but not noisy RF.

**Single-decoder results (DM+PLL):** T1=744, T2=386, T3=85, T4=63 → 1278 (58.8% DW)

**DM+PLL tuning sweep results:**
- T1 optimal: err_shift=8, smooth=0, beta=0, alpha=600 → 972 hard
- T2 optimal: err_shift=6, smooth=0, beta=0, alpha=800 → 417 hard
- Smoothing always hurts; beta always hurts; err_shift=8 is good default
- Individual alpha tuning (600/800 vs 936/400) gained <1 frame in multi-decoder
  ensemble — Goertzel array already captures most DM-decodable frames

### 3. Correlation (Mixer) — Dire Wolf Style

**Pipeline:** `Audio → BPF → NCO×input → 4-channel LPF → I²+Q² → Bresenham`

Generates mark and space reference tones via NCO (numerically controlled
oscillator), multiplies against input, low-pass filters to extract energy in
each tone's band. This is mathematically equivalent to a matched filter.

**LPF cutoff sweep (T2 results, single decoder):**

| Cutoff (Hz) | T2 Frames |
|-------------|-----------|
| 400 | 623 |
| **500** | **639** |
| 550 | 635 |
| 600 | 595 |
| 650 | 582 |
| 700 | 571 |
| 800 | 558 |
| 900 | 541 |

500 Hz optimal for T2 (priority target). 550 Hz is global optimum across all
tracks but T2 dominates our optimization goals.

**Single-decoder results:** T1=698, T2=639, T3=95, T4=70 → 1502 (69.1% DW)

**Multi-phase results:**
- 2-phase: T1=935, T2=902, T3=100, T4=96 → 2033 (93.5% DW)
- 3-phase: T1=943, T2=906, T3=100, T4=97 → 2046 (94.1% DW)

### 4. CorrSlicerDecoder (Multi-Slicer Correlation with Frequency Diversity)

**Architecture:** `Shared BPF → M freq channels × N gain slicers`

Each FreqChannel maintains its own NCO phases, 4-channel LPF, Bresenham timing,
and gain slicers. This provides both frequency offset diversity and amplitude
diversity in a single integrated decoder.

- std: 3 freq offsets (0, −50, +50 Hz) × 8 gain slicers = 24 channels
- no_std: 1 freq offset (0 Hz) × 4 gain slicers = 4 channels
- Gains (Q8): [64, 107, 181, 256, 511, 868, 1440, 4057]
- Phase scoring selects best Bresenham phase per frequency channel

**Results:**
- Slicer 3f×8g: T1=748, T2=693, T3=95, T4=75 → 1611 (74.1% DW)
- Slicer 3f×8g×3phase: T1=965, T2=929, T3=100, T4=104 → 2098 (96.5% DW)

CPU cost: ~3× single correlation decoder (3 NCO+LPF chains).

---

## Diversity Techniques (What Worked)

### Multi-Decoder (38×) — MAJOR WIN

The single most impactful technique. Runs 38 parallel decoders with diversity
across 5 dimensions:

1. **BPF bandwidth:** standard (1600 Hz), narrow (1200 Hz), wide (2000 Hz)
2. **Timing phase:** 4 Bresenham offsets (0, 1, 2, 3 samples)
3. **Frequency offset:** −100, −50, 0, +50, +100 Hz shifted Goertzel coefficients
4. **Gain:** 6 space-energy gain multipliers handle de-emphasis variation
5. **AGC:** on/off (leaky-max peak tracker)

Composition: 32 Goertzel + 6 DM (2 PLL + 3 Bresenham d=8 + 1 Bresenham d=5).
FNV-1a hash deduplication with time-windowed expiry merges results.

no_std: 23 decoders (17 Goertzel + 6 DM) for ESP32 memory budget.

**Result:** T2=935 (96.0% DW). The gap from single-decoder (563) to multi (935)
is +66% — diversity is by far the most effective technique.

### Timing Diversity (3 Bresenham Phases) — BIG WIN

Running the same demodulator at 3 different Bresenham phase offsets (0, ⅓, ⅔
of symbol period) dramatically improves correlation demodulator results:

- Single phase: T2=639
- 3 phases: T2=906 (+42%)

Short Mic-E bursts have random arrival phase. A fixed Bresenham clock may sample
at a poor point for half the burst. Three phases guarantee at least one clock
is within ±⅙ symbol of optimal.

### Frequency Offset Diversity — MODERATE WIN

Shifting Goertzel coefficients by ±50/100 Hz compensates for transmitter crystal
offset and Doppler. The −50 Hz offset decoder is the single most valuable
individual decoder in attribution analysis (captures 63% of T2 frames alone).

Attribution coverage curve (T2, greedy set-cover):

| Step | Decoder | Cumulative | % of 929 |
|------|---------|------------|----------|
| 1 | G:freq-50/t2 | 491 | 63.1% |
| 2 | G:narrow/t0 | 721 | 92.7% |
| 3 | G:narrow/t1 | 757 | 97.3% |

### Gain/Slicer Diversity — MODERATE WIN

De-emphasized signals have unequal mark/space energy. Eight gain levels on the
space energy channel compensate for varying de-emphasis curves across
transmitters. Each gain level effectively shifts the decision threshold to favor
different mark/space ratios.

### Combined Multi+CorrSlicer — SMALL WIN (+16 frames)

Running MultiDecoder (Goertzel-based) and CorrSlicerDecoder (NCO correlation)
on the same audio and merging with caller-level FNV-1a dedup. The two
architectures use fundamentally different math to detect tones, so they fail on
different frames.

- T1: 1002 (100% DW parity)
- T2: 946 (97.1% DW)
- T4: 105 (exceeds DW's 98)
- Total: 2153 (99.0% DW)

9 CorrSlicer-exclusive frames on T2 (all valid: 6 clean CRC, 3 marginal SNR).

### MiniDecoder ("Smart 3") — ESP32 WIN

Attribution analysis identified the 3 most valuable decoders via greedy
set-cover: `G:freq-50/t2`, `G:narrow/t0`, `G:narrow/t1`. These 3 capture 97.3%
of multi-decoder's output at 8% of the compute cost (3/38 decoders).

Memory: 3 × 8.5KB = 25.5KB (feasible on ESP32 with 320KB SRAM).

---

## Soft Decode Enhancements (What Worked)

### SoftHdlcDecoder Recovery Chain

When hard HDLC decode fails CRC, the soft decoder uses log-likelihood ratios
(LLR) to identify the least confident bits and tries systematic corrections:

1. **CRC syndrome correction:** O(n) single-bit fix using reflected polynomial
   0x8408 and residue 0x0F47. No trial CRC — computes error position directly
   from syndrome. Runs first because it catches errors in high-confidence bits
   that confidence-based methods would miss.
2. **Single bit flip:** Try flipping each of top-12 lowest-confidence bits.
3. **Pair flip:** Try all C(12,2)=66 pairs of low-confidence bits.
4. **NRZI pair flip:** Adjacent bit pairs `(i-1, i)` that correspond to single
   pre-NRZI errors.
5. **Triple flip:** C(8,3)=56 combinations of top-8 candidates.
6. **NRZI triple flip:** Pattern `(i-1, i, i+1)` for 2 adjacent pre-NRZI errors.

Total budget: ~90 CRC checks max per failed frame.

Constants: `MAX_FLIP_CANDIDATES=12`, `FLIP_CONFIDENCE_THRESHOLD=96`,
`TRIPLE_FLIP_LIMIT=8`.

**Best-candidate selection:** All 6 phases run to completion. If multiple phases
find valid CRC, the one with the lowest total flip cost (sum of flipped bit
confidences) wins. This avoids committing to an early but suboptimal correction.

### Energy-Based LLR — WIN

Replaced fixed ±64 LLR values with energy ratio: `(mark_energy - space_energy) * 127 / total_energy`.
This gives the soft decoder meaningful confidence gradients instead of binary
hard decisions, enabling effective bit-flip targeting.

Enabled on all Goertzel decoders in multi-decoder via `.with_energy_llr()`.

### LLR Calibration — SMALL WIN

Removed pessimistic `confidence >>= 1` on space bits in both CorrelationDemodulator
and CorrSlicerDecoder. Space bits were getting half the confidence of mark bits
due to an early conservative assumption about de-emphasis. Giving space bits
full energy-ratio confidence improved soft decode targeting.

---

## Things That DIDN'T Work

### Gardner PLL on Goertzel — REJECTED

**Hypothesis:** PLL clock recovery (already working for DM path) should improve
Goertzel timing by tracking symbol boundaries adaptively.

**What was tried:**
- DM discriminator output + LPF as PLL timing error signal for Goertzel sampling
- DM passthrough (no LPF) as PLL input
- Hilbert instantaneous frequency as PLL input

**Results:**
- DM+LPF: T2 = -1 to -31 packets vs Bresenham. T1/T3/T4: catastrophic (-56 to -385)
- DM passthrough: T2 Quality = -1, but T1 drops -341 (unfiltered 2f component)
- Hilbert inst_freq: 15-sample group delay → 0 packets on Quality path

**Root cause:** PLL syncs to DM transitions, which are group-delay-shifted from
the Goertzel accumulator's optimal evaluation point. The DM delay (2-8 samples)
plus LPF (~3-4 samples) creates a systematic phase offset. Multi-decoder timing
diversity works precisely because each decoder has a FIXED Bresenham phase — they
hedge by trying all phases simultaneously, not by tracking.

**Verdict:** DO NOT RE-ATTEMPT. The Goertzel accumulator's natural integration
window is incompatible with DM-derived timing signals.

### Gardner PLL on Correlation — REJECTED

**Hypothesis:** PLL should help correlation demodulator track drifting symbol clocks.

**Results:** Normalized discriminator output (±127) peaked at 506 frames vs
Bresenham's 595 on T2. PLL overcorrects even with heavy damping at 9.2
samples/symbol (11025 Hz / 1200 baud). The low samples-per-symbol ratio
means PLL corrections are a large fraction of the symbol period.

`.with_pll()` / `.with_custom_pll()` kept as opt-in API for future experiments
at higher sample rates.

### PLL Beta > 0 — UNIVERSALLY HARMFUL

**Hypothesis:** Frequency correction (beta term) should improve tracking of
off-frequency transmitters.

**Results across every test:**
- alpha=936, beta=0: 564 frames (T2, DM+PLL)
- alpha=936, beta=1: 0 frames (catastrophic)

**Root cause:** The leaky integrator (shift=3, ~7 sample group delay) creates
a systematic positive bias in phase error at every transition. Alpha corrects
per-transition (bias averages out). Beta ACCUMULATES this bias into frequency,
pushing freq_offset to the ±2% clamp. Without leaky integrator: raw DM output
has too many false transitions from BPF ringing, which also creates systematic
beta bias.

Beta=0 is optimal in every configuration tested. Frequency offset diversity
(static ±50/100 Hz) is the correct way to handle off-frequency transmitters.

### Pre-Emphasis — HARMFUL

**Hypothesis:** Pre-emphasis filter `y[n] = x[n] - 0.95·x[n-1]` should
compensate for de-emphasized signals on Track 2.

**Results:** T2 = 405 vs 564 without pre-emphasis. T3 (flat signal) = neutral.

**Root cause:** Pre-emphasis amplifies everything above ~300 Hz, including noise
above 2200 Hz. On de-emphasized signals the noise floor is already elevated at
high frequencies; pre-emphasis makes it worse. The Goertzel energy comparison
inherently handles mild de-emphasis because both tones are affected similarly.

### Adaptive Threshold (Midpoint Shift) — NEUTRAL/HARMFUL

**Hypothesis:** Shift the DM decision threshold to midpoint between mark and
space peak amplitudes to handle asymmetric signals.

**Results:** With de-emphasis: mark_peak=+100, space_peak=-30 → threshold=+35.
But transition symbols have accumulator near 0, which falls in a "dead zone"
(0 < acc < 35 → classified as space, should be mark). Result: 0 frames.

**Alternative tried:** AGC rescale (scale weaker-side accumulator by ratio of
peak magnitudes). Result: neutral — doesn't hurt, doesn't help. The DM
discriminator doesn't actually need amplitude compensation.

### Cascaded BPF on Correlation — REJECTED

**Hypothesis:** Steeper bandpass rolloff (-12 dB/oct) should improve tone
selectivity.

**Results:** Regressed all four tracks. The additional group delay from cascaded
filters worsens symbol timing alignment, and the narrower passband rejects
legitimate off-frequency signals.

### DM+PLL Soft Decode — ZERO VALUE

**Results:** 0 soft saves on both T1 and T2 with SoftHdlcDecoder.

**Root cause:** DM accumulator magnitude (`accumulator.abs() >> 6`) doesn't
produce useful confidence gradients. The accumulate-and-dump process over a full
symbol period averages out the fine structure that would indicate borderline
bits. All DM LLR values cluster at either very high or very low confidence —
there's no middle ground for the soft decoder to exploit.

### Individual DM Alpha Tuning in Ensemble — NEGLIGIBLE

Swept alpha across DM decoders in multi-decoder: α=600/800 vs default 936/400.
Gained <1 frame difference. The Goertzel array already captures the vast
majority of frames that DM decoders can find.

### Adaptive Goertzel Retune on Multi — NEUTRAL

Enabled preamble-adaptive Goertzel retuning (Hilbert → InstFreq → retune
coefficients) on 3 timing-0 base Goertzel decoders (indices 0, 3, 6). No gain
on WA8LMF benchmark tracks (transmitters are well-calibrated in test suite).

Kept in code because: zero cost after preamble lock, may help real-world
crystal-drifted transmitters. Uses ±200 Hz sanity check to avoid wild retunes.

---

## Key Insights

1. **Diversity >> single-decoder optimization.** The 38× multi-decoder achieves
   98.3% of Dire Wolf; no single-decoder improvement came close. The fundamental
   problem is parameter uncertainty (timing phase, tone frequency, gain
   imbalance), and diversity hedges against all of them simultaneously.

2. **Fixed Bresenham timing with phase diversity beats PLL tracking.** At 9.2
   samples/symbol, PLL corrections are too coarse. Three fixed phases guarantee
   one is within ±1.5 samples of optimal. PLL tracking adds group-delay-induced
   bias that hurts more than adaptive tracking helps.

3. **Different algorithms find different frames.** Goertzel energy detection and
   NCO correlation use different math and fail on different signals. Combined
   mode's 23 exclusive frames (14 Multi-only + 9 CorrSlicer-only on T2) prove
   algorithmic diversity has value beyond parameter diversity.

4. **Soft decode helps Goertzel but not DM.** Energy-ratio LLR from Goertzel
   provides meaningful confidence gradients (28 soft saves in multi-decoder).
   DM accumulator magnitude doesn't — the accumulate-and-dump process destroys
   the fine structure needed for soft targeting.

5. **Track 4 exceeds Dire Wolf.** Multi-decoder + DM diversity finds 104-105
   frames vs DW's 98. DM's complementary failure mode captures weak signals
   that energy-based Goertzel detection misses.

6. **The gap is parameter uncertainty, not SNR.** Attribution analysis shows that
   the −50 Hz frequency offset decoder alone captures 63% of T2 frames.
   Multi-decoder proves that with the right parameter settings, the signal is
   decodable — we just don't know which settings a priori.

---

## Chronological Development Path

For reference, the implementation order was:

1. **Goertzel+Bresenham** — adopted after DM loopback failures (filter coefficient bugs, DM asymmetry)
2. **HDLC + AX.25** — bit-level framing and packet parsing
3. **DM discriminator** — complementary architecture with BPF+LPF+accumulate
4. **PLL clock recovery** — Gardner TED for DM path (beta=0 discovery)
5. **Multi-decoder** — 38× parallel with 5-dimension diversity
6. **Soft HDLC** — LLR + 6-phase recovery chain
7. **Correlation demod** — Dire Wolf-style NCO mixer
8. **CorrSlicerDecoder** — multi-slicer + frequency diversity
9. **Combined mode** — cross-architecture merging
10. **Attribution tooling** — per-decoder frame provenance → MiniDecoder selection
