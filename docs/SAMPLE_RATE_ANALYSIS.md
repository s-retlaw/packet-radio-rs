# Sample Rate Analysis for AFSK Demodulation

## Current: 11025 Hz

11025 Hz = 44100/4, a standard audio rate supported by soundcards and WAV files.
Dire Wolf uses it. Gives **9.19 samples/symbol** at 1200 baud — enough to
demodulate, cheap enough for MCUs.

### Why 11025 Is Non-Ideal

- **Fractional samples/symbol** (9.19): Bresenham timing can only resolve to
  ~11% of a symbol period. Requires timing-phase diversity (3-5 decoders) to
  cover the parameter space.
- **Non-integer tone alignment**: 11025/1200 = 9.1875, 11025/2200 = 5.011.
  Goertzel windows don't align perfectly with either tone period.

## Candidate Sample Rates

| Rate | sps | Mark align | Space align | CPU vs 11025 | Notes |
|------|-----|-----------|-------------|-------------|-------|
| 11025 | 9.19 | No | No | 1.0× | Current, cheapest |
| 13200 | 11.0 | Yes (11×1200) | Yes (6×2200) | 1.2× | **Lowest "perfect" rate** |
| 22050 | 18.375 | No | No | 2.0× | CD/2, common WAV rate |
| 26400 | 22.0 | Yes (22×1200) | Yes (12×2200) | 2.4× | Mobilinkd TNC3 uses this |
| 44100 | 36.75 | No | No | 4.0× | CD rate, desktop only |

### 13200 Hz — Best MCU Candidate

The lowest rate where both tones are integer multiples:
- 13200 / 1200 = **11 samples/symbol** (exact)
- 13200 / 2200 = **6 samples/cycle** (exact)
- Only 20% more CPU than 11025 Hz
- Bresenham resolves to **9.1% of symbol** (vs 10.9% at 11025)
- Goertzel window aligns with both tone periods

### 26400 Hz — Desktop / High-Performance Candidate

Mobilinkd's choice (see `mobilinkd/afsk-demodulator` notebook):
- 26400 / 1200 = **22 samples/symbol** (exact)
- 26400 / 2200 = **12 samples/cycle** (exact)
- Bresenham resolves to **4.5% of symbol** — may reduce need for timing diversity
- Goertzel windows align perfectly with mark tone periods
- 2.4× CPU cost — feasible on ESP32-S3 (240 MHz), tight on RP2040 (125 MHz)
- This is the rate the Mobilinkd TNC3 (Cortex-M4) uses successfully

## Benefits of Integer Tone Alignment

When the sample rate is an integer multiple of both tones:

1. **Goertzel accuracy**: Window boundaries align with complete tone cycles,
   eliminating spectral leakage. Energy estimates are exact.
2. **Delay-multiply precision**: Optimal delay is an exact integer number of
   samples, no fractional-sample error.
3. **Bresenham regularity**: Symbol boundaries fall on exact sample indices,
   reducing quantization jitter.
4. **Filter design**: FIR filters can be designed with exact integer taps per
   tone cycle, improving stopband rejection.

## What's Hardcoded to 11025 Hz

The architecture passes `DemodConfig.sample_rate` through to most DSP:
- Goertzel coefficients: computed from `goertzel_coeff(freq, sample_rate)` —
  has precomputed Q14 lookup for 1200/11025 and 2200/11025, runtime fallback
  for other rates
- PLL: `ClockRecoveryPll::new(sample_rate, baud_rate, alpha, beta)` — adapts
- Bresenham: phase increments from `sample_rate` — adapts
- Hilbert/InstFreq: parameterized by `sample_rate` — adapts

### Requires New Precomputed Constants

To support a new sample rate (e.g., 13200), need:
1. **BPF coefficients**: `afsk_bandpass_13200()` — biquad BPF 900-2500 Hz
2. **Narrow/Wide BPF variants**: for multi-decoder diversity
3. **Post-detect LPF**: `post_detect_lpf_13200()` — 1200 Hz cutoff
4. **Correlation LPFs**: `corr_lpf_13200()` — 500 Hz cutoff + variants
5. **DM delay values**: optimal delay for the new rate
6. **Multi-decoder tuning**: re-benchmark all 38 decoders, retune diversity

### Adapts Automatically

- Goertzel coefficients (runtime computation for non-lookup rates)
- PLL nominal period and correction gains
- Bresenham phase increments
- Hilbert transform / instant frequency detector
- HDLC decoder (bit-level, rate-independent)
- KISS protocol (byte-level, rate-independent)

## ADC Clock Dividers

### RP2040 (48 MHz ADC clock)

Formula: `rate = 48MHz / (1 + int + frac/256)`

| Target Rate | int | frac | Actual Rate | Error |
|-------------|-----|------|-------------|-------|
| 11025 Hz | 4352 | 190 | 11024.97 Hz | 0.001% |
| 13200 Hz | 3635 | 71 | 13199.96 Hz | 0.001% |
| 22050 Hz | 2175 | 222 | 22049.94 Hz | 0.001% |
| 26400 Hz | 1817 | 35 | 26399.93 Hz | 0.001% |

### ESP32-S3 (I2S or ADC with timer)

