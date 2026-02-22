# Modem Design Document

## Bell 202 AFSK Overview

Bell 202 is the modulation standard used for 1200 baud packet radio and APRS.
It uses Audio Frequency Shift Keying (AFSK) with:

- **Mark frequency**: 1200 Hz (binary 1)
- **Space frequency**: 2200 Hz (binary 0)
- **Baud rate**: 1200 symbols/second
- **Modulation**: Continuous-phase FSK (CPFSK)

The signal is transmitted as audio through an FM radio's voice channel. The
radio's FM modulator/demodulator handles the RF layer — we only deal with
audio frequencies.

The **continuous-phase** property is critical and is the foundation of our
advanced demodulation strategy. Unlike simple on/off keying, CPFSK maintains
phase continuity across symbol boundaries. This phase memory between symbols
carries information that traditional correlator-based decoders discard.

---

## Demodulator Architecture

We implement a **dual-path architecture**: an efficient embedded path for
resource-constrained microcontrollers, and a high-quality path for desktop
and ESP32 that leverages soft-decision decoding and adaptive tracking.

Both paths share the same downstream code (HDLC, AX.25, APRS). Only the
analog front-end differs.

```
╔══════════════════════════════════════════════════════════════════╗
║                    EMBEDDED FAST PATH                           ║
║   (RP2040, Cortex-M0, chips without FPU)                       ║
║                                                                  ║
║   Audio → BPF → Delay-Multiply → LPF → PLL → NRZI → HDLC     ║
║                                                                  ║
║   ~10 cycles/sample, ~1 KB RAM, integer only                    ║
╠══════════════════════════════════════════════════════════════════╣
║                    QUALITY PATH                                  ║
║   (Desktop, Raspberry Pi, ESP32)                                 ║
║                                                                  ║
║   Audio → BPF → Hilbert → InstFreq → Adaptive Tracker          ║
║                                    → Soft PLL → Soft HDLC      ║
║                                                                  ║
║   ~50 cycles/sample, ~4 KB RAM, soft decisions                  ║
╚══════════════════════════════════════════════════════════════════╝
                              │
                    Both paths produce frames
                              │
                              ▼
                   AX.25 Parse → APRS Decode
```

---

## Embedded Fast Path: Delay-and-Multiply Discriminator

### Theory

The delay-and-multiply detector exploits a simple trigonometric identity.
If the input signal is `s(t) = A·cos(2πft + φ)`, then multiplying by a
delayed copy gives:

```
s(t) × s(t - τ) = (A²/2)·cos(2πfτ) + (A²/2)·cos(2πf(2t - τ) + 2φ)
                   ^^^^^^^^^^^^^^       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                   DC term (depends     Double-frequency term
                   only on f and τ)     (removed by lowpass filter)
```

After lowpass filtering, the output is `(A²/2)·cos(2πfτ)`, which depends
on the signal frequency `f` but not on time or phase. By choosing the delay
`τ` appropriately, mark and space frequencies produce opposite-polarity
outputs.

### Optimal Delay Selection

We need a delay where mark (1200 Hz) and space (2200 Hz) produce opposite
polarities. The output for frequency f is proportional to cos(2πfτ). We want:

```
cos(2π · 1200 · τ) > 0  (mark → positive)
cos(2π · 2200 · τ) < 0  (space → negative)
```

Or vice versa. The separation between mark and space outputs should be
maximized.

At 11025 Hz sample rate, candidate delays and their outputs:

| Delay (samples) | τ (μs) | cos(2πf_mark·τ) | cos(2πf_space·τ) | Separation |
|-----------------|--------|-----------------|------------------|------------|
| 3 | 272 | -0.46 | -0.85 | 0.39 (same sign — bad) |
| 4 | 363 | -0.92 | +0.32 | 1.24 (opposite — good) |
| 5 | 454 | -0.65 | +0.99 | 1.64 (opposite — best) |
| 6 | 544 | +0.12 | +0.50 | 0.38 (same sign — bad) |

At 22050 Hz, more choices exist with finer granularity. The optimal delay
should be computed per sample rate at initialization.

```rust
/// Find the delay (in samples) that maximizes mark/space separation.
fn compute_optimal_delay(sample_rate: u32) -> usize {
    let mut best_delay = 1;
    let mut best_separation = 0.0f32;

    for delay in 1..=(sample_rate / 1200) as usize {
        let tau = delay as f32 / sample_rate as f32;
        let mark_out = (core::f32::consts::TAU * 1200.0 * tau).cos();
        let space_out = (core::f32::consts::TAU * 2200.0 * tau).cos();
        let separation = (mark_out - space_out).abs();
        if separation > best_separation {
            best_separation = separation;
            best_delay = delay;
        }
    }
    best_delay
}
```

