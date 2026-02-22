//! AFSK Modulator — generates Bell 202 audio from a bit stream.
//!
//! Uses a phase accumulator (NCO) with a 256-entry sine lookup table
//! to generate continuous-phase FSK. Phase continuity across mark/space
//! transitions is maintained automatically.

use super::{ModConfig, SIN_TABLE_Q15};

/// AFSK Modulator — generates audio samples from data bits.
pub struct AfskModulator {
    config: ModConfig,
    /// Phase accumulator (upper 8 bits index into sin table)
    phase: u32,
    /// Phase increment per sample for mark tone
    mark_step: u32,
    /// Phase increment per sample for space tone
    space_step: u32,
    /// Current NRZI tone state (true = mark, false = space)
    nrzi_state: bool,
}

impl AfskModulator {
    /// Create a new modulator.
    pub fn new(config: ModConfig) -> Self {
        // phase_step = (frequency × 2^32) / sample_rate
        let mark_step = ((config.mark_freq as u64) << 32) / config.sample_rate as u64;
        let space_step = ((config.space_freq as u64) << 32) / config.sample_rate as u64;

        Self {
            config,
            phase: 0,
            mark_step: mark_step as u32,
            space_step: space_step as u32,
            nrzi_state: false,
        }
    }

    /// Reset modulator state.
    pub fn reset(&mut self) {
        self.phase = 0;
        self.nrzi_state = false;
    }

    /// Generate audio for one data bit.
    ///
    /// NRZI encoding: bit=0 → toggle tone, bit=1 → maintain tone.
    /// Writes `samples_per_symbol()` samples into `out`.
    /// Returns the number of samples written.
    pub fn modulate_bit(&mut self, bit: bool, out: &mut [i16]) -> usize {
        // NRZI: toggle on 0, maintain on 1
        if !bit {
            self.nrzi_state = !self.nrzi_state;
        }

        let step = if self.nrzi_state { self.mark_step } else { self.space_step };
        let count = self.samples_per_symbol().min(out.len());

        for sample in out[..count].iter_mut() {
            let idx = (self.phase >> 24) as usize; // Top 8 bits → table index
            let sin_val = SIN_TABLE_Q15[idx];
            *sample = ((sin_val as i32 * self.config.amplitude as i32) >> 15) as i16;
            self.phase = self.phase.wrapping_add(step);
        }

        count
    }

    /// Generate a flag byte (0x7E = 01111110) for preamble/postamble.
    ///
    /// Unlike `modulate_bit`, this does NOT apply NRZI encoding or bit
    /// stuffing — the flag pattern is sent raw.
    pub fn modulate_flag(&mut self, out: &mut [i16]) -> usize {
        let flag_bits: [bool; 8] = [false, true, true, true, true, true, true, false];
        let sps = self.samples_per_symbol();
        let mut total = 0;

        for &bit in &flag_bits {
            // For flags, we bypass NRZI and send the raw bit pattern
            // 0 = space (2200 Hz), 1 = mark (1200 Hz)
            let step = if bit { self.mark_step } else { self.space_step };
            let count = sps.min(out.len() - total);

            for sample in out[total..total + count].iter_mut() {
                let idx = (self.phase >> 24) as usize;
                let sin_val = SIN_TABLE_Q15[idx];
                *sample = ((sin_val as i32 * self.config.amplitude as i32) >> 15) as i16;
                self.phase = self.phase.wrapping_add(step);
            }
            total += count;
        }

        total
    }

    /// Number of audio samples per data symbol.
    pub fn samples_per_symbol(&self) -> usize {
        (self.config.sample_rate / self.config.baud_rate) as usize
    }

    /// Get the current NRZI state (for testing/debugging).
    pub fn nrzi_state(&self) -> bool {
        self.nrzi_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_mod() -> AfskModulator {
        AfskModulator::new(ModConfig::default_1200())
    }

    #[test]
    fn test_modulator_creation() {
        let m = default_mod();
        assert_eq!(m.phase, 0);
        assert!(!m.nrzi_state);
        assert_eq!(m.samples_per_symbol(), 11025 / 1200);
    }

    #[test]
    fn test_modulate_bit_produces_samples() {
        let mut m = default_mod();
        let mut buf = [0i16; 64];
        let n = m.modulate_bit(true, &mut buf);
        assert_eq!(n, 11025 / 1200);
        // Should have non-zero samples (sine wave)
        assert!(buf[..n].iter().any(|&s| s != 0),
            "Modulated output should contain non-zero samples");
    }

    #[test]
    fn test_modulate_amplitude_in_range() {
        let mut m = default_mod();
        let mut buf = [0i16; 64];
        m.modulate_bit(true, &mut buf);

        for &s in &buf[..m.samples_per_symbol()] {
            assert!(s.abs() <= m.config.amplitude,
                "Sample {} exceeds amplitude {}", s, m.config.amplitude);
        }
    }

    #[test]
    fn test_nrzi_encoding() {
        let mut m = default_mod();
        let mut buf = [0i16; 64];

        // Bit 1: no toggle
        let initial_state = m.nrzi_state();
        m.modulate_bit(true, &mut buf);
        assert_eq!(m.nrzi_state(), initial_state);

        // Bit 0: toggle
        m.modulate_bit(false, &mut buf);
        assert_ne!(m.nrzi_state(), initial_state);

        // Another bit 0: toggle again (back to original)
        m.modulate_bit(false, &mut buf);
        assert_eq!(m.nrzi_state(), initial_state);
    }

    #[test]
    fn test_phase_continuity() {
        // Verify no discontinuities at symbol boundaries
        let mut m = default_mod();
        let mut buf1 = [0i16; 64];
        let mut buf2 = [0i16; 64];

        let n1 = m.modulate_bit(false, &mut buf1); // Toggle to space
        let n2 = m.modulate_bit(false, &mut buf2); // Toggle to mark

        // The last sample of buf1 and first sample of buf2 should be
        // reasonably close (no large jump)
        let jump = (buf2[0] as i32 - buf1[n1 - 1] as i32).abs();
        // At 11025 Hz, one sample step is ~1/11025 seconds
        // Maximum expected jump depends on frequency, but should be modest
        assert!(jump < 10000,
            "Phase discontinuity at symbol boundary: jump = {}", jump);
    }

    #[test]
    fn test_reset() {
        let mut m = default_mod();
        let mut buf = [0i16; 64];
        m.modulate_bit(false, &mut buf);
        m.modulate_bit(true, &mut buf);
        m.reset();
        assert_eq!(m.phase, 0);
        assert!(!m.nrzi_state());
    }
}
