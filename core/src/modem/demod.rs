//! AFSK Demodulator — Bell 202 audio to bit stream.
//!
//! Dual-path architecture using Goertzel tone detection + Bresenham symbol timing:
//!
//! **Fast path**: Bandpass → Goertzel mark/space energy → Bresenham timing →
//! NRZI decode → hard bits. Minimal CPU and memory for embedded targets.
//!
//! **Quality path**: Same Goertzel+Bresenham core, plus Hilbert transform →
//! instantaneous frequency → adaptive tracker for LLR confidence values.
//! Feeds `SoftHdlcDecoder` for bit-flip error recovery (1-2 bit corrections).
//!
//! Both paths produce NRZI-decoded bits that feed into the HDLC decoder.
//! The multi-decoder (`multi.rs`) runs multiple fast-path instances with
//! filter and timing diversity for maximum decode performance.

use super::DemodConfig;
use super::filter::BiquadFilter;

// Quality path imports
use super::hilbert::{HilbertTransform, InstFreqDetector, hilbert_31};
use super::adaptive::AdaptiveTracker;

// Delay-multiply path imports
use super::delay_multiply::DelayMultiplyDetector;
use super::pll::ClockRecoveryPll;

/// Demodulated symbol with optional soft information.
#[derive(Clone, Copy, Debug)]
pub struct DemodSymbol {
    /// Hard bit decision: true = 1 (mark), false = 0 (space)
    pub bit: bool,
    /// Soft value / log-likelihood ratio.
    /// +127 = definitely mark, −127 = definitely space.
    /// Only meaningful when using the quality path.
    pub llr: i8,
}

/// Right-shift applied to Goertzel energies before AGC peak tracking/comparison.
/// Raw energies ~1e8–1e11; shifting by 8 gives ~1e0–1e3, keeping cross-products in i64 range.
const AGC_ENERGY_SHIFT: u32 = 8;

/// AGC peak decay rate as a bit shift. Peak decays by 1/2^N per symbol.
/// N=6 → 1.5% decay per symbol, ~50% decay in 40 symbols.
const AGC_DECAY_SHIFT: u32 = 6;

/// Fast-path AFSK demodulator (Goertzel tone detection).
///
/// Suitable for Cortex-M0, RP2040, and other resource-constrained targets.
/// Uses ~200 bytes of RAM and ~30-50 cycles per sample.
///
/// Uses Goertzel filters to compare mark (1200 Hz) and space (2200 Hz)
/// energy over each symbol period with Bresenham-style timing.
///
/// Optional AGC mode (`.with_agc()`) tracks exponential moving averages of
/// mark and space energies and normalizes the decision threshold, compensating
/// for frequency-dependent gain differences such as de-emphasis.
pub struct FastDemodulator {
    #[allow(dead_code)]
    config: DemodConfig,
    bpf: BiquadFilter,
    prev_nrzi_bit: bool,
    samples_processed: u64,
    /// Goertzel state for mark tone (1200 Hz)
    mark_s1: i64,
    mark_s2: i64,
    /// Goertzel state for space tone (2200 Hz)
    space_s1: i64,
    space_s2: i64,
    /// Goertzel coefficients (Q14): 2·cos(2π·f/Fs)
    mark_coeff: i32,
    space_coeff: i32,
    /// Bresenham fractional bit timing
    bit_phase: u32,
    /// Space energy gain in Q8 (256 = 0 dB, 512 = +3 dB energy).
    /// Models Dire Wolf's multi-slicer: different gain levels on space
    /// tone compensate for de-emphasis and varying audio paths.
    space_gain_q8: u16,
    /// Whether AGC (automatic gain control) is enabled.
    agc_enabled: bool,
    /// Leaky-max peak tracker for mark energy (right-shifted by AGC_ENERGY_SHIFT).
    /// Tracks the on-tone energy level for the mark frequency.
    mark_energy_peak: i64,
    /// Leaky-max peak tracker for space energy (right-shifted by AGC_ENERGY_SHIFT).
    /// Tracks the on-tone energy level for the space frequency.
    space_energy_peak: i64,
}

impl FastDemodulator {
    /// Select the appropriate BPF for a given sample rate.
    fn select_bpf(sample_rate: u32) -> BiquadFilter {
        match sample_rate {
            22050 => super::filter::afsk_bandpass_22050(),
            44100 => super::filter::afsk_bandpass_44100(),
            _ => super::filter::afsk_bandpass_11025(),
        }
    }

    /// Create a new fast-path demodulator.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = Self::select_bpf(config.sample_rate);

        let mark_coeff = goertzel_coeff(config.mark_freq, config.sample_rate);
        let space_coeff = goertzel_coeff(config.space_freq, config.sample_rate);

        Self {
            config,
            bpf,
            prev_nrzi_bit: false,
            samples_processed: 0,
            mark_s1: 0,
            mark_s2: 0,
            space_s1: 0,
            space_s2: 0,
            mark_coeff,
            space_coeff,
            bit_phase: 0,
            space_gain_q8: 256,
            agc_enabled: false,
            mark_energy_peak: 1,
            space_energy_peak: 1,
        }
    }

    /// Create with a custom bandpass filter.
    pub fn with_filter(config: DemodConfig, bpf: BiquadFilter) -> Self {
        let mark_coeff = goertzel_coeff(config.mark_freq, config.sample_rate);
        let space_coeff = goertzel_coeff(config.space_freq, config.sample_rate);

        Self {
            config,
            bpf,
            prev_nrzi_bit: false,
            samples_processed: 0,
            mark_s1: 0,
            mark_s2: 0,
            space_s1: 0,
            space_s2: 0,
            mark_coeff,
            space_coeff,
            bit_phase: 0,
            space_gain_q8: 256,
            agc_enabled: false,
            mark_energy_peak: 1,
            space_energy_peak: 1,
        }
    }

    /// Create with custom filter and initial timing offset.
    pub fn with_filter_and_offset(config: DemodConfig, bpf: BiquadFilter, phase_offset: u32) -> Self {
        let mut d = Self::with_filter(config, bpf);
        d.bit_phase = phase_offset;
        d
    }

    /// Create with custom filter, timing offset, and frequency offset.
    ///
    /// The mark/space frequencies are shifted by `freq_offset` Hz, allowing
    /// the decoder to handle transmitters with crystal frequency error.
    pub fn with_filter_freq_and_offset(
        config: DemodConfig,
        bpf: BiquadFilter,
        phase_offset: u32,
        mark_freq: u32,
        space_freq: u32,
    ) -> Self {
        let mark_coeff = goertzel_coeff(mark_freq, config.sample_rate);
        let space_coeff = goertzel_coeff(space_freq, config.sample_rate);

        Self {
            config,
            bpf,
            prev_nrzi_bit: false,
            samples_processed: 0,
            mark_s1: 0,
            mark_s2: 0,
            space_s1: 0,
            space_s2: 0,
            mark_coeff,
            space_coeff,
            bit_phase: phase_offset,
            space_gain_q8: 256,
            agc_enabled: false,
            mark_energy_peak: 1,
            space_energy_peak: 1,
        }
    }

    /// Set space energy gain for multi-slicer diversity.
    ///
    /// Q8 format: 256 = 0 dB (no gain), higher values boost space energy
    /// relative to mark. Used to handle de-emphasized audio where the
    /// space tone (2200 Hz) is weaker than mark (1200 Hz).
    pub fn with_space_gain(mut self, gain_q8: u16) -> Self {
        self.space_gain_q8 = gain_q8;
        self
    }

    /// Enable AGC (Automatic Gain Control).
    ///
    /// Tracks exponential moving averages of mark and space Goertzel energies
    /// and normalizes the bit decision by cross-multiplying, so that
    /// frequency-dependent gain differences (e.g. de-emphasis) are compensated
    /// without needing a fixed gain parameter.
    pub fn with_agc(mut self) -> Self {
        self.agc_enabled = true;
        self
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        self.bpf.reset();
        self.prev_nrzi_bit = false;
        self.samples_processed = 0;
        self.mark_s1 = 0;
        self.mark_s2 = 0;
        self.space_s1 = 0;
        self.space_s2 = 0;
        self.bit_phase = 0;
        self.mark_energy_peak = 1;
        self.space_energy_peak = 1;
    }

    /// Process a buffer of audio samples.
    ///
    /// Decoded symbols are written to `symbols_out`. Returns the number
    /// of symbols produced.
    pub fn process_samples(
        &mut self,
        samples: &[i16],
        symbols_out: &mut [DemodSymbol],
    ) -> usize {
        let mut sym_count = 0;
        let sample_rate = self.config.sample_rate;
        let baud_rate = self.config.baud_rate;

        for &sample in samples {
            self.samples_processed += 1;

            // 1. Bandpass filter
            let filtered = self.bpf.process(sample);
            let s = filtered as i64;

            // 2. Goertzel iteration for mark and space
            let mark_s0 = s + ((self.mark_coeff as i64 * self.mark_s1) >> 14) - self.mark_s2;
            self.mark_s2 = self.mark_s1;
            self.mark_s1 = mark_s0;

            let space_s0 = s + ((self.space_coeff as i64 * self.space_s1) >> 14) - self.space_s2;
            self.space_s2 = self.space_s1;
            self.space_s1 = space_s0;

            // 3. Bresenham symbol timing
            self.bit_phase += baud_rate;
            if self.bit_phase >= sample_rate {
                self.bit_phase -= sample_rate;

                // 4. Goertzel energy comparison for hard bit decision
                let mark_energy = self.mark_s1 * self.mark_s1
                    + self.mark_s2 * self.mark_s2
                    - ((self.mark_coeff as i64 * self.mark_s1 * self.mark_s2) >> 14);
                let space_energy = self.space_s1 * self.space_s1
                    + self.space_s2 * self.space_s2
                    - ((self.space_coeff as i64 * self.space_s1 * self.space_s2) >> 14);

                // Bit decision: AGC normalizes by tracked peak levels,
                // otherwise apply static space gain (multi-slicer).
                let raw_bit = if self.agc_enabled {
                    let mark_s = mark_energy >> AGC_ENERGY_SHIFT;
                    let space_s = space_energy >> AGC_ENERGY_SHIFT;
                    // Leaky-max peak tracker: captures on-tone energy level
                    // for each frequency, decays slowly when off-tone.
                    if mark_s > self.mark_energy_peak {
                        self.mark_energy_peak = mark_s;
                    } else {
                        self.mark_energy_peak -= self.mark_energy_peak >> AGC_DECAY_SHIFT;
                    }
                    if space_s > self.space_energy_peak {
                        self.space_energy_peak = space_s;
                    } else {
                        self.space_energy_peak -= self.space_energy_peak >> AGC_DECAY_SHIFT;
                    }
                    let m_peak = self.mark_energy_peak.max(1);
                    let s_peak = self.space_energy_peak.max(1);
                    // Cross-multiply: mark/mark_peak > space/space_peak
                    mark_s * s_peak > space_s * m_peak
                } else {
                    mark_energy * 256 > space_energy * (self.space_gain_q8 as i64)
                };

                // 5. NRZI decode
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                if sym_count < symbols_out.len() {
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr: if decoded_bit { 64 } else { -64 },
                    };
                    sym_count += 1;
                }

                // Reset Goertzel state for next symbol
                self.mark_s1 = 0;
                self.mark_s2 = 0;
                self.space_s1 = 0;
                self.space_s2 = 0;
            }
        }

        sym_count
    }
}