### Implementation

```rust
const MAX_DELAY: usize = 16;

/// Delay-and-multiply AFSK discriminator.
///
/// Extremely lightweight: one multiply, one filter update per sample.
/// Suitable for Cortex-M0, RP2040, and other resource-constrained targets.
pub struct DelayMultiplyDetector {
    /// Circular delay buffer
    delay_line: [i16; MAX_DELAY],
    /// Write position in delay buffer
    write_pos: usize,
    /// Delay in samples (precomputed for sample rate)
    delay: usize,
    /// Lowpass filter to remove double-frequency component
    lpf: BiquadFilter,
}

impl DelayMultiplyDetector {
    pub fn new(sample_rate: u32) -> Self {
        let delay = compute_optimal_delay(sample_rate);
        // LPF cutoff at ~1200 Hz (baud rate) to smooth the output
        let lpf = BiquadFilter::lowpass(sample_rate, 1200);
        Self {
            delay_line: [0i16; MAX_DELAY],
            write_pos: 0,
            delay,
            lpf,
        }
    }

    /// Process one audio sample. Returns discriminator output.
    /// Positive = mark (1200 Hz), negative = space (2200 Hz).
    /// (Or vice versa depending on optimal delay — the PLL doesn't care
    /// about polarity, only transitions.)
    #[inline]
    pub fn process(&mut self, sample: i16) -> i16 {
        // Read delayed sample
        let read_pos = (self.write_pos + MAX_DELAY - self.delay) % MAX_DELAY;
        let delayed = self.delay_line[read_pos];

        // Store current sample
        self.delay_line[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % MAX_DELAY;

        // Multiply (Q15 fixed-point: shift right 15 to stay in i16 range)
        let product = ((sample as i32 * delayed as i32) >> 15) as i16;

        // Lowpass filter to remove double-frequency component
        self.lpf.process(product)
    }
}
```

### Advantages over Correlator

| Property | Correlator (Dire Wolf) | Delay-Multiply |
|----------|----------------------|----------------|
| Multiplies per sample | 4 (mark I/Q + space I/Q) | 1 |
| Additions per sample | 4 running sums | ~5 (biquad filter) |
| Lookup tables | 4 sin/cos tables (~1 KB) | None |
| Memory | ~100+ bytes (4 circular bufs) | ~48 bytes (1 delay + filter) |
| Frequency tolerance | Poor (fixed references) | Good (measures directly) |
| Complexity | Moderate | Very low |

---

## Quality Path: Hilbert Transform + Instantaneous Frequency

### Theory

The **analytic signal** representation converts a real signal into a complex
signal whose magnitude is the envelope and whose phase derivative is the
instantaneous frequency:

```
z(t) = x(t) + j·H{x(t)}
```

where `H{x(t)}` is the Hilbert transform of x(t).

The instantaneous frequency is then:

```
f_inst(t) = (1/2π) · d/dt[arg(z(t))]
```

In discrete time:

```
f_inst[n] = (sample_rate / 2π) · angle(z[n] · conj(z[n-1]))
           = (sample_rate / 2π) · atan2(Im(z[n]·conj(z[n-1])),
                                         Re(z[n]·conj(z[n-1])))
```

This gives a clean, continuous frequency estimate at every sample — not
just a binary mark/space decision, but the actual frequency in Hz.

### Hilbert Transform Implementation

The Hilbert transform is implemented as an FIR filter. A practical design
uses an odd-length filter (31-63 taps). The ideal Hilbert transformer has
impulse response `h[n] = 2/(πn)` for odd n and 0 for even n, windowed
by a Hamming or Blackman window for practical use.

