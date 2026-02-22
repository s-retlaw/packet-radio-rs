//! Clock Recovery PLL — **experimental, not integrated**.
//!
//! This module implements a digital phase-locked loop but is NOT used by the
//! active demodulators, which use Bresenham fixed-rate symbol timing instead.
//! Kept as reference for potential future use with adaptive symbol timing.
//!
//! The Bresenham approach was chosen over PLL because it exactly matches the
//! modulator's timing, avoiding PLL lock/jitter issues on noisy signals.

/// Digital PLL for clock recovery.
pub struct ClockRecoveryPll {
    /// Phase accumulator (wraps at nominal_freq)
    phase: i32,
    /// Current phase increment per sample (tracks baud rate)
    freq: i32,
    /// Nominal phase increment (sample_rate / baud_rate × scaling)
    nominal_freq: i32,
    /// Phase correction gain — how fast we correct phase errors
    alpha: i16,
    /// Frequency correction gain — how fast we correct baud rate drift
    beta: i16,
    /// Previous discriminator output (for transition detection)
    prev_sample: i16,
    /// Whether the PLL is locked (enough transitions seen)
    pub locked: bool,
    /// Count of transitions seen since last reset
    lock_count: u16,
}

/// PLL scaling factor. We multiply samples_per_symbol by this to get
/// sufficient fixed-point resolution in the phase accumulator.
const PLL_SCALE: i32 = 256;

impl ClockRecoveryPll {
    /// Create a PLL for the given sample rate and baud rate.
    ///
    /// `alpha` and `beta` control loop bandwidth:
    /// - Higher alpha = faster phase tracking, more jitter
    /// - Higher beta = faster frequency tracking, less stable
    ///
    /// Good defaults: alpha=936, beta=74 (moderate bandwidth).
    pub fn new(sample_rate: u32, baud_rate: u32, alpha: i16, beta: i16) -> Self {
        let nominal = (sample_rate as i32 * PLL_SCALE) / baud_rate as i32;
        Self {
            phase: 0,
            freq: nominal,
            nominal_freq: nominal,
            alpha,
            beta,
            prev_sample: 0,
            locked: false,
            lock_count: 0,
        }
    }

    /// Process one discriminator sample.
    ///
    /// Returns `Some(discriminator_value)` at each detected symbol boundary.
    /// The returned value is the discriminator output at the optimal sampling
    /// instant:
    /// - **Fast path**: `value > 0` → mark (1), `value ≤ 0` → space (0)
    /// - **Quality path**: Use the magnitude as confidence for soft decoding
    #[inline]
    pub fn update(&mut self, disc_out: i16) -> Option<i16> {
        let mut symbol_sample = None;

        // Advance phase by one sample step
        self.phase += PLL_SCALE;

        // Symbol boundary: phase exceeds the current symbol period
        if self.phase >= self.freq {
            self.phase -= self.freq;
            symbol_sample = Some(disc_out);
        }

        // Detect transitions (sign changes in discriminator)
        let transition = (disc_out > 0) != (self.prev_sample > 0)
            && (disc_out != 0 || self.prev_sample != 0);
        self.prev_sample = disc_out;

        if transition {
            self.lock_count = self.lock_count.saturating_add(1);

            // Phase error: distance from the ideal mid-symbol transition point.
            // Ideal transition occurs at phase = nominal_freq / 2.
            let ideal = self.nominal_freq / 2;
            let error = self.phase - ideal;

            // Proportional correction (phase — fast response)
            let phase_correction = (error as i64 * self.alpha as i64) >> 15;
            self.phase -= phase_correction as i32;

            // Integral correction (frequency — slow adaptation)
            let freq_correction = (error as i64 * self.beta as i64) >> 15;
            self.freq -= freq_correction as i32;

            // Clamp frequency to ±2% of nominal baud rate
            let max_drift = self.nominal_freq / 50;
            self.freq = self.freq.clamp(
                self.nominal_freq - max_drift,
                self.nominal_freq + max_drift,
            );

            // Consider locked after enough transitions
            if self.lock_count > 20 {
                self.locked = true;
            }
        }

        symbol_sample
    }

    /// Update the nominal baud rate from the adaptive tracker.
    ///
    /// `samples_per_symbol_fp` is in fixed-point (×256).
    pub fn adapt_baud_rate(&mut self, samples_per_symbol_fp: i32) {
        if samples_per_symbol_fp > 0 {
            // Convert from ×256 to ×PLL_SCALE
            let new_nominal = samples_per_symbol_fp * PLL_SCALE / 256;
            if new_nominal > 0 {
                self.nominal_freq = new_nominal;
                // Gently move freq toward new nominal
                self.freq = (self.freq + self.nominal_freq) / 2;
            }
        }
    }

    /// Reset PLL state (for next packet).
    pub fn reset(&mut self) {
        self.phase = 0;
        self.freq = self.nominal_freq;
        self.prev_sample = 0;
        self.locked = false;
        self.lock_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pll_creation() {
        let pll = ClockRecoveryPll::new(11025, 1200, 936, 74);
        assert!(!pll.locked);
        // Nominal freq should be ~9.1875 × 256 ≈ 2352
        let expected = (11025 * PLL_SCALE) / 1200;
        assert_eq!(pll.nominal_freq, expected);
    }

    #[test]
    fn test_pll_produces_symbols() {
        let mut pll = ClockRecoveryPll::new(11025, 1200, 936, 74);
        let mut symbol_count = 0;

        // Feed 11025 samples (1 second) of alternating positive/negative
        // (simulating a clean demodulated signal with transitions)
        let samples_per_symbol = 11025 / 1200; // ~9
        for i in 0..11025 {
            let symbol_idx = i / samples_per_symbol;
            let disc_out: i16 = if symbol_idx % 2 == 0 { 10000 } else { -10000 };
            if pll.update(disc_out).is_some() {
                symbol_count += 1;
            }
        }

        // Should produce approximately 1200 symbols in 1 second
        assert!(
            symbol_count > 1100 && symbol_count < 1300,
            "Expected ~1200 symbols, got {}", symbol_count
        );
    }

    #[test]
    fn test_pll_locks() {
        let mut pll = ClockRecoveryPll::new(11025, 1200, 936, 74);
        let sps = 11025 / 1200;

        // Feed clean alternating signal
        for i in 0..500 {
            let val: i16 = if (i / sps) % 2 == 0 { 10000 } else { -10000 };
            pll.update(val);
        }

        assert!(pll.locked, "PLL should be locked after 500 samples");
    }

    #[test]
    fn test_pll_reset() {
        let mut pll = ClockRecoveryPll::new(11025, 1200, 936, 74);

        // Get it locked
        let sps = 11025 / 1200;
        for i in 0..500 {
            let val: i16 = if (i / sps) % 2 == 0 { 10000 } else { -10000 };
            pll.update(val);
        }
        assert!(pll.locked);

        pll.reset();
        assert!(!pll.locked);
        assert_eq!(pll.freq, pll.nominal_freq);
    }
}
