//! AFSK Demodulator — Bell 202 audio to bit stream.
//!
//! Dual-path architecture:
//!
//! **Fast path**: Bandpass → Delay-multiply discriminator → PLL → hard bits.
//! Minimal CPU and memory for embedded targets.
//!
//! **Quality path**: Bandpass → Hilbert → instantaneous frequency →
//! adaptive tracker → PLL → soft bits → bit-flip recovery.
//! Better decode performance for desktop and ESP32.
//!
//! Both paths produce NRZI-decoded bits that feed into the HDLC decoder.
//! See docs/MODEM_DESIGN.md for the full design rationale.

use super::DemodConfig;
use super::delay_multiply::DelayMultiplyDetector;
use super::filter::BiquadFilter;
use super::pll::ClockRecoveryPll;

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

/// Fast-path AFSK demodulator (delay-multiply discriminator).
///
/// Suitable for Cortex-M0, RP2040, and other resource-constrained targets.
/// Uses ~180 bytes of RAM and ~30-50 cycles per sample.
pub struct FastDemodulator {
    #[allow(dead_code)]
    config: DemodConfig,
    bpf: BiquadFilter,
    detector: DelayMultiplyDetector,
    pll: ClockRecoveryPll,
    prev_nrzi_bit: bool,
    samples_processed: u64,
}

impl FastDemodulator {
    /// Create a new fast-path demodulator.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = super::filter::afsk_bandpass_11025();
        let lpf = super::filter::post_detect_lpf_11025();
        let detector = DelayMultiplyDetector::new(config.sample_rate, lpf);
        let pll = ClockRecoveryPll::new(
            config.sample_rate,
            config.baud_rate,
            config.pll_alpha,
            config.pll_beta,
        );

        Self {
            config,
            bpf,
            detector,
            pll,
            prev_nrzi_bit: false,
            samples_processed: 0,
        }
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        self.bpf.reset();
        self.detector.reset();
        self.pll.reset();
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

        for &sample in samples {
            self.samples_processed += 1;

            // 1. Bandpass filter
            let filtered = self.bpf.process(sample);

            // 2. Delay-multiply discriminator
            let disc_out = self.detector.process(filtered);

            // 3. Clock recovery PLL — outputs at symbol boundaries
            if let Some(symbol_val) = self.pll.update(disc_out) {
                // 4. Hard decision
                let raw_bit = symbol_val > 0;

                // 5. NRZI decode
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                if sym_count < symbols_out.len() {
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr: if decoded_bit { 64 } else { -64 }, // Moderate confidence
                    };
                    sym_count += 1;
                }
            }
        }

        sym_count
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
    pll: ClockRecoveryPll,
    prev_nrzi_bit: bool,
    samples_processed: u64,
    sample_index: u32,
}

impl QualityDemodulator {
    /// Create a new quality-path demodulator.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = super::filter::afsk_bandpass_11025();
        let hilbert = hilbert_31();
        let inst_freq = InstFreqDetector::new(config.sample_rate);
        let tracker = AdaptiveTracker::new(config.sample_rate);
        let pll = ClockRecoveryPll::new(
            config.sample_rate,
            config.baud_rate,
            config.pll_alpha,
            config.pll_beta,
        );

        Self {
            config,
            bpf,
            hilbert,
            inst_freq,
            tracker,
            pll,
            prev_nrzi_bit: false,
            samples_processed: 0,
            sample_index: 0,
        }
    }

    /// Reset demodulator state.
    pub fn reset(&mut self) {
        self.bpf.reset();
        self.hilbert.reset();
        self.inst_freq.reset();
        self.tracker.reset();
        self.pll.reset();
        self.prev_nrzi_bit = false;
        self.samples_processed = 0;
        self.sample_index = 0;
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

        for &sample in samples {
            self.samples_processed += 1;
            self.sample_index = self.sample_index.wrapping_add(1);

            // 1. Bandpass filter
            let filtered = self.bpf.process(sample);

            // 2. Hilbert transform → analytic signal
            let (real, imag) = self.hilbert.process(filtered);

            // 3. Instantaneous frequency
            let freq_fp = self.inst_freq.process(real, imag);

            // 4. Feed to adaptive tracker (trains during preamble)
            self.tracker.feed(freq_fp, self.sample_index);

            // 5. Convert frequency to discriminator-like output for PLL
            // Use the tracker's threshold for the decision boundary
            let disc_out = ((freq_fp - self.tracker.threshold) >> 4)
                .clamp(-32768, 32767) as i16;

            // 6. Clock recovery PLL
            if let Some(_symbol_val) = self.pll.update(disc_out) {
                // 7. Generate soft bit (LLR) from frequency
                let llr = self.tracker.freq_to_llr(freq_fp);
                let raw_bit = llr >= 0;

                // 8. NRZI decode
                let decoded_bit = raw_bit == self.prev_nrzi_bit;
                self.prev_nrzi_bit = raw_bit;

                // Preserve soft info through NRZI
                // (NRZI decode is XOR, so confidence propagates)
                let decoded_llr = if decoded_bit { llr.abs() } else { -(llr.abs()) };

                if sym_count < symbols_out.len() {
                    symbols_out[sym_count] = DemodSymbol {
                        bit: decoded_bit,
                        llr: decoded_llr,
                    };
                    sym_count += 1;
                }
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

    // Full integration tests (modulate → demodulate → verify) will be
    // added once both the modulator and demodulator are fully functional.
    // See docs/TEST_PLAN.md §5 for round-trip loopback test design.
}