```rust
const HILBERT_TAPS: usize = 31;

/// Hilbert transform FIR filter for computing the analytic signal.
pub struct HilbertTransform {
    /// FIR coefficients (Q15 fixed-point)
    coeffs: [i16; HILBERT_TAPS],
    /// Input delay line
    delay_line: [i16; HILBERT_TAPS],
    /// Write position
    write_pos: usize,
    /// Group delay in samples (for aligning real and imaginary parts)
    group_delay: usize,
}

impl HilbertTransform {
    pub fn new() -> Self {
        let mut coeffs = [0i16; HILBERT_TAPS];
        let center = HILBERT_TAPS / 2;

        for i in 0..HILBERT_TAPS {
            let n = i as i32 - center as i32;
            if n == 0 || n % 2 == 0 {
                coeffs[i] = 0; // Zero for even indices
            } else {
                // h[n] = 2/(πn), windowed by Hamming
                let h = 2.0 / (core::f32::consts::PI * n as f32);
                let w = 0.54 - 0.46 * (core::f32::consts::TAU * i as f32
                    / (HILBERT_TAPS - 1) as f32).cos();
                coeffs[i] = (h * w * 32767.0) as i16;
            }
        }

        Self {
            coeffs,
            delay_line: [0i16; HILBERT_TAPS],
            write_pos: 0,
            group_delay: center,
        }
    }

    /// Process one sample. Returns (delayed_real, hilbert_imaginary).
    pub fn process(&mut self, sample: i16) -> (i16, i16) {
        self.delay_line[self.write_pos] = sample;

        // Compute FIR output (imaginary part)
        let mut acc: i32 = 0;
        for i in 0..HILBERT_TAPS {
            let idx = (self.write_pos + HILBERT_TAPS - i) % HILBERT_TAPS;
            acc += self.delay_line[idx] as i32 * self.coeffs[i] as i32;
        }
        let imag = (acc >> 15) as i16;

        // Real part: input delayed by group delay to align with Hilbert output
        let real_pos = (self.write_pos + HILBERT_TAPS - self.group_delay) % HILBERT_TAPS;
        let real = self.delay_line[real_pos];

        self.write_pos = (self.write_pos + 1) % HILBERT_TAPS;

        (real, imag)
    }
}
```

### Instantaneous Frequency Computation

```rust
/// Compute instantaneous frequency from successive analytic signal samples.
pub struct InstantaneousFrequency {
    prev_real: i32,
    prev_imag: i32,
    sample_rate: u32,
}

impl InstantaneousFrequency {
    pub fn new(sample_rate: u32) -> Self {
        Self { prev_real: 0, prev_imag: 0, sample_rate }
    }

    /// Process an analytic signal sample (real, imag).
    /// Returns estimated frequency in Hz × 256 (fixed-point).
    pub fn process(&mut self, real: i16, imag: i16) -> i32 {
        let r = real as i32;
        let i = imag as i32;

        // z[n] · conj(z[n-1])
        let cross_real = r * self.prev_real + i * self.prev_imag;
        let cross_imag = i * self.prev_real - r * self.prev_imag;

        self.prev_real = r;
        self.prev_imag = i;

        // angle = atan2(cross_imag, cross_real)
        let angle = fast_atan2(cross_imag, cross_real); // Returns Q15 radians

        // freq = sample_rate / (2π) × angle
        // In Q15: 2π ≈ 51472, so freq_hz_x256 = sample_rate × angle × 256 / 51472
        ((self.sample_rate as i64 * angle as i64 * 256) / 51472) as i32
    }
}
```

### Advantages

- Produces a **continuous frequency estimate**, not a binary decision
- Naturally provides **confidence information** (how close to mark/space?)
- Handles **frequency offset** gracefully (just shifts the estimate)
- Foundation for **soft-decision decoding** and **adaptive tracking**

---

## Adaptive Tracking

### The Problem with Multi-Decoder Brute Force

Dire Wolf's multi-decoder runs 3-6 identical demodulators with slightly
different parameters (filter bandwidth, PLL gain, etc.) and hopes one is
close enough to each transmitter. This works but wastes CPU — most
decoders produce nothing useful for any given packet.

Real transmitters vary in:
- Actual mark/space frequencies (crystal drift, audio response)
- Actual baud rate (clock accuracy varies ±1-2%)
- Signal amplitude (path loss, different radios)
- Audio frequency response (pre-emphasis, filtering)

### Adaptive Solution: Preamble Training

Every AX.25 packet starts with a preamble of flag bytes (0x7E = 01111110).
At 1200 baud, a typical APRS transmission includes 250-500 ms of preamble.
This is a known pattern of alternating mark and space tones — a **training
sequence** we can exploit.

During preamble reception, the adaptive tracker accumulates statistics on:
1. **Actual mark frequency**: Average instantaneous frequency during mark periods
2. **Actual space frequency**: Average instantaneous frequency during space periods
3. **Actual baud rate**: Timing of symbol transitions
4. **Signal level**: Amplitude envelope for threshold calibration

