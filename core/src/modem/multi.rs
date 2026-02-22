//! Multi-Decoder — parallel demodulators with filter/timing/frequency diversity.
//!
//! Runs multiple `FastDemodulator` instances with different bandpass filters,
//! timing offsets, and frequency offsets, deduplicating output frames. This is
//! analogous to Dire Wolf's multi-decoder approach.
//!
//! Default configuration (with `std`):
//! - 3 BPF variants × 3 timing offsets = 9 base decoders
//! - ±50 Hz offset × 3 timing = 6 frequency-shifted decoders
//! - ±100 Hz offset × 1 timing = 2 frequency-shifted decoders
//! - 9 space gain levels (-6 to +12 dB) = 9 gain-diverse decoders
//! - 4 cross-product (freq offset + gain) decoders
//! - Total: 30 parallel decoders

use super::demod::{DemodSymbol, FastDemodulator};
use super::filter::BiquadFilter;
use super::DemodConfig;
use crate::ax25::frame::HdlcDecoder;

/// Maximum number of parallel decoders.
/// 9 base (3 BPF × 3 timing) + 8 frequency-shifted + 9 gain-diverse = 26.
const MAX_DECODERS: usize = 32;

/// Maximum unique frames tracked for deduplication.
const DEDUP_RING_SIZE: usize = 64;

/// Maximum number of output frames per `process_samples` call.
const MAX_OUTPUT_FRAMES: usize = 16;

/// A decoded frame with its content.
pub struct DecodedFrame {
    pub data: [u8; 330],
    pub len: usize,
}

/// Multi-decoder output buffer.
pub struct MultiOutput {
    frames: [DecodedFrame; MAX_OUTPUT_FRAMES],
    count: usize,
}

impl MultiOutput {
    fn new() -> Self {
        Self {
            frames: core::array::from_fn(|_| DecodedFrame {
                data: [0u8; 330],
                len: 0,
            }),
            count: 0,
        }
    }

    /// Number of unique frames decoded in this batch.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether no frames were decoded.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Get a decoded frame by index.
    pub fn frame(&self, index: usize) -> &[u8] {
        &self.frames[index].data[..self.frames[index].len]
    }
}

/// Multi-decoder: runs N parallel demodulators with diversity.
pub struct MultiDecoder {
    decoders: [FastDemodulator; MAX_DECODERS],
    hdlc: [HdlcDecoder; MAX_DECODERS],
    num_active: usize,
    /// Ring buffer of (hash, generation) for time-windowed deduplication.
    recent_hashes: [(u32, u32); DEDUP_RING_SIZE],
    recent_write: usize,
    recent_count: usize,
    /// Generation counter — incremented each process_samples call.
    generation: u32,
    /// Total frames decoded (including duplicates caught).
    pub total_decoded: u64,
    /// Total unique frames output.
    pub total_unique: u64,
}

