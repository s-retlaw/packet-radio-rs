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

/// Maximum window table length for windowed Goertzel (covers up to 48000/1200=40 SPB).
const MAX_WINDOW_LEN: usize = 48;

/// Unity gain in Q8 fixed-point (256 = 1.0, i.e. 0 dB).
const UNITY_GAIN_Q8: u16 = 256;

/// Goertzel window type for ISI reduction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GoertzelWindow {
    /// Standard rectangular window (no tapering). Default.
    Rectangular,
    /// Hann window: w[n] = 0.5*(1 - cos(2*pi*n/(N-1))). Zeros at edges.
    Hann,
    /// Hamming window: w[n] = 0.54 - 0.46*cos(2*pi*n/(N-1)). Non-zero at edges.
    Hamming,
    /// Blackman window: low sidelobes, wider main lobe.
    Blackman,
    /// Flat middle with cosine taper on first/last 2 samples.
    EdgeTaper,
}

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
/// Optional adaptive state for FastDemodulator Goertzel re-tuning.
///
/// Runs Hilbert transform + instantaneous frequency estimation during
/// preamble to measure actual mark/space frequencies, then re-tunes
/// the Goertzel coefficients to match the transmitter.
struct AdaptiveState {
    hilbert: HilbertTransform<31>,
    inst_freq: InstFreqDetector,
    tracker: AdaptiveTracker,
    /// Whether Goertzel coefficients have been re-tuned for this packet.
    retuned: bool,
    /// Sample index counter for the tracker.
    sample_index: u32,
}

pub struct FastDemodulator {
    #[allow(dead_code)]
    config: DemodConfig,
    bpf: BiquadFilter,
    /// Optional second BPF stage for -12 dB/octave cascaded filtering.
    bpf2: Option<BiquadFilter>,
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
    /// Whether to produce energy-based LLR (default: fixed ±64).
    energy_llr: bool,
    /// Optional adaptive Goertzel re-tuning from preamble measurements.
    adaptive: Option<AdaptiveState>,
    /// Whether adaptive preamble gain measurement is enabled.
    adaptive_gain_enabled: bool,
    /// Shift register for NRZI-decoded bits (flag detection).
    demod_shift_reg: u8,
    /// Accumulated mark energy during preamble (on mark-tone symbols).
    preamble_mark_energy: i64,
    /// Accumulated space energy during preamble (on space-tone symbols).
    preamble_space_energy: i64,
    /// Number of mark-tone symbols accumulated during preamble.
    preamble_mark_count: u16,
    /// Number of space-tone symbols accumulated during preamble.
    preamble_space_count: u16,
    /// Number of HDLC flags (0x7E) detected in current preamble.
    preamble_flag_count: u8,
    /// Symbols since last flag detection (for preamble-end detection).
    symbols_since_last_flag: u8,
    /// Optional PLL for adaptive symbol timing (Gardner TED).
    /// When present, replaces Bresenham fixed-rate timing.
    /// Used for 300 baud variable-speed tracking.
    pll: Option<ClockRecoveryPll>,
    /// Override baud rate for Bresenham timing.
    /// When set, the symbol timing uses this rate instead of config.baud_rate.
    /// Used for baud-rate diversity in multi-decoder (300 baud variable speed).
    timing_baud_rate: u32,
    /// Q8 window coefficients for windowed Goertzel (256 = 1.0).
    /// Pre-multiplying input samples by these weights reduces ISI when
    /// Bresenham timing is slightly misaligned with symbol boundaries.
    window_q8: [u16; MAX_WINDOW_LEN],
    /// Length of window table (nominal samples per symbol). 0 = disabled.
    window_len: u8,
    /// Current sample index within symbol (resets at each boundary).
    sym_sample_idx: u8,
}

impl FastDemodulator {
    /// Select the appropriate BPF for a given config (baud rate + sample rate).
    fn select_bpf(config: &DemodConfig) -> BiquadFilter {
        if config.baud_rate == 300 {
            match config.sample_rate {
                8000 => super::filter::afsk_300_bandpass_8000(),
                22050 => super::filter::afsk_300_bandpass_22050(),
                44100 => super::filter::afsk_300_bandpass_44100(),
                48000 => super::filter::afsk_300_bandpass_48000(),
                _ => super::filter::afsk_300_bandpass_11025(),
            }
        } else {
            match config.sample_rate {
                12000 => super::filter::afsk_bandpass_12000(),
                13200 => super::filter::afsk_bandpass_13200(),
                22050 => super::filter::afsk_bandpass_22050(),
                26400 => super::filter::afsk_bandpass_26400(),
                44100 => super::filter::afsk_bandpass_44100(),
                48000 => super::filter::afsk_bandpass_48000(),
                _ => super::filter::afsk_bandpass_11025(),
            }
        }
    }

    /// Common internal constructor. All public constructors delegate here.
    fn create(
        config: DemodConfig,
        bpf: BiquadFilter,
        mark_coeff: i32,
        space_coeff: i32,
        phase_offset: u32,
    ) -> Self {
        let timing_baud_rate = config.baud_rate;
        Self {
            config,
            bpf,
            bpf2: None,
            prev_nrzi_bit: false,
            samples_processed: 0,
            mark_s1: 0,
            mark_s2: 0,
            space_s1: 0,
            space_s2: 0,
            mark_coeff,
            space_coeff,
            bit_phase: phase_offset,
            space_gain_q8: UNITY_GAIN_Q8,
            agc_enabled: false,
            window_q8: [UNITY_GAIN_Q8; MAX_WINDOW_LEN],
            window_len: 0,
            sym_sample_idx: 0,
            mark_energy_peak: 1,
            space_energy_peak: 1,
            energy_llr: false,
            adaptive: None,
            adaptive_gain_enabled: false,
            demod_shift_reg: 0,
            preamble_mark_energy: 0,
            preamble_space_energy: 0,
            preamble_mark_count: 0,
            preamble_space_count: 0,
            preamble_flag_count: 0,
            symbols_since_last_flag: 255,
            pll: None,
            timing_baud_rate,
        }
    }

    /// Create a new fast-path demodulator.
    #[must_use]
    pub fn new(config: DemodConfig) -> Self {
        let bpf = Self::select_bpf(&config);
        let mark_coeff = goertzel_coeff(config.mark_freq, config.sample_rate);
        let space_coeff = goertzel_coeff(config.space_freq, config.sample_rate);
        Self::create(config, bpf, mark_coeff, space_coeff, 0)
    }

    /// Set a custom bandpass filter, replacing the auto-selected one.
    #[must_use]
    pub fn filter(mut self, bpf: BiquadFilter) -> Self {
        self.bpf = bpf;
        self
    }

    /// Set the initial Bresenham timing phase offset.
    #[must_use]
    pub fn phase_offset(mut self, offset: u32) -> Self {
        self.bit_phase = offset;
        self
    }

    /// Set custom mark/space Goertzel frequencies.
    ///
    /// Re-computes Goertzel coefficients for the given frequencies, allowing
    /// the decoder to handle transmitters with crystal frequency error.
    #[must_use]
    pub fn frequencies(mut self, mark_freq: u32, space_freq: u32) -> Self {
        self.mark_coeff = goertzel_coeff(mark_freq, self.config.sample_rate);
        self.space_coeff = goertzel_coeff(space_freq, self.config.sample_rate);
        self
    }

    /// Override baud rate for Bresenham symbol timing.
    ///
    /// The BPF and Goertzel coefficients remain tuned for the nominal baud rate,
    /// but the symbol timing runs at a different rate. Used for baud-rate diversity
    /// in multi-decoder to handle variable-speed transmitters.
    #[must_use]
    pub fn with_timing_baud_rate(mut self, baud_rate: u32) -> Self {
        self.timing_baud_rate = baud_rate;
        self
    }

    /// Set space energy gain for multi-slicer diversity.
    ///
    /// Q8 format: 256 = 0 dB (no gain), higher values boost space energy
    /// relative to mark. Used to handle de-emphasized audio where the
    /// space tone (2200 Hz) is weaker than mark (1200 Hz).
    #[must_use]
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
    #[must_use]
    pub fn with_agc(mut self) -> Self {
        self.agc_enabled = true;
        self
    }

