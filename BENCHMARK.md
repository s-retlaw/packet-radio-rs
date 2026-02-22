# BENCHMARK.md — WA8LMF TNC Test CD Targets & Decoder Specification
#
# This file is a self-contained specification for implementing and testing
# the AFSK demodulator. Hand this to a Claude Code agent along with CLAUDE.md.

## What Is This Benchmark?

The WA8LMF TNC Test CD (created by Stephen Smith, WA8LMF) is the universal
standard for comparing APRS/AX.25 1200-baud packet radio decoder performance.
It contains real-world APRS recordings from the Los Angeles area with every
kind of signal quality issue you encounter on-air: over/under-deviated signals,
packet collisions, rapid-fire packets, raw NMEA trackers, CW ID on packet,
weak signals mixed with strong ones.

Download: http://wa8lmf.net/TNCtest/index.htm (Version 2.0 FLAC recommended)

The metric is simple: **how many valid, error-free AX.25 frames does your
decoder extract from each track?**

---

## Track Descriptions

### Track 1 — Flat Audio Response
- Raw receiver discriminator output with flat frequency response
- Easier to decode — mark and space tones are roughly balanced
- Mark/space amplitude ratio: 0.53 to 1.38, median 0.81
- Duration: ~25 minutes of compressed real-world traffic
- Used less often as the benchmark (Track 2 is the standard)

### Track 2 — De-emphasized (THE Standard Benchmark)
- Same audio as Track 1 but with 6 dB/octave de-emphasis applied
- Simulates typical speaker/volume-control output of an FM receiver
- This is what real TNCs actually hear in practice
- Mark/space amplitude ratio: 1.73 to 3.81, median 2.48
- The 2200 Hz space tone is significantly weaker than the 1200 Hz mark
- **This is the track everyone uses for comparison. All published numbers
  reference Track 2 unless explicitly stated otherwise.**

### Track 3 — Identical to Track 1
- Included for reference, same as Track 1

### Track 4 — Sensitivity Test
- 100 identical packets at decreasing signal levels
- Used to measure weak-signal threshold, not aggregate decode count
- Score: percentage of 100 packets decoded at each RF level

---

## World-Class Benchmark Results (Track 2)

These numbers are compiled from the Dire Wolf documentation
(WA8LMF-TNC-Test-CD-Results.pdf, January 2019 update) and the
WB2OSZ "A Better APRS Packet Demodulator" technical paper.

### Tier 1: State of the Art (1000+ packets, Track 2)

| Decoder | Track 1 | Track 2 | Notes |
|---------|---------|---------|-------|
| UZ7HO SoundModem 0.97b | 1027 | 1022 | Virtual audio cable, default settings |
| Dire Wolf 1.5 (E+, FIX_BITS=1) | ~1021 | ~1022 | Multi-slicer + single-bit fix-up |
| Dire Wolf 1.5 (E+, FIX_BITS=0) | 1012 | 1008 | Multi-slicer, error-free only |
| Dire Wolf 1.2 (E+, FIX_BITS=1) | 1021 | 1022 | Multi-slicer + single-bit fix-up |
| Dire Wolf 1.2 (E+, FIX_BITS=0) | 1011 | 1004 | Multi-slicer, error-free only |
| APRSpro v2.1 | 1012 | 958 | Uses Dire Wolf demodulator internally |
| Dire Wolf 1.2 (E, AGC, no fix) | 993 | 988 | Single decoder with AGC |
| ARM32M4F TNC platform | — | 994-998 | Hardware Cortex-M4F TNC |
| WX3in1 Plus 2.0 | 960 | 981 | Hardware |
| KPC-3 Plus (optimal volume) | 989 | 925 | Hardware TNC, very volume-sensitive |

### Tier 2: Good (900-999 packets, Track 2)

| Decoder | Track 1 | Track 2 | Notes |
|---------|---------|---------|-------|
| AX25 Java Soundcard Modem (4X6IZ) | 966 | 964 | Dual decoder approach |
| PocketPacket v2.2 | 964 | 942 | |
| NinoTNC A2 | — | 940 | Hardware |
| Tracker 2 with TCM3105 | — | 991 | Best hardware result |
| uTNT | — | 970 | |
| MicroModem | — | 905 | Embedded |