impl MultiDecoder {
    /// Create a multi-decoder with default filter/timing/frequency/gain diversity.
    ///
    /// Uses 3 bandpass filters (narrow, standard, wide) × 3 timing offsets
    /// = 9 base decoders, plus frequency-shifted decoders for crystal offset
    /// tolerance, plus gain-diverse decoders (Dire Wolf multi-slicer approach)
    /// for de-emphasis and varying audio paths.
    pub fn new(config: DemodConfig) -> Self {
        let std_bpf = match config.sample_rate {
            22050 => super::filter::afsk_bandpass_22050(),
            44100 => super::filter::afsk_bandpass_44100(),
            _ => super::filter::afsk_bandpass_11025(),
        };
        let filters = [
            super::filter::afsk_bandpass_narrow_11025(),
            std_bpf,
            super::filter::afsk_bandpass_wide_11025(),
        ];

        // Timing offsets: 0, 1/3 symbol, 2/3 symbol (in phase accumulator units)
        // The Bresenham counter wraps at sample_rate, so 1/3 symbol = sample_rate/3
        let offsets = [0u32, config.sample_rate / 3, 2 * config.sample_rate / 3];

        let mut decoders: [FastDemodulator; MAX_DECODERS] =
            core::array::from_fn(|_| FastDemodulator::new(config));
        let hdlc: [HdlcDecoder; MAX_DECODERS] =
            core::array::from_fn(|_| HdlcDecoder::new());

        // 9 base decoders: 3 BPF × 3 timing offsets (nominal frequencies)
        let mut idx = 0;
        for f in 0..3 {
            for o in 0..3 {
                if idx < MAX_DECODERS {
                    decoders[idx] =
                        FastDemodulator::with_filter_and_offset(config, filters[f], offsets[o]);
                    idx += 1;
                }
            }
        }

        // Frequency-shifted decoders: shift both BPF center AND Goertzel
        // frequencies to handle transmitters with crystal offset.
        // Uses runtime-computed BPF when std is available.
        // Each offset gets 3 timing variants for full diversity.
        #[cfg(feature = "std")]
        {
            let freq_offsets: [i32; 4] = [-50, 50, -100, 100];
            for &offset in &freq_offsets {
                let mark = (config.mark_freq as i32 + offset) as u32;
                let space = (config.space_freq as i32 + offset) as u32;
                let center = (super::MID_FREQ as i32 + offset) as f64;
                let bpf = super::filter::bandpass_coeffs(config.sample_rate, center, 2000.0);
                // Only first offset pair gets timing diversity (to stay within budget)
                let timing_variants = if offset.abs() <= 50 { &offsets[..] } else { &offsets[..1] };
                for &phase in timing_variants {
                    if idx < MAX_DECODERS {
                        decoders[idx] = FastDemodulator::with_filter_freq_and_offset(
                            config, bpf, phase, mark, space,
                        );
                        idx += 1;
                    }
                }
            }
        }
        #[cfg(not(feature = "std"))]
        {
            // On no_std, use wide BPF with shifted Goertzel only
            let wide_bpf = super::filter::afsk_bandpass_wide_11025();
            let freq_offsets: [i32; 2] = [-50, 50];
            for &offset in &freq_offsets {
                if idx < MAX_DECODERS {
                    let mark = (config.mark_freq as i32 + offset) as u32;
                    let space = (config.space_freq as i32 + offset) as u32;
                    decoders[idx] = FastDemodulator::with_filter_freq_and_offset(
                        config, wide_bpf, 0, mark, space,
                    );
                    idx += 1;
                }
            }
        }

        // Gain diversity decoders (Dire Wolf multi-slicer approach).
        // Different space energy gains handle de-emphasis and varying audio paths.
        // Q8 format: 256 = 0 dB. Values are 10^(dB/10) × 256 for amplitude dB:
        //   -6.0, -3.8, -1.5, +0.8, +3.0, +5.3, +7.5, +9.8, +12.0 dB
        // These match Dire Wolf's 9-level multi-slicer gain set.
        #[cfg(feature = "std")]
        {
            let gains_q8: [u16; 9] = [64, 107, 181, 308, 511, 868, 1440, 2445, 4057];
            for &gain in &gains_q8 {
                if idx < MAX_DECODERS {
                    decoders[idx] = FastDemodulator::new(config).with_space_gain(gain);
                    idx += 1;
                }
            }
        }
        #[cfg(not(feature = "std"))]
        {
            // Fewer gain levels on embedded to save RAM/CPU
            let gains_q8: [u16; 4] = [181, 511, 1440, 4057];
            for &gain in &gains_q8 {
                if idx < MAX_DECODERS {
                    decoders[idx] = FastDemodulator::new(config).with_space_gain(gain);
                    idx += 1;
                }
            }
        }

        // Cross-product decoders: freq offset + gain for transmitters with
        // both crystal offset AND de-emphasized audio (common in practice).
        #[cfg(feature = "std")]
        {
            let cross_combos: [(i32, u16); 4] = [
                (-50, 868),   // -50 Hz, +5.3 dB
                (50, 868),    // +50 Hz, +5.3 dB
                (-50, 1440),  // -50 Hz, +7.5 dB
                (50, 1440),   // +50 Hz, +7.5 dB
            ];
            for &(offset, gain) in &cross_combos {
                if idx < MAX_DECODERS {
                    let mark = (config.mark_freq as i32 + offset) as u32;
                    let space = (config.space_freq as i32 + offset) as u32;
                    let center = (super::MID_FREQ as i32 + offset) as f64;
                    let bpf = super::filter::bandpass_coeffs(config.sample_rate, center, 2000.0);
                    decoders[idx] = FastDemodulator::with_filter_freq_and_offset(
                        config, bpf, 0, mark, space,
                    ).with_space_gain(gain);
                    idx += 1;
                }
            }
        }

        Self {
            decoders,
            hdlc,
            num_active: idx,
            recent_hashes: [(0u32, 0u32); DEDUP_RING_SIZE],
            recent_write: 0,
            recent_count: 0,
            generation: 0,
            total_decoded: 0,
            total_unique: 0,
        }
    }