/// Compute Goertzel coefficient for a given frequency: 2·cos(2π·f/Fs) in Q14.
fn goertzel_coeff(freq: u32, sample_rate: u32) -> i32 {
    // Using the lookup-based approach for common frequencies to avoid
    // floating-point at runtime.
    // For other frequencies, we precompute at initialization time.
    match (freq, sample_rate) {
        (1200, 11025) => 25328,  // 2·cos(2π·1200/11025) × 16384
        (2200, 11025) => 10126,  // 2·cos(2π·2200/11025) × 16384
        (1200, 22050) => 30870,  // 2·cos(2π·1200/22050) × 16384
        (2200, 22050) => 26537,  // 2·cos(2π·2200/22050) × 16384
        (1200, 44100) => 32290,  // 2·cos(2π·1200/44100) × 16384
        (2200, 44100) => 31171,  // 2·cos(2π·2200/44100) × 16384
        _ => {
            // Approximate using integer arithmetic.
            // For unsupported rates, fall back to a rough calculation.
            // 2·cos(2π·f/Fs) in Q14
            // This path is only called at init, so a simple approximation is OK.
            #[cfg(feature = "std")]
            {
                let w = 2.0 * core::f64::consts::PI * freq as f64 / sample_rate as f64;
                (2.0 * libm::cos(w) * 16384.0) as i32
            }
            #[cfg(not(feature = "std"))]
            {
                // Rough approximation; add more entries to the match above
                // for production use on no_std targets.
                0
            }
        }
    }
}

/// Quality-path AFSK demodulator (Hilbert + adaptive + soft decisions).
///
/// Suitable for desktop, Raspberry Pi, ESP32. Uses ~1 KB of RAM and
/// ~100-200 cycles per sample, but produces significantly better decode
/// performance through adaptive tracking and soft-decision information.
pub struct QualityDemodulator {
    #[allow(dead_code)]
    config: DemodConfig,
    bpf: BiquadFilter,
    hilbert: HilbertTransform<31>,
    inst_freq: InstFreqDetector,
    tracker: AdaptiveTracker,
    prev_nrzi_bit: bool,
    samples_processed: u64,
    sample_index: u32,
    /// Goertzel state for mark/space energy (used for hard decision)
    mark_s1: i64,
    mark_s2: i64,
    space_s1: i64,
    space_s2: i64,
    mark_coeff: i32,
    space_coeff: i32,
    /// Bresenham fractional bit timing
    bit_phase: u32,
    /// Accumulated frequency estimate over symbol period
    freq_accum: i64,
    freq_count: u32,
}