### Tier 3: Legacy (below 900, Track 2)

| Decoder | Track 2 | Notes |
|---------|---------|-------|
| AEA PK-90 | 728 | 1980s hardware TNC |
| MFJ-1274 | 883 | Hardware |
| BluetoothLE APRS TNC | 641 | |
| AGWPE | 513 | First-gen soundcard modem |
| Linux soundmodem | 412-450 | First-gen soundcard modem |
| Linux multimon | 130 | Essentially broken for this purpose |

### Key Takeaways from the Data

1. **The magic number is ~1000 on Track 2.** Getting above 1000 error-free
   frames puts you in the top tier with Dire Wolf and UZ7HO SoundModem.

2. **Single-decoder ceiling is ~983-988.** Dire Wolf's single "E" decoder
   with AGC gets 988 on Track 2. The multi-slicer (9 parallel comparators
   with different gains) adds ~16-20 more frames.

3. **FIX_BITS adds ~14-18 frames.** Dire Wolf's single-bit error correction
   (trying to flip each bit individually when CRC fails) recovers additional
   packets. This is crude compared to our soft-decision approach.

4. **Track 2 is harder than Track 1.** The de-emphasis makes the space tone
   (2200 Hz) about 2.5× weaker than mark (1200 Hz). Decoders that assume
   equal tone amplitudes perform badly on Track 2.

5. **Volume sensitivity matters.** The KPC-3 Plus scores range from 261 to
   925 on Track 2 depending on audio volume setting. Software decoders with
   AGC or adaptive gain are far less sensitive to input level.

---

## Our Targets

### Phase 1: Basic Decoder (Single Decoder, Hard Decisions)

| Metric | Target | World-Class Ref |
|--------|--------|-----------------|
| Track 1 frames | ≥ 980 | Dire Wolf single = 993 |
| Track 2 frames | ≥ 970 | Dire Wolf single = 988 |
| False positives | 0 | CRC must be valid |
| Processing speed | > 10× real-time | On x86_64 desktop |
| Memory (core decoder) | < 2 KB | Embedded-viable |

This target uses the delay-multiply or Hilbert discriminator with AGC but
no multi-decoder and no bit-flipping. Matching Dire Wolf's single-decoder
performance validates the core DSP pipeline.

### Phase 2: Adaptive Tracker (Single Decoder, Preamble Training)

| Metric | Target | World-Class Ref |
|--------|--------|-----------------|
| Track 1 frames | ≥ 1000 | Dire Wolf multi = 1011 |
| Track 2 frames | ≥ 1000 | Dire Wolf multi = 1004 |
| False positives | 0 | |
| Processing speed | > 10× real-time | |
| Memory (core decoder) | < 4 KB | |

The adaptive tracker should let a single decoder match Dire Wolf's
multi-decoder by tuning to each transmitter's actual frequencies/baud rate
during the preamble. This is the key architectural win.

### Phase 3: Soft-Decision Decoder (Bit-Flipping Error Correction)

| Metric | Target | World-Class Ref |
|--------|--------|-----------------|
| Track 1 frames | ≥ 1025 | Dire Wolf E+ FIX_BITS=1: 1021 |
| Track 2 frames | ≥ 1025 | Dire Wolf E+ FIX_BITS=1: 1022 |
| Bit-flip recoveries | Track separately | |
| False positives | 0 | |
| Processing speed | > 5× real-time | Bit-flip is slower |
| Memory | < 8 KB | Soft bit buffer needed |

Soft-decision decoding with confidence-guided bit-flipping should exceed
Dire Wolf's FIX_BITS because we target the weakest bits rather than
trying all bits sequentially. Our bit-flip budget is 1-2 bit errors
(Dire Wolf FIX_BITS=1 only corrects single-bit errors).

### Phase 4: Beat the World Record

| Metric | Target | Notes |
|--------|--------|-------|
| Track 2 frames | ≥ 1040 | Beyond any published result |
| Combined techniques | All of the above | |

This requires the full stack: Hilbert instantaneous frequency, adaptive
tracking, soft-decision decoding, and possibly Viterbi CPFSK detection.

### Embedded Targets (ESP32, RP2040)