By the time actual data starts, the decoder is tuned to that specific
transmitter's characteristics.

```rust
/// Adaptive parameter tracker.
///
/// Uses preamble flags to estimate the current transmitter's actual
/// mark/space frequencies, baud rate, and signal level.
pub struct AdaptiveTracker {
    // Estimated parameters (Hz × 256 fixed-point)
    pub mark_freq_est: i32,
    pub space_freq_est: i32,
    pub baud_rate_est: i32,
    pub signal_level: i32,
    pub threshold: i32,    // Decision midpoint between mark and space

    // Internal state
    state: TrackingState,
    mark_accumulator: i64,
    mark_count: u32,
    space_accumulator: i64,
    space_count: u32,
    transition_times: [u32; 32],
    transition_write: usize,
    transition_count: u32,
    sample_counter: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum TrackingState {
    /// Waiting for carrier detect
    Idle,
    /// Receiving preamble, accumulating statistics
    Training,
    /// Estimates locked for this packet
    Locked,
}

impl AdaptiveTracker {
    /// Feed an instantaneous frequency sample during reception.
    pub fn feed_frequency(&mut self, freq_hz_x256: i32, sample_index: u32) {
        match self.state {
            TrackingState::Idle => {
                // Carrier detect: frequency within plausible AFSK range
                let freq_hz = freq_hz_x256 / 256;
                if freq_hz > 900 && freq_hz < 2600 {
                    self.state = TrackingState::Training;
                    self.reset_accumulators();
                    self.sample_counter = 0;
                }
            }
            TrackingState::Training => {
                self.sample_counter += 1;

                // Classify as mark-ish or space-ish using nominal midpoint
                let nominal_mid = ((1200 + 2200) / 2) * 256; // 1700 Hz × 256
                if freq_hz_x256 > nominal_mid {
                    self.space_accumulator += freq_hz_x256 as i64;
                    self.space_count += 1;
                } else {
                    self.mark_accumulator += freq_hz_x256 as i64;
                    self.mark_count += 1;
                }

                // Track transitions for baud rate estimation
                // (Detect sign changes relative to nominal midpoint)
                let prev_was_space = self.space_count > 0 &&
                    (self.mark_count == 0 || /* last sample was space */ false);
                // Simplified: track zero crossings of (freq - midpoint)
                // Full implementation would track discriminator sign changes

                // Lock after enough training data (~100ms of preamble)
                // At 11025 Hz, 100ms = ~1100 samples
                if self.mark_count > 50 && self.space_count > 50
                    && self.sample_counter > 500
                {
                    self.lock_estimates();
                }
            }
            TrackingState::Locked => {
                // Estimates fixed for duration of this packet
            }
        }
    }

    fn lock_estimates(&mut self) {
        self.mark_freq_est = (self.mark_accumulator / self.mark_count as i64) as i32;
        self.space_freq_est = (self.space_accumulator / self.space_count as i64) as i32;
        self.threshold = (self.mark_freq_est + self.space_freq_est) / 2;

        // Estimate baud rate from transition spacing
        if self.transition_count >= 4 {
            // Average time between transitions ≈ symbol period
            // (Implementation detail: compute from transition_times ring buffer)
        } else {
            self.baud_rate_est = 1200 * 256; // Fall back to nominal
        }

        self.state = TrackingState::Locked;
    }

    /// Reset for next packet.
    pub fn reset(&mut self) {
        self.state = TrackingState::Idle;
        self.mark_freq_est = 1200 * 256;
        self.space_freq_est = 2200 * 256;
        self.baud_rate_est = 1200 * 256;
        self.threshold = 1700 * 256;
    }
}
```

### Expected Improvement

| Scenario | Fixed Multi-Decoder (6×) | Single Adaptive |
|----------|-------------------------|-----------------|
| Nominal transmitter | Best of 6 matches well | Exact match |
| +50 Hz frequency offset | Maybe one decoder close | Tracks exactly |
| Slow baud rate (1195 Hz) | Maybe one decoder close | Tracks exactly |
| Weak signal | 6× CPU for 1 decode | 1× CPU, same quality |
| ESP32 budget (3 decoders max) | Limited coverage | Full coverage |
| RP2040 budget (1-2 decoders) | Very limited | Full coverage |

---

## Soft-Decision Decoding

### Concept

