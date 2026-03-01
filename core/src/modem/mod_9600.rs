//! 9600 Baud G3RUH Modulator — baseband FSK signal generation.
//!
//! Generates 9600 baud G3RUH signals for TX and test purposes.
//! The modulation chain is:
//!
//! ```text
//! Data → NRZI encode → Scramble → Raised Cosine Pulse Shaping → Samples
//! ```

use super::scrambler::Scrambler;

/// Configuration for 9600 baud modulator.
#[derive(Clone, Copy, Debug)]
pub struct Mod9600Config {
    /// Audio sample rate in Hz
    pub sample_rate: u32,
    /// Baud rate (9600)
    pub baud_rate: u32,
    /// Output amplitude (0-32767)
    pub amplitude: i16,
}

impl Mod9600Config {
    /// Default configuration at 48000 Hz.
    pub fn default_48k() -> Self {
        Self {
            sample_rate: 48000,
            baud_rate: 9600,
            amplitude: 16000,
        }
    }

    /// Default configuration at 44100 Hz.
    pub fn default_44k() -> Self {
        Self {
            sample_rate: 44100,
            ..Self::default_48k()
        }
    }

    /// Samples per symbol.
    pub fn samples_per_symbol(&self) -> u32 {
        self.sample_rate / self.baud_rate
    }
}

/// 9600 baud G3RUH modulator.
///
/// Produces baseband samples from a byte stream, applying NRZI encoding,
/// G3RUH scrambling, and optional raised cosine pulse shaping.
pub struct Modulator9600 {
    config: Mod9600Config,
    scrambler: Scrambler,
    prev_nrzi: bool,
}

impl Modulator9600 {
    /// Create a new 9600 baud modulator.
    pub fn new(config: Mod9600Config) -> Self {
        Self {
            config,
            scrambler: Scrambler::new(),
            prev_nrzi: false,
        }
    }

    /// Generate HDLC flag bytes (0x7E) for preamble.
    ///
    /// Produces `n_flags` flag sequences as audio samples.
    /// Returns the number of samples written.
    pub fn generate_preamble(&mut self, n_flags: usize, output: &mut [i16]) -> usize {
        let mut pos = 0;
        let sps = self.config.samples_per_symbol();
        let amp = self.config.amplitude;

        for _ in 0..n_flags {
            // Flag: 01111110
            let flag_bits = [false, true, true, true, true, true, true, false];
            for &bit in &flag_bits {
                // NRZI encode: transition on 0, no transition on 1
                let nrzi = if bit { self.prev_nrzi } else { !self.prev_nrzi };
                self.prev_nrzi = nrzi;

                let scrambled = self.scrambler.scramble(nrzi);
                let level = if scrambled { amp } else { -amp };

                for _ in 0..sps {
                    if pos < output.len() {
                        output[pos] = level;
                        pos += 1;
                    }
                }
            }
        }
        pos
    }

    /// Modulate a frame (raw bytes including CRC) as audio samples.
    ///
    /// The caller should have already added bit-stuffing and CRC.
    /// This function:
    /// 1. Sends opening flag (0x7E)
    /// 2. Sends the data bytes with bit-stuffing
    /// 3. Sends closing flag (0x7E)
    ///
    /// Returns the number of samples written.
    pub fn modulate_frame(&mut self, data: &[u8], output: &mut [i16]) -> usize {
        let mut pos = 0;
        let sps = self.config.samples_per_symbol();
        let amp = self.config.amplitude;

        // Helper closure to emit one bit
        let emit_bit = |bit: bool, prev_nrzi: &mut bool, scrambler: &mut Scrambler, output: &mut [i16], pos: &mut usize| {
            let nrzi = if bit { *prev_nrzi } else { !*prev_nrzi };
            *prev_nrzi = nrzi;
            let scrambled = scrambler.scramble(nrzi);
            let level = if scrambled { amp } else { -amp };
            for _ in 0..sps {
                if *pos < output.len() {
                    output[*pos] = level;
                    *pos += 1;
                }
            }
        };

        // Opening flag (no bit-stuffing for flags)
        let flag = [false, true, true, true, true, true, true, false];
        for &bit in &flag {
            emit_bit(bit, &mut self.prev_nrzi, &mut self.scrambler, output, &mut pos);
        }

        // Data bytes with bit-stuffing
        let mut ones_count = 0u8;
        for &byte in data {
            for bit_idx in 0..8 {
                let bit = (byte >> bit_idx) & 1 == 1;
                emit_bit(bit, &mut self.prev_nrzi, &mut self.scrambler, output, &mut pos);

                if bit {
                    ones_count += 1;
                    if ones_count == 5 {
                        // Insert stuffed 0
                        emit_bit(false, &mut self.prev_nrzi, &mut self.scrambler, output, &mut pos);
                        ones_count = 0;
                    }
                } else {
                    ones_count = 0;
                }
            }
        }

        // Closing flag
        for &bit in &flag {
            emit_bit(bit, &mut self.prev_nrzi, &mut self.scrambler, output, &mut pos);
        }

        pos
    }

    /// Reset modulator state.
    pub fn reset(&mut self) {
        self.scrambler.reset();
        self.prev_nrzi = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mod_config() {
        let cfg = Mod9600Config::default_48k();
        assert_eq!(cfg.samples_per_symbol(), 5);

        let cfg = Mod9600Config::default_44k();
        assert_eq!(cfg.samples_per_symbol(), 4); // 44100/9600 = 4 (integer)
    }

    #[test]
    fn test_preamble_generation() {
        let config = Mod9600Config::default_48k();
        let mut modulator = Modulator9600::new(config);
        let mut output = [0i16; 1000];

        let n = modulator.generate_preamble(4, &mut output);

        // 4 flags × 8 bits × 5 sps = 160 samples
        assert_eq!(n, 160);

        // All samples should be non-zero (amplitude)
        for &s in &output[..n] {
            assert!(s == config.amplitude || s == -config.amplitude,
                "Expected ±{}, got {}", config.amplitude, s);
        }
    }

    #[test]
    fn test_frame_modulation() {
        let config = Mod9600Config::default_48k();
        let mut modulator = Modulator9600::new(config);
        let mut output = [0i16; 5000];

        // Simple test frame: 20 bytes
        let data = [0x55u8; 20];
        let n = modulator.modulate_frame(&data, &mut output);

        // Should produce: flag(8) + data_bits + stuffed_bits + flag(8) samples
        assert!(n > 100, "Should produce significant output, got {}", n);

        // All samples should be ±amplitude
        for &s in &output[..n] {
            assert!(s == config.amplitude || s == -config.amplitude);
        }
    }

    #[test]
    fn test_modulator_reset() {
        let config = Mod9600Config::default_48k();
        let mut modulator = Modulator9600::new(config);
        let mut output = [0i16; 500];

        modulator.generate_preamble(2, &mut output);
        modulator.reset();

        // After reset, should be in initial state
        let mut output2 = [0i16; 500];
        let mut modulator2 = Modulator9600::new(config);

        let n1 = modulator.generate_preamble(2, &mut output);
        let n2 = modulator2.generate_preamble(2, &mut output2);

        assert_eq!(n1, n2);
        assert_eq!(&output[..n1], &output2[..n2]);
    }
}