    /// Create with a specific number of filter variants and timing offsets.
    pub fn with_diversity(
        config: DemodConfig,
        filters: &[BiquadFilter],
        timing_offsets: &[u32],
    ) -> Self {
        let mut decoders: [FastDemodulator; MAX_DECODERS] =
            core::array::from_fn(|_| FastDemodulator::new(config));
        let hdlc: [HdlcDecoder; MAX_DECODERS] =
            core::array::from_fn(|_| HdlcDecoder::new());

        let mut idx = 0;
        for f in filters {
            for &o in timing_offsets {
                if idx < MAX_DECODERS {
                    decoders[idx] = FastDemodulator::with_filter_and_offset(config, *f, o);
                    idx += 1;
                }
            }
        }

        Self {
            decoders,
            hdlc,
            num_active: idx,
            recent_hashes: [(0u32, 0u32); DEDUP_RING_SIZE],
            recent_write: 0,
            recent_count: 0,
            generation: 0,
            total_decoded: 0,
            total_unique: 0,
        }
    }

    /// Process audio samples through all decoders.
    ///
    /// Returns a `MultiOutput` containing unique decoded frames.
    /// Deduplication uses a time-windowed approach: only frames decoded
    /// within the last ~4 chunks are considered duplicates.
    pub fn process_samples(&mut self, samples: &[i16]) -> MultiOutput {
        self.generation = self.generation.wrapping_add(1);
        let mut output = MultiOutput::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

        for d in 0..self.num_active {
            let n = self.decoders[d].process_samples(samples, &mut symbols);
            for i in 0..n {
                if let Some(frame_bytes) = self.hdlc[d].feed_bit(symbols[i].bit) {
                    self.total_decoded += 1;

                    // Copy frame data to break borrow before calling self methods
                    let len = frame_bytes.len().min(330);
                    let mut frame_copy = [0u8; 330];
                    frame_copy[..len].copy_from_slice(&frame_bytes[..len]);

                    // Compute hash for dedup
                    let hash = frame_hash(&frame_copy[..len]);
                    if !self.is_duplicate(hash) {
                        self.record_hash(hash);
                        self.total_unique += 1;

                        // Copy to output
                        if output.count < MAX_OUTPUT_FRAMES {
                            output.frames[output.count].data[..len]
                                .copy_from_slice(&frame_copy[..len]);
                            output.frames[output.count].len = len;
                            output.count += 1;
                        }
                    }
                }
            }
        }

        output
    }