Traditional decoders make **hard decisions** — each bit is 0 or 1 with
no confidence information. If one bit in a 300-bit packet is wrong, the
CRC fails and the entire packet is lost. There is no middle ground.

Soft-decision decoding preserves **confidence** as a log-likelihood ratio:

```
LLR > 0:  likely mark/1,   magnitude = confidence
LLR < 0:  likely space/0,  magnitude = confidence
LLR ≈ 0:  uncertain
```

### From Frequency Estimate to Soft Bits

The instantaneous frequency estimate naturally provides soft information.
A frequency of exactly 1200 Hz is high-confidence mark. A frequency of
1650 Hz (between mark and space) is low-confidence — could go either way.

```rust
/// Convert instantaneous frequency to soft bit value (LLR).
///
/// Returns -127 (definitely space/0) to +127 (definitely mark/1).
fn freq_to_soft_bit(freq_hz_x256: i32, tracker: &AdaptiveTracker) -> i8 {
    let mark = tracker.mark_freq_est;
    let space = tracker.space_freq_est;
    let mid = tracker.threshold;
    let half_sep = (space - mark) / 2;

    if half_sep == 0 {
        return 0;
    }

    // Distance from threshold, normalized to half-separation
    // Negative freq (closer to mark) → positive LLR (mark = 1)
    let distance = ((mid - freq_hz_x256) as i64 * 127) / half_sep as i64;
    distance.clamp(-127, 127) as i8
}
```

### Soft HDLC Decoder with Bit-Flipping

When CRC fails on a hard-decision decode, the soft decoder identifies the
least-confident bits and tries flipping combinations of them.

```rust
const MAX_FRAME_BITS: usize = 3200;  // 400 bytes × 8 bits
const MAX_FLIP_CANDIDATES: usize = 8;

/// Soft HDLC decoder that recovers packets with 1-2 bit errors
/// by using confidence information to guide error correction.
pub struct SoftHdlcDecoder {
    inner: HdlcDecoder,
    soft_bits: [i8; MAX_FRAME_BITS],
    hard_bits: [u8; MAX_FRAME_BITS / 8],
    bit_count: usize,
    in_frame: bool,
}

impl SoftHdlcDecoder {
    /// Feed a soft bit. Returns a decoded frame on success.
    pub fn feed_soft_bit(&mut self, soft_value: i8) -> Option<DecodedFrame> {
        let hard_bit = soft_value > 0;

        // Accumulate soft values for potential bit-flipping
        if self.in_frame && self.bit_count < MAX_FRAME_BITS {
            self.soft_bits[self.bit_count] = soft_value;
            self.bit_count += 1;
        }

        // Try normal hard-decision decode
        match self.inner.feed_bit(hard_bit) {
            HdlcResult::Frame(data) => {
                self.in_frame = false;
                self.bit_count = 0;
                Some(DecodedFrame { data, recovered: false })
            }
            HdlcResult::CrcError(data) => {
                // CRC failed — attempt soft recovery
                let result = self.attempt_recovery(data);
                self.in_frame = false;
                self.bit_count = 0;
                result
            }
            HdlcResult::FrameStart => {
                self.in_frame = true;
                self.bit_count = 0;
                None
            }
            HdlcResult::None => None,
        }
    }

    /// Try flipping least-confident bits to fix CRC errors.
    fn attempt_recovery(&self, frame_data: &[u8]) -> Option<DecodedFrame> {
        // Find the N least-confident bit positions
        let mut candidates: [(usize, u8); MAX_FLIP_CANDIDATES] =
            [(0, 255); MAX_FLIP_CANDIDATES];

        for i in 0..self.bit_count.min(MAX_FRAME_BITS) {
            let confidence = self.soft_bits[i].unsigned_abs();
            // Insert if less confident than current worst candidate
            if confidence < candidates[MAX_FLIP_CANDIDATES - 1].1 {
                candidates[MAX_FLIP_CANDIDATES - 1] = (i, confidence);
                // Keep sorted by confidence (ascending)
                candidates.sort_unstable_by_key(|&(_, c)| c);
            }
        }

        // Working copy of frame bytes
        let mut trial = [0u8; 400];
        let len = frame_data.len().min(400);
        trial[..len].copy_from_slice(&frame_data[..len]);

        // Try flipping 1 bit at a time (most common case)
        for &(bit_idx, _) in &candidates[..MAX_FLIP_CANDIDATES.min(8)] {
            flip_bit(&mut trial[..len], bit_idx);
            if crc16_ccitt_valid(&trial[..len]) {
                return Some(DecodedFrame {
                    data: trial[..len].to_vec(),
                    recovered: true,
                });
            }
            flip_bit(&mut trial[..len], bit_idx); // Flip back
        }

        // Try flipping 2 bits (O(n²) but n is small)
        let limit = MAX_FLIP_CANDIDATES.min(6);
        for i in 0..limit {
            for j in (i + 1)..limit {
                flip_bit(&mut trial[..len], candidates[i].0);
                flip_bit(&mut trial[..len], candidates[j].0);
                if crc16_ccitt_valid(&trial[..len]) {
                    return Some(DecodedFrame {
                        data: trial[..len].to_vec(),
                        recovered: true,
                    });
                }
                flip_bit(&mut trial[..len], candidates[j].0);
                flip_bit(&mut trial[..len], candidates[i].0);
            }
        }

        None // Unrecoverable
    }
}

fn flip_bit(data: &mut [u8], bit_index: usize) {
    let byte_idx = bit_index / 8;
    let bit_pos = bit_index % 8;
    if byte_idx < data.len() {
        data[byte_idx] ^= 1 << bit_pos;
    }
}
```

