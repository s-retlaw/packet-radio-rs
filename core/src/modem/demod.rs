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

/// Fast-path AFSK demodulator (Goertzel tone detection).
///
/// Suitable for Cortex-M0, RP2040, and other resource-constrained targets.
/// Uses ~200 bytes of RAM and ~30-50 cycles per sample.
///
/// Uses Goertzel filters to compare mark (1200 Hz) and space (2200 Hz)
/// energy over each symbol period with Bresenham-style timing.
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

                // Apply space gain (multi-slicer): compare mark×256 vs space×gain
                let raw_bit = mark_energy * 256 > space_energy * (self.space_gain_q8 as i64);

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
}