| Platform | Decoder | Track 2 Target | Notes |
|----------|---------|----------------|-------|
| ESP32 (240 MHz, FPU) | Quality path | ≥ 1000 | Full adaptive + soft |
| ESP32 (240 MHz, FPU) | Fast path | ≥ 970 | Delay-multiply only |
| RP2040 (133 MHz, no FPU) | Fast path | ≥ 950 | Integer only |
| STM32F411 (168 MHz, FPU) | Fast path | ≥ 970 | |

---

## How Dire Wolf's Decoder Works (For Reference)

Understanding what Dire Wolf does helps us know what to improve on.

### Architecture

```
Audio → Bandpass (optional) → Mark BPF → |envelope| → LPF → AGC ─┐
                             → Space BPF → |envelope| → LPF → AGC ─┤
                                                                     ├─ Compare → Bit
                                                          (× N gains)
```

### Key Design Choices

1. **Correlator-based detection**: Four multiply-accumulate operations per
   sample (mark I, mark Q, space I, space Q). Uses precomputed sin/cos
   tables for the reference frequencies. More expensive than delay-multiply.

2. **AGC (version C)**: Automatic gain control normalizes mark and space
   amplitudes independently before comparison. Handles the de-emphasis
   imbalance well. Single decoder gets 988 on Track 2.

3. **Multi-slicer (version E+)**: Instead of AGC, runs 9 parallel
   comparators with different gains applied to the space tone:
   -6.0, -3.8, -1.5, +0.8, +3.0, +5.3, +7.5, +9.8, +12.0 dB.
   Each produces a bit stream that feeds its own HDLC decoder.
   Deduplicate the output. Gets 1004 on Track 2 (error-free).

   On Track 2, the optimal single gain is +7.5 dB (983 frames).
   Running all 9 and deduplicating gives 1004 — a 2% improvement
   at 9× the HDLC decoder cost.

4. **FIX_BITS**: When CRC fails, try flipping each bit one at a time
   and recheck CRC. If flipping bit N produces a valid CRC, accept the
   frame. This is O(N) where N is frame length in bits. Only fixes
   single-bit errors. Combined with multi-slicer: 1022 on Track 2.

### What Dire Wolf Doesn't Do

- No preamble-based parameter estimation
- No soft-decision information (hard 0/1 from comparator)
- No confidence-guided bit-flipping (tries all bits sequentially)
- No adaptive baud rate tracking (fixed at 1200)
- No adaptive frequency tracking (fixed mark/space references)
- No exploitation of continuous-phase property

**These are our opportunities to beat it.**

---

## Implementation Specification

### Demodulator Interface

The demodulator must implement this trait:

```rust
/// Core demodulator trait. Platform-independent, no_std compatible.
pub trait Demodulator {
    /// Process a buffer of audio samples.
    /// Returns the number of complete AX.25 frames decoded.
    /// Frames are delivered via the callback.
    fn process_samples(
        &mut self,
        samples: &[i16],
        frame_callback: &mut dyn FnMut(&[u8]),  // raw AX.25 frame bytes
    ) -> usize;

    /// Reset demodulator state (between files, on carrier loss, etc.)
    fn reset(&mut self);

    /// Return decoder statistics
    fn stats(&self) -> DemodStats;
}

pub struct DemodStats {
    pub frames_decoded: u32,
    pub crc_failures: u32,
    pub bit_flips_recovered: u32,  // frames recovered by soft decode
    pub bits_processed: u64,
    pub pll_locked: bool,
}
```

### Benchmark Runner Specification

Build a binary crate `benchmark` that:

1. Reads a WAV file (16-bit PCM, mono, any sample rate)
2. Feeds all samples through the demodulator
3. Counts valid AX.25 frames (unique, based on content hash)
4. Reports results in a standardized format

```
Usage:
  cargo run --release -p benchmark -- [OPTIONS] <wav_file>

Options:
  --decoder <fast|quality|all>   Which decoder path to test (default: all)
  --compare <file>               Compare against reference packet list
  --output <file>                Write decoded packets to file
  --verbose                      Show each decoded packet
  --stats                        Show detailed decoder statistics

Output format:
  Track: 02_Track_2.wav
  Decoder: quality (hilbert + adaptive + soft)
  Duration: 1549.2 seconds (25:49)
  Sample rate: 44100 Hz

  === Results ===
  Total frames decoded: 1037
  Unique frames: 1031
  Duplicates: 6
  CRC failures (hard): 47
  Bit-flip recoveries: 23
  False positives: 0

  === Phase Breakdown ===
  Hard-decision decodes: 1008
  1-bit flip recoveries: 19
  2-bit flip recoveries: 4

  === Comparison (if --compare given) ===
  Reference packets: 1022
  Matched: 1015
  We decoded, reference missed: 16
  Reference decoded, we missed: 7
```