    /// Enable energy-based LLR output.
    ///
    /// Replaces the fixed ±64 LLR with actual mark/space energy ratio,
    /// enabling SoftHdlcDecoder bit-flip recovery on the fast path.
    #[must_use]
    pub fn with_energy_llr(mut self) -> Self {
        self.energy_llr = true;
        self
    }

    /// Enable cascaded (4th-order) bandpass filtering.
    ///
    /// Adds a second BPF stage in series with the first, doubling the
    /// rolloff to -12 dB/octave. Improves out-of-band noise rejection.
    /// Cost: ~5 extra ops/sample.
    #[must_use]
    pub fn with_cascade_bpf(mut self) -> Self {
        self.bpf2 = Some(self.bpf);
        self
    }

    /// Apply a window function to the Goertzel accumulator input.
    ///
    /// Pre-multiplies each sample by window coefficients (Q8) before the
    /// Goertzel recursion. Tapered windows (Hann, Hamming) reduce ISI
    /// when Bresenham timing is slightly misaligned with symbol boundaries.
    /// At 9.2 samples/symbol, a 1-sample timing error contributes 11% ISI
    /// with rectangular windowing but near-zero with Hann.
    ///
    /// Cost: one extra integer multiply per sample.
    #[must_use]
    pub fn with_window(mut self, window_type: GoertzelWindow) -> Self {
        let spb = self.config.sample_rate / self.config.baud_rate;
        if spb == 0 || spb > MAX_WINDOW_LEN as u32 {
            return self; // too long for table, skip
        }
        if window_type == GoertzelWindow::Rectangular {
            return self; // no-op
        }
        self.window_len = spb as u8;
        let n = spb as usize;
        for i in 0..n {
            let w = compute_window_coeff(window_type, i, n);
            self.window_q8[i] = w;
        }
        self
    }

    /// Enable adaptive Goertzel re-tuning from preamble measurements.
    ///
    /// Runs Hilbert transform + instantaneous frequency estimation during
    /// the preamble. When the AdaptiveTracker locks, re-computes Goertzel
    /// coefficients to match the transmitter's actual mark/space frequencies.
    ///
    /// Additional cost: ~46 ops/sample during preamble (~920 samples at 11025 Hz).
    /// After lock, the adaptive path is bypassed (just retuned Goertzel runs).
    #[must_use]
    pub fn with_adaptive_retune(mut self) -> Self {
        self.adaptive = Some(AdaptiveState {
            hilbert: hilbert_31(),
            inst_freq: InstFreqDetector::new(self.config.sample_rate),
            tracker: AdaptiveTracker::new_for_config(&self.config),
            retuned: false,
            sample_index: 0,
        });
        self
    }

    /// Enable Gardner PLL timing recovery with default parameters.
    ///
    /// Uses a normalized Goertzel energy discriminator to drive the PLL.
    /// Particularly useful for 300 baud where 37 samples/symbol gives
    /// PLL much better convergence than at 1200 baud (9 sps).
    #[must_use]
    pub fn with_pll(mut self) -> Self {
        self.pll = Some(
            ClockRecoveryPll::new_gardner(self.config.sample_rate, self.config.baud_rate, 936, 0)
                .with_error_shift(8)
        );
        self
    }

    /// Enable PLL timing recovery with custom parameters.
    #[must_use]
    pub fn with_custom_pll(mut self, pll: ClockRecoveryPll) -> Self {
        self.pll = Some(pll);
        self
    }

    /// Enable adaptive mark/space gain from preamble measurement.
    ///
    /// Measures relative mark and space Goertzel energy during the HDLC flag
    /// preamble and sets `space_gain_q8` to compensate for de-emphasis or
    /// frequency-dependent gain differences. Re-measures on each new preamble.
    ///
    /// Only effective when AGC is disabled (AGC already handles gain).
    #[must_use]
    pub fn with_adaptive_gain(mut self) -> Self {
        self.adaptive_gain_enabled = true;
        self
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        self.bpf.reset();
        if let Some(ref mut bpf2) = self.bpf2 {
            bpf2.reset();
        }
        self.prev_nrzi_bit = false;
        self.samples_processed = 0;
        self.mark_s1 = 0;
        self.mark_s2 = 0;
        self.space_s1 = 0;
        self.space_s2 = 0;
        self.bit_phase = 0;
        self.mark_energy_peak = 1;
        self.space_energy_peak = 1;
        self.sym_sample_idx = 0;
        // Reset adaptive gain state
        if self.adaptive_gain_enabled {
            self.space_gain_q8 = 256;
            self.demod_shift_reg = 0;
            self.preamble_mark_energy = 0;
            self.preamble_space_energy = 0;
            self.preamble_mark_count = 0;
            self.preamble_space_count = 0;
            self.preamble_flag_count = 0;
            self.symbols_since_last_flag = 255;
        }
        // Reset adaptive state and restore original Goertzel coefficients
        if let Some(ref mut adaptive) = self.adaptive {
            adaptive.hilbert.reset();
            adaptive.inst_freq.reset();
            adaptive.tracker.reset();
            adaptive.retuned = false;
            adaptive.sample_index = 0;
            self.mark_coeff = goertzel_coeff(self.config.mark_freq, self.config.sample_rate);
            self.space_coeff = goertzel_coeff(self.config.space_freq, self.config.sample_rate);
        }
        if let Some(ref mut pll) = self.pll {
            pll.reset();
        }
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
        let timing_baud = self.timing_baud_rate;

        for &sample in samples {
            self.samples_processed += 1;

            // 1. Bandpass filter (optionally cascaded for -12 dB/oct rolloff)
            let bpf1_out = self.bpf.process(sample);
            let filtered = if let Some(ref mut bpf2) = self.bpf2 {
                bpf2.process(bpf1_out)
            } else {
                bpf1_out
            };
            let s = filtered as i64;

            // 1b. Adaptive: feed Hilbert+InstFreq into tracker (only until lock)
            if let Some(ref mut adaptive) = self.adaptive {
                if !adaptive.retuned {
                    adaptive.sample_index = adaptive.sample_index.wrapping_add(1);
                    let (real, imag) = adaptive.hilbert.process(filtered);
                    let freq_fp = adaptive.inst_freq.process(real, imag);
                    adaptive.tracker.feed(freq_fp, adaptive.sample_index);

                    // Re-tune Goertzel coefficients when tracker locks
                    if adaptive.tracker.is_locked() {
                        let mark_hz = (adaptive.tracker.mark_freq_est >> 8) as u32;
                        let space_hz = (adaptive.tracker.space_freq_est >> 8) as u32;
                        // Only retune if estimates are reasonable (within ±200 Hz of nominal)
                        let mark_delta = (mark_hz as i32 - self.config.mark_freq as i32).unsigned_abs();
                        let space_delta = (space_hz as i32 - self.config.space_freq as i32).unsigned_abs();
                        if mark_delta < 200 && space_delta < 200 && mark_hz > 0 && space_hz > 0 {
                            self.mark_coeff = goertzel_coeff(mark_hz, sample_rate);
                            self.space_coeff = goertzel_coeff(space_hz, sample_rate);
                            // Reset Goertzel accumulators — old state is invalid under new coefficients
                            self.mark_s1 = 0; self.mark_s2 = 0;
                            self.space_s1 = 0; self.space_s2 = 0;
                        }
                        adaptive.retuned = true;
                    }
                }
            }

            // 2. Apply window (if enabled) and Goertzel iteration
            let sw = if self.window_len > 0 {
                let idx = (self.sym_sample_idx as usize).min(self.window_len.saturating_sub(1) as usize);
                self.sym_sample_idx = self.sym_sample_idx.saturating_add(1);
                (s * self.window_q8[idx] as i64) >> 8
            } else {
                s
            };

            let mark_s0 = sw + ((self.mark_coeff as i64 * self.mark_s1) >> 14) - self.mark_s2;
            self.mark_s2 = self.mark_s1;
            self.mark_s1 = mark_s0;

            let space_s0 = sw + ((self.space_coeff as i64 * self.space_s1) >> 14) - self.space_s2;
            self.space_s2 = self.space_s1;
            self.space_s1 = space_s0;

            // 3. Symbol timing: PLL or Bresenham
            let boundary = if let Some(ref mut pll) = self.pll {
                // Normalized discriminator from Goertzel state: bounded ±127.
                // |mark_s1| vs |space_s1| gives instantaneous tone strength.
                let mark_mag = self.mark_s1.abs();
                let space_mag = self.space_s1.abs();
                let total = mark_mag + space_mag;
                let disc = if total > 0 {
                    (((mark_mag - space_mag) * 127) / total).clamp(-127, 127) as i16
                } else {
                    0
                };
                pll.update(disc).is_some()
            } else {
                self.bit_phase += timing_baud;
                if self.bit_phase >= sample_rate {
                    self.bit_phase -= sample_rate;
                    true
                } else {
                    false
                }
            };

            if boundary {
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

                // 5b. Flag detection for adaptive gain
                if self.adaptive_gain_enabled {
                    self.demod_shift_reg = (self.demod_shift_reg << 1) | (decoded_bit as u8);

                    if self.demod_shift_reg == 0x7E {
                        self.preamble_flag_count = self.preamble_flag_count.saturating_add(1);
                        self.symbols_since_last_flag = 0;
                    } else {
                        self.symbols_since_last_flag = self.symbols_since_last_flag.saturating_add(1);
                    }
                }

                // 5c. Adaptive preamble gain accumulation
                if self.adaptive_gain_enabled && !self.agc_enabled {
                    if self.preamble_flag_count >= 1 && self.symbols_since_last_flag <= 8 {
                        if mark_energy > space_energy {
                            self.preamble_mark_energy += mark_energy >> AGC_ENERGY_SHIFT;
                            self.preamble_mark_count += 1;
                        } else {
                            self.preamble_space_energy += space_energy >> AGC_ENERGY_SHIFT;
                            self.preamble_space_count += 1;
                        }
                    }

                    if self.symbols_since_last_flag > 8
                        && self.preamble_flag_count >= 2
                        && self.preamble_mark_count > 0
                        && self.preamble_space_count > 0
                    {
                        let mark_avg = self.preamble_mark_energy / self.preamble_mark_count as i64;
                        let space_avg = self.preamble_space_energy / self.preamble_space_count as i64;
                        if space_avg > 0 {
                            let measured = (mark_avg * 256) / space_avg;
                            // Only increase gain (compensate de-emphasis), never decrease.
                            // Apply 25% of measured excess above unity.
                            let excess = (measured - 256).max(0);
                            let gain = 256 + (excess >> 2);
                            self.space_gain_q8 = (gain as u16).min(512);
                        }
                        self.preamble_mark_energy = 0;
                        self.preamble_space_energy = 0;
                        self.preamble_mark_count = 0;
                        self.preamble_space_count = 0;
                        self.preamble_flag_count = 0;
                    }
                }

                // 6. LLR: energy-based or fixed
                let llr = if self.energy_llr {
                    let total = mark_energy + space_energy;
                    if total > 0 {
                        let energy_ratio = ((mark_energy - space_energy) * 127) / total;
                        let mut confidence = energy_ratio.unsigned_abs().clamp(1, 127) as i8;
                        // At NRZI transitions the Goertzel window spans both tones,
                        // making the energy ratio unreliable. Halve confidence so
                        // SoftHdlcDecoder targets genuinely uncertain bits.
                        if !decoded_bit {
                            confidence >>= 1;
                            if confidence == 0 { confidence = 1; }
                        }
                        if decoded_bit { confidence } else { -confidence }
                    } else {
                        0
                    }
                } else if decoded_bit {
                    64
                } else {
                    -64
                };

                if sym_count < symbols_out.len() {
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr,
                    };
                    sym_count += 1;
                }

                // Reset Goertzel state for next symbol
                self.mark_s1 = 0;
                self.mark_s2 = 0;
                self.space_s1 = 0;
                self.space_s2 = 0;
                self.sym_sample_idx = 0;
            }
        }