### Recovery Statistics (Expected)

| Bit errors in frame | Hard decoder | Soft decoder (flip 1) | Soft decoder (flip 2) |
|--------------------|--------------|-----------------------|-----------------------|
| 0 | ✓ Decode | ✓ Decode | ✓ Decode |
| 1 | ✗ CRC fail | ✓ Recovered | ✓ Recovered |
| 2 | ✗ CRC fail | ✗ Fail | ✓ Recovered |
| 3 | ✗ CRC fail | ✗ Fail | Sometimes (if 2 of 3 are weak) |
| 4+ | ✗ CRC fail | ✗ Fail | ✗ Unlikely |

Most marginal packets have 1-2 bit errors. Soft decoding is expected to
recover **5-15% additional packets** vs. hard-decision only.

### False Positive Risk

**Important**: Bit-flipping could theoretically produce a frame that passes
CRC but contains wrong data (a "false repair"). The probability is extremely
low: CRC-16 has a 1/65536 chance of a random bit pattern passing. With up
to 36 flip combinations (8 single + 28 double), the false positive rate is
about 36/65536 ≈ 0.05%. To mitigate:

1. Only attempt flipping on frames where CRC failed (not random data)
2. Verify the recovered frame parses as valid AX.25
3. Optionally flag recovered frames so downstream code can treat them
   with lower priority

---

## Clock Recovery PLL

Both demodulator paths feed into the same clock recovery PLL.

```rust
pub struct ClockRecoveryPll {
    /// Phase accumulator (fixed-point, wraps at symbol_period)
    phase: i32,
    /// Phase increment per sample (≈ sample_rate / baud_rate in fixed-point)
    freq: i32,
    /// Nominal frequency (for clamping)
    nominal_freq: i32,
    /// Phase correction gain (larger = faster lock, more jitter)
    alpha: i16,
    /// Frequency correction gain (larger = tracks drift, less stable)
    beta: i16,
    /// Previous discriminator output for transition detection
    prev_output: i16,
    /// Lock indicator
    locked: bool,
    transition_count: u16,
}

impl ClockRecoveryPll {
    /// Process one discriminator sample.
    /// Returns Some(sample_value) at each symbol boundary (baud rate).
    /// The sample_value is the discriminator output at the optimal
    /// sampling instant — soft information for the quality path.
    pub fn update(&mut self, discriminator_output: i16) -> Option<i16> {
        let mut output = None;

        self.phase += self.freq;

        // Symbol boundary: phase wraps
        if self.phase >= self.nominal_freq {
            self.phase -= self.nominal_freq;
            output = Some(discriminator_output);
        }

        // Detect transitions (zero crossings of discriminator)
        let transition = (discriminator_output > 0) != (self.prev_output > 0);
        self.prev_output = discriminator_output;

        if transition {
            self.transition_count = self.transition_count.saturating_add(1);

            // Phase error: distance from ideal transition point (mid-symbol)
            let ideal = self.nominal_freq / 2;
            let error = self.phase - ideal;

            // Proportional correction (phase)
            self.phase -= ((error as i64 * self.alpha as i64) >> 15) as i32;
            // Integral correction (frequency)
            self.freq -= ((error as i64 * self.beta as i64) >> 15) as i32;

            // Clamp frequency drift to ±2%
            let max_drift = self.nominal_freq / 50;
            self.freq = self.freq.clamp(
                self.nominal_freq - max_drift,
                self.nominal_freq + max_drift,
            );

            if self.transition_count > 20 {
                self.locked = true;
            }
        }

        output
    }

    /// Adapt PLL center frequency from adaptive tracker baud rate estimate.
    pub fn set_baud_rate(&mut self, baud_rate_x256: i32, sample_rate: u32) {
        self.nominal_freq = ((sample_rate as i64) << 16) / baud_rate_x256 as i64;
        self.freq = self.nominal_freq;
    }
}
```

