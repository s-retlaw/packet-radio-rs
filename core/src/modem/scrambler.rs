//! G3RUH Scrambler/Descrambler — x^17 + x^12 + 1 LFSR.
//!
//! Used for 9600 baud FSK packet radio. The scrambler whitens the data
//! to ensure transitions for clock recovery. The descrambler is
//! self-synchronizing: it needs only 17 received bits to sync, regardless
//! of initial shift register state.
//!
//! A single channel bit error produces 3 output errors (at delays 0, 12, 17)
//! due to the self-synchronizing feedback structure.
//!
//! Reference: James Miller G3RUH, "A New Packet-Radio Modem"

/// G3RUH descrambler — self-synchronizing x^17 + x^12 + 1.
///
/// Feeds received bits into the shift register (not output bits),
/// so it self-synchronizes after 17 bits regardless of initial state.
pub struct Descrambler {
    /// 17-bit shift register (bits 0..16 used)
    shift_reg: u32,
}

impl Default for Descrambler {
    fn default() -> Self {
        Self::new()
    }
}

impl Descrambler {
    /// Create a new descrambler with zeroed shift register.
    pub const fn new() -> Self {
        Self { shift_reg: 0 }
    }

    /// Descramble a single bit.
    ///
    /// The descrambler computes: `output = input XOR sr[12] XOR sr[17]`
    /// then shifts the *input* bit into the register.
    #[inline]
    pub fn descramble(&mut self, bit: bool) -> bool {
        let tap12 = (self.shift_reg >> 11) & 1; // bit 12 (0-indexed as 11)
        let tap17 = (self.shift_reg >> 16) & 1; // bit 17 (0-indexed as 16)
        let output = (bit as u32) ^ tap12 ^ tap17;

        // Shift input bit into register (self-synchronizing: uses input, not output)
        self.shift_reg = (self.shift_reg << 1) | (bit as u32);
        self.shift_reg &= 0x1FFFF; // keep only 17 bits

        output != 0
    }

    /// Reset the shift register to zero.
    pub fn reset(&mut self) {
        self.shift_reg = 0;
    }
}

/// G3RUH scrambler — x^17 + x^12 + 1 (for TX / test signal generation).
///
/// Unlike the descrambler, the scrambler feeds the *output* bit back
/// into the shift register.
pub struct Scrambler {
    /// 17-bit shift register (bits 0..16 used)
    shift_reg: u32,
}

impl Default for Scrambler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scrambler {
    /// Create a new scrambler with zeroed shift register.
    pub const fn new() -> Self {
        Self { shift_reg: 0 }
    }

    /// Scramble a single bit.
    ///
    /// The scrambler computes: `output = input XOR sr[12] XOR sr[17]`
    /// then shifts the *output* bit into the register.
    #[inline]
    pub fn scramble(&mut self, bit: bool) -> bool {
        let tap12 = (self.shift_reg >> 11) & 1;
        let tap17 = (self.shift_reg >> 16) & 1;
        let output = (bit as u32) ^ tap12 ^ tap17;

        // Shift output bit into register (scrambler feedback)
        self.shift_reg = (self.shift_reg << 1) | output;
        self.shift_reg &= 0x1FFFF;

        output != 0
    }