        sym_count
    }
}

/// Compute Goertzel coefficient for a given frequency: 2·cos(2π·f/Fs) in Q14.
pub fn goertzel_coeff(freq: u32, sample_rate: u32) -> i32 {
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
        (1200, 26400) => 31441,  // 2·cos(2π·1200/26400) × 16384
        (2200, 26400) => 28378,  // 2·cos(2π·2200/26400) × 16384 = √3 × 16384
        (1200, 13200) => 27566,  // 2·cos(2π·1200/13200) × 16384
        (2200, 13200) => 16384,  // 2·cos(2π·2200/13200) × 16384 = 1.0 × 16384
        (1200, 48000) => 32365,  // 2·cos(2π·1200/48000) × 16384
        (2200, 48000) => 31419,  // 2·cos(2π·2200/48000) × 16384
        (1200, 12000) => 26510,  // 2·cos(2π·1200/12000) × 16384
        (2200, 12000) => 13328,  // 2·cos(2π·2200/12000) × 16384
        // 300 baud: mark=1600 Hz, space=1800 Hz
        (1600, 8000) => 10126,   // 2·cos(2π·1600/8000) × 16384
        (1800, 8000) => 5126,    // 2·cos(2π·1800/8000) × 16384
        (1600, 11025) => 20063,  // 2·cos(2π·1600/11025) × 16384
        (1800, 11025) => 16987,  // 2·cos(2π·1800/11025) × 16384
        (1600, 22050) => 29421,  // 2·cos(2π·1600/22050) × 16384
        (1800, 22050) => 28551,  // 2·cos(2π·1800/22050) × 16384
        (1600, 44100) => 31920,  // 2·cos(2π·1600/44100) × 16384
        (1800, 44100) => 31696,  // 2·cos(2π·1800/44100) × 16384
        (1600, 48000) => 32052,  // 2·cos(2π·1600/48000) × 16384
        (1800, 48000) => 31863,  // 2·cos(2π·1800/48000) × 16384
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
                // Integer cosine via 3rd-order polynomial: cos(x) ≈ 1 - x²/2 + x⁴/24
                // where x = 2π·f/Fs, computed in Q20 fixed-point.
                // Accuracy: <0.5% for x in [0, π], sufficient for Goertzel.
                // pi_q20 = π × 2^20 ≈ 3294199
                const PI_Q20: i64 = 3_294_199;
                let w_q20 = 2 * PI_Q20 * freq as i64 / sample_rate as i64; // 2π·f/Fs in Q20
                // w² in Q20 (shift down 20 to stay in Q20)
                let w2 = (w_q20 * w_q20) >> 20;
                // w⁴ in Q20
                let w4 = (w2 * w2) >> 20;
                // cos(w) ≈ 1 - w²/2 + w⁴/24 in Q20
                let one_q20: i64 = 1 << 20;
                let cos_q20 = one_q20 - (w2 >> 1) + w4 / 24;
                // 2·cos(w) in Q14 = 2·cos_q20 >> 6
                ((2 * cos_q20) >> 6) as i32
            }
        }
    }
}