impl QualityDemodulator {
    /// Create a new quality-path demodulator.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = match config.sample_rate {
            22050 => super::filter::afsk_bandpass_22050(),
            44100 => super::filter::afsk_bandpass_44100(),
            _ => super::filter::afsk_bandpass_11025(),
        };
        let hilbert = hilbert_31();
        let inst_freq = InstFreqDetector::new(config.sample_rate);
        let tracker = AdaptiveTracker::new(config.sample_rate);
        let mark_coeff = goertzel_coeff(config.mark_freq, config.sample_rate);
        let space_coeff = goertzel_coeff(config.space_freq, config.sample_rate);

        Self {
            config,
            bpf,
            hilbert,
            inst_freq,
            tracker,
            prev_nrzi_bit: false,
            samples_processed: 0,
            sample_index: 0,
            mark_s1: 0,
            mark_s2: 0,
            space_s1: 0,
            space_s2: 0,
            mark_coeff,
            space_coeff,
            bit_phase: 0,
            freq_accum: 0,
            freq_count: 0,
        }
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        self.bpf.reset();
        self.hilbert.reset();
        self.inst_freq.reset();
        self.tracker.reset();
        self.prev_nrzi_bit = false;
        self.samples_processed = 0;
        self.sample_index = 0;
        self.mark_s1 = 0;
        self.mark_s2 = 0;
        self.space_s1 = 0;
        self.space_s2 = 0;
        self.bit_phase = 0;
        self.freq_accum = 0;
        self.freq_count = 0;
    }

    /// Process a buffer of audio samples.
    ///
    /// Decoded symbols include soft (confidence) information that can be
    /// used by the SoftHdlcDecoder for bit-flip error recovery.
    pub fn process_samples(
        &mut self,
        samples: &[i16],
        symbols_out: &mut [DemodSymbol],
    ) -> usize {
        let mut sym_count = 0;
        let sample_rate = self.config.sample_rate;
        let baud_rate = self.config.baud_rate;

        for &sample in samples {
            self.samples_processed += 1;
            self.sample_index = self.sample_index.wrapping_add(1);

            // 1. Bandpass filter
            let filtered = self.bpf.process(sample);
            let s = filtered as i64;

            // 2. Goertzel iteration for mark/space energy
            let mark_s0 = s + ((self.mark_coeff as i64 * self.mark_s1) >> 14) - self.mark_s2;
            self.mark_s2 = self.mark_s1;
            self.mark_s1 = mark_s0;

            let space_s0 = s + ((self.space_coeff as i64 * self.space_s1) >> 14) - self.space_s2;
            self.space_s2 = self.space_s1;
            self.space_s1 = space_s0;

            // 3. Hilbert transform → instantaneous frequency (for soft decisions)
            let (real, imag) = self.hilbert.process(filtered);
            let freq_fp = self.inst_freq.process(real, imag);
            self.tracker.feed(freq_fp, self.sample_index);
            self.freq_accum += freq_fp as i64;
            self.freq_count += 1;

            // 4. Bresenham symbol timing
            self.bit_phase += baud_rate;
            if self.bit_phase >= sample_rate {
                self.bit_phase -= sample_rate;

                // 5. Goertzel energy comparison for hard bit decision
                let mark_energy = self.mark_s1 * self.mark_s1
                    + self.mark_s2 * self.mark_s2
                    - ((self.mark_coeff as i64 * self.mark_s1 * self.mark_s2) >> 14);
                let space_energy = self.space_s1 * self.space_s1
                    + self.space_s2 * self.space_s2
                    - ((self.space_coeff as i64 * self.space_s1 * self.space_s2) >> 14);

                let raw_bit = mark_energy > space_energy;

                // 6. Generate LLR from Goertzel energy ratio
                // This provides natural confidence variation: symbols where
                // mark and space energies are similar get low confidence,
                // enabling SoftHdlcDecoder to identify bits to flip.
                let total = mark_energy + space_energy;
                let energy_llr = if total > 0 {
                    let ratio = ((mark_energy - space_energy) * 127) / total;
                    ratio.clamp(-127, 127) as i8
                } else {
                    0i8
                };

                // 7. NRZI decode
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                let confidence = energy_llr.unsigned_abs().max(1);
                let decoded_llr = if decoded_bit { confidence as i8 } else { -(confidence as i8) };

                if sym_count < symbols_out.len() {
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr: decoded_llr,
                    };
                    sym_count += 1;
                }

                // Reset Goertzel and frequency accumulator for next symbol
                self.mark_s1 = 0;
                self.mark_s2 = 0;
                self.space_s1 = 0;
                self.space_s2 = 0;
                self.freq_accum = 0;
                self.freq_count = 0;

            }
        }

        sym_count
    }

    /// Access the adaptive tracker (for diagnostics / testing).
    pub fn tracker(&self) -> &AdaptiveTracker {
        &self.tracker
    }

    /// Check if the tracker has locked onto a signal.
    pub fn is_tracking(&self) -> bool {
        self.tracker.is_locked()
    }
}

/// Delay-Multiply demodulator — continuous discriminator + integrate-and-dump.
///
/// Pipeline: [Pre-emph →] [BPF →] Delay-Multiply [→ LPF] → Accumulate →
///           {Bresenham | PLL} → [Adaptive Threshold →] NRZI → HDLC
///
/// Uses a delay discriminator for continuous frequency detection. The
/// discriminator output is accumulated (integrated) over each symbol period
/// and a hard bit decision is made from the accumulator polarity.
///
/// **Timing recovery**: Either fixed-rate Bresenham (default) or PLL clock
/// recovery (`with_bpf_pll()`). PLL adapts to transmitter baud rate drift.
///
/// **Adaptive threshold**: Leaky-max peak tracker for mark and space
/// discriminator output compensates for de-emphasis amplitude imbalance.
///
/// **Pre-emphasis**: 1st-order high-pass filter before BPF compensates for
/// de-emphasis roll-off (space tone at 2200 Hz attenuated relative to mark).
pub struct DmDemodulator {
    #[allow(dead_code)]
    config: DemodConfig,
    /// Optional bandpass filter (for real-world signals)
    bpf: Option<BiquadFilter>,
    detector: DelayMultiplyDetector,
    prev_nrzi_bit: bool,
    samples_processed: u64,
    /// Bresenham fractional bit timing (used when pll is None)
    bit_phase: u32,
    /// PLL clock recovery (replaces Bresenham when Some)
    pll: Option<ClockRecoveryPll>,
    /// Accumulated discriminator output over current symbol period
    accumulator: i64,
    /// Number of samples accumulated in current symbol (for normalization)
    accum_count: u32,
    /// True when mark tone produces negative discriminator output.
    /// Depends on the delay value and sample rate.
    mark_is_negative: bool,
    /// Adaptive threshold: leaky-max peak of mark (positive) accumulator values
    mark_peak: i64,
    /// Adaptive threshold: leaky-max peak of space (negative) accumulator values
    space_peak: i64,
    /// Decision threshold: midpoint of mark_peak and space_peak
    adaptive_threshold: i64,
    /// Whether adaptive threshold is enabled
    use_adaptive: bool,
    /// Symbols processed (delays adaptive threshold activation)
    symbol_count: u32,
    /// Pre-emphasis coefficient in Q15. y[n] = x[n] - alpha * x[n-1].
    /// 0 = disabled. ~31130 (0.95) for standard de-emphasis compensation.
    preemph_alpha_q15: i16,
    /// Previous sample for pre-emphasis filter
    preemph_prev: i16,
    /// Leaky integrator for PLL input — smooths per-sample disc_out so the
    /// PLL's transition detector sees clean mark↔space crossings instead of
    /// noisy per-sample values. Decay shift of 3 gives ~8 sample window
    /// (≈1 symbol at 11025/1200).
    pll_leaky: i64,
}

impl DmDemodulator {
    /// Select delay optimized for DM demodulation.
    ///
    /// Short delays minimize transition artifacts (~16-22% of symbol period),
    /// which is critical for detecting single-symbol tones like the space in
    /// a flag pattern (MMMMMMMS). All chosen delays give mark→positive,
    /// space→negative polarity.
    fn dm_delay(sample_rate: u32) -> usize {
        match sample_rate {
            11025 => 2,  // 181 μs: mark→+0.20, space→−0.81, 2/9=22%
            22050 => 3,  // 136 μs: mark→+0.52, space→−0.30, 3/18=16%
            44100 => 7,  // 159 μs: mark→+0.37, space→−0.58, 7/37=19%
            48000 => 8,  // 167 μs: mark→+0.31, space→−0.67, 8/40=20%
            _ => {
                let approx = sample_rate / 6000;
                if approx < 1 { 1 }
                else if approx >= super::MAX_DELAY as u32 { super::MAX_DELAY - 1 }
                else { approx as usize }
            }
        }
    }

    /// Determine if mark tone produces negative discriminator output.
    ///
    /// Based on cos(2π·1200·delay/sample_rate): negative means mark→negative.
    /// Precomputed for common configurations; uses libm when available.
    fn is_mark_negative(delay: usize, sample_rate: u32) -> bool {
        // cos(2π·1200·d/fs) < 0 when 2π·1200·d/fs is in (π/2, 3π/2).
        // That means d/fs > 1/(4·1200) and d/fs < 3/(4·1200),
        // i.e., d > fs/4800 and d < 3·fs/4800.
        // For common rates:
        match (delay, sample_rate) {
            // dm_delay (short, clean signals): all mark→positive
            (1, _) | (2, 11025) | (3, 22050) | (7, 44100) | (8, 48000) => false,
            // dm_delay_filtered (long, real-world): all mark→positive
            (8, 11025) | (16, 22050) | (31, 44100) | (31, 48000) => false,
            // d=5 at 11025 (alt delay): mark→negative
            (5, 11025) | (10, 22050) | (20, 44100) => true,
            // General case: approximate using integer check
            _ => {
                #[cfg(feature = "std")]
                {
                    let tau = delay as f64 / sample_rate as f64;
                    libm::cos(2.0 * core::f64::consts::PI * 1200.0 * tau) < 0.0
                }
                #[cfg(not(feature = "std"))]
                {
                    // d/fs > 1/4800 and d/fs < 3/4800
                    // d * 4800 > fs and d * 4800 < 3 * fs
                    let prod = delay as u64 * 4800;
                    prod > sample_rate as u64 && prod < 3 * sample_rate as u64
                }
            }
        }
    }

