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
//! - 3 AGC decoders (one per BPF variant)
//! - 8 space gain levels = 8 gain-diverse decoders
//! - 4 cross-product (freq offset + gain) decoders
//! - Total: 32 parallel decoders

use super::demod::{DemodSymbol, DmDemodulator, FastDemodulator};
use super::filter::BiquadFilter;
use super::DemodConfig;
use crate::ax25::frame::HdlcDecoder;

/// Maximum number of parallel fast decoders.
const MAX_DECODERS: usize = 32;

/// Maximum number of parallel DM decoders.
const MAX_DM_DECODERS: usize = 6;

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
///
/// Combines fast-path Goertzel decoders with delay-multiply decoders
/// for maximum decode performance across different signal conditions.
pub struct MultiDecoder {
    decoders: [FastDemodulator; MAX_DECODERS],
    hdlc: [HdlcDecoder; MAX_DECODERS],
    num_active: usize,
    /// DM decoders (BPF+LPF, long delay) for complementary coverage.
    dm_decoders: [DmDemodulator; MAX_DM_DECODERS],
    dm_hdlc: [HdlcDecoder; MAX_DM_DECODERS],
    dm_active: usize,
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

        // AGC decoders: one per BPF variant, adapts to mark/space imbalance.
        // These replace 3 of the static gain decoders.
        for f in 0..3 {
            if idx < MAX_DECODERS {
                decoders[idx] = FastDemodulator::with_filter(config, filters[f]).with_agc();
                idx += 1;
            }
        }

