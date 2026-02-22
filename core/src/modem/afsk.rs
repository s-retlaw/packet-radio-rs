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
    /// Fractional bit timing accumulator (Bresenham-style)
    bit_phase: u32,
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
            bit_phase: 0,
        }
    }

    /// Reset modulator state.
    pub fn reset(&mut self) {
        self.phase = 0;
        self.nrzi_state = false;
        self.bit_phase = 0;
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
        // Bresenham-style fractional timing: accumulate sample_rate,
        // divide by baud_rate, keep remainder. This produces symbol lengths
        // of floor(sr/br) or ceil(sr/br) samples that average exactly sr/br.
        self.bit_phase += self.config.sample_rate;
        let count = ((self.bit_phase / self.config.baud_rate) as usize).min(out.len());
        self.bit_phase %= self.config.baud_rate;

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
    /// Flags go through NRZI encoding just like data bits. The HDLC
    /// receiver detects flags after NRZI decoding based on the resulting
    /// tone pattern. 0x7E sent LSB first = 0,1,1,1,1,1,1,0.
    pub fn modulate_flag(&mut self, out: &mut [i16]) -> usize {
        // 0x7E = 01111110, sent LSB first
        let flag_bits: [bool; 8] = [false, true, true, true, true, true, true, false];
        let mut total = 0;

        for &bit in &flag_bits {
            let n = self.modulate_bit(bit, &mut out[total..]);
            total += n;
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
        let _n2 = m.modulate_bit(false, &mut buf2); // Toggle to mark

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

    #[test]
    fn test_modulate_flag_equals_individual_bits() {
        // modulate_flag must produce the same output as calling modulate_bit
        // for each bit of 0x7E (LSB first: 0,1,1,1,1,1,1,0)
        let flag_bits: [bool; 8] = [false, true, true, true, true, true, true, false];

        // Generate via modulate_flag
        let mut m1 = default_mod();
        let mut buf_flag = [0i16; 256];
        let n_flag = m1.modulate_flag(&mut buf_flag);

        // Generate via individual modulate_bit calls
        let mut m2 = default_mod();
        let mut buf_bits = [0i16; 256];
        let mut n_bits = 0;
        for &bit in &flag_bits {
            let n = m2.modulate_bit(bit, &mut buf_bits[n_bits..]);
            n_bits += n;
        }

        assert_eq!(n_flag, n_bits, "Flag and individual bit counts differ");
        assert_eq!(&buf_flag[..n_flag], &buf_bits[..n_bits],
            "Flag output differs from individual bit output");
        assert_eq!(m1.nrzi_state(), m2.nrzi_state(),
            "NRZI state differs after flag vs individual bits");
        assert_eq!(m1.phase, m2.phase,
            "Phase differs after flag vs individual bits");
    }

    #[test]
    fn test_modulate_flag_applies_nrzi() {
        // Verify that modulate_flag changes NRZI state (the flag byte
        // 0x7E has two 0-bits which toggle NRZI)
        let mut m = default_mod();
        let initial = m.nrzi_state();
        let mut buf = [0i16; 256];
        m.modulate_flag(&mut buf);
        // 0x7E LSB first: 0,1,1,1,1,1,1,0
        // Two 0-bits toggle NRZI, net effect: no change (toggled twice)
        assert_eq!(m.nrzi_state(), initial,
            "Two toggles in flag should return to original NRZI state");
    }
}