### WAV File Reader

A minimal WAV reader for the benchmark tool. Must handle:
- 16-bit signed PCM (the standard format)
- 8-bit unsigned PCM (less common)
- Mono or stereo (use left channel if stereo)
- Sample rates: 11025, 22050, 44100, 48000 Hz
- Large files (25+ minutes of audio)

Do NOT use external WAV crate dependencies — keep it simple, just parse
the RIFF header yourself. It's < 50 lines of code.

```rust
pub struct WavFile {
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub channels: u16,
    pub samples: Vec<i16>,  // Mono, normalized to i16
}

impl WavFile {
    pub fn open(path: &str) -> Result<Self, WavError> { ... }
}
```

### Reference Packet List Format

For --compare mode, reference packet files are one hex-encoded AX.25 frame
per line, optionally preceded by a timestamp:

```
# Reference packets from Dire Wolf 1.5, Track 2
# Format: [timestamp] hex_frame_bytes
0.123 a]9cc29292868a96406403f0...
1.456 a]9cc28e9e9a8460...
```

Generate reference data by running Dire Wolf on the same WAV file:
```bash
direwolf -r 44100 -t 0 -d x 02_Track_2.wav 2>&1 | grep "^[0-9]" > reference.txt
```

---

## Demodulator Implementation Roadmap

### Step 1: Bandpass Filter + Delay-Multiply Detector

**Files to create/modify:**
- `core/src/modem/filter.rs` — Biquad bandpass/lowpass coefficient computation
- `core/src/modem/demod.rs` — DelayMultiplyDetector struct

**Test:** Generate a clean 1200 Hz tone, feed through detector, verify
positive output. Generate 2200 Hz tone, verify negative output. Generate
alternating tones, verify correct polarity sequence.

**Validation:** This is purely DSP — test with synthetic signals first.

### Step 2: Clock Recovery PLL

**Files to create/modify:**
- `core/src/modem/demod.rs` — ClockRecoveryPll struct

**Test:** Feed a known bit pattern through modulator → detector → PLL.
Verify PLL locks within 10-20 symbol periods and outputs bits at the
correct baud rate. Test with ±2% baud rate offset.

### Step 3: NRZI Decode + Wire to HDLC

Connect detector → PLL → NRZI → existing HDLC decoder.

**Test:** Full modulator → demodulator loopback. Generate a known packet,
modulate it, demodulate it, verify the frame matches bit-for-bit.

### Step 4: WAV File Processing

**Files to create:**
- `benchmark/src/main.rs`
- `benchmark/src/wav.rs`

**Test:** Process a WAV file containing a known modulated packet.
Verify it decodes.

### Step 5: TNC Test CD Benchmark (Phase 1 Target)

Run against Track 1 and Track 2 WAV files. Target: ≥970 on Track 2.

**If below target:** Tune bandpass filter bandwidth, LPF cutoff, PLL
gains, delay value. Plot the discriminator output to visualize issues.

### Step 6: Hilbert Transform + Instantaneous Frequency (Quality Path)

**Files to create/modify:**
- `core/src/modem/hilbert.rs` — HilbertTransform struct
- `core/src/modem/demod.rs` — InstantaneousFrequency struct, QualityDemodulator

**Test:** Feed known tones, verify frequency estimates are accurate.
Compare to delay-multiply on same test signals.

### Step 7: Adaptive Tracker

**Files to create:**
- `core/src/modem/adaptive.rs` — AdaptiveTracker struct

**Test:** Generate packets with various frequency offsets (+50 Hz, -50 Hz,
+100 Hz). Verify the tracker estimates the correct frequencies during
preamble and adjusts the threshold.

**Benchmark:** Should improve Track 2 score by 10-30 frames over Phase 1.

### Step 8: Soft-Decision Decoding