/// Compute a single window coefficient in Q8 (256 = 1.0).
///
/// Uses `libm::cos` on std, integer polynomial on no_std.
/// Only called at initialization, not in the hot path.
fn compute_window_coeff(window_type: GoertzelWindow, i: usize, n: usize) -> u16 {
    if n <= 1 {
        return 256;
    }
    let nm1 = n - 1;

    // Integer cosine helper: cos(pi * num / den) in Q8 (256 = 1.0)
    // Uses libm on std, polynomial on no_std.
    #[inline]
    fn cos_q8(num: usize, den: usize) -> i16 {
        #[cfg(feature = "std")]
        {
            let theta = core::f64::consts::PI * num as f64 / den as f64;
            (libm::cos(theta) * 256.0) as i16
        }
        #[cfg(not(feature = "std"))]
        {
            // cos(pi * num / den) via Q20 polynomial: cos(x) ≈ 1 - x²/2 + x⁴/24
            const PI_Q20: i64 = 3_294_199;
            let x_q20 = PI_Q20 * num as i64 / den as i64;
            let x2 = (x_q20 * x_q20) >> 20;
            let x4 = (x2 * x2) >> 20;
            let cos_q20 = (1i64 << 20) - (x2 >> 1) + (x4 / 24);
            ((cos_q20 * 256) >> 20).clamp(0, 256) as i16
        }
    }

    let w_q8: i16 = match window_type {
        GoertzelWindow::Rectangular => 256,
        GoertzelWindow::Hann => {
            // 0.5 * (1 - cos(2*pi*i/(N-1))) = 128 - cos(2*pi*i/(N-1)) * 128 / 256
            128 - (cos_q8(2 * i, nm1) / 2)
        }
        GoertzelWindow::Hamming => {
            // 0.54 - 0.46 * cos(2*pi*i/(N-1))
            // In Q8: 138 - 118 * cos / 256
            138 - ((cos_q8(2 * i, nm1) as i32 * 118) / 256) as i16
        }
        GoertzelWindow::Blackman => {
            // 0.42 - 0.5*cos(2pi*i/(N-1)) + 0.08*cos(4pi*i/(N-1))
            // In Q8: 108 - 128*cos1/256 + 20*cos2/256
            let c1 = cos_q8(2 * i, nm1) as i32;
            let c2 = cos_q8(4 * i, nm1) as i32;
            (108 - (c1 * 128 / 256) + (c2 * 20 / 256)) as i16
        }
        GoertzelWindow::EdgeTaper => {
            if i < 2 {
                // cosine taper up: 0.5 * (1 - cos(pi*i/2))
                128 - (cos_q8(i, 2) / 2)
            } else if i >= n - 2 {
                128 - (cos_q8(n - 1 - i, 2) / 2)
            } else {
                256
            }
        }
    };
    w_q8.clamp(0, 256) as u16
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
    /// Whether Goertzel coefficients have been re-tuned from tracker estimates.
    retuned: bool,
    /// Space energy gain in Q8 (256 = unity). Set by adaptive gain.
    space_gain_q8: u16,
    /// Whether adaptive preamble gain measurement is enabled.
    adaptive_gain_enabled: bool,
    /// Shift register for NRZI-decoded bits (flag detection).
    demod_shift_reg: u8,
    /// Accumulated mark energy during preamble (on mark-tone symbols).
    preamble_mark_energy: i64,
    /// Accumulated space energy during preamble (on space-tone symbols).
    preamble_space_energy: i64,
    /// Number of mark-tone symbols accumulated during preamble.
    preamble_mark_count: u16,
    /// Number of space-tone symbols accumulated during preamble.
    preamble_space_count: u16,
    /// Number of HDLC flags (0x7E) detected in current preamble.
    preamble_flag_count: u8,
    /// Symbols since last flag detection (for preamble-end detection).
    symbols_since_last_flag: u8,
}

impl QualityDemodulator {
    /// Create a new quality-path demodulator.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = match config.sample_rate {
            12000 => super::filter::afsk_bandpass_12000(),
            13200 => super::filter::afsk_bandpass_13200(),
            22050 => super::filter::afsk_bandpass_22050(),
            26400 => super::filter::afsk_bandpass_26400(),
            44100 => super::filter::afsk_bandpass_44100(),
            48000 => super::filter::afsk_bandpass_48000(),
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
            retuned: false,
            space_gain_q8: 256,
            adaptive_gain_enabled: false,
            demod_shift_reg: 0,
            preamble_mark_energy: 0,
            preamble_space_energy: 0,
            preamble_mark_count: 0,
            preamble_space_count: 0,
            preamble_flag_count: 0,
            symbols_since_last_flag: 255,
        }
    }

    /// Enable adaptive mark/space gain from preamble measurement.
    pub fn with_adaptive_gain(mut self) -> Self {
        self.adaptive_gain_enabled = true;
        self
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
        // Restore original Goertzel coefficients if they were retuned
        if self.retuned {
            self.mark_coeff = goertzel_coeff(self.config.mark_freq, self.config.sample_rate);
            self.space_coeff = goertzel_coeff(self.config.space_freq, self.config.sample_rate);
            self.retuned = false;
        }
        // Reset adaptive gain state
        if self.adaptive_gain_enabled {
            self.space_gain_q8 = 256;
            self.demod_shift_reg = 0;
            self.preamble_mark_energy = 0;
            self.preamble_space_energy = 0;
            self.preamble_mark_count = 0;
            self.preamble_space_count = 0;
            self.preamble_flag_count = 0;
            self.symbols_since_last_flag = 255;
        }
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

            // 3b. Re-tune Goertzel coefficients when tracker locks
            if !self.retuned && self.tracker.is_locked() {
                let mark_hz = (self.tracker.mark_freq_est >> 8) as u32;
                let space_hz = (self.tracker.space_freq_est >> 8) as u32;
                let mark_delta = (mark_hz as i32 - self.config.mark_freq as i32).unsigned_abs();
                let space_delta = (space_hz as i32 - self.config.space_freq as i32).unsigned_abs();
                if mark_delta < 200 && space_delta < 200 && mark_hz > 0 && space_hz > 0 {
                    self.mark_coeff = goertzel_coeff(mark_hz, sample_rate);
                    self.space_coeff = goertzel_coeff(space_hz, sample_rate);
                    // Reset Goertzel accumulators — old state is invalid under new coefficients
                    self.mark_s1 = 0; self.mark_s2 = 0;
                    self.space_s1 = 0; self.space_s2 = 0;
                }
                self.retuned = true;
            }

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

                let raw_bit = mark_energy * 256 > space_energy * (self.space_gain_q8 as i64);

                // 6. Hybrid LLR: combine energy-based and frequency-based confidence
                // Energy confidence from Goertzel ratio
                let total = mark_energy + space_energy;
                let energy_conf = if total > 0 {
                    let ratio = ((mark_energy - space_energy) * 127) / total;
                    ratio.unsigned_abs().clamp(1, 127) as u8
                } else {
                    1u8
                };

                // Frequency confidence from per-symbol average instantaneous frequency.
                // Only use after tracker lock — before lock, Hilbert estimates are noisy.
                let hybrid_conf = if self.retuned && self.freq_count > 0 {
                    let avg_freq = (self.freq_accum / self.freq_count as i64) as i32;
                    let freq_conf = self.tracker.freq_to_llr(avg_freq).unsigned_abs().max(1);
                    // Use minimum confidence: if either domain shows uncertainty, bit is suspect
                    energy_conf.min(freq_conf).max(1)
                } else {
                    energy_conf
                };

                // 7. NRZI decode
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                // 7b. Adaptive preamble gain (same as FastDemodulator)
                if self.adaptive_gain_enabled {
                    self.demod_shift_reg = (self.demod_shift_reg << 1) | (decoded_bit as u8);

                    if self.demod_shift_reg == 0x7E {
                        self.preamble_flag_count = self.preamble_flag_count.saturating_add(1);
                        self.symbols_since_last_flag = 0;
                    } else {
                        self.symbols_since_last_flag = self.symbols_since_last_flag.saturating_add(1);
                    }

                    if self.preamble_flag_count >= 1 && self.symbols_since_last_flag <= 8 {
                        if mark_energy > space_energy {
                            self.preamble_mark_energy += mark_energy >> AGC_ENERGY_SHIFT;
                            self.preamble_mark_count += 1;
                        } else {
                            self.preamble_space_energy += space_energy >> AGC_ENERGY_SHIFT;
                            self.preamble_space_count += 1;
                        }
                    }

                    if self.symbols_since_last_flag > 8
                        && self.preamble_flag_count >= 2
                        && self.preamble_mark_count > 0
                        && self.preamble_space_count > 0
                    {
                        let mark_avg = self.preamble_mark_energy / self.preamble_mark_count as i64;
                        let space_avg = self.preamble_space_energy / self.preamble_space_count as i64;
                        if space_avg > 0 {
                            let measured = (mark_avg * 256) / space_avg;
                            // Only increase gain (compensate de-emphasis), never decrease.
                            // Apply 25% of measured excess above unity.
                            let excess = (measured - 256).max(0);
                            let gain = 256 + (excess >> 2);
                            self.space_gain_q8 = (gain as u16).min(512);
                        }
                        self.preamble_mark_energy = 0;
                        self.preamble_space_energy = 0;
                        self.preamble_mark_count = 0;
                        self.preamble_space_count = 0;
                        self.preamble_flag_count = 0;
                    }
                }

                // At NRZI transitions the Goertzel window spans both tones,
                // making the energy ratio unreliable. Halve confidence.
                let adj_conf = if !decoded_bit {
                    (hybrid_conf >> 1).max(1)
                } else {
                    hybrid_conf
                };

                let decoded_llr = if decoded_bit { adj_conf as i8 } else { -(adj_conf as i8) };

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
    /// Leaky integrator state for optional PLL input smoothing.
    pll_smooth: i64,
    /// Right-shift for PLL input smoothing. 0 = disabled, 3 = moderate (~7 sample group delay).
    pll_smooth_shift: u8,
    /// Right-shift for LLR confidence mapping from accumulator magnitude.
    /// Default 6. Larger shift → lower confidence → more targeted soft recovery.
    llr_shift: u8,
}