    /// Reset all decoders.
    pub fn reset(&mut self) {
        for d in 0..self.num_active {
            self.decoders[d].reset();
            self.hdlc[d].reset();
        }
        self.recent_hashes = [(0u32, 0u32); DEDUP_RING_SIZE];
        self.recent_write = 0;
        self.recent_count = 0;
        self.generation = 0;
    }

    /// Number of active parallel decoders.
    pub fn num_decoders(&self) -> usize {
        self.num_active
    }

    /// Check if a hash was seen recently (within last DEDUP_WINDOW generations).
    fn is_duplicate(&self, hash: u32) -> bool {
        /// Only dedup within this many process_samples calls (~4 chunks ≈ 370ms).
        const DEDUP_WINDOW: u32 = 4;
        let limit = self.recent_count.min(DEDUP_RING_SIZE);
        for i in 0..limit {
            let (h, gen) = self.recent_hashes[i];
            if h == hash && self.generation.wrapping_sub(gen) <= DEDUP_WINDOW {
                return true;
            }
        }
        false
    }

    fn record_hash(&mut self, hash: u32) {
        self.recent_hashes[self.recent_write] = (hash, self.generation);
        self.recent_write = (self.recent_write + 1) % DEDUP_RING_SIZE;
        if self.recent_count < DEDUP_RING_SIZE {
            self.recent_count += 1;
        }
    }
}

/// Simple hash for frame deduplication (FNV-1a 32-bit).
fn frame_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_decoder_creation() {
        let config = DemodConfig::default_1200();
        let multi = MultiDecoder::new(config);
        // 9 base + 8 freq + 9 gain + 4 cross = 30 (std)
        // 9 base + 2 freq + 4 gain = 15 (no_std)
        #[cfg(feature = "std")]
        assert_eq!(multi.num_decoders(), 30);
        #[cfg(not(feature = "std"))]
        assert_eq!(multi.num_decoders(), 15);
        assert_eq!(multi.total_decoded, 0);
        assert_eq!(multi.total_unique, 0);
    }

    #[test]
    fn test_multi_decoder_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut multi = MultiDecoder::new(config);
        let silence = [0i16; 1024];
        let output = multi.process_samples(&silence);
        assert!(output.is_empty());
    }

    #[test]
    fn test_multi_decoder_loopback() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Multi");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut audio_len = 0;

        // Preamble
        for _ in 0..30 {
            let n = modulator.modulate_flag(&mut audio[audio_len..]);
            audio_len += n;
        }
        // Data
        for i in 0..encoded.bit_count {
            let n = modulator.modulate_bit(encoded.bits[i] != 0, &mut audio[audio_len..]);
            audio_len += n;
        }
        // Trailing silence
        audio_len += 20;

        let config = DemodConfig::default_1200();
        let mut multi = MultiDecoder::new(config);
        let output = multi.process_samples(&audio[..audio_len]);

        // At least one decoder should find the frame (deduplicated to 1)
        assert_eq!(output.len(), 1, "Multi-decoder should decode exactly 1 unique frame");
        assert_eq!(output.frame(0), raw);
        assert!(multi.total_decoded >= 1, "At least 1 decoder should find it");
    }

    #[test]
    fn test_frame_hash_consistency() {
        let data1 = b"Hello, World!";
        let data2 = b"Hello, World!";
        let data3 = b"Hello, World?";

        assert_eq!(frame_hash(data1), frame_hash(data2));
        assert_ne!(frame_hash(data1), frame_hash(data3));
    }

    #[test]
    fn test_dedup_ring() {
        let config = DemodConfig::default_1200();
        let mut multi = MultiDecoder::new(config);

        // Set generation so dedup window works
        multi.generation = 1;

        // Test duplicate detection
        multi.record_hash(12345);
        assert!(multi.is_duplicate(12345));
        assert!(!multi.is_duplicate(67890));

        // Test time-window expiry: advance generation past DEDUP_WINDOW
        multi.generation = 10;
        assert!(!multi.is_duplicate(12345), "old hash should expire");
    }
}