**Files to modify:**
- `core/src/ax25/frame.rs` — SoftHdlcDecoder
- `core/src/modem/demod.rs` — output soft bits from quality path

**Test:** Generate packets, add noise to flip 1-2 bits, verify soft decoder
recovers them. Count recovery rate at various SNR levels.

**Benchmark:** Should push Track 2 score above 1020.

---

## Testing Methodology

### Synthetic Signal Tests (No WAV files needed)

These tests verify the DSP pipeline using programmatically generated signals.
They should be implemented as `#[test]` functions in the core crate.

```rust
// Generate a perfect AFSK signal for a known packet
fn generate_test_signal(packet: &[u8], sample_rate: u32, snr_db: Option<f64>) -> Vec<i16>;

// Add white Gaussian noise at a specified SNR
fn add_noise(samples: &mut [i16], snr_db: f64);

// Shift all frequencies by an offset (simulates crystal drift)
fn frequency_shift(samples: &[i16], shift_hz: f64, sample_rate: u32) -> Vec<i16>;

// Resample to simulate baud rate offset
fn resample(samples: &[i16], ratio: f64) -> Vec<i16>;

// Apply de-emphasis filter (simulates Track 2 vs Track 1 difference)
fn apply_deemphasis(samples: &[i16], sample_rate: u32) -> Vec<i16>;
```

#### Test Matrix for Synthetic Signals

| Test | Parameters | Phase 1 Target | Phase 3 Target |
|------|-----------|----------------|----------------|
| Clean loopback | SNR=∞, no offset | 100% decode | 100% decode |
| Noisy (20 dB SNR) | Moderate noise | ≥ 95% decode | ≥ 98% decode |
| Noisy (10 dB SNR) | Heavy noise | ≥ 50% decode | ≥ 70% decode |
| Noisy (6 dB SNR) | Very heavy noise | ≥ 10% decode | ≥ 30% decode |
| Freq offset +50 Hz | Mark=1250, Space=2250 | ≥ 90% decode | ≥ 98% decode |
| Freq offset +100 Hz | Mark=1300, Space=2300 | ≥ 50% decode | ≥ 90% decode |
| Baud rate +2% | 1224 baud | ≥ 80% decode | ≥ 95% decode |
| Baud rate -2% | 1176 baud | ≥ 80% decode | ≥ 95% decode |
| De-emphasis | 6 dB/octave rolloff | ≥ 90% decode | ≥ 98% decode |
| Low amplitude | -20 dB signal level | ≥ 90% decode | ≥ 95% decode |
| DC offset | +25% DC bias | ≥ 95% decode | ≥ 98% decode |
| Combined worst case | +50Hz, +1%, 15dB SNR, de-emph | ≥ 30% | ≥ 60% |

Run each test with 100 random packets and report the decode percentage.

#### Stress Test

Generate 1000 random valid APRS packets with:
- Random callsigns (valid format)
- Random info fields (position, message, status)
- Random inter-packet gaps (50-500 ms)
- Clean audio (no noise)

Modulate all into one long WAV. Demodulate. **Must recover 100% (1000/1000).**
This is the basic sanity check — if clean loopback isn't perfect, something
is fundamentally wrong.

Then repeat with progressive noise: 30 dB, 20 dB, 15 dB, 10 dB SNR.
Plot decode rate vs. SNR to characterize the decoder's sensitivity curve.

### TNC Test CD Tests

These require the WAV files from the TNC Test CD (not included in repo).

```bash
# Download and extract (user must do this manually)
# Place WAV files in tests/wav/
# Expected filenames: 01_Track_1.wav, 02_Track_2.wav

# Run benchmark
cargo run --release -p benchmark -- tests/wav/02_Track_2.wav

# Run with comparison against Dire Wolf reference
cargo run --release -p benchmark -- tests/wav/02_Track_2.wav \
    --compare tests/expected/direwolf_track2.txt

# Run all decoders for comparison
cargo run --release -p benchmark -- tests/wav/02_Track_2.wav --decoder all
```

### Regression Testing

After any change to the modem code:

1. Run the full synthetic test matrix — no regressions allowed
2. Run the stress test — must remain 100% on clean loopback
3. If TNC Test CD files are available, run benchmark — packet count must
   not decrease unless a false positive was removed (document why)