impl DmDemodulator {
    /// Select delay optimized for DM demodulation.
    ///
    /// Short delays minimize transition artifacts (~16-22% of symbol period),
    /// which is critical for detecting single-symbol tones like the space in
    /// a flag pattern (MMMMMMMS). All chosen delays give mark→positive,
    /// space→negative polarity.
    fn dm_delay(config: &DemodConfig) -> usize {
        if config.baud_rate == 300 {
            // 300 baud: τ ≈ 1/(1600+1800) ≈ 294 μs
            match config.sample_rate {
                8000 => 2,    // 250 μs
                11025 => 3,   // 272 μs
                22050 => 6,   // 272 μs
                44100 => 13,  // 295 μs
                48000 => 14,  // 292 μs
                _ => {
                    let approx = config.sample_rate / 3400;
                    approx.clamp(1, super::MAX_DELAY as u32 - 1) as usize
                }
            }
        } else {
            match config.sample_rate {
                11025 => 2,  // 181 μs: mark→+0.20, space→−0.81, 2/9=22%
                13200 => 2,  // 152 μs: mark→+0.36, space→−0.54, 2/11=18%
                22050 => 3,  // 136 μs: mark→+0.52, space→−0.30, 3/18=16%
                26400 => 4,  // 152 μs: mark→+0.36, space→−0.54, 4/22=18%
                44100 => 7,  // 159 μs: mark→+0.37, space→−0.58, 7/37=19%
                48000 => 8,  // 167 μs: mark→+0.31, space→−0.67, 8/40=20%
                _ => {
                    let approx = config.sample_rate / 6000;
                    if approx < 1 { 1 }
                    else if approx >= super::MAX_DELAY as u32 { super::MAX_DELAY - 1 }
                    else { approx as usize }
                }
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
            (1, _) | (2, 11025) | (2, 13200) | (3, 22050) | (4, 26400) | (7, 44100) | (8, 48000) => false,
            // dm_delay_filtered (long, real-world): all mark→positive
            (8, 11025) | (10, 13200) | (16, 22050) | (19, 26400) | (31, 44100) | (31, 48000) => false,
            // d=5 at 11025 (alt delay): mark→negative
            (5, 11025) | (6, 13200) | (10, 22050) | (12, 26400) | (20, 44100) => true,
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
            13200 => 10,  // 758 μs: ~1 symbol at 13200 Hz (11 sps)
            22050 => 16,  // 726 μs: same τ
            26400 => 19,  // 720 μs: ~1 symbol at 26400 Hz (22 sps)
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
            Self::dm_delay_filtered_for_config(&config)
        } else {
            Self::dm_delay(&config)
        };
        let lpf = if use_bpf {
            Self::post_detect_lpf_for_config(&config)
        } else {
            BiquadFilter::passthrough()
        };
        let detector = DelayMultiplyDetector::with_delay(delay, lpf);

        let bpf = if use_bpf {
            Some(Self::select_dm_bpf(&config))
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
            pll_smooth: 0,
            pll_smooth_shift: 0,
            llr_shift: 6,
        }
    }

    /// Create with BPF + PLL clock recovery for real-world signals.
    ///
    /// Uses the filtered delay (d=8 at 11025 Hz) with BPF+LPF preprocessing
    /// for clean discriminator output, and Gardner TED PLL for adaptive symbol
    /// timing with both phase and frequency correction.
    pub fn with_bpf_pll(config: DemodConfig) -> Self {
        Self::make_pll(config, config.pll_alpha, config.pll_beta, 0)
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

    /// Baud-aware filtered delay selection.
    fn dm_delay_filtered_for_config(config: &DemodConfig) -> usize {
        if config.baud_rate == 300 {
            // 300 baud: delay ≈ 1 symbol period = sample_rate / 300
            match config.sample_rate {
                8000 => 27,   // 3375 μs (~1 symbol)
                11025 => 37,  // 3356 μs (~1 symbol)
                22050 => 37,  // 1678 μs (~half symbol, limited by MAX_DELAY)
                44100 => 47,  // 1066 μs (~1/3 symbol, limited by MAX_DELAY)
                48000 => 47,  // 979 μs (~1/3 symbol, limited by MAX_DELAY)
                _ => {
                    let d = config.sample_rate as usize / 300;
                    d.clamp(1, super::MAX_DELAY - 1)
                }
            }
        } else {
            Self::dm_delay_filtered(config.sample_rate)
        }
    }

    /// Baud-aware post-detection LPF selection.
    fn post_detect_lpf_for_config(config: &DemodConfig) -> BiquadFilter {
        if config.baud_rate == 300 {
            super::filter::post_detect_lpf_300(config.sample_rate)
        } else {
            super::filter::post_detect_lpf(config.sample_rate)
        }
    }

    /// Select the appropriate BPF for DM demodulator based on baud rate.
    fn select_dm_bpf(config: &DemodConfig) -> BiquadFilter {
        if config.baud_rate == 300 {
            match config.sample_rate {
                8000 => super::filter::afsk_300_bandpass_8000(),
                22050 => super::filter::afsk_300_bandpass_22050(),
                44100 => super::filter::afsk_300_bandpass_44100(),
                48000 => super::filter::afsk_300_bandpass_48000(),
                _ => super::filter::afsk_300_bandpass_11025(),
            }
        } else {
            match config.sample_rate {
                13200 => super::filter::afsk_bandpass_13200(),
                22050 => super::filter::afsk_bandpass_22050(),
                26400 => super::filter::afsk_bandpass_26400(),
                44100 => super::filter::afsk_bandpass_44100(),
                _ => super::filter::afsk_bandpass_11025(),
            }
        }
    }

    fn make_pll(config: DemodConfig, alpha: i16, beta: i16, hysteresis: i16) -> Self {
        let delay = Self::dm_delay_filtered_for_config(&config);
        let lpf = Self::post_detect_lpf_for_config(&config);
        let detector = DelayMultiplyDetector::with_delay(delay, lpf);
        let bpf = Some(Self::select_dm_bpf(&config));
        let mark_is_negative = Self::is_mark_negative(delay, config.sample_rate);
        let pll = ClockRecoveryPll::new_gardner(config.sample_rate, config.baud_rate, alpha, beta)
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
            pll_smooth: 0,
            pll_smooth_shift: 0,
            llr_shift: 6,
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

    /// Enable PLL input smoothing with a leaky integrator (builder pattern).
    ///
    /// `shift` controls the smoothing time constant: `smooth -= smooth >> shift; smooth += disc_out`.
    /// 0 = disabled (default), 3 = moderate (~7 sample group delay), 5 = heavy smoothing.
    /// With Gardner TED (immune to group delay), smoothing may help de-emphasized signals.
    pub fn with_pll_smoothing(mut self, shift: u8) -> Self {
        self.pll_smooth_shift = shift;
        self
    }

    /// Set the LLR confidence right-shift (builder pattern).
    ///
    /// Maps accumulator magnitude to [1,127] confidence: `abs(accumulator) >> shift`.
    /// Default 6. At 11025 Hz, accumulators ~9000 → shift=6 gives ~140 (clamped to 127).
    /// Larger shift → lower confidence → more targeted soft recovery on weak symbols.
    pub fn with_llr_shift(mut self, shift: u8) -> Self {
        self.llr_shift = shift;
        self
    }

    /// Set the PLL Gardner error right-shift (builder pattern).
    ///
    /// Forwarded to the underlying `ClockRecoveryPll::with_error_shift()`.
    /// Only effective when PLL is enabled (via `with_bpf_pll` constructors).
    pub fn with_pll_error_shift(mut self, shift: u8) -> Self {
        if let Some(ref mut pll) = self.pll {
            pll.set_error_shift(shift);
        }
        self
    }

    /// Set PLL maximum drift range.
    /// `denom` is the denominator: max_drift = nominal / denom.
    /// E.g., denom=20 → ±5%, denom=50 → ±2% (default).
    pub fn with_pll_max_drift(mut self, denom: i32) -> Self {
        if let Some(ref mut pll) = self.pll {
            pll.set_max_drift(denom);
        }
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
            13200 => super::filter::afsk_bandpass_13200(),
            22050 => super::filter::afsk_bandpass_22050(),
            26400 => super::filter::afsk_bandpass_26400(),
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
            pll_smooth: 0,
            pll_smooth_shift: 0,
            llr_shift: 6,
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
        self.pll_smooth = 0;
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
                // Optional leaky integrator smoothing before PLL input.
                // With Gardner TED (immune to group delay), smoothing may
                // help de-emphasized signals without breaking beta correction.
                let pll_input = if self.pll_smooth_shift > 0 {
                    self.pll_smooth -= self.pll_smooth >> self.pll_smooth_shift;
                    self.pll_smooth += disc_out as i64;
                    (self.pll_smooth >> self.pll_smooth_shift).clamp(-32768, 32767) as i16
                } else {
                    disc_out
                };
                // Safety: use_pll is set from self.pll.is_some() above
                self.pll.as_mut().expect("PLL checked above").update(pll_input).is_some()
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
                    // LLR from accumulator magnitude: large |accumulator| =
                    // consistent tone = high confidence; small = transition/noise.
                    let confidence = (self.accumulator.abs() >> self.llr_shift).clamp(1, 127) as i8;
                    let llr = if decoded_bit { confidence } else { -confidence };
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr,
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

/// Correlation (mixer) demodulator — DireWolf-style tone detection.
///
/// Pipeline: [BPF →] Mix with mark sin/cos + space sin/cos → 4× LPF →
///           envelope (I²+Q²) → Bresenham timing → NRZI → HDLC
///
/// This is fundamentally different from Goertzel: instead of computing
/// energy over a rectangular window that resets every symbol, we multiply
/// the input by local oscillators and lowpass filter to extract continuous
/// envelopes. The LPF acts as a matched filter whose bandwidth determines
/// selectivity (not a window length).
///
/// Advantages over Goertzel:
/// - Continuous integration (no window reset artifacts at symbol boundaries)
/// - LPF response is independent of tone frequency (handles de-emphasis gracefully)
/// - I/Q output provides phase information
///
/// Cost: ~24 ops/sample (4 multiplies + 4 LPF updates) vs Goertzel's ~10.
/// Memory: ~180 bytes (4 biquad states + NCO + Bresenham).
pub struct CorrelationDemodulator {
    #[allow(dead_code)]
    config: DemodConfig,
    bpf: BiquadFilter,
    /// Optional second BPF stage for cascaded filtering.
    bpf2: Option<BiquadFilter>,
    /// NCO phase accumulators for mark and space local oscillators.
    /// Phase is in Q24 format: 0..16777216 maps to 0..2π.
    mark_phase: u32,
    space_phase: u32,
    /// NCO phase increment per sample (Q24): freq * 2^24 / sample_rate
    mark_phase_inc: u32,
    space_phase_inc: u32,
    /// Lowpass filters for the 4 mixer output channels
    mark_i_lpf: BiquadFilter,
    mark_q_lpf: BiquadFilter,
    space_i_lpf: BiquadFilter,
    space_q_lpf: BiquadFilter,
    /// Decimation factor for mixer outputs (1 = none, 2 or 4).
    /// For 300 baud at high sample rates, we decimate mixer outputs so the LPF
    /// runs at ~11025 Hz where Q15 coefficients have adequate precision.
    corr_decim_factor: u8,
    /// Current count within decimation block.
    corr_decim_count: u8,
    /// Accumulators for mixer output decimation: [mark_i, mark_q, space_i, space_q].
    corr_decim_acc: [i32; 4],
    /// Effective sample rate after decimation (for Bresenham/PLL timing).
    effective_sample_rate: u32,
    /// Bresenham fractional bit timing
    bit_phase: u32,
    prev_nrzi_bit: bool,
    samples_processed: u64,
    /// Space energy gain in Q8 (256 = unity)
    space_gain_q8: u16,
    /// Whether to produce energy-based LLR
    energy_llr: bool,
    /// Whether adaptive preamble gain measurement is enabled
    adaptive_gain_enabled: bool,
    /// Shift register for NRZI-decoded bits (flag detection)
    demod_shift_reg: u8,
    /// Accumulated mark energy during preamble
    preamble_mark_energy: i64,
    /// Accumulated space energy during preamble
    preamble_space_energy: i64,
    /// Number of mark-tone symbols accumulated during preamble
    preamble_mark_count: u16,
    /// Number of space-tone symbols accumulated during preamble
    preamble_space_count: u16,
    /// Number of HDLC flags detected in current preamble
    preamble_flag_count: u8,
    /// Symbols since last flag detection
    symbols_since_last_flag: u8,
    /// Optional PLL for adaptive symbol timing (Gardner TED).
    /// When present, replaces Bresenham fixed-rate timing.
    pll: Option<ClockRecoveryPll>,
}

impl CorrelationDemodulator {
    /// Compute NCO phase increment for a given frequency.
    /// Returns freq * 2^24 / sample_rate.
    fn phase_inc(freq: u32, sample_rate: u32) -> u32 {
        ((freq as u64 * (1u64 << 24)) / sample_rate as u64) as u32
    }

    /// Select the appropriate BPF for a given config (baud rate + sample rate).
    fn select_bpf(config: &DemodConfig) -> BiquadFilter {
        if config.baud_rate == 300 {
            match config.sample_rate {
                8000 => super::filter::afsk_300_bandpass_8000(),
                22050 => super::filter::afsk_300_bandpass_22050(),
                44100 => super::filter::afsk_300_bandpass_44100(),
                48000 => super::filter::afsk_300_bandpass_48000(),
                _ => super::filter::afsk_300_bandpass_11025(),
            }
        } else {
            match config.sample_rate {
                12000 => super::filter::afsk_bandpass_12000(),
                13200 => super::filter::afsk_bandpass_13200(),
                22050 => super::filter::afsk_bandpass_22050(),
                26400 => super::filter::afsk_bandpass_26400(),
                44100 => super::filter::afsk_bandpass_44100(),
                48000 => super::filter::afsk_bandpass_48000(),
                _ => super::filter::afsk_bandpass_11025(),
            }
        }
    }

    /// Create a new correlation demodulator.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = Self::select_bpf(&config);

        // For 300 baud at high sample rates, decimate mixer outputs so the LPF
        // runs at ~11025 Hz where Q15 coefficients have adequate precision.
        // Without decimation, 120 Hz LPF at 44100 Hz has b0=2 (truncates to 0).
        let (decim_factor, effective_rate) = if config.baud_rate == 300 {
            if config.sample_rate >= 44100 {
                (4u8, config.sample_rate / 4)  // 44100→11025, 48000→12000
            } else if config.sample_rate >= 22050 {
                (2u8, config.sample_rate / 2)  // 22050→11025
            } else {
                (1u8, config.sample_rate)
            }
        } else {
            (1u8, config.sample_rate)
        };

        // Select LPF for effective (decimated) sample rate
        let lpf = super::filter::corr_lpf_for_config(
            config.mark_freq, config.space_freq, config.baud_rate, effective_rate,
        );

        Self {
            config,
            bpf,
            bpf2: None,
            mark_phase: 0,
            space_phase: 0,
            mark_phase_inc: Self::phase_inc(config.mark_freq, config.sample_rate),
            space_phase_inc: Self::phase_inc(config.space_freq, config.sample_rate),
            mark_i_lpf: lpf,
            mark_q_lpf: lpf,
            space_i_lpf: lpf,
            space_q_lpf: lpf,
            corr_decim_factor: decim_factor,
            corr_decim_count: 0,
            corr_decim_acc: [0; 4],
            effective_sample_rate: effective_rate,
            bit_phase: 0,
            prev_nrzi_bit: false,
            samples_processed: 0,
            space_gain_q8: 256,
            energy_llr: false,
            adaptive_gain_enabled: false,
            demod_shift_reg: 0,
            preamble_mark_energy: 0,
            preamble_space_energy: 0,
            preamble_mark_count: 0,
            preamble_space_count: 0,
            preamble_flag_count: 0,
            symbols_since_last_flag: 255,
            pll: None,
        }
    }

    /// Create with a custom bandpass filter and initial timing offset.
    pub fn with_filter_and_offset(config: DemodConfig, bpf: BiquadFilter, phase_offset: u32) -> Self {
        let mut d = Self::new(config);
        d.bpf = bpf;
        d.bit_phase = phase_offset;
        d
    }

    /// Set space energy gain for multi-slicer diversity (Q8 format).
    pub fn with_space_gain(mut self, gain_q8: u16) -> Self {
        self.space_gain_q8 = gain_q8;
        self
    }

    /// Set the initial Bresenham timing phase.
    pub fn set_bit_phase(&mut self, phase: u32) {
        self.bit_phase = phase;
    }

    /// Enable cascaded (4th-order) bandpass filtering.
    pub fn with_cascade_bpf(mut self) -> Self {
        self.bpf2 = Some(self.bpf);
        self
    }

    /// Set custom LPF for correlation channels.
    ///
    /// Overrides the default 600 Hz cutoff LPF with a custom filter.
    /// Useful for sweeping LPF cutoff to find the optimal value.
    pub fn with_corr_lpf(mut self, lpf: BiquadFilter) -> Self {
        self.mark_i_lpf = lpf;
        self.mark_q_lpf = lpf;
        self.space_i_lpf = lpf;
        self.space_q_lpf = lpf;
        // Disable decimation when explicitly overriding LPF
        self.corr_decim_factor = 1;
        self.effective_sample_rate = self.config.sample_rate;
        self
    }

    /// Enable energy-based LLR output.
    pub fn with_energy_llr(mut self) -> Self {
        self.energy_llr = true;
        self
    }

    /// Enable adaptive mark/space gain from preamble measurement.
    pub fn with_adaptive_gain(mut self) -> Self {
        self.adaptive_gain_enabled = true;
        self
    }

    /// Enable Gardner PLL timing recovery with default parameters.
    ///
    /// Uses an absolute-value envelope discriminator to drive the PLL:
    /// `disc = (|mark_i| + |mark_q|) - (|space_i| + |space_q|)`
    /// This uses the **same signal path** as the bit decision (no group delay
    /// mismatch), unlike the failed Goertzel+DM PLL experiments.
    pub fn with_pll(mut self) -> Self {
        self.pll = Some(
            ClockRecoveryPll::new_gardner(self.effective_sample_rate, self.config.baud_rate, 936, 0)
                .with_error_shift(8)
        );
        self
    }

    /// Enable PLL timing recovery with a custom-configured PLL.
    pub fn with_custom_pll(mut self, pll: ClockRecoveryPll) -> Self {
        self.pll = Some(pll);
        self
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        self.bpf.reset();
        if let Some(ref mut bpf2) = self.bpf2 {
            bpf2.reset();
        }
        self.mark_phase = 0;
        self.space_phase = 0;
        self.mark_i_lpf.reset();
        self.mark_q_lpf.reset();
        self.space_i_lpf.reset();
        self.space_q_lpf.reset();
        self.corr_decim_count = 0;
        self.corr_decim_acc = [0; 4];
        self.bit_phase = 0;
        self.prev_nrzi_bit = false;
        self.samples_processed = 0;
        if self.adaptive_gain_enabled {
            self.space_gain_q8 = 256;
            self.demod_shift_reg = 0;
            self.preamble_mark_energy = 0;
            self.preamble_space_energy = 0;
            self.preamble_mark_count = 0;
            self.preamble_space_count = 0;
            self.preamble_flag_count = 0;
            self.symbols_since_last_flag = 255;
        }
        if let Some(ref mut pll) = self.pll {
            pll.reset();
        }
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
        let baud_rate = self.config.baud_rate;

        for &sample in samples {
            self.samples_processed += 1;

            // 1. Bandpass filter (optionally cascaded)
            let bpf1_out = self.bpf.process(sample);
            let filtered = if let Some(ref mut bpf2) = self.bpf2 {
                bpf2.process(bpf1_out)
            } else {
                bpf1_out
            };
            let x = filtered as i32;

            // 2. Mix with local oscillators (NCO)
            // Phase is Q24: top 8 bits index into 256-entry sin table
            let mark_sin_idx = (self.mark_phase >> 16) as u8;
            let mark_cos_idx = mark_sin_idx.wrapping_add(64); // cos = sin + π/2
            let space_sin_idx = (self.space_phase >> 16) as u8;
            let space_cos_idx = space_sin_idx.wrapping_add(64);

            // Mixer outputs (Q15 × Q15 → Q30, shift to Q15)
            let mark_i_raw = (x * super::SIN_TABLE_Q15[mark_sin_idx as usize] as i32) >> 15;
            let mark_q_raw = (x * super::SIN_TABLE_Q15[mark_cos_idx as usize] as i32) >> 15;
            let space_i_raw = (x * super::SIN_TABLE_Q15[space_sin_idx as usize] as i32) >> 15;
            let space_q_raw = (x * super::SIN_TABLE_Q15[space_cos_idx as usize] as i32) >> 15;

            // Advance NCO phase
            self.mark_phase = self.mark_phase.wrapping_add(self.mark_phase_inc);
            self.space_phase = self.space_phase.wrapping_add(self.space_phase_inc);

            // 3. Decimation + Lowpass filter
            // For 300 baud at high sample rates, accumulate mixer outputs and
            // process LPF/timing at the decimated rate (~11025 Hz).
            self.corr_decim_acc[0] += mark_i_raw;
            self.corr_decim_acc[1] += mark_q_raw;
            self.corr_decim_acc[2] += space_i_raw;
            self.corr_decim_acc[3] += space_q_raw;
            self.corr_decim_count += 1;

            if self.corr_decim_count < self.corr_decim_factor {
                continue;
            }

            // Decimation complete: average and reset accumulators
            let df = self.corr_decim_factor as i32;
            let dec_mi = (self.corr_decim_acc[0] / df) as i16;
            let dec_mq = (self.corr_decim_acc[1] / df) as i16;
            let dec_si = (self.corr_decim_acc[2] / df) as i16;
            let dec_sq = (self.corr_decim_acc[3] / df) as i16;
            self.corr_decim_acc = [0; 4];
            self.corr_decim_count = 0;

            // Lowpass filter at the decimated rate
            let mark_i = self.mark_i_lpf.process(dec_mi);
            let mark_q = self.mark_q_lpf.process(dec_mq);
            let space_i = self.space_i_lpf.process(dec_si);
            let space_q = self.space_q_lpf.process(dec_sq);

            // 4. Symbol timing: PLL or Bresenham (at decimated rate)
            let eff_rate = self.effective_sample_rate;
            let boundary = if let Some(ref mut pll) = self.pll {
                let mark_env = (mark_i as i32).abs() + (mark_q as i32).abs();
                let space_env = (space_i as i32).abs() + (space_q as i32).abs();
                let total = mark_env + space_env;
                let disc = if total > 0 {
                    (((mark_env - space_env) as i64 * 127) / total as i64).clamp(-127, 127) as i16
                } else {
                    0
                };
                pll.update(disc).is_some()
            } else {
                self.bit_phase += baud_rate;
                if self.bit_phase >= eff_rate {
                    self.bit_phase -= eff_rate;
                    true
                } else {
                    false
                }
            };

            if boundary {
                // 5. Envelope detection: I² + Q²
                let mark_energy = (mark_i as i64) * (mark_i as i64)
                    + (mark_q as i64) * (mark_q as i64);
                let space_energy = (space_i as i64) * (space_i as i64)
                    + (space_q as i64) * (space_q as i64);

                // 6. Hard bit decision with space gain
                let raw_bit = mark_energy * 256 > space_energy * (self.space_gain_q8 as i64);

                // 7. NRZI decode
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                // 7b. Adaptive preamble gain
                if self.adaptive_gain_enabled {
                    self.demod_shift_reg = (self.demod_shift_reg << 1) | (decoded_bit as u8);

                    if self.demod_shift_reg == 0x7E {
                        self.preamble_flag_count = self.preamble_flag_count.saturating_add(1);
                        self.symbols_since_last_flag = 0;
                    } else {
                        self.symbols_since_last_flag = self.symbols_since_last_flag.saturating_add(1);
                    }

                    if self.preamble_flag_count >= 1 && self.symbols_since_last_flag <= 8 {
                        if mark_energy > space_energy {
                            self.preamble_mark_energy += mark_energy >> AGC_ENERGY_SHIFT;
                            self.preamble_mark_count += 1;
                        } else {
                            self.preamble_space_energy += space_energy >> AGC_ENERGY_SHIFT;
                            self.preamble_space_count += 1;
                        }
                    }

                    if self.symbols_since_last_flag > 8
                        && self.preamble_flag_count >= 2
                        && self.preamble_mark_count > 0
                        && self.preamble_space_count > 0
                    {
                        let mark_avg = self.preamble_mark_energy / self.preamble_mark_count as i64;
                        let space_avg = self.preamble_space_energy / self.preamble_space_count as i64;
                        if space_avg > 0 {
                            let measured = (mark_avg * 256) / space_avg;
                            let excess = (measured - 256).max(0);
                            let gain = 256 + (excess >> 2);
                            self.space_gain_q8 = (gain as u16).min(512);
                        }
                        self.preamble_mark_energy = 0;
                        self.preamble_space_energy = 0;
                        self.preamble_mark_count = 0;
                        self.preamble_space_count = 0;
                        self.preamble_flag_count = 0;
                    }
                }

                // 8. LLR
                let llr = if self.energy_llr {
                    let total = mark_energy + space_energy;
                    if total > 0 {
                        let energy_ratio = ((mark_energy - space_energy) * 127) / total;
                        let confidence = energy_ratio.unsigned_abs().clamp(1, 127) as i8;
                        if decoded_bit { confidence } else { -confidence }
                    } else {
                        0
                    }
                } else if decoded_bit {
                    64
                } else {
                    -64
                };

                if sym_count < symbols_out.len() {
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr,
                    };
                    sym_count += 1;
                }
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

    #[test]
    fn test_corr_demod_creation() {
        let config = DemodConfig::default_1200();
        let demod = CorrelationDemodulator::new(config);
        assert_eq!(demod.samples_processed, 0);
    }

    #[test]
    fn test_corr_demod_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut demod = CorrelationDemodulator::new(config);
        let silence = [0i16; 1000];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 200];

        let n = demod.process_samples(&silence, &mut symbols);
        assert!(n < 200);
    }

    #[test]
    fn test_corr_demod_pll_creation() {
        let config = DemodConfig::default_1200();
        let demod = CorrelationDemodulator::new(config).with_pll();
        assert!(demod.pll.is_some());
    }

    #[test]
    fn test_corr_demod_pll_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut demod = CorrelationDemodulator::new(config).with_pll();
        let silence = [0i16; 1000];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 200];

        let n = demod.process_samples(&silence, &mut symbols);
        // PLL should still produce symbols (at approximately baud rate)
        assert!(n < 200);
    }

    #[test]
    fn test_corr_demod_reset() {
        let config = DemodConfig::default_1200();
        let mut demod = CorrelationDemodulator::new(config);
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
                13200 => crate::modem::filter::afsk_bandpass_13200,
                22050 => crate::modem::filter::afsk_bandpass_22050,
                26400 => crate::modem::filter::afsk_bandpass_26400,
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
    fn test_loopback_corr_clean() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Corr");
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
        let mut demod = CorrelationDemodulator::new(config);
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

        let (dec_buf, dec_len) = decoded.expect("Correlation demod should decode clean signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "Correlation demod decoded frame doesn't match original");
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

    // ─── 300 Baud Loopback Tests ─────────────────────────────────────────

    #[test]
    fn test_300_baud_loopback_fast() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Test 300 baud");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        let mut modulator = AfskModulator::new(ModConfig::default_300());
        let mut audio = [0i16; 262144]; // 300 baud needs 4x more samples
        let mut audio_len = 0;

        // Extra preamble for 300 baud (longer symbols need more flags)
        for _ in 0..80 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }

        for i in 0..encoded.bit_count {
            let bit = encoded.bits[i] != 0;
            let n = modulator.modulate_bit(bit, &mut audio[audio_len..]);
            audio_len += n;
        }

        // Trailing silence
        for _ in 0..800 {
            if audio_len < audio.len() {
                audio_len += 1;
            }
        }

        let config = DemodConfig::default_300();
        let mut demod = FastDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 16384];
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

        let (dec_buf, dec_len) = decoded.expect("300 baud fast path should decode clean signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "300 baud fast path decoded frame doesn't match original");
    }

    #[test]
    fn test_300_baud_loopback_dm() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Test 300 baud DM");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        let mut modulator = AfskModulator::new(ModConfig::default_300());
        let mut audio = [0i16; 262144];
        let mut audio_len = 0;

        for _ in 0..80 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }

        for i in 0..encoded.bit_count {
            let bit = encoded.bits[i] != 0;
            let n = modulator.modulate_bit(bit, &mut audio[audio_len..]);
            audio_len += n;
        }

        for _ in 0..800 {
            if audio_len < audio.len() {
                audio_len += 1;
            }
        }

        let config = DemodConfig::default_300();
        let mut demod = DmDemodulator::with_bpf(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 16384];
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

        let (dec_buf, dec_len) = decoded.expect("300 baud DM path should decode clean signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "300 baud DM decoded frame doesn't match original");
    }

    #[test]
    fn test_300_baud_loopback_corr() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Test 300 baud Corr");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        let mut modulator = AfskModulator::new(ModConfig::default_300());
        let mut audio = [0i16; 262144];
        let mut audio_len = 0;

        for _ in 0..80 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }

        for i in 0..encoded.bit_count {
            let bit = encoded.bits[i] != 0;
            let n = modulator.modulate_bit(bit, &mut audio[audio_len..]);
            audio_len += n;
        }

        for _ in 0..800 {
            if audio_len < audio.len() {
                audio_len += 1;
            }
        }

        let config = DemodConfig::default_300();
        let mut demod = CorrelationDemodulator::new(config);
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 16384];
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

        let (dec_buf, dec_len) = decoded.expect("300 baud correlation path should decode clean signal");
        assert_eq!(&dec_buf[..dec_len], raw,
            "300 baud correlation decoded frame doesn't match original");
    }
}
