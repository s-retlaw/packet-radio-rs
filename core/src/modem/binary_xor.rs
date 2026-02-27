//! Binary XOR Correlator — 4th demodulator architecture.
//!
//! Inspired by the Mobilinkd TNC3 digital correlator. Digitizes audio at
//! zero-crossings before correlation (XOR vs analog multiply), making it
//! naturally twist-immune (amplitude discarded) and extremely cheap.
//!
//! ```text
//! Audio → BPF → Zero-Crossing Digitize → Delay Line → XOR(bit[n], bit[n−τ])
//!   → Map XOR to ±1 → LPF → Accumulate → Bresenham Timing → NRZI → HDLC
//! ```
//!
//! **Key difference from DM**: Digitizes BEFORE correlation. DM multiplies
//! analog samples; XOR operates on binary (1-bit quantized) values. This
//! discards amplitude info, making it inherently robust to de-emphasis and
//! level variations.

use super::DemodConfig;
use super::demod::DemodSymbol;
use super::filter::BiquadFilter;

/// Binary XOR correlator demodulator.
///
/// Ultra-cheap (~100 bytes RAM per instance). Uses zero-crossing digitization
/// followed by delayed XOR correlation, giving twist immunity and robustness
/// to amplitude variations.
pub struct BinaryXorDemodulator {
    config: DemodConfig,
    bpf: BiquadFilter,

    /// Zero-crossing digitizer — binary delay line (packed u32, 1 bit per sample).
    /// Bit 0 = newest sample, bit[delay] = delayed sample.
    delay_bits: u32,

    /// Delay in samples (e.g. 8 at 11025 Hz).
    delay: usize,

    /// Post-detection LPF (smooths XOR output, removes 2f component).
    lpf: BiquadFilter,

    /// Accumulator (integrates LPF output over symbol period).
    accumulator: i64,

    /// Number of samples accumulated in current symbol.
    accum_count: u32,

    /// Bresenham fractional bit timing.
    bit_phase: u32,

    /// Previous NRZI bit for differential decoding.
    prev_nrzi_bit: bool,

    /// Whether mark tone produces positive XOR→LPF output.
    /// Depends on delay τ — same analysis as DM's `is_mark_negative`.
    mark_is_positive: bool,

    /// LLR confidence right-shift: maps accumulator magnitude to [1..127].
    llr_shift: u8,

    /// Total samples processed (for diagnostics).
    pub samples_processed: u64,
}

impl BinaryXorDemodulator {
    /// Select the appropriate BPF for a given sample rate.
    fn select_bpf(sample_rate: u32) -> BiquadFilter {
        match sample_rate {
            13200 => super::filter::afsk_bandpass_13200(),
            22050 => super::filter::afsk_bandpass_22050(),
            26400 => super::filter::afsk_bandpass_26400(),
            44100 => super::filter::afsk_bandpass_44100(),
            _ => super::filter::afsk_bandpass_11025(),
        }
    }

    /// Optimal delay for XOR correlation with BPF+LPF smoothing.
    ///
    /// Uses τ ≈ 726 μs (~1 symbol period), same as DM filtered delay.
    /// At this delay, mark (1200 Hz) and space (2200 Hz) produce maximally
    /// different XOR patterns after LPF smoothing.
    fn xor_delay(sample_rate: u32) -> usize {
        match sample_rate {
            11025 => 8,   // 726 μs
            13200 => 10,  // 758 μs
            22050 => 16,  // 726 μs
            26400 => 19,  // 720 μs
            44100 => 31,  // 703 μs
            48000 => 31,  // 646 μs
            _ => {
                let d = sample_rate as usize / 1400;
                d.clamp(1, 31)
            }
        }
    }

    /// Determine if mark tone produces positive XOR→LPF output.
    ///
    /// For the XOR correlator, mark (1200 Hz) at delay τ: the zero-crossing
    /// pattern repeats with period T=1/1200. When τ ≈ T, consecutive bits
    /// are nearly identical → XOR ≈ 0 → LPF output negative (mapped: 0→-16000).
    /// Space (2200 Hz) at delay τ: faster oscillation means more phase change
    /// → XOR ≈ 1 → LPF output positive.
    ///
    /// This is the INVERSE of DM's `is_mark_negative` — when DM says
    /// mark→positive (delay product positive for in-phase), XOR says
    /// mark→negative (XOR=0 for in-phase → mapped to -16000).
    fn is_mark_positive(delay: usize, sample_rate: u32) -> bool {
        // XOR polarity is opposite to DM multiply polarity.
        // DM: in-phase → positive product → mark positive (for standard delays).
        // XOR: in-phase → XOR=0 → mapped to -16000 → mark negative.
        //
        // So mark_is_positive = !dm_mark_is_positive = dm_is_mark_negative.
        // Reuse the same logic as DM's is_mark_negative.
        match (delay, sample_rate) {
            // Standard delays where DM says mark→positive (not negative):
            // XOR inverts: mark→negative. So is_mark_positive = false.
            (1, _) | (2, 11025) | (2, 13200) | (3, 22050) | (4, 26400) | (7, 44100) | (8, 48000) => false,
            (8, 11025) | (10, 13200) | (16, 22050) | (19, 26400) | (31, 44100) | (31, 48000) => false,
            // d=5 at 11025 etc: DM says mark→negative, so XOR says mark→positive
            (5, 11025) | (6, 13200) | (10, 22050) | (12, 26400) | (20, 44100) => true,
            _ => {
                #[cfg(feature = "std")]
                {
                    let tau = delay as f64 / sample_rate as f64;
                    // DM is_mark_negative when cos(2π·1200·τ) < 0
                    // XOR is_mark_positive when DM is_mark_negative
                    libm::cos(2.0 * core::f64::consts::PI * 1200.0 * tau) < 0.0
                }
                #[cfg(not(feature = "std"))]
                {
                    let prod = delay as u64 * 4800;
                    prod > sample_rate as u64 && prod < 3 * sample_rate as u64
                }
            }
        }
    }