    /// Reset the shift register to zero.
    pub fn reset(&mut self) {
        self.shift_reg = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scramble_descramble_roundtrip() {
        let mut scrambler = Scrambler::new();
        let mut descrambler = Descrambler::new();

        // Test pattern: alternating bits
        let input_bits: [bool; 64] = core::array::from_fn(|i| i % 2 == 0);
        let mut scrambled = [false; 64];
        let mut recovered = [false; 64];

        for i in 0..64 {
            scrambled[i] = scrambler.scramble(input_bits[i]);
        }

        for i in 0..64 {
            recovered[i] = descrambler.descramble(scrambled[i]);
        }

        // After 17 bits of sync, output should match input
        for i in 17..64 {
            assert_eq!(
                recovered[i], input_bits[i],
                "Mismatch at bit {} (after sync): expected {}, got {}",
                i, input_bits[i], recovered[i]
            );
        }
    }

    #[test]
    fn test_self_sync_after_17_bits() {
        let mut scrambler = Scrambler::new();

        // Scramble a known pattern
        let input_bits: [bool; 100] = core::array::from_fn(|i| (i * 7 + 3) % 5 < 3);
        let mut scrambled = [false; 100];
        for i in 0..100 {
            scrambled[i] = scrambler.scramble(input_bits[i]);
        }

        // Descrambler starts with wrong state — should still sync
        let mut descrambler = Descrambler { shift_reg: 0x1ABCD };
        let mut recovered = [false; 100];
        for i in 0..100 {
            recovered[i] = descrambler.descramble(scrambled[i]);
        }

        // After 17 bits of sync, output must match
        for i in 17..100 {
            assert_eq!(recovered[i], input_bits[i], "Self-sync failed at bit {}", i);
        }
    }

    #[test]
    fn test_all_zeros_scrambled_is_not_all_zeros() {
        let mut scrambler = Scrambler::new();
        // Pre-fill with some state
        for _ in 0..20 {
            scrambler.scramble(true);
        }

        let mut all_zero = true;
        for _ in 0..100 {
            if scrambler.scramble(false) {
                all_zero = false;
            }
        }
        // Scrambler should produce non-zero output from zero input
        // (once the LFSR has non-zero state)
        assert!(!all_zero, "Scrambler should whiten all-zero input");
    }

    #[test]
    fn test_single_error_produces_three_errors() {
        let mut scrambler = Scrambler::new();
        let input: [bool; 50] = core::array::from_fn(|i| (i * 3 + 1) % 4 < 2);
        let mut scrambled = [false; 50];
        for i in 0..50 {
            scrambled[i] = scrambler.scramble(input[i]);
        }

        // Inject single bit error at position 20
        let mut corrupted = scrambled;
        corrupted[20] = !corrupted[20];

        // Descramble both clean and corrupted
        let mut desc_clean = Descrambler::new();
        let mut desc_corrupt = Descrambler::new();
        let mut clean_out = [false; 50];
        let mut corrupt_out = [false; 50];
        for i in 0..50 {
            clean_out[i] = desc_clean.descramble(scrambled[i]);
            corrupt_out[i] = desc_corrupt.descramble(corrupted[i]);
        }

        // Count differences — should be exactly 3 (at offsets 0, +5, +5 from error)
        // Actually: error at bit 20 causes errors at 20, 20+5=25 (tap12 delay), 20+5=25...
        // Wait — the delays are 0, 12-bit tap (5 positions back), 17-bit tap
        // Let me just count: should be exactly 3 errors
        let mut error_count = 0;
        for i in 17..50 {
            // skip sync period
            if clean_out[i] != corrupt_out[i] {
                error_count += 1;
            }
        }
        assert_eq!(
            error_count, 3,
            "Single channel error should produce exactly 3 output errors, got {}",
            error_count
        );
    }

    #[test]
    fn test_reset() {
        let mut scrambler = Scrambler::new();
        for _ in 0..50 {
            scrambler.scramble(true);
        }
        scrambler.reset();
        assert_eq!(scrambler.shift_reg, 0);

        let mut descrambler = Descrambler::new();
        for _ in 0..50 {
            descrambler.descramble(true);
        }
        descrambler.reset();
        assert_eq!(descrambler.shift_reg, 0);
    }

    #[test]
    fn test_known_vector() {
        // G3RUH scrambler with all-zeros initial state, all-zeros input
        // should produce the LFSR sequence itself.
        // With SR=0 and input=0: output = 0 XOR 0 XOR 0 = 0 for the first 17 bits
        // (no taps set). Then feedback starts.
        let mut scrambler = Scrambler::new();
        let mut output = [false; 40];
        for i in 0..40 {
            output[i] = scrambler.scramble(false);
        }
        // First 17 bits should all be false (empty register, zero input)
        for i in 0..17 {
            assert!(!output[i], "Bit {} should be false with zero init", i);
        }
        // Bits 17+ should also be false (because register is still all zeros
        // when input is all zeros and initial state is zero)
        // Actually: all zeros in → SR stays all zeros → output stays zero
        // This is a degenerate case; scrambler needs non-zero input to produce output
    }

    #[test]
    fn test_scrambler_produces_transitions() {
        // Feed all-ones (constant mark) — scrambler should still produce transitions
        let mut scrambler = Scrambler::new();
        let mut transitions = 0u32;
        let mut prev = false;
        for i in 0..200 {
            let out = scrambler.scramble(true);
            if i > 0 && out != prev {
                transitions += 1;
            }
            prev = out;
        }
        // Good scrambler should produce roughly 50% transitions
        assert!(
            transitions > 50 && transitions < 150,
            "Expected ~100 transitions in 200 bits, got {}",
            transitions
        );
    }
}