        // Gain diversity decoders (Dire Wolf multi-slicer approach).
        // Different space energy gains handle de-emphasis and varying audio paths.
        // Q8 format: 256 = 0 dB.  Reduced from 9 to 8 entries since AGC decoders
        // now handle adaptive gain compensation; +0.8 dB (308) dropped as closest
        // to default 0 dB.
        #[cfg(feature = "std")]
        {
            let gains_q8: [u16; 8] = [64, 107, 181, 511, 868, 1440, 2445, 4057];
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
            let gains_q8: [u16; 3] = [181, 1440, 4057];
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

        // DM decoders with BPF+LPF and delay/timing/PLL diversity.
        // These use a different algorithm (delay-multiply discriminator) that
        // is complementary to Goertzel — it decodes frames that Goertzel misses.
        // PLL decoders adapt to clock drift; Bresenham decoders use fixed timing.
        let mut dm_decoders: [DmDemodulator; MAX_DM_DECODERS] =
            core::array::from_fn(|_| DmDemodulator::with_bpf(config));
        let dm_hdlc: [HdlcDecoder; MAX_DM_DECODERS] =
            core::array::from_fn(|_| HdlcDecoder::new());

        let mut dm_idx = 0;
        // DM+PLL decoders (Gardner TED with phase + frequency correction)
        // Alpha=936 is optimal single-decoder; alpha=400 provides diversity.
        // Beta=0 universally optimal (frequency correction hurts).
        // Tune sweep showed alpha=600-800 best per-track, but multi-decoder
        // ensemble makes individual DM alpha tuning negligible (<1 frame).
        if dm_idx < MAX_DM_DECODERS {
            dm_decoders[dm_idx] = DmDemodulator::with_bpf_pll_custom(config, 936, 0);
            dm_idx += 1;
        }
        if dm_idx < MAX_DM_DECODERS {
            dm_decoders[dm_idx] = DmDemodulator::with_bpf_pll_custom(config, 400, 0);
            dm_idx += 1;
        }
        // DM+Bresenham decoders with timing diversity (complementary to PLL)
        for &phase in &offsets {
            if dm_idx < MAX_DM_DECODERS {
                dm_decoders[dm_idx] = DmDemodulator::with_bpf_and_offset(config, phase);
                dm_idx += 1;
            }
        }
        // DM+Bresenham with alternate delay (d=5 at 11025 Hz ≈ τ=454μs).
        // This delay has the highest mark/space separation (1.96) and decodes
        // different frames than d=8.
        let alt_delay = match config.sample_rate {
            11025 => 5,
            22050 => 10,
            44100 => 20,
            _ => config.sample_rate as usize / 2400,
        };
        if dm_idx < MAX_DM_DECODERS {
            dm_decoders[dm_idx] = DmDemodulator::with_bpf_delay_and_offset(config, alt_delay, 0);
            dm_idx += 1;
        }

        Self {
            decoders,
            hdlc,
            num_active: idx,
            dm_decoders,
            dm_hdlc,
            dm_active: dm_idx,
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
        let dm_decoders: [DmDemodulator; MAX_DM_DECODERS] =
            core::array::from_fn(|_| DmDemodulator::with_bpf(config));
        let dm_hdlc: [HdlcDecoder; MAX_DM_DECODERS] =
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
            dm_decoders,
            dm_hdlc,
            dm_active: 0, // no DM decoders in custom diversity mode
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

        // Process fast (Goertzel) decoders
        for d in 0..self.num_active {
            let n = self.decoders[d].process_samples(samples, &mut symbols);
            for i in 0..n {
                if let Some(frame_bytes) = self.hdlc[d].feed_bit(symbols[i].bit) {
                    self.total_decoded += 1;
                    let len = frame_bytes.len().min(330);
                    let mut frame_copy = [0u8; 330];
                    frame_copy[..len].copy_from_slice(&frame_bytes[..len]);
                    let hash = frame_hash(&frame_copy[..len]);
                    if !self.is_duplicate(hash) {
                        self.record_hash(hash);
                        self.total_unique += 1;
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

        // Process DM (delay-multiply) decoders
        for d in 0..self.dm_active {
            let n = self.dm_decoders[d].process_samples(samples, &mut symbols);
            for i in 0..n {
                if let Some(frame_bytes) = self.dm_hdlc[d].feed_bit(symbols[i].bit) {
                    self.total_decoded += 1;
                    let len = frame_bytes.len().min(330);
                    let mut frame_copy = [0u8; 330];
                    frame_copy[..len].copy_from_slice(&frame_bytes[..len]);
                    let hash = frame_hash(&frame_copy[..len]);
                    if !self.is_duplicate(hash) {
                        self.record_hash(hash);
                        self.total_unique += 1;
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
        for d in 0..self.dm_active {
            self.dm_decoders[d].reset();
            self.dm_hdlc[d].reset();
        }
        self.recent_hashes = [(0u32, 0u32); DEDUP_RING_SIZE];
        self.recent_write = 0;
        self.recent_count = 0;
        self.generation = 0;
    }

    /// Number of active parallel decoders (fast + DM).
    pub fn num_decoders(&self) -> usize {
        self.num_active + self.dm_active
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

// ─── MiniDecoder ("Smart 3") ─────────────────────────────────────────────

/// Maximum decoders in the MiniDecoder.
const MINI_DECODERS: usize = 3;

/// MiniDecoder — runs the 3 attribution-optimal decoders identified by
/// greedy set-cover analysis of the full 38-decoder ensemble.
///
/// These 3 decoders capture ~97% of the multi-decoder output at 8% of
/// the compute cost, making this suitable for ESP32 and other MCU targets.
///
/// Decoder configuration:
/// 1. `G:freq-50/t2` — Goertzel with −50 Hz frequency offset, timing phase 2
/// 2. `G:narrow/t0`  — Goertzel with narrow BPF, timing phase 0
/// 3. `G:narrow/t1`  — Goertzel with narrow BPF, timing phase 1
pub struct MiniDecoder {
    decoders: [FastDemodulator; MINI_DECODERS],
    hdlc: [HdlcDecoder; MINI_DECODERS],
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

impl MiniDecoder {
    /// Create a MiniDecoder with the 3 attribution-optimal configurations.
    pub fn new(config: DemodConfig) -> Self {
        let offsets = [0u32, config.sample_rate / 3, 2 * config.sample_rate / 3];
        let narrow_bpf = super::filter::afsk_bandpass_narrow_11025();

        // Decoder 1: G:freq-50/t2 — frequency-shifted −50 Hz, timing phase 2
        let mark_shifted = (config.mark_freq as i32 - 50) as u32;
        let space_shifted = (config.space_freq as i32 - 50) as u32;
        #[cfg(feature = "std")]
        let shifted_bpf = {
            let center = (super::MID_FREQ as i32 - 50) as f64;
            super::filter::bandpass_coeffs(config.sample_rate, center, 2000.0)
        };
        #[cfg(not(feature = "std"))]
        let shifted_bpf = super::filter::afsk_bandpass_wide_11025();

        let decoders = [
            FastDemodulator::with_filter_freq_and_offset(
                config, shifted_bpf, offsets[2], mark_shifted, space_shifted,
            ),
            FastDemodulator::with_filter_and_offset(config, narrow_bpf, offsets[0]),
            FastDemodulator::with_filter_and_offset(config, narrow_bpf, offsets[1]),
        ];

        Self {
            decoders,
            hdlc: core::array::from_fn(|_| HdlcDecoder::new()),
            recent_hashes: [(0u32, 0u32); DEDUP_RING_SIZE],
            recent_write: 0,
            recent_count: 0,
            generation: 0,
            total_decoded: 0,
            total_unique: 0,
        }
    }

    /// Process audio samples through all 3 decoders.
    pub fn process_samples(&mut self, samples: &[i16]) -> MultiOutput {
        self.generation = self.generation.wrapping_add(1);
        let mut output = MultiOutput::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

        for d in 0..MINI_DECODERS {
            let n = self.decoders[d].process_samples(samples, &mut symbols);
            for i in 0..n {
                if let Some(frame_bytes) = self.hdlc[d].feed_bit(symbols[i].bit) {
                    self.total_decoded += 1;
                    let len = frame_bytes.len().min(330);
                    let mut frame_copy = [0u8; 330];
                    frame_copy[..len].copy_from_slice(&frame_bytes[..len]);
                    let hash = frame_hash(&frame_copy[..len]);
                    if !self.is_duplicate(hash) {
                        self.record_hash(hash);
                        self.total_unique += 1;
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
        for d in 0..MINI_DECODERS {
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
        MINI_DECODERS
    }

    /// Check if a hash was seen recently (within last DEDUP_WINDOW generations).
    fn is_duplicate(&self, hash: u32) -> bool {
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

// ─── Attribution tracking (per-decoder frame provenance) ──────────────────

#[cfg(feature = "attribution")]
extern crate alloc;

/// Human-readable description of a decoder's configuration.
#[cfg(feature = "attribution")]
#[derive(Clone, Debug)]
pub struct DecoderConfig {
    /// Decoder index (0-based).
    pub index: usize,
    /// Short label, e.g. "G:std/t0/0Hz/agc" or "DM:pll/a936/b0".
    pub label: alloc::string::String,
    /// Algorithm: "goertzel" or "dm".
    pub algorithm: &'static str,
    /// Dimension tags for aggregate grouping.
    pub tags: alloc::vec::Vec<&'static str>,
}

/// Per-decoder statistics accumulated across all audio chunks.
#[cfg(feature = "attribution")]
#[derive(Clone, Debug, Default)]
pub struct DecoderStats {
    /// Total frames found by this decoder (before dedup).
    pub total: usize,
    /// Frames where this decoder was the *first* to find it (won the dedup race).
    pub first: usize,
    /// Frames found *only* by this decoder (no other decoder found it).
    pub exclusive: usize,
}

/// Attribution data from a single process_samples_attributed() call.
#[cfg(feature = "attribution")]
pub struct AttributedOutput {
    /// The deduplicated output (same as process_samples).
    pub output: MultiOutput,
    /// For each frame found (before dedup): (decoder_index, frame_hash).
    pub raw_hits: alloc::vec::Vec<(usize, u32)>,
    /// For each unique frame in output: (frame_hash, list of decoder indices that found it).
    pub frame_decoders: alloc::vec::Vec<(u32, alloc::vec::Vec<usize>)>,
}

/// Accumulated attribution report across all audio.
#[cfg(feature = "attribution")]
pub struct AttributionReport {
    pub configs: alloc::vec::Vec<DecoderConfig>,
    pub stats: alloc::vec::Vec<DecoderStats>,
    /// All unique frame hashes seen, mapped to set of decoder indices.
    pub frame_map: alloc::collections::BTreeMap<u32, alloc::vec::Vec<usize>>,
}

#[cfg(feature = "attribution")]
impl AttributionReport {
    pub fn new(configs: alloc::vec::Vec<DecoderConfig>) -> Self {
        let n = configs.len();
        Self {
            configs,
            stats: alloc::vec![DecoderStats::default(); n],
            frame_map: alloc::collections::BTreeMap::new(),
        }
    }

    /// Merge results from one process_samples_attributed() call.
    pub fn merge(&mut self, attributed: &AttributedOutput) {
        // Count raw hits per decoder
        for &(dec_idx, _) in &attributed.raw_hits {
            if dec_idx < self.stats.len() {
                self.stats[dec_idx].total += 1;
            }
        }
        // Track which decoders found each unique frame
        for (hash, decoders) in &attributed.frame_decoders {
            let entry = self.frame_map.entry(*hash).or_insert_with(alloc::vec::Vec::new);
            for &d in decoders {
                if !entry.contains(&d) {
                    entry.push(d);
                }
            }
            // First decoder in list won the race
            if let Some(&first) = decoders.first() {
                if first < self.stats.len() {
                    self.stats[first].first += 1;
                }
            }
        }
    }

    /// Finalize: compute exclusive counts from the accumulated frame_map.
    pub fn finalize(&mut self) {
        // Reset exclusive counts
        for s in &mut self.stats {
            s.exclusive = 0;
        }
        for (_hash, decoders) in &self.frame_map {
            if decoders.len() == 1 {
                let d = decoders[0];
                if d < self.stats.len() {
                    self.stats[d].exclusive += 1;
                }
            }
        }
    }

    /// Total unique frames across all decoders.
    pub fn total_unique(&self) -> usize {
        self.frame_map.len()
    }

    /// Greedy set-cover: returns vec of (decoder_index, cumulative_frames) showing
    /// how many decoders are needed to reach N% coverage.
    pub fn coverage_curve(&self) -> alloc::vec::Vec<(usize, usize)> {
        use alloc::collections::BTreeSet;

        let total = self.frame_map.len();
        if total == 0 {
            return alloc::vec::Vec::new();
        }

        // Build per-decoder frame sets
        let n = self.configs.len();
        let mut decoder_frames: alloc::vec::Vec<BTreeSet<u32>> =
            alloc::vec![BTreeSet::new(); n];
        for (&hash, decoders) in &self.frame_map {
            for &d in decoders {
                if d < n {
                    decoder_frames[d].insert(hash);
                }
            }
        }

        let mut covered: BTreeSet<u32> = BTreeSet::new();
        let mut used: BTreeSet<usize> = BTreeSet::new();
        let mut curve = alloc::vec::Vec::new();

        while covered.len() < total && used.len() < n {
            // Find decoder that adds the most uncovered frames
            let mut best_idx = 0;
            let mut best_new = 0;
            for d in 0..n {
                if used.contains(&d) {
                    continue;
                }
                let new_count = decoder_frames[d].difference(&covered).count();
                if new_count > best_new {
                    best_new = new_count;
                    best_idx = d;
                }
            }
            if best_new == 0 {
                break;
            }
            for h in &decoder_frames[best_idx] {
                covered.insert(*h);
            }
            used.insert(best_idx);
            curve.push((best_idx, covered.len()));
        }

        curve
    }

    /// Aggregate stats by tag (e.g., "agc", "pll", "dm", "goertzel").
    pub fn stats_by_tag(&self) -> alloc::collections::BTreeMap<&'static str, DecoderStats> {
        let _n = self.configs.len();
        let mut tag_decoders: alloc::collections::BTreeMap<&'static str, alloc::vec::Vec<usize>> =
            alloc::collections::BTreeMap::new();
        for (i, cfg) in self.configs.iter().enumerate() {
            for &tag in &cfg.tags {
                tag_decoders.entry(tag).or_insert_with(alloc::vec::Vec::new).push(i);
            }
        }

        let mut result = alloc::collections::BTreeMap::new();
        for (tag, decoders) in &tag_decoders {
            let mut total = 0usize;
            let mut exclusive = 0usize;
            // Count frames found by any decoder with this tag
            let decoder_set: alloc::collections::BTreeSet<usize> = decoders.iter().copied().collect();
            let mut tag_frames = 0usize;
            for (_hash, frame_decoders) in &self.frame_map {
                let any_in_tag = frame_decoders.iter().any(|d| decoder_set.contains(d));
                if any_in_tag {
                    tag_frames += 1;
                    // Exclusive to this tag = no decoder outside tag found it
                    let all_in_tag = frame_decoders.iter().all(|d| decoder_set.contains(d));
                    if all_in_tag {
                        exclusive += 1;
                    }
                }
            }
            for &d in decoders {
                total += self.stats[d].total;
            }
            result.insert(*tag, DecoderStats {
                total,
                first: tag_frames,  // reuse "first" to mean "any in tag found"
                exclusive,
            });
        }
        result
    }
}

#[cfg(feature = "attribution")]
impl MultiDecoder {
    /// Return human-readable labels for all active decoders, matching construction order.
    pub fn decoder_configs(&self) -> alloc::vec::Vec<DecoderConfig> {
        use alloc::format;
        use alloc::string::String;
        use alloc::vec;
        use alloc::vec::Vec;

        let mut configs = Vec::new();
        let mut idx = 0;

        // 9 base decoders: 3 BPF × 3 timing
        let bpf_names = ["narrow", "std", "wide"];
        let timing_names = ["t0", "t1", "t2"];
        for f in 0..3 {
            for o in 0..3 {
                if idx < self.num_active {
                    configs.push(DecoderConfig {
                        index: idx,
                        label: format!("G:{}/{}",  bpf_names[f], timing_names[o]),
                        algorithm: "goertzel",
                        tags: vec!["goertzel", "base", bpf_names[f], timing_names[o]],
                    });
                    idx += 1;
                }
            }
        }

        // Frequency-shifted decoders
        #[cfg(feature = "std")]
        {
            let freq_offsets: [i32; 4] = [-50, 50, -100, 100];
            for &offset in &freq_offsets {
                let timing_count = if offset.abs() <= 50 { 3 } else { 1 };
                for t in 0..timing_count {
                    if idx < self.num_active {
                        configs.push(DecoderConfig {
                            index: idx,
                            label: format!("G:freq{:+}/{}",  offset, timing_names[t]),
                            algorithm: "goertzel",
                            tags: vec!["goertzel", "freq-shift"],
                        });
                        idx += 1;
                    }
                }
            }
        }
        #[cfg(not(feature = "std"))]
        {
            let freq_offsets: [i32; 2] = [-50, 50];
            for &offset in &freq_offsets {
                if idx < self.num_active {
                    configs.push(DecoderConfig {
                        index: idx,
                        label: format!("G:freq{:+}/t0", offset),
                        algorithm: "goertzel",
                        tags: vec!["goertzel", "freq-shift"],
                    });
                    idx += 1;
                }
            }
        }

        // AGC decoders
        for f in 0..3 {
            if idx < self.num_active {
                configs.push(DecoderConfig {
                    index: idx,
                    label: format!("G:{}/agc", bpf_names[f]),
                    algorithm: "goertzel",
                    tags: vec!["goertzel", "agc"],
                });
                idx += 1;
            }
        }

        // Gain diversity decoders
        #[cfg(feature = "std")]
        {
            let gains_q8: [u16; 8] = [64, 107, 181, 511, 868, 1440, 2445, 4057];
            let gain_db: [&str; 8] = ["-12dB", "-7.6dB", "-3dB", "+6dB", "+10.6dB", "+15dB", "+19.6dB", "+24dB"];
            for (i, &_gain) in gains_q8.iter().enumerate() {
                if idx < self.num_active {
                    configs.push(DecoderConfig {
                        index: idx,
                        label: format!("G:gain/{}", gain_db[i]),
                        algorithm: "goertzel",
                        tags: vec!["goertzel", "gain"],
                    });
                    idx += 1;
                }
            }
        }
        #[cfg(not(feature = "std"))]
        {
            let gain_db: [&str; 3] = ["-3dB", "+15dB", "+24dB"];
            for db in &gain_db {
                if idx < self.num_active {
                    configs.push(DecoderConfig {
                        index: idx,
                        label: format!("G:gain/{}", db),
                        algorithm: "goertzel",
                        tags: vec!["goertzel", "gain"],
                    });
                    idx += 1;
                }
            }
        }

        // Cross-product decoders (std only)
        #[cfg(feature = "std")]
        {
            let cross_labels = [
                "G:freq-50/+10.6dB",
                "G:freq+50/+10.6dB",
                "G:freq-50/+15dB",
                "G:freq+50/+15dB",
            ];
            for &label in &cross_labels {
                if idx < self.num_active {
                    configs.push(DecoderConfig {
                        index: idx,
                        label: String::from(label),
                        algorithm: "goertzel",
                        tags: vec!["goertzel", "cross"],
                    });
                    idx += 1;
                }
            }
        }

        // DM decoders
        let dm_start = self.num_active; // DM indices are offset
        let mut dm_idx = 0;

        // DM+PLL decoders
        let dm_pll_labels = ["DM:pll/a936/b0", "DM:pll/a400/b0"];
        for &label in &dm_pll_labels {
            if dm_idx < self.dm_active {
                configs.push(DecoderConfig {
                    index: dm_start + dm_idx,
                    label: String::from(label),
                    algorithm: "dm",
                    tags: vec!["dm", "pll"],
                });
                dm_idx += 1;
            }
        }

        // DM+Bresenham with timing diversity (d=8)
        for t in 0..3 {
            if dm_idx < self.dm_active {
                configs.push(DecoderConfig {
                    index: dm_start + dm_idx,
                    label: format!("DM:bres/d8/{}", timing_names[t]),
                    algorithm: "dm",
                    tags: vec!["dm", "bresenham"],
                });
                dm_idx += 1;
            }
        }

        // DM+Bresenham alternate delay
        if dm_idx < self.dm_active {
            configs.push(DecoderConfig {
                index: dm_start + dm_idx,
                label: String::from("DM:bres/d5/t0"),
                algorithm: "dm",
                tags: vec!["dm", "bresenham", "alt-delay"],
            });
            #[allow(unused_assignments)]
            { dm_idx += 1; }
        }

        configs
    }

    /// Process audio with per-decoder attribution tracking.
    pub fn process_samples_attributed(&mut self, samples: &[i16]) -> AttributedOutput {
        use alloc::vec::Vec;
        use alloc::collections::BTreeMap;

        self.generation = self.generation.wrapping_add(1);
        let mut output = MultiOutput::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        let mut raw_hits: Vec<(usize, u32)> = Vec::new();
        // hash -> list of decoder indices
        let mut frame_decoder_map: BTreeMap<u32, Vec<usize>> = BTreeMap::new();

        // Process fast (Goertzel) decoders
        for d in 0..self.num_active {
            let n = self.decoders[d].process_samples(samples, &mut symbols);
            for i in 0..n {
                if let Some(frame_bytes) = self.hdlc[d].feed_bit(symbols[i].bit) {
                    self.total_decoded += 1;
                    let len = frame_bytes.len().min(330);
                    let mut frame_copy = [0u8; 330];
                    frame_copy[..len].copy_from_slice(&frame_bytes[..len]);
                    let hash = frame_hash(&frame_copy[..len]);
                    raw_hits.push((d, hash));
                    frame_decoder_map.entry(hash).or_insert_with(Vec::new).push(d);
                    if !self.is_duplicate(hash) {
                        self.record_hash(hash);
                        self.total_unique += 1;
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

        // Process DM decoders — use dm_start offset for decoder index
        let dm_start = self.num_active;
        for d in 0..self.dm_active {
            let n = self.dm_decoders[d].process_samples(samples, &mut symbols);
            for i in 0..n {
                if let Some(frame_bytes) = self.dm_hdlc[d].feed_bit(symbols[i].bit) {
                    self.total_decoded += 1;
                    let len = frame_bytes.len().min(330);
                    let mut frame_copy = [0u8; 330];
                    frame_copy[..len].copy_from_slice(&frame_bytes[..len]);
                    let hash = frame_hash(&frame_copy[..len]);
                    raw_hits.push((dm_start + d, hash));
                    frame_decoder_map.entry(hash).or_insert_with(Vec::new).push(dm_start + d);
                    if !self.is_duplicate(hash) {
                        self.record_hash(hash);
                        self.total_unique += 1;
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

        let frame_decoders: Vec<(u32, Vec<usize>)> = frame_decoder_map.into_iter().collect();

        AttributedOutput {
            output,
            raw_hits,
            frame_decoders,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_decoder_creation() {
        let config = DemodConfig::default_1200();
        let multi = MultiDecoder::new(config);
        // Fast: 9 base + 8 freq + 3 AGC + 8 gain + 4 cross = 32 (std)
        // Fast: 9 base + 2 freq + 3 AGC + 3 gain = 17 (no_std)
        // DM: 2 PLL + 3 Bresenham d=8 + 1 Bresenham d=5 = 6 decoders
        #[cfg(feature = "std")]
        assert_eq!(multi.num_decoders(), 38);
        #[cfg(not(feature = "std"))]
        assert_eq!(multi.num_decoders(), 23);
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
    fn test_mini_decoder_creation() {
        let config = DemodConfig::default_1200();
        let mini = MiniDecoder::new(config);
        assert_eq!(mini.num_decoders(), 3);
        assert_eq!(mini.total_decoded, 0);
        assert_eq!(mini.total_unique, 0);
    }

    #[test]
    fn test_mini_decoder_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut mini = MiniDecoder::new(config);
        let silence = [0i16; 1024];
        let output = mini.process_samples(&silence);
        assert!(output.is_empty());
    }

    #[test]
    fn test_mini_decoder_loopback() {
        use crate::ax25::frame::{build_test_frame, hdlc_encode};
        use crate::modem::afsk::AfskModulator;
        use crate::modem::ModConfig;

        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Mini");
        let raw = &frame_data[..frame_len];
        let encoded = hdlc_encode(raw);

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
        audio_len += 20;

        let config = DemodConfig::default_1200();
        let mut mini = MiniDecoder::new(config);
        let output = mini.process_samples(&audio[..audio_len]);

        assert_eq!(output.len(), 1, "MiniDecoder should decode exactly 1 unique frame");
        assert_eq!(output.frame(0), raw);
        assert!(mini.total_decoded >= 1, "At least 1 decoder should find it");
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