    /// Optimal delay for real-world signals (with BPF+LPF).
    ///
    /// Uses τ ≈ 726 μs (delay ≈ samples_per_symbol), which gives good
    /// mark/space separation with BPF+LPF smoothing transition artifacts.
    /// This delay works because the LPF removes the double-frequency component,
    /// leaving a stable discriminator output over each symbol.
    fn dm_delay_filtered(sample_rate: u32) -> usize {
        match sample_rate {
            11025 => 8,   // 726 μs: mark→+0.66, space→−0.85, sep=1.51
            22050 => 16,  // 726 μs: same τ
            44100 => 31,  // 703 μs: near MAX_DELAY limit
            48000 => 31,  // 646 μs: near MAX_DELAY limit
            _ => {
                // τ ≈ 1/baud_rate ≈ 833μs → delay ≈ sample_rate / 1200
                let d = sample_rate as usize / 1400; // slightly shorter than 1 symbol
                d.clamp(1, super::MAX_DELAY - 1)
            }
        }
    }

    fn make(config: DemodConfig, phase_offset: u32, use_bpf: bool) -> Self {
        let delay = if use_bpf {
            Self::dm_delay_filtered(config.sample_rate)
        } else {
            Self::dm_delay(config.sample_rate)
        };
        let lpf = if use_bpf {
            super::filter::post_detect_lpf(config.sample_rate)
        } else {
            BiquadFilter::passthrough()
        };
        let detector = DelayMultiplyDetector::with_delay(delay, lpf);

        let bpf = if use_bpf {
            Some(match config.sample_rate {
                22050 => super::filter::afsk_bandpass_22050(),
                44100 => super::filter::afsk_bandpass_44100(),
                _ => super::filter::afsk_bandpass_11025(),
            })
        } else {
            None
        };

        let mark_is_negative = Self::is_mark_negative(delay, config.sample_rate);

        Self {
            config,
            bpf,
            detector,
            prev_nrzi_bit: false,
            samples_processed: 0,
            bit_phase: phase_offset,
            pll: None,
            accumulator: 0,
            accum_count: 0,
            mark_is_negative,
            mark_peak: 0,
            space_peak: 0,
            adaptive_threshold: 0,
            use_adaptive: false,
            symbol_count: 0,
            preemph_alpha_q15: 0,
            preemph_prev: 0,
            pll_leaky: 0,
        }
    }

    /// Create with BPF + PLL clock recovery for real-world signals.
    ///
    /// Uses the filtered delay (d=8 at 11025 Hz) with BPF+LPF preprocessing
    /// for clean discriminator output, and PLL for adaptive symbol timing.
    /// Uses alpha-only phase correction (beta=0) because the leaky integrator's
    /// group delay biases frequency correction.
    pub fn with_bpf_pll(config: DemodConfig) -> Self {
        Self::make_pll(config, config.pll_alpha, 0, 0)
    }

    /// Create with BPF + PLL and custom loop bandwidth.
    pub fn with_bpf_pll_custom(config: DemodConfig, alpha: i16, beta: i16) -> Self {
        Self::make_pll(config, alpha, beta, 0)
    }

    /// Set PLL transition hysteresis threshold.
    ///
    /// When > 0, the PLL only detects transitions when the discriminator
    /// crosses from below `-threshold` to above `+threshold` (or vice versa).
    /// Prevents false transitions from noise near zero crossing.
    pub fn with_pll_hysteresis(mut self, threshold: i16) -> Self {
        if let Some(ref mut pll) = self.pll {
            // Rebuild PLL with hysteresis — the PLL's with_hysteresis is
            // a builder that consumed self, so we set it directly.
            pll.set_hysteresis(threshold);
        }
        self
    }

    fn make_pll(config: DemodConfig, alpha: i16, beta: i16, hysteresis: i16) -> Self {
        let delay = Self::dm_delay_filtered(config.sample_rate);
        let lpf = super::filter::post_detect_lpf(config.sample_rate);
        let detector = DelayMultiplyDetector::with_delay(delay, lpf);
        let bpf = Some(match config.sample_rate {
            22050 => super::filter::afsk_bandpass_22050(),
            44100 => super::filter::afsk_bandpass_44100(),
            _ => super::filter::afsk_bandpass_11025(),
        });
        let mark_is_negative = Self::is_mark_negative(delay, config.sample_rate);
        let pll = ClockRecoveryPll::new(config.sample_rate, config.baud_rate, alpha, beta)
            .with_hysteresis(hysteresis);

        Self {
            config,
            bpf,
            detector,
            prev_nrzi_bit: false,
            samples_processed: 0,
            bit_phase: 0,
            pll: Some(pll),
            accumulator: 0,
            accum_count: 0,
            mark_is_negative,
            mark_peak: 0,
            space_peak: 0,
            adaptive_threshold: 0,
            use_adaptive: false,
            symbol_count: 0,
            preemph_alpha_q15: 0,
            preemph_prev: 0,
            pll_leaky: 0,
        }
    }

    /// Enable adaptive discriminator threshold.
    ///
    /// Tracks peak positive (mark) and negative (space) discriminator accumulator
    /// values using leaky-max trackers, and sets the decision threshold at their
    /// midpoint. Compensates for de-emphasis amplitude imbalance where space
    /// (2200 Hz) is weaker than mark (1200 Hz).
    pub fn with_adaptive(mut self) -> Self {
        self.use_adaptive = true;
        self
    }

    /// Enable pre-emphasis filter before BPF.
    ///
    /// Applies `y[n] = x[n] - alpha * x[n-1]` to boost high frequencies,
    /// compensating for de-emphasis roll-off. `alpha_q15` is in Q15 format:
    /// - 31130 ≈ 0.95: standard 6 dB/octave de-emphasis compensation
    /// - 29491 ≈ 0.90: moderate compensation
    /// - 0: disabled (default)
    pub fn with_preemph(mut self, alpha_q15: i16) -> Self {
        self.preemph_alpha_q15 = alpha_q15;
        self
    }

    /// Create a new delay-multiply demodulator (no BPF — for clean signals).
    pub fn new(config: DemodConfig) -> Self {
        Self::make(config, 0, false)
    }

    /// Create with BPF and LPF for real-world signals.
    pub fn with_bpf(config: DemodConfig) -> Self {
        Self::make(config, 0, true)
    }

    /// Create with a timing offset (for multi-decoder diversity).
    pub fn with_offset(config: DemodConfig, phase_offset: u32) -> Self {
        Self::make(config, phase_offset, false)
    }

    /// Create with BPF and a timing offset.
    pub fn with_bpf_and_offset(config: DemodConfig, phase_offset: u32) -> Self {
        Self::make(config, phase_offset, true)
    }

    /// Create with BPF, custom delay, and timing offset.
    ///
    /// For multi-decoder diversity: different delays decode different frames.
    pub fn with_bpf_delay_and_offset(config: DemodConfig, delay: usize, phase_offset: u32) -> Self {
        let lpf = super::filter::post_detect_lpf(config.sample_rate);
        let detector = DelayMultiplyDetector::with_delay(delay, lpf);
        let bpf = Some(match config.sample_rate {
            22050 => super::filter::afsk_bandpass_22050(),
            44100 => super::filter::afsk_bandpass_44100(),
            _ => super::filter::afsk_bandpass_11025(),
        });
        Self {
            config,
            bpf,
            detector,
            prev_nrzi_bit: false,
            samples_processed: 0,
            bit_phase: phase_offset,
            pll: None,
            accumulator: 0,
            accum_count: 0,
            mark_is_negative: Self::is_mark_negative(delay, config.sample_rate),
            mark_peak: 0,
            space_peak: 0,
            adaptive_threshold: 0,
            use_adaptive: false,
            symbol_count: 0,
            preemph_alpha_q15: 0,
            preemph_prev: 0,
            pll_leaky: 0,
        }
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        if let Some(ref mut bpf) = self.bpf {
            bpf.reset();
        }
        self.detector.reset();
        self.prev_nrzi_bit = false;
        self.samples_processed = 0;
        self.bit_phase = 0;
        if let Some(ref mut pll) = self.pll {
            pll.reset();
        }
        self.accumulator = 0;
        self.accum_count = 0;
        self.mark_peak = 0;
        self.space_peak = 0;
        self.adaptive_threshold = 0;
        self.symbol_count = 0;
        self.preemph_prev = 0;
        self.pll_leaky = 0;
    }