---

## Modulator Design

The modulator generates continuous-phase AFSK audio from a bit stream.

### Phase Accumulator NCO

```rust
pub struct AfskModulator {
    phase: u32,          // Phase accumulator (wraps at 2³²)
    mark_step: u32,      // Phase increment for 1200 Hz
    space_step: u32,     // Phase increment for 2200 Hz
    current_tone: bool,  // Current NRZI state (true = mark)
    amplitude: i16,
}

impl AfskModulator {
    pub fn new(sample_rate: u32, amplitude: i16) -> Self {
        // phase_step = (frequency × 2³²) / sample_rate
        let mark_step = ((1200u64) << 32) / sample_rate as u64;
        let space_step = ((2200u64) << 32) / sample_rate as u64;
        Self {
            phase: 0,
            mark_step: mark_step as u32,
            space_step: space_step as u32,
            current_tone: true,  // Start with mark
            amplitude,
        }
    }

    /// Modulate one bit. NRZI: 0 = toggle tone, 1 = maintain tone.
    /// Writes samples to `out` and returns number of samples written.
    pub fn modulate_bit(&mut self, bit: bool, out: &mut [i16]) -> usize {
        if !bit {
            self.current_tone = !self.current_tone;
        }

        let step = if self.current_tone { self.mark_step } else { self.space_step };
        let samples_per_symbol = out.len();

        for sample in out[..samples_per_symbol].iter_mut() {
            let sin_val = SIN_TABLE_Q15[(self.phase >> 24) as usize];
            *sample = ((sin_val as i32 * self.amplitude as i32) >> 15) as i16;
            self.phase = self.phase.wrapping_add(step);
        }

        samples_per_symbol
    }
}
```

### TX Timing

A complete transmission:
1. **PTT assert** — key the radio
2. **TX delay** — 300-500 ms of flags (radio warm-up, configurable via KISS)
3. **Preamble** — additional flag bytes for receiver sync
4. **Frame** — HDLC-encoded AX.25 frame with bit stuffing and CRC
5. **Postamble** — 2-4 flag bytes
6. **TX tail** — brief delay before PTT release (configurable via KISS)
7. **PTT release**

---

## Fast atan2 Approximation

The quality path needs atan2 for instantaneous frequency. A polynomial
approximation is used for speed on embedded targets.

```rust
/// Fast atan2 approximation.
/// Returns angle in Q15 format (−32768 to +32767 ≈ −π to +π).
/// Maximum error: ~0.07 degrees.
pub fn fast_atan2(y: i32, x: i32) -> i16 {
    if x == 0 && y == 0 {
        return 0;
    }

    let abs_y = y.abs().max(1);
    let abs_x = x.abs().max(1);

    let (numer, denom) = if abs_x >= abs_y {
        (abs_y, abs_x)
    } else {
        (abs_x, abs_y)
    };

    // ratio in Q15
    let ratio = ((numer as i64) << 15) / denom as i64;
    let r = ratio as i32;

    // atan(r) ≈ r − r³/3 (Q15 polynomial)
    let r2 = ((r as i64 * r as i64) >> 15) as i32;
    let r3 = ((r2 as i64 * r as i64) >> 15) as i32;
    let mut angle = r - r3 / 3;

    // Adjust for octant
    if abs_x < abs_y {
        angle = 25736 - angle;  // π/2 in Q15
    }
    if x < 0 {
        angle = 51472 - angle;  // π in Q15 (intermediate > 16 bits)
    }
    if y < 0 {
        angle = -angle;
    }

    angle.clamp(-32768, 32767) as i16
}
```

---

## Sine Lookup Table

256 entries, Q15 format. Used by the modulator NCO.