    /// Create a new Binary XOR demodulator with BPF + LPF.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = Self::select_bpf(config.sample_rate);
        let delay = Self::xor_delay(config.sample_rate);
        let lpf = super::filter::post_detect_lpf(config.sample_rate);
        let mark_is_positive = Self::is_mark_positive(delay, config.sample_rate);

        Self {
            config,
            bpf,
            delay_bits: 0,
            delay,
            lpf,
            accumulator: 0,
            accum_count: 0,
            bit_phase: 0,
            prev_nrzi_bit: false,
            mark_is_positive,
            llr_shift: 6,
            samples_processed: 0,
        }
    }

    /// Create with a custom BPF filter and timing phase offset.
    pub fn with_filter_and_offset(config: DemodConfig, bpf: BiquadFilter, phase_offset: u32) -> Self {
        let delay = Self::xor_delay(config.sample_rate);
        let lpf = super::filter::post_detect_lpf(config.sample_rate);
        let mark_is_positive = Self::is_mark_positive(delay, config.sample_rate);

        Self {
            config,
            bpf,
            delay_bits: 0,
            delay,
            lpf,
            accumulator: 0,
            accum_count: 0,
            bit_phase: phase_offset,
            prev_nrzi_bit: false,
            mark_is_positive,
            llr_shift: 6,
            samples_processed: 0,
        }
    }

    /// Override the delay value (builder pattern).
    pub fn with_delay(mut self, delay: usize) -> Self {
        assert!(delay > 0 && delay <= 31, "delay must be 1..=31 (packed in u32)");
        self.delay = delay;
        self.mark_is_positive = Self::is_mark_positive(delay, self.config.sample_rate);
        self
    }

    /// Enable energy-ratio LLR (builder pattern).
    ///
    /// Maps accumulator magnitude to confidence [1..127] using right-shift.
    /// Default shift=6 is calibrated for 11025 Hz.
    pub fn with_energy_llr(self) -> Self {
        // LLR is always on — this is a no-op for API consistency.
        // The accumulator magnitude naturally provides LLR.
        self
    }

    /// Set the LLR confidence right-shift (builder pattern).
    pub fn with_llr_shift(mut self, shift: u8) -> Self {
        self.llr_shift = shift;
        self
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        self.bpf.reset();
        self.lpf.reset();
        self.delay_bits = 0;
        self.accumulator = 0;
        self.accum_count = 0;
        self.bit_phase = 0;
        self.prev_nrzi_bit = false;
        self.samples_processed = 0;
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

            // 2. Zero-crossing digitize: 1 if positive, 0 if non-positive
            let current_bit = filtered > 0;

            // 3. Shift delay line and insert new bit
            self.delay_bits = (self.delay_bits << 1) | (current_bit as u32);

            // 4. Extract delayed bit
            let delayed_bit = ((self.delay_bits >> self.delay) & 1) != 0;

            // 5. XOR correlation
            let xor_out = current_bit ^ delayed_bit;

            // 6. Map to ±16000: XOR=1 → +16000, XOR=0 → -16000
            let mapped: i16 = if xor_out { 16000 } else { -16000 };

            // 7. Low-pass filter to smooth
            let smooth = self.lpf.process(mapped);

            // 8. Accumulate over symbol period
            self.accumulator += smooth as i64;
            self.accum_count += 1;

            // 9. Bresenham symbol timing
            self.bit_phase += baud_rate;
            if self.bit_phase >= sample_rate {
                self.bit_phase -= sample_rate;

                // 10. Hard bit decision
                let raw_bit = if self.mark_is_positive {
                    self.accumulator > 0
                } else {
                    self.accumulator < 0
                };

                // 11. NRZI decode: same as previous → 1, different → 0
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                if sym_count < symbols_out.len() {
                    // LLR from accumulator magnitude
                    let confidence = (self.accumulator.abs() >> self.llr_shift).min(127).max(1) as i8;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xor_demod_creation() {
        let config = DemodConfig::default_1200();
        let demod = BinaryXorDemodulator::new(config);
        assert_eq!(demod.samples_processed, 0);
        assert_eq!(demod.delay, 8); // 11025 Hz → delay 8
    }

    #[test]
    fn test_xor_demod_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut demod = BinaryXorDemodulator::new(config);
        let silence = [0i16; 1000];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 200];

        let n = demod.process_samples(&silence, &mut symbols);
        // Should produce some symbols without panicking
        assert!(n < 200);
        assert!(n > 0);
    }

    #[test]
    fn test_xor_demod_reset() {
        let config = DemodConfig::default_1200();
        let mut demod = BinaryXorDemodulator::new(config);
        let noise = [1000i16; 100];
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 50];

        demod.process_samples(&noise, &mut symbols);
        assert!(demod.samples_processed > 0);
        demod.reset();
        assert_eq!(demod.samples_processed, 0);
    }

    #[test]
    fn test_xor_demod_with_delay() {
        let config = DemodConfig::default_1200();
        let demod = BinaryXorDemodulator::new(config).with_delay(5);
        assert_eq!(demod.delay, 5);
    }

    #[test]
    fn test_xor_demod_mark_space_separation() {
        // Generate pure mark (1200 Hz) and space (2200 Hz) tones
        // and verify the demodulator produces distinct accumulator sign.
        //
        // Both pure tones produce constant raw_bit → NRZI decoded_bit=true
        // (no transitions). So we can't distinguish by LLR sign. Instead,
        // verify that both produce high-confidence symbols (|llr| near max)
        // and that the demod doesn't crash or produce garbage.
        let config = DemodConfig::default_1200();
        let sample_rate = config.sample_rate as f64;

        use core::f64::consts::PI;

        // Generate mark tone (1200 Hz) into fixed buffer
        let mut mark_samples = [0i16; 1000];
        for i in 0..1000 {
            let t = i as f64 / sample_rate;
            mark_samples[i] = ((2.0 * PI * 1200.0 * t).sin() * 16000.0) as i16;
        }

        // Generate space tone (2200 Hz) into fixed buffer
        let mut space_samples = [0i16; 1000];
        for i in 0..1000 {
            let t = i as f64 / sample_rate;
            space_samples[i] = ((2.0 * PI * 2200.0 * t).sin() * 16000.0) as i16;
        }

        let mut demod_mark = BinaryXorDemodulator::new(config);
        let mut demod_space = BinaryXorDemodulator::new(config);
        let mut sym_mark = [DemodSymbol { bit: false, llr: 0 }; 200];
        let mut sym_space = [DemodSymbol { bit: false, llr: 0 }; 200];

        let n_mark = demod_mark.process_samples(&mark_samples, &mut sym_mark);
        let n_space = demod_space.process_samples(&space_samples, &mut sym_space);

        assert!(n_mark > 0, "should produce symbols for mark tone");
        assert!(n_space > 0, "should produce symbols for space tone");

        // Both pure tones: constant raw decision → NRZI=true (no transitions) → positive LLR.
        // Verify high confidence for settled symbols (skip first few for filter settling).
        let settled_mark = &sym_mark[n_mark/2..n_mark];
        let settled_space = &sym_space[n_space/2..n_space];

        let avg_mark_conf: i32 = settled_mark.iter().map(|s| s.llr.abs() as i32).sum::<i32>()
            / settled_mark.len().max(1) as i32;
        let avg_space_conf: i32 = settled_space.iter().map(|s| s.llr.abs() as i32).sum::<i32>()
            / settled_space.len().max(1) as i32;

        // Both should have reasonable confidence (> 10)
        assert!(
            avg_mark_conf > 10,
            "mark tone should produce confident symbols (avg |LLR| = {})",
            avg_mark_conf
        );
        assert!(
            avg_space_conf > 10,
            "space tone should produce confident symbols (avg |LLR| = {})",
            avg_space_conf
        );
    }

    #[test]
    fn test_xor_loopback() {
        // Full pipeline loopback: modulate → demodulate → verify frame recovery
        use crate::ax25::frame::{build_test_frame, hdlc_encode, HdlcDecoder};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Test");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        // Modulate with preamble flags + data + trailing flags
        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;
        for _ in 0..40 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        for i in 0..encoded.bit_count {
            let n = modulator.modulate_bit(encoded.bits[i] != 0, &mut audio[audio_len..]);
            audio_len += n;
        }
        for _ in 0..4 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }

        // Demodulate with Binary XOR
        let config = DemodConfig::default_1200();
        let mut demod = BinaryXorDemodulator::new(config);
        let mut hdlc = HdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        let mut decoded_frames = 0;

        for chunk in audio[..audio_len].chunks(256) {
            let n = demod.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if let Some(_frame) = hdlc.feed_bit(symbols[i].bit) {
                    decoded_frames += 1;
                }
            }
        }

        assert!(
            decoded_frames >= 1,
            "XOR demod should decode at least 1 frame from clean loopback (got {})",
            decoded_frames
        );
    }
}