    /// Process a buffer of audio samples.
    ///
    /// Decoded symbols are written to `symbols_out`. Returns the number
    /// of symbols produced.
    pub fn process_samples(
        &mut self,
        samples: &[i16],
        symbols_out: &mut [DemodSymbol],
    ) -> usize {
        let mut sym_count = 0;
        let sample_rate = self.config.sample_rate;
        let baud_rate = self.config.baud_rate;
        let use_pll = self.pll.is_some();

        for &sample in samples {
            self.samples_processed += 1;

            // 0. Optional pre-emphasis: y[n] = x[n] - alpha * x[n-1]
            // Boosts high frequencies to compensate for de-emphasis roll-off.
            let preemph_out = if self.preemph_alpha_q15 != 0 {
                let y = sample as i32
                    - ((self.preemph_alpha_q15 as i32 * self.preemph_prev as i32) >> 15);
                self.preemph_prev = sample;
                y.clamp(-32768, 32767) as i16
            } else {
                sample
            };

            // 1. Optional BPF, then delay-multiply discriminator
            let filtered = if let Some(ref mut bpf) = self.bpf {
                bpf.process(preemph_out)
            } else {
                preemph_out
            };
            let disc_out = self.detector.process(filtered);

            // 2. Accumulate discriminator output over symbol period
            self.accumulator += disc_out as i64;
            self.accum_count += 1;

            // 3. Timing recovery: Bresenham (fixed-rate) or PLL (adaptive)
            let symbol_boundary = if use_pll {
                // Leaky integrator smooths per-sample disc_out for PLL.
                // Raw disc_out noise causes false transitions; the leaky
                // integrator provides ~1 symbol window of smoothing for
                // clean transition detection.
                // NOTE: The group delay from this filter biases the PLL's
                // frequency (beta) correction. Use beta=0 (alpha-only).
                const PLL_LEAK_SHIFT: u32 = 3; // Decay by 1/8 per sample
                self.pll_leaky -= self.pll_leaky >> PLL_LEAK_SHIFT;
                self.pll_leaky += disc_out as i64;
                let pll_input = self.pll_leaky.clamp(-32000, 32000) as i16;
                self.pll.as_mut().unwrap().update(pll_input).is_some()
            } else {
                self.bit_phase += baud_rate;
                if self.bit_phase >= sample_rate {
                    self.bit_phase -= sample_rate;
                    true
                } else {
                    false
                }
            };

            if symbol_boundary {
                // 4. Update adaptive threshold trackers (leaky-max peak)
                if self.use_adaptive {
                    // Normalize accumulator by sample count so PLL phase
                    // corrections (which vary symbol length) don't affect peaks.
                    let norm = if self.accum_count > 0 {
                        self.accumulator / self.accum_count as i64
                    } else {
                        self.accumulator
                    };
                    // Track mark peaks (positive discriminator mean)
                    if norm > self.mark_peak {
                        self.mark_peak = norm;
                    } else {
                        self.mark_peak -= self.mark_peak >> AGC_DECAY_SHIFT;
                    }
                    // Track space peaks (negative discriminator mean)
                    if norm < self.space_peak {
                        self.space_peak = norm;
                    } else {
                        self.space_peak -= self.space_peak >> AGC_DECAY_SHIFT;
                    }
                    self.symbol_count += 1;
                    if self.symbol_count > 20 {
                        self.adaptive_threshold =
                            (self.mark_peak + self.space_peak) / 2;
                    }
                }

                // 5. Hard bit decision — polarity depends on delay
                // Adaptive AGC: rescale the weaker-side accumulator so mark
                // and space have equal magnitudes, then compare against 0.
                // This compensates for de-emphasis without shifting the
                // threshold (which would misclassify transition symbols).
                //
                // mark_peak tracks positive accumulator peaks.
                // space_peak tracks negative accumulator peaks.
                // The weaker side has the smaller absolute peak.
                let decision_val = if self.use_adaptive
                    && self.symbol_count > 20
                    && self.mark_peak > 0
                    && self.space_peak < 0
                {
                    let pos_mag = self.mark_peak;
                    let neg_mag = -self.space_peak;
                    if pos_mag > neg_mag && self.accumulator < 0 && neg_mag > 0 {
                        // Negative (space) side is weaker — scale it up
                        self.accumulator * pos_mag / neg_mag
                    } else if neg_mag > pos_mag && self.accumulator > 0 && pos_mag > 0 {
                        // Positive (mark) side is weaker — scale it up
                        self.accumulator * neg_mag / pos_mag
                    } else {
                        self.accumulator
                    }
                } else {
                    self.accumulator
                };
                let raw_bit = if self.mark_is_negative {
                    decision_val < 0
                } else {
                    decision_val > 0
                };

                // 6. NRZI decode: same as previous → 1, different → 0
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                if sym_count < symbols_out.len() {
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr: if decoded_bit { 64 } else { -64 },
                    };
                    sym_count += 1;
                }

                // Reset accumulator for next symbol
                self.accumulator = 0;
                self.accum_count = 0;
            }
        }

        sym_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_demod_creation() {
        let config = DemodConfig::default_1200();
        let demod = FastDemodulator::new(config);
        assert_eq!(demod.samples_processed, 0);
    }

    #[test]
    fn test_quality_demod_creation() {
        let config = DemodConfig::default_1200();
        let demod = QualityDemodulator::new(config);
        assert_eq!(demod.samples_processed, 0);
        assert!(!demod.is_tracking());
    }