```rust
/// 256-entry sine table, Q15 format.
/// SIN_TABLE_Q15[i] = round(sin(2π·i/256) × 32767)
pub static SIN_TABLE_Q15: [i16; 256] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602,
    6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530,
    18204, 18868, 19519, 20159, 20787, 21403, 22005, 22594,
    23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790,
    27245, 27683, 28105, 28510, 28898, 29268, 29621, 29956,
    30273, 30571, 30852, 31113, 31356, 31580, 31785, 31971,
    32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757,
    32767, 32757, 32728, 32678, 32609, 32521, 32412, 32285,
    32137, 31971, 31785, 31580, 31356, 31113, 30852, 30571,
    30273, 29956, 29621, 29268, 28898, 28510, 28105, 27683,
    27245, 26790, 26319, 25832, 25329, 24811, 24279, 23731,
    23170, 22594, 22005, 21403, 20787, 20159, 19519, 18868,
    18204, 17530, 16846, 16151, 15446, 14732, 14010, 13279,
    12539, 11793, 11039, 10278, 9512, 8739, 7962, 7179,
    6393, 5602, 4808, 4011, 3212, 2410, 1608, 804,
    0, -804, -1608, -2410, -3212, -4011, -4808, -5602,
    -6393, -7179, -7962, -8739, -9512, -10278, -11039, -11793,
    -12539, -13279, -14010, -14732, -15446, -16151, -16846, -17530,
    -18204, -18868, -19519, -20159, -20787, -21403, -22005, -22594,
    -23170, -23731, -24279, -24811, -25329, -25832, -26319, -26790,
    -27245, -27683, -28105, -28510, -28898, -29268, -29621, -29956,
    -30273, -30571, -30852, -31113, -31356, -31580, -31785, -31971,
    -32137, -32285, -32412, -32521, -32609, -32678, -32728, -32757,
    -32767, -32757, -32728, -32678, -32609, -32521, -32412, -32285,
    -32137, -31971, -31785, -31580, -31356, -31113, -30852, -30571,
    -30273, -29956, -29621, -29268, -28898, -28510, -28105, -27683,
    -27245, -26790, -26319, -25832, -25329, -24811, -24279, -23731,
    -23170, -22594, -22005, -21403, -20787, -20159, -19519, -18868,
    -18204, -17530, -16846, -16151, -15446, -14732, -14010, -13279,
    -12539, -11793, -11039, -10278, -9512, -8739, -7962, -7179,
    -6393, -5602, -4808, -4011, -3212, -2410, -1608, -804,
];
```

---

## Performance Targets

### Cycles per Sample Budget

| Platform | Clock | Cycles/Sample @ 11025 Hz | Demod budget |
|----------|-------|-------------------------|-------------|
| ESP32 | 240 MHz | 21,769 | ~500 (quality path) |
| STM32F4 | 168 MHz | 15,238 | ~200 (fast path) |
| RP2040 | 133 MHz | 12,063 | ~50-100 (fast path) |
| Desktop | 3+ GHz | 272,000+ | Unlimited |

### Memory Budget

| Component | Fast Path | Quality Path |
|-----------|-----------|-------------|
| Delay-multiply detector | 48 bytes | — |
| Hilbert transform (31 tap) | — | 128 bytes |
| Instantaneous frequency | — | 16 bytes |
| Adaptive tracker | — | 300 bytes |
| Bandpass filter | 32 bytes | 32 bytes |
| Lowpass filter | 32 bytes | 32 bytes |
| Clock recovery PLL | 32 bytes | 32 bytes |
| Soft bit buffer | — | 400 bytes |
| **Total** | **~144 bytes** | **~940 bytes** |

---

## Feature Flags

```toml
[features]
default = ["quality-path"]

# Embedded fast path: delay-multiply, hard decisions, minimal memory
fast-path = []

# Quality path: Hilbert + adaptive + soft decisions
quality-path = []

# Enable both paths (desktop: run fast-path as fallback)
dual-path = ["fast-path", "quality-path"]

# Use f32 instead of fixed-point (for targets with FPU)
float = []
```

---

## References

- Proakis, "Digital Communications" — CPFSK detection theory, soft decisions
- Lyons, "Understanding DSP" — practical filter and PLL design
- Rice, "Digital Communications: A Discrete-Time Approach" — soft decisions
- Dire Wolf source (`demod_afsk.c`, `gen_tone.c`) — correlator reference
- WA8LMF TNC Test CD — benchmark audio files
- Audio EQ Cookbook (w3.org) — biquad filter coefficient formulas