Store baseline results in `tests/expected/baseline.json`:

```json
{
  "date": "2026-02-22",
  "decoder": "quality",
  "track1_frames": 1015,
  "track2_frames": 1008,
  "stress_clean": 1000,
  "stress_20db": 962,
  "stress_10db": 531
}
```

### Comparison Methodology

To fairly compare against Dire Wolf, match their test conditions:

1. **Use WAV files directly** — don't go through sound cards (avoids
   analog audio path adding noise or frequency response changes)
2. **Disable FIX_BITS for apples-to-apples** — our soft decoder is a
   different (better) approach to the same problem
3. **Count unique frames** — deduplicate based on content, not timing
4. **Zero false positives** — every frame must have valid CRC
5. **Report both with and without soft-decision recovery** separately

---

## Key Technical Parameters to Tune

These are the knobs that affect decode performance. Tune them empirically
using the TNC Test CD as the objective function.

| Parameter | Starting Value | Range to Sweep | Affects |
|-----------|---------------|----------------|---------|
| Bandpass filter center | 1700 Hz | 1600-1800 Hz | Both tones must pass |
| Bandpass filter width | 1200 Hz | 800-1600 Hz | Noise rejection vs. signal pass |
| Delay-multiply τ | 3 samples @22050 | 2-6 samples | Mark/space separation |
| LPF cutoff | 1200 Hz | 800-1600 Hz | Smoothing vs. response time |
| PLL alpha (phase gain) | 0.05 | 0.01-0.15 | Lock speed vs. jitter |
| PLL beta (freq gain) | 0.002 | 0.001-0.01 | Frequency tracking speed |
| PLL baud rate tolerance | ±2% | ±1-5% | Track badly-clocked transmitters |
| Carrier detect threshold | -30 dB? | -20 to -40 dB | Sensitivity vs. false triggers |
| Adaptive training samples | 50 mark + 50 space | 20-100 each | Training speed vs. accuracy |
| Soft bit flip candidates | 8 | 4-16 | Recovery rate vs. CPU cost |
| Max bit flips per frame | 2 | 1-3 | Recovery rate vs. false positive risk |
| Hilbert FIR taps | 31 | 15-63 | Frequency resolution vs. CPU/latency |

### Tuning Procedure

1. Start with the default parameters listed above
2. Run Track 2 benchmark, record baseline
3. Sweep ONE parameter at a time across its range
4. Plot frames decoded vs. parameter value
5. Pick the value that maximizes Track 2 decode count
6. Repeat for the next parameter
7. After tuning all parameters individually, verify no interactions
   by running a coarse grid search on the top 3-4 most sensitive params
8. Final validation: run Track 1 to make sure Track 2 tuning didn't
   break Track 1 performance (they stress different things)

---

## File Structure for Benchmark Crate

```
benchmark/
├── Cargo.toml
├── src/
│   ├── main.rs          # CLI entry point, argument parsing
│   ├── wav.rs           # Minimal WAV file reader
│   ├── runner.rs        # Benchmark runner (feeds samples, counts frames)
│   ├── compare.rs       # Reference comparison logic
│   └── report.rs        # Output formatting and statistics
└── README.md
```

### Cargo.toml for Benchmark

```toml
[package]
name = "benchmark"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "benchmark"
path = "src/main.rs"

[dependencies]
packet-radio-core = { path = "../core", features = ["std", "alloc"] }
```

No external dependencies needed. The WAV reader and CLI parsing can be
done with just std.

---

## Success Criteria Summary

| Phase | Track 2 Target | Key Technique | When to Celebrate |
|-------|----------------|---------------|-------------------|
| 1 | ≥ 970 | Delay-multiply + PLL + HDLC | "It works!" |
| 2 | ≥ 1000 | + Adaptive tracker | "Matches Dire Wolf multi-decoder" |
| 3 | ≥ 1025 | + Soft-decision bit-flip | "Beats Dire Wolf" |
| 4 | ≥ 1040 | + Hilbert + Viterbi | "World class" |

When Track 2 hits 1000 with a single decoder and no bit-fixing, we've
proven the adaptive approach works. When it hits 1025+ with soft decode,
we've built something better than the state of the art.

Let's go. 🐺