I2S peripheral can generate any sample rate from its PLL. ADC can be timer-triggered
at arbitrary rates. No constraints on rate selection.

## Mobilinkd Notebook Key Insights

Source: `github.com/mobilinkd/afsk-demodulator/blob/master/afsk-demodulator.ipynb`

### Architecture: Digital Correlator
Pipeline: BPF → zero-crossing digitize → XOR(signal, signal[delay]) → LPF → re-digitize → PLL → NRZI → HDLC

This is a binary (1-bit quantized) version of our delay-multiply path. Key differences:
- Digitizes **before** correlation (XOR vs analog multiply)
- Inherently twist-immune (amplitude discarded at zero-crossing)
- Computationally trivial (XOR instead of multiply)

### Filter Parameters (at 26400 Hz)
- **BPF**: 141-tap FIR, Hann window, 1100-2300 Hz passband, Q15 fixed-point
- **LPF**: 101-tap FIR, Hann window, 760 Hz cutoff, Q15 fixed-point
- Mobilinkd uses FIR for linear phase (no ISI contribution from filter)

### PLL Clock Recovery
- **Loop filter**: 64 Hz Bessel IIR (b=[0.145, 0.145], a=[1.0, -0.711])
- **Lock filter**: 40 Hz Bessel IIR (b=[0.095, 0.095], a=[1.0, -0.810])
- **Lock hysteresis**: lock when jitter < 2.5% of sps, unlock when > 15% of sps
- **Correction gain**: 1.2% when locked, 4.8% when unlocked (fast acquire, slow track)
- Works on binary transitions after LPF — avoids group delay issues we hit with Goertzel+PLL

### Twist Handling
- TNC3 runs **3 parallel decoders tuned for different twist levels**
- Digital correlator is naturally twist-resistant (binary input)
- Heavy twist can still cause problems — not fully solved by digitization alone

## Benchmark Results (2026-02-27)

With precomputed Q15 biquad filter coefficients tuned for each sample rate
(BPF standard/narrow/wide, post-detect LPF, correlation LPF), plus rate-specific
DM delay values.

### Single-Decoder (Fast Path) — Primary MCU Target

| Rate | T1 | T2 | T3 | T4 | Total | %DW | Delta vs 11025 |
|------|-----|-----|-----|-----|-------|-----|-----------------|
| 11025 | 581 | 563 | 90 | 60 | 1294 | 59.5% | baseline |
| 13200 | 635 | 607 | 90 | 66 | 1398 | 64.3% | **+104 (+8.0%)** |
| 26400 | 661 | 631 | 90 | 67 | 1449 | 66.7% | **+155 (+12.0%)** |

### Multi-Decoder (38×) — Desktop

| Rate | T1 | T2 | T3 | T4 | Total | %DW | Delta vs 11025 |
|------|------|-----|------|------|-------|-----|-----------------|
| 11025 | 998 | 935 | 100 | 104 | 2137 | 98.3% | baseline |
| 13200 | 994 | 932 | 100 | 101 | 2127 | 97.8% | -10 (neutral) |
| 26400 | 1000 | 936 | 100 | 103 | 2139 | 98.4% | +2 (neutral) |

### Smart3 (MiniDecoder, 3×)

| Rate | T1 | T2 | T3 | T4 | Total | %DW |
|------|-----|-----|------|------|-------|-----|
| 13200 | 956 | 909 | 100 | 89 | 2054 | 94.5% |
| 26400 | 959 | 916 | 100 | 89 | 2064 | 95.0% |

### Key Findings

1. **Single-decoder gains are large**: +8% at 13200 Hz, +12% at 26400 Hz.
   Integer tone alignment eliminates Goertzel spectral leakage and Bresenham
   quantization jitter — exactly as predicted.
2. **Multi-decoder is neutral**: 38× timing/freq/gain diversity already
   compensates for 11025's fractional alignment. The gains from integer
   alignment are redundant with what multi-decoder diversity provides.
3. **13200 Hz is the sweet spot for MCU**: +104 frames for only 20% more CPU.
   26400 Hz gets +155 but costs 2.4× CPU — tight on RP2040 (125 MHz).
4. **Track 2 (priority)**: 563→607→631. The de-emphasized Mic-E bursts benefit
   most from cleaner Goertzel accumulation and tighter symbol timing.

## Ideas for Future Experimentation

### 1. Binary XOR Correlator as 4th Architecture
Add alongside Goertzel, DM, and Correlation. Very cheap (XOR vs multiply),
partially decorrelated error patterns. Could add exclusive frames in Combined mode.

### 2. Asymmetric Twist-Tuned Decoders
Add 2-3 decoders to Multi with explicit pre-emphasis/de-emphasis curves rather
than flat symmetric gain. Track 2 (de-emphasized audio) is the priority target.

### 3. 13200 Hz Sample Rate Test
Benchmark at 13200 Hz on desktop to measure impact of integer tone alignment.
Compare single-decoder and multi-decoder results against 11025 Hz baseline.

### 4. Lock Detection / DCD from Jitter
Use Mobilinkd-style jitter measurement to gate HDLC decoding. Don't waste CPU
on noise periods. Could reduce false decode attempts.