    #[test]
    fn test_fast_demod_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut demod = FastDemodulator::new(config);
        let silence = [0i16; 1000];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 200];

        let n = demod.process_samples(&silence, &mut symbols);
        // Silence should produce some symbols (PLL runs, but data is garbage)
        // Key test: no panics, no overflow
        assert!(n < 200);
    }

    #[test]
    fn test_quality_demod_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut demod = QualityDemodulator::new(config);
        let silence = [0i16; 1000];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 200];

        let n = demod.process_samples(&silence, &mut symbols);
        assert!(n < 200);
    }

    #[test]
    fn test_fast_demod_reset() {
        let config = DemodConfig::default_1200();
        let mut demod = FastDemodulator::new(config);
        let noise = [1000i16; 100];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 50];

        demod.process_samples(&noise, &mut symbols);
        demod.reset();
        assert_eq!(demod.samples_processed, 0);
    }

    // ── Full pipeline loopback tests ────────────────────────────────

    /// Diagnostic test: Goertzel mark/space energy detection.
    #[test]
    fn test_loopback_diagnostic() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Test");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        // Modulate with preamble
        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;
        for _ in 0..30 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        for i in 0..encoded.bit_count {
            let n = modulator.modulate_bit(encoded.bits[i] != 0, &mut audio[audio_len..]);
            audio_len += n;
        }

        // Add trailing silence so the last symbol boundary fires
        audio_len += 20;

        // === Goertzel mark/space detector + fixed-rate Bresenham ===
        // Goertzel coefficient: coeff = 2·cos(2π·f/Fs)
        // For mark (1200 Hz): 2·cos(2π·1200/11025) = 2·cos(0.6838) = 2·0.7732 = 1.5464
        // For space (2200 Hz): 2·cos(2π·2200/11025) = 2·cos(1.2538) = 2·0.3090 = 0.6180
        // In Q14 (×16384):
        let mark_coeff: i32 = 25328;  // 1.5464 × 16384
        let space_coeff: i32 = 10126; // 0.6180 × 16384

        let mut mark_s1: i64 = 0;
        let mut mark_s2: i64 = 0;
        let mut space_s1: i64 = 0;
        let mut space_s2: i64 = 0;

        let mut prev_nrzi = false;
        let mut decoder = HdlcDecoder::new();
        let mut frame_found = false;
        let mut flag_count: u32 = 0;
        let mut shift_reg: u8 = 0;
        // Fixed-rate Bresenham
        let sample_rate: u32 = 11025;
        let baud_rate: u32 = 1200;
        let mut bit_phase: u32 = 0;

        for i in 0..audio_len {
            let s = audio[i] as i64;

            // Goertzel iteration for mark
            let mark_s0 = s + ((mark_coeff as i64 * mark_s1) >> 14) - mark_s2;
            mark_s2 = mark_s1;
            mark_s1 = mark_s0;

            // Goertzel iteration for space
            let space_s0 = s + ((space_coeff as i64 * space_s1) >> 14) - space_s2;
            space_s2 = space_s1;
            space_s1 = space_s0;

            bit_phase += baud_rate;
            if bit_phase >= sample_rate {
                bit_phase -= sample_rate;

                // Compute energy: |X(k)|² = s1² + s2² - coeff·s1·s2
                let mark_energy = mark_s1 * mark_s1 + mark_s2 * mark_s2
                    - ((mark_coeff as i64 * mark_s1 * mark_s2) >> 14);
                let space_energy = space_s1 * space_s1 + space_s2 * space_s2
                    - ((space_coeff as i64 * space_s1 * space_s2) >> 14);

                // Mark > space → mark tone → raw_bit based on mark/space
                // mark = 1200 Hz (NRZI: same as previous)
                let raw_bit = mark_energy > space_energy;

                let decoded_bit = raw_bit == prev_nrzi;
                prev_nrzi = raw_bit;
                shift_reg = (shift_reg >> 1) | if decoded_bit { 0x80 } else { 0x00 };
                if shift_reg == 0x7E { flag_count += 1; }
                if let Some(frame) = decoder.feed_bit(decoded_bit) {
                    assert_eq!(frame, raw, "Decoded frame doesn't match");
                    frame_found = true;
                }

                // Reset Goertzel state for next symbol
                mark_s1 = 0; mark_s2 = 0;
                space_s1 = 0; space_s2 = 0;
            }
        }

        if !frame_found {
            // Re-run collecting bits for comparison
            let mut mark_s1b: i64 = 0; let mut mark_s2b: i64 = 0;
            let mut space_s1b: i64 = 0; let mut space_s2b: i64 = 0;
            let mut prev2 = false;
            let mut bp2: u32 = 0;
            let mut all_bits = [false; 512];
            let mut sym2 = 0usize;

            for i in 0..audio_len {
                let s = audio[i] as i64;
                let ms0 = s + ((mark_coeff as i64 * mark_s1b) >> 14) - mark_s2b;
                mark_s2b = mark_s1b; mark_s1b = ms0;
                let ss0 = s + ((space_coeff as i64 * space_s1b) >> 14) - space_s2b;
                space_s2b = space_s1b; space_s1b = ss0;

                bp2 += baud_rate;
                if bp2 >= sample_rate {
                    bp2 -= sample_rate;
                    let me = mark_s1b*mark_s1b + mark_s2b*mark_s2b
                        - ((mark_coeff as i64 * mark_s1b * mark_s2b) >> 14);
                    let se = space_s1b*space_s1b + space_s2b*space_s2b
                        - ((space_coeff as i64 * space_s1b * space_s2b) >> 14);
                    let rb = me > se;
                    let db = rb == prev2;
                    prev2 = rb;
                    if sym2 < 512 { all_bits[sym2] = db; sym2 += 1; }
                    mark_s1b = 0; mark_s2b = 0;
                    space_s1b = 0; space_s2b = 0;
                }
            }

            // Build expected bits
            let flag_bits_arr = [false, true, true, true, true, true, true, false];
            let mut expected = [false; 512];
            let mut exp_len = 0;
            for _ in 0..30 {
                for &b in &flag_bits_arr {
                    if exp_len < 512 { expected[exp_len] = b; exp_len += 1; }
                }
            }
            for j in 0..encoded.bit_count {
                if exp_len < 512 { expected[exp_len] = encoded.bits[j] != 0; exp_len += 1; }
            }

            let cmp = sym2.min(exp_len);
            let mut errs = 0;
            let mut first_err = 0;
            for j in 1..cmp {
                if all_bits[j] != expected[j] {
                    errs += 1;
                    if errs == 1 { first_err = j; }
                }
            }

            // Show bits around first error
            let s = first_err.saturating_sub(5);
            let e = (first_err + 20).min(cmp);
            let mut act = [0u8; 64];
            let mut exp = [0u8; 64];
            let mut mrk = [0u8; 64];
            for j in s..e {
                let idx = j - s;
                act[idx] = if all_bits[j] { b'1' } else { b'0' };
                exp[idx] = if expected[j] { b'1' } else { b'0' };
                mrk[idx] = if all_bits[j] != expected[j] { b'^' } else { b' ' };
            }
            let len = e - s;

            panic!("Pipeline: {} flags, {} errors in {} bits, first at bit {}\n\
                    Bits {}-{}: act={}\n                    exp={}\n                    err={}",
                    flag_count, errs, cmp, first_err, s, e,
                    core::str::from_utf8(&act[..len]).unwrap_or("?"),
                    core::str::from_utf8(&exp[..len]).unwrap_or("?"),
                    core::str::from_utf8(&mrk[..len]).unwrap_or("?"));
        }
    }

    #[test]
    fn test_loopback_fast_path_clean() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        // Build a test frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Test");
        let raw = &frame_data[..frame_len];

        // HDLC encode
        let encoded = hdlc_encode(raw);

        // Modulate to audio with extended preamble for PLL lock
        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;

        // Generate extra preamble flags for PLL to lock (50 flags = ~400 bits)
        for _ in 0..50 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }

        // Then modulate the actual encoded frame (which has its own flags + data)
        for i in 0..encoded.bit_count {
            let bit = encoded.bits[i] != 0;
            let n = modulator.modulate_bit(bit, &mut audio[audio_len..]);
            audio_len += n;
        }

        // Add trailing silence for the decoder to flush
        for _ in 0..200 {
            audio_len += 1; // zero samples
        }

        // Demodulate with fast path
        let config = DemodConfig::default_1200();
        let mut demod = FastDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 8192];
        let num_symbols = demod.process_samples(&audio[..audio_len], &mut symbols);

        // Feed demodulated bits into HDLC decoder
        let mut decoder = HdlcDecoder::new();
        let mut decoded: Option<([u8; 330], usize)> = None;
        for i in 0..num_symbols {
            if let Some(frame) = decoder.feed_bit(symbols[i].bit) {
                let mut buf = [0u8; 330];
                let len = frame.len();
                buf[..len].copy_from_slice(frame);
                decoded = Some((buf, len));
            }
        }

        let (dec_buf, dec_len) = decoded.expect("Fast path should decode clean signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "Fast path decoded frame doesn't match original");
    }

    #[test]
    fn test_loopback_quality_path_clean() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;
        use crate::modem::soft_hdlc::SoftHdlcDecoder;

        // Build a test frame
        let (frame_data, frame_len) = build_test_frame("WA1ABC", "APRS", b"=4903.50N/07201.75W>status");
        let raw = &frame_data[..frame_len];

        // HDLC encode and modulate with preamble + trailing silence
        let encoded = hdlc_encode(raw);
        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;

        // Extended preamble for Hilbert transform settling
        for _ in 0..30 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        for i in 0..encoded.bit_count {
            let n = modulator.modulate_bit(encoded.bits[i] != 0, &mut audio[audio_len..]);
            audio_len += n;
        }
        // Trailing silence
        audio_len += 20;

        // Demodulate with quality path
        let config = DemodConfig::default_1200();
        let mut demod = QualityDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 8192];
        let num_symbols = demod.process_samples(&audio[..audio_len], &mut symbols);

        // Feed into soft HDLC decoder
        let mut decoder = SoftHdlcDecoder::new();
        let mut decoded: Option<([u8; 330], usize)> = None;
        for i in 0..num_symbols {
            if let Some(result) = decoder.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    crate::modem::soft_hdlc::FrameResult::Valid(d) => *d,
                    crate::modem::soft_hdlc::FrameResult::Recovered { data, .. } => *data,
                };
                let mut buf = [0u8; 330];
                let len = data.len();
                buf[..len].copy_from_slice(data);
                decoded = Some((buf, len));
            }
        }

        let (dec_buf, dec_len) = decoded.expect("Quality path should decode clean signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "Quality path decoded frame doesn't match original");
    }

    #[test]
    fn test_loopback_fast_agc_clean() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-AGC");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;

        for _ in 0..50 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        for i in 0..encoded.bit_count {
            let n = modulator.modulate_bit(encoded.bits[i] != 0, &mut audio[audio_len..]);
            audio_len += n;
        }
        audio_len += 200;

        let config = DemodConfig::default_1200();
        let mut demod = FastDemodulator::new(config).with_agc();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 8192];
        let num_symbols = demod.process_samples(&audio[..audio_len], &mut symbols);

        let mut decoder = HdlcDecoder::new();
        let mut decoded: Option<([u8; 330], usize)> = None;
        for i in 0..num_symbols {
            if let Some(frame) = decoder.feed_bit(symbols[i].bit) {
                let mut buf = [0u8; 330];
                let len = frame.len();
                buf[..len].copy_from_slice(frame);
                decoded = Some((buf, len));
            }
        }

        let (dec_buf, dec_len) = decoded.expect("AGC fast path should decode clean signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "AGC fast path decoded frame doesn't match original");
    }

    #[test]
    fn test_agc_handles_deemphasis() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-DeEmph");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        let mod_config = ModConfig::default_1200();
        let mut modulator = AfskModulator::new(mod_config.clone());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;

        for _ in 0..50 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        for i in 0..encoded.bit_count {
            let n = modulator.modulate_bit(encoded.bits[i] != 0, &mut audio[audio_len..]);
            audio_len += n;
        }
        audio_len += 200;

        // Apply de-emphasis: two passes of a 1-pole LPF to create ~8 dB
        // relative attenuation of space (2200 Hz) vs mark (1200 Hz).
        // y[n] = alpha * x[n] + (1 - alpha) * y[n-1], alpha ≈ 0.35
        for _pass in 0..2 {
            let mut prev: i32 = 0;
            for i in 0..audio_len {
                let x = audio[i] as i32;
                let y = (91 * x + 165 * prev) >> 8;
                audio[i] = y.clamp(-32768, 32767) as i16;
                prev = y;
            }
        }

        // AGC decoder should succeed on de-emphasized signal
        let config = DemodConfig::default_1200();
        let mut demod_agc = FastDemodulator::new(config).with_agc();
        let mut symbols_agc = [DemodSymbol { bit: false, llr: 0 }; 8192];
        let num_agc = demod_agc.process_samples(&audio[..audio_len], &mut symbols_agc);

        let mut decoder_agc = HdlcDecoder::new();
        let mut agc_decoded: Option<([u8; 330], usize)> = None;
        for i in 0..num_agc {
            if let Some(frame) = decoder_agc.feed_bit(symbols_agc[i].bit) {
                let mut buf = [0u8; 330];
                let len = frame.len();
                buf[..len].copy_from_slice(frame);
                agc_decoded = Some((buf, len));
            }
        }

        let (dec_buf, dec_len) = agc_decoded.expect("AGC should decode de-emphasized signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "AGC decoded frame doesn't match original");
    }

    #[test]
    fn test_loopback_multiple_frames() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        // Build and modulate 3 different frames back-to-back
        let frames: [(&str, &str, &[u8]); 3] = [
            ("N0CALL", "APRS", b"Frame one"),
            ("WA1ABC", "CQ", b"Frame two!"),
            ("VE3XYZ", "APRS", b"Third frame"),
        ];

        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;

        let mut raw_frames: [([u8; 330], usize); 3] = [([0u8; 330], 0); 3];

        for (idx, &(src, dest, info)) in frames.iter().enumerate() {
            let (frame_data, frame_len) = build_test_frame(src, dest, info);
            raw_frames[idx].0[..frame_len].copy_from_slice(&frame_data[..frame_len]);
            raw_frames[idx].1 = frame_len;

            let encoded = hdlc_encode(&frame_data[..frame_len]);
            for i in 0..encoded.bit_count {
                let bit = encoded.bits[i] != 0;
                let n = modulator.modulate_bit(bit, &mut audio[audio_len..]);
                audio_len += n;
            }
        }

        // Add trailing silence so the last symbol boundary fires
        audio_len += 20;

        // Demodulate
        let config = DemodConfig::default_1200();
        let mut demod = FastDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 8192];
        let num_symbols = demod.process_samples(&audio[..audio_len], &mut symbols);

        // Decode
        let mut decoder = HdlcDecoder::new();
        let mut decoded_count = 0usize;
        for i in 0..num_symbols {
            if let Some(frame) = decoder.feed_bit(symbols[i].bit) {
                if decoded_count < 3 {
                    let (ref raw_buf, raw_len) = raw_frames[decoded_count];
                    assert_eq!(frame, &raw_buf[..raw_len],
                        "Frame {} mismatch", decoded_count);
                }
                decoded_count += 1;
            }
        }

        assert_eq!(decoded_count, 3, "Should decode all 3 frames, got {}", decoded_count);
    }

    // ── DmDemodulator tests ──────────────────────────────────────────

    #[test]
    fn test_dm_discriminator_with_bpf() {
        // Feed pure mark and space tones through BPF → delay-multiply → LPF
        // and verify opposite-sign outputs at both 11025 and 22050 Hz.
        use crate::modem::delay_multiply::DelayMultiplyDetector;

        for &sample_rate in &[11025u32, 22050] {
            let num_samples = sample_rate as usize; // 1 second

            // Generate mark tone (1200 Hz)
            let mut mark_audio = [0i16; 22050];
            for i in 0..num_samples {
                let t = i as f64 / sample_rate as f64;
                mark_audio[i] = (16000.0 * (2.0 * core::f64::consts::PI * 1200.0 * t).sin()) as i16;
            }

            // Generate space tone (2200 Hz)
            let mut space_audio = [0i16; 22050];
            for i in 0..num_samples {
                let t = i as f64 / sample_rate as f64;
                space_audio[i] = (16000.0 * (2.0 * core::f64::consts::PI * 2200.0 * t).sin()) as i16;
            }

            // BPF → delay-multiply with LPF → sample last output
            let bpf_fn = match sample_rate {
                22050 => crate::modem::filter::afsk_bandpass_22050,
                _ => crate::modem::filter::afsk_bandpass_11025,
            };
            let lpf = crate::modem::filter::post_detect_lpf(sample_rate);

            let mut bpf_m = bpf_fn();
            let mut det_m = DelayMultiplyDetector::new(sample_rate, lpf);
            let mut mark_last: i16 = 0;
            for &s in &mark_audio[..num_samples] {
                let filtered = bpf_m.process(s);
                mark_last = det_m.process(filtered);
            }

            let mut bpf_s = bpf_fn();
            let lpf2 = crate::modem::filter::post_detect_lpf(sample_rate);
            let mut det_s = DelayMultiplyDetector::new(sample_rate, lpf2);
            let mut space_last: i16 = 0;
            for &s in &space_audio[..num_samples] {
                let filtered = bpf_s.process(s);
                space_last = det_s.process(filtered);
            }

            assert!(
                (mark_last > 0 && space_last < 0) || (mark_last < 0 && space_last > 0),
                "BPF+DM+LPF at {} Hz: mark_last={}, space_last={} — should have opposite polarity",
                sample_rate, mark_last, space_last
            );
        }
    }

    #[test]
    fn test_dm_flag_detection_22k() {
        // Verify DmDemodulator can detect flag patterns at 22050 Hz.
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let sample_rate = 22050u32;
        let mut mod_config = ModConfig::default_1200();
        mod_config.sample_rate = sample_rate;
        let mut modulator = AfskModulator::new(mod_config);
        let mut audio = [0i16; 8192];
        let mut audio_len = 0;
        for _ in 0..10 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        audio_len += 100;

        let config = DemodConfig::default_1200_22k();
        let mut demod = DmDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 256];
        let num = demod.process_samples(&audio[..audio_len], &mut symbols);

        let mut shift_reg: u8 = 0;
        let mut flag_count = 0u32;
        for i in 0..num {
            shift_reg = (shift_reg >> 1) | if symbols[i].bit { 0x80 } else { 0 };
            if shift_reg == 0x7E { flag_count += 1; }
        }

        assert!(flag_count >= 5,
            "DM at 22050 Hz should detect >= 5 flags from 10 flag preamble, got {}",
            flag_count);
    }

    #[test]
    fn test_dm_demod_flags_short() {
        // Test DmDemodulator on 5 flags at 22050 Hz — should find flag patterns
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let sample_rate = 22050u32;
        let mut mod_config = ModConfig::default_1200();
        mod_config.sample_rate = sample_rate;
        let mut modulator = AfskModulator::new(mod_config);
        let mut audio = [0i16; 8192];
        let mut audio_len = 0;
        for _ in 0..5 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        audio_len += 100; // trailing silence

        let config = DemodConfig::default_1200_22k();
        let mut demod = DmDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 256];
        let num_symbols = demod.process_samples(&audio[..audio_len], &mut symbols);

        // Check for flags
        let mut shift_reg: u8 = 0;
        let mut flag_count = 0u32;
        let mut bits_str = [0u8; 256];
        for i in 0..num_symbols {
            shift_reg = (shift_reg >> 1) | if symbols[i].bit { 0x80 } else { 0 };
            if shift_reg == 0x7E { flag_count += 1; }
            if i < 256 { bits_str[i] = if symbols[i].bit { b'1' } else { b'0' }; }
        }
        let show = num_symbols.min(256);
        assert!(
            flag_count >= 2,
            "DM on 5 flags at {} Hz: {} symbols, {} flags\n\
             Bits: {}",
            sample_rate, num_symbols, flag_count,
            core::str::from_utf8(&bits_str[..show]).unwrap_or("?")
        );
    }

    #[test]
    fn test_dm_demod_creation() {
        let config = DemodConfig::default_1200();
        let demod = DmDemodulator::new(config);
        assert_eq!(demod.samples_processed, 0);
    }

    #[test]
    fn test_dm_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut demod = DmDemodulator::new(config);
        let silence = [0i16; 1000];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 200];

        let n = demod.process_samples(&silence, &mut symbols);
        // Silence should produce some symbols (PLL runs), no panics/overflow
        assert!(n < 200);
    }

    #[test]
    fn test_dm_reset() {
        let config = DemodConfig::default_1200();
        let mut demod = DmDemodulator::new(config);
        let noise = [1000i16; 100];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 50];

        demod.process_samples(&noise, &mut symbols);
        demod.reset();
        assert_eq!(demod.samples_processed, 0);
    }

    #[test]
    fn test_loopback_dm_clean() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        // Build a test frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-DM");
        let raw = &frame_data[..frame_len];

        // HDLC encode
        let encoded = hdlc_encode(raw);

        // Modulate at 22050 Hz for the DM path — the delay-multiply approach
        // works much better at higher sample rates (18 samples/symbol vs 9).
        let mut mod_config = ModConfig::default_1200();
        mod_config.sample_rate = 22050;
        let mut modulator = AfskModulator::new(mod_config);
        let mut audio = [0i16; 131072];
        let mut audio_len = 0;

        // Generate extra preamble flags for PLL to lock
        for _ in 0..50 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }

        // Modulate the encoded frame
        for i in 0..encoded.bit_count {
            let bit = encoded.bits[i] != 0;
            let n = modulator.modulate_bit(bit, &mut audio[audio_len..]);
            audio_len += n;
        }

        // Trailing silence
        audio_len += 400;

        // Demodulate with DM path at 22050 Hz
        let config = DemodConfig::default_1200_22k();
        let mut demod = DmDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 16384];
        let num_symbols = demod.process_samples(&audio[..audio_len], &mut symbols);

        // Feed demodulated bits into HDLC decoder
        let mut decoder = HdlcDecoder::new();
        let mut decoded: Option<([u8; 330], usize)> = None;
        for i in 0..num_symbols {
            if let Some(frame) = decoder.feed_bit(symbols[i].bit) {
                let mut buf = [0u8; 330];
                let len = frame.len();
                buf[..len].copy_from_slice(frame);
                decoded = Some((buf, len));
            }
        }

        // Diagnostic: compare DM output with expected bits from encoding
        if decoded.is_none() {
            // Reconstruct expected NRZI-decoded bits from the encoded frame
            // The preamble flags are separate (50 × flag), then the encoded bits
            let flag_bits: [bool; 8] = [false, true, true, true, true, true, true, false];
            let mut expected = [false; 1024];
            let mut exp_len = 0;
            for _ in 0..50 {
                for &b in &flag_bits { if exp_len < 1024 { expected[exp_len] = b; exp_len += 1; } }
            }
            for j in 0..encoded.bit_count {
                if exp_len < 1024 { expected[exp_len] = encoded.bits[j] != 0; exp_len += 1; }
            }

            // Count errors after preamble (where flags are reliable)
            let data_start = 50 * 8; // first data bit
            let cmp_end = num_symbols.min(exp_len);
            let mut errs = 0;
            let mut first_err = cmp_end;
            for j in data_start..cmp_end {
                if symbols[j].bit != expected[j] {
                    errs += 1;
                    if first_err == cmp_end { first_err = j; }
                }
            }

            // Show bits around first error
            let s = first_err.saturating_sub(4);
            let e = (first_err + 20).min(cmp_end);
            let mut act = [0u8; 64];
            let mut exp_s = [0u8; 64];
            let mut mrk = [0u8; 64];
            for j in s..e {
                let idx = j - s;
                act[idx] = if symbols[j].bit { b'1' } else { b'0' };
                exp_s[idx] = if expected[j] { b'1' } else { b'0' };
                mrk[idx] = if symbols[j].bit != expected[j] { b'^' } else { b' ' };
            }
            let len = e - s;

            panic!(
                "DM path: {} symbols, {} data errors in {} data bits, first at {}\n\
                 Bits {}-{}: act={}\n                 exp={}\n                 err={}",
                num_symbols, errs, cmp_end - data_start, first_err, s, e,
                core::str::from_utf8(&act[..len]).unwrap_or("?"),
                core::str::from_utf8(&exp_s[..len]).unwrap_or("?"),
                core::str::from_utf8(&mrk[..len]).unwrap_or("?"));
        }

        let (dec_buf, dec_len) = decoded.unwrap();
        assert_eq!(&dec_buf[..dec_len], raw,
            "DM path decoded frame doesn't match original");
    }

    #[test]
    fn test_dm_multiple_frames() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let frames: [(&str, &str, &[u8]); 3] = [
            ("N0CALL", "APRS", b"DM Frame one"),
            ("WA1ABC", "CQ", b"DM Frame two!"),
            ("VE3XYZ", "APRS", b"DM Third frame"),
        ];

        // Modulate at 22050 Hz — DM needs higher sample rate
        let mut mod_config = ModConfig::default_1200();
        mod_config.sample_rate = 22050;
        let mut modulator = AfskModulator::new(mod_config);
        let mut audio = [0i16; 131072];
        let mut audio_len = 0;

        let mut raw_frames: [([u8; 330], usize); 3] = [([0u8; 330], 0); 3];

        for (idx, &(src, dest, info)) in frames.iter().enumerate() {
            let (frame_data, frame_len) = build_test_frame(src, dest, info);
            raw_frames[idx].0[..frame_len].copy_from_slice(&frame_data[..frame_len]);
            raw_frames[idx].1 = frame_len;

            let encoded = hdlc_encode(&frame_data[..frame_len]);
            for i in 0..encoded.bit_count {
                let bit = encoded.bits[i] != 0;
                let n = modulator.modulate_bit(bit, &mut audio[audio_len..]);
                audio_len += n;
            }
        }

        audio_len += 400;

        let config = DemodConfig::default_1200_22k();
        let mut demod = DmDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 16384];
        let num_symbols = demod.process_samples(&audio[..audio_len], &mut symbols);

        let mut decoder = HdlcDecoder::new();
        let mut decoded_count = 0usize;
        for i in 0..num_symbols {
            if let Some(frame) = decoder.feed_bit(symbols[i].bit) {
                if decoded_count < 3 {
                    let (ref raw_buf, raw_len) = raw_frames[decoded_count];
                    assert_eq!(frame, &raw_buf[..raw_len],
                        "DM Frame {} mismatch", decoded_count);
                }
                decoded_count += 1;
            }
        }

        assert_eq!(decoded_count, 3, "DM should decode all 3 frames, got {}", decoded_count);
    }
}
