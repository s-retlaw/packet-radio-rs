//! Delay-and-Multiply Frequency Discriminator.
//!
//! Used by `DmDemodulator` as part of the delay-multiply demodulation pipeline:
//! BPF → Delay-Multiply → LPF → PLL → NRZI → HDLC.
//!
//! Produces continuous sample-by-sample discriminator output where polarity
//! indicates mark vs space frequency. Only 1 multiply per sample.
//!
//! See docs/MODEM_DESIGN.md for the mathematical derivation.

use super::filter::BiquadFilter;
use super::MAX_DELAY;

/// Delay-and-multiply AFSK frequency discriminator.
///
/// # How It Works
///
/// ```text
/// s(t) × s(t−τ) = (A²/2)·cos(2πfτ) + (A²/2)·cos(high freq term)
/// ```
///
/// The lowpass filter removes the high-frequency term, leaving an output
/// whose polarity depends on the input frequency `f` and the chosen delay `τ`.
pub struct DelayMultiplyDetector {
    /// Circular delay buffer
    delay_line: [i16; MAX_DELAY],
    /// Write position in delay buffer
    write_pos: usize,
    /// Delay in samples
    delay: usize,
    /// Post-detection lowpass filter
    lpf: BiquadFilter,
}

impl DelayMultiplyDetector {
    /// Create a new detector for the given sample rate.
    ///
    /// Automatically selects the optimal delay for mark/space separation.
    pub fn new(sample_rate: u32, lpf: BiquadFilter) -> Self {
        let delay = optimal_delay(sample_rate);
        Self {
            delay_line: [0i16; MAX_DELAY],
            write_pos: 0,
            delay,
            lpf,
        }
    }

    /// Create with an explicit delay value (for testing or tuning).
    pub fn with_delay(delay: usize, lpf: BiquadFilter) -> Self {
        assert!(delay > 0 && delay < MAX_DELAY);
        Self {
            delay_line: [0i16; MAX_DELAY],
            write_pos: 0,
            delay,
            lpf,
        }
    }

    /// Process one audio sample.
    ///
    /// Returns the filtered discriminator output:
    /// - **Positive** → mark frequency (1200 Hz)
    /// - **Negative** → space frequency (2200 Hz)
    /// - **Magnitude** → signal strength
    #[inline]
    pub fn process(&mut self, sample: i16) -> i16 {
        // Read delayed sample
        let read_pos = (self.write_pos + MAX_DELAY - self.delay) % MAX_DELAY;
        let delayed = self.delay_line[read_pos];

        // Store current sample
        self.delay_line[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % MAX_DELAY;

        // Multiply current × delayed in Q15
        let product = ((sample as i32 * delayed as i32) >> 15) as i16;

        // Lowpass to remove double-frequency component
        self.lpf.process(product)
    }

    /// Reset detector state.
    pub fn reset(&mut self) {
        self.delay_line = [0i16; MAX_DELAY];
        self.write_pos = 0;
        self.lpf.reset();
    }

    /// Get the current delay in samples.
    pub fn delay_samples(&self) -> usize {
        self.delay
    }
}

/// Compute the optimal delay in samples for a given sample rate.
///
/// The delay is chosen so that mark (1200 Hz) and space (2200 Hz) produce
/// opposite-polarity outputs, maximizing the separation between them.
///
/// This is done by searching integer sample delays for the one that
/// maximizes |cos(2π·f_mark·τ) − cos(2π·f_space·τ)| while keeping
/// the outputs of opposite sign.
pub fn optimal_delay(sample_rate: u32) -> usize {
    // We search delays from 1 to MAX_DELAY-1 and pick the one with
    // the best mark/space separation. Using integer arithmetic with
    // a precomputed lookup would be ideal; for now, we do it at init
    // time only so a small amount of floating-point is acceptable.
    //
    // On no_std targets without float, use the hardcoded values below.

    // Hardcoded optimal delays for common sample rates.
    // All use τ ≈ 363 μs which gives mark→negative, space→positive.
    match sample_rate {
        11025 => 4,   // 363 μs: mark→−0.92, space→+0.30
        22050 => 8,   // 363 μs: mark→−0.92, space→+0.30
        44100 => 16,  // 363 μs: mark→−0.92, space→+0.30
        48000 => 17,  // 354 μs: mark→−0.89, space→+0.18
        _ => {
            // For other sample rates, approximate: τ ≈ 1/(f_mark+f_space)
            // In samples: delay ≈ sample_rate / (1200 + 2200)
            let approx = sample_rate / 3400;
            if approx < 1 { 1 }
            else if approx >= MAX_DELAY as u32 { MAX_DELAY - 1 }
            else { approx as usize }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec::Vec;
    use super::*;
    use super::super::filter::BiquadFilter;

    /// Generate a sine wave at a given frequency.
    fn generate_tone(freq_hz: f64, sample_rate: u32, num_samples: usize) -> Vec<i16> {
        use core::f64::consts::PI;
        (0..num_samples)
            .map(|i| {
                let t = i as f64 / sample_rate as f64;
                (16000.0 * (2.0 * PI * freq_hz * t).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn test_optimal_delay_common_rates() {
        assert!(optimal_delay(11025) > 0);
        assert!(optimal_delay(11025) < MAX_DELAY);
        assert!(optimal_delay(22050) > 0);
        assert!(optimal_delay(44100) > 0);
    }

    #[test]
    fn test_mark_space_separation() {
        // Verify that mark and space tones produce opposite-polarity outputs
        let sample_rate = 11025;
        let delay = optimal_delay(sample_rate);

        // Process pure mark tone (1200 Hz)
        let mut det = DelayMultiplyDetector::with_delay(delay, BiquadFilter::passthrough());
        let mark_tone = generate_tone(1200.0, sample_rate, 200);
        let mut mark_sum: i64 = 0;
        for &s in &mark_tone[50..] { // Skip transient
            mark_sum += det.process(s) as i64;
        }
        let mark_avg = mark_sum / 150;

        // Process pure space tone (2200 Hz)
        det.reset();
        let space_tone = generate_tone(2200.0, sample_rate, 200);
        let mut space_sum: i64 = 0;
        for &s in &space_tone[50..] {
            space_sum += det.process(s) as i64;
        }
        let space_avg = space_sum / 150;

        // They should have opposite signs
        assert!(
            (mark_avg > 0 && space_avg < 0) || (mark_avg < 0 && space_avg > 0),
            "Mark ({}) and space ({}) should have opposite polarity",
            mark_avg, space_avg
        );
    }

    #[test]
    fn test_reset_clears_state() {
        let mut det = DelayMultiplyDetector::new(11025, BiquadFilter::passthrough());
        // Feed some samples
        for i in 0..100 {
            det.process((i * 100) as i16);
        }
        det.reset();
        // Delay line should be zeroed
        for &s in &det.delay_line {
            assert_eq!(s, 0);
        }
    }
}
