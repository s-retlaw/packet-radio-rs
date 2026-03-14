//! AFSK Modem — Bell 202 modulation and demodulation
//!
//! This module provides the core DSP for 1200 baud AFSK as used in APRS and
//! packet radio. Four demodulation modes are available:
//!
//! **Fast path**: Goertzel tone detection with Bresenham symbol timing.
//! Hard-decision decoding. Optional AGC normalizes mark/space energy averages
//! to compensate for de-emphasis and other frequency-dependent gain differences.
//! Minimal CPU/RAM — suitable for Cortex-M0, RP2040.
//!
//! **Quality path**: Same Goertzel+Bresenham core, plus Hilbert transform for
//! LLR confidence values. Feeds SoftHdlcDecoder for 1-2 bit error recovery.
//!
//! **Delay-multiply path**: Continuous discriminator (delay-and-multiply) with
//! PLL clock recovery. Produces sample-by-sample output and adapts to
//! transmitter baud rate drift. Lower CPU than Goertzel (1 multiply/sample).
//!
//! **Multi-decoder** (`multi-decoder` feature): 32 parallel fast-path decoders
//! with filter bandwidth, timing offset, frequency offset, gain diversity,
//! and AGC (automatic gain control). AGC decoders adapt to mark/space energy
//! imbalance from de-emphasis. Gain diversity (Dire Wolf multi-slicer approach)
//! provides static compensation. Cross-product decoders handle combined freq
//! offset + de-emphasis.
//!
//! # Architecture
//!
//! ```text
//! Fast:    Samples → BPF → Goertzel → Bresenham → NRZI → HDLC
//! Quality: Samples → BPF → Goertzel+Hilbert → Bresenham → NRZI → SoftHDLC
//! DM:      Samples → BPF → Delay-Multiply → LPF → PLL → NRZI → HDLC
//! Multi:   Samples → [N× diverse decoders → HDLC] → Dedup
//! Mod:     Bits → NRZI Encode → Phase Accumulator (NCO) → Samples
//! ```
//!
//! See docs/MODEM_DESIGN.md for detailed algorithm descriptions.

pub mod afsk;
pub mod demod;
pub mod delay_multiply;
pub mod filter;
pub mod hilbert;
pub mod adaptive;
pub mod pll;
pub mod soft_hdlc;
#[cfg(feature = "multi-decoder")]
pub mod multi;
pub mod corr_slicer;
pub mod binary_xor;
pub mod fixed_vec;
pub mod frame_output;
pub mod hdlc_bank;

// 9600 baud G3RUH modules
#[cfg(feature = "9600-baud")]
pub mod scrambler;
#[cfg(feature = "9600-baud")]
pub mod demod_9600;
#[cfg(feature = "9600-baud")]
pub mod mod_9600;
#[cfg(all(feature = "9600-baud", feature = "multi-decoder"))]
pub mod multi_9600;

/// Standard Bell 202 mark frequency (Hz)
pub const MARK_FREQ: u32 = 1200;

/// Standard Bell 202 space frequency (Hz)
pub const SPACE_FREQ: u32 = 2200;

/// Standard baud rate
pub const BAUD_RATE: u32 = 1200;

/// Midpoint frequency between mark and space (Hz)
pub const MID_FREQ: u32 = (MARK_FREQ + SPACE_FREQ) / 2; // 1700 Hz

/// 300 baud mark frequency (Hz) — Bell 103/HF packet convention
pub const MARK_FREQ_300: u32 = 1600;

/// 300 baud space frequency (Hz)
pub const SPACE_FREQ_300: u32 = 1800;

/// 300 baud rate
pub const BAUD_RATE_300: u32 = 300;

/// Maximum delay line length (samples) for the delay-multiply detector.
/// 48 supports 300 baud DM at 11025 Hz (delay=37 samples).
pub const MAX_DELAY: usize = 48;

/// Maximum number of bits in a single frame (for soft bit buffer)
/// AX.25 max frame = 330 bytes × 8 bits + flags + stuffing ≈ 3000 bits
pub const MAX_FRAME_BITS: usize = 3200;

/// Number of candidate bits to consider for bit-flip recovery.
/// Top-12 used for single/pair flips, top-8 for triple flips.
pub const MAX_FLIP_CANDIDATES: usize = 12;

/// Confidence threshold for candidate inclusion in bit-flip recovery.
/// Only bits with |LLR| < this value are considered for flipping.
pub const FLIP_CONFIDENCE_THRESHOLD: u8 = 96;

/// Maximum candidates to use for triple-flip search (controls compute budget).
pub const TRIPLE_FLIP_LIMIT: usize = 8;

/// Demodulator configuration
#[derive(Clone, Copy, Debug)]
pub struct DemodConfig {
    /// Audio sample rate in Hz
    pub sample_rate: u32,
    /// Mark frequency in Hz (nominally 1200)
    pub mark_freq: u32,
    /// Space frequency in Hz (nominally 2200)
    pub space_freq: u32,
    /// Baud rate (nominally 1200)
    pub baud_rate: u32,
    /// PLL bandwidth factor (higher = faster lock, more jitter)
    pub pll_alpha: i16,
    /// PLL frequency correction gain
    pub pll_beta: i16,
}

impl DemodConfig {
    /// Default configuration for 1200 baud AFSK at 11025 Hz sample rate.
    pub fn default_1200() -> Self {
        Self {
            sample_rate: 11025,
            mark_freq: MARK_FREQ,
            space_freq: SPACE_FREQ,
            baud_rate: BAUD_RATE,
            pll_alpha: 936,   // ~0.0286 in Q15 — moderate tracking
            pll_beta: 0,      // beta=0 universally optimal (frequency correction hurts)
        }
    }

    /// Default configuration for 300 baud AFSK at 11025 Hz sample rate.
    /// Uses mark=1600 Hz, space=1800 Hz (200 Hz separation).
    pub fn default_300() -> Self {
        Self {
            sample_rate: 11025,
            mark_freq: MARK_FREQ_300,
            space_freq: SPACE_FREQ_300,
            baud_rate: BAUD_RATE_300,
            pll_alpha: 936,
            pll_beta: 0,
        }
    }

    /// Configuration for 300 baud AFSK at 8000 Hz sample rate.
    pub fn default_300_8k() -> Self {
        Self {
            sample_rate: 8000,
            ..Self::default_300()
        }
    }

    /// Configuration at 22050 Hz sample rate (better quality, more CPU).
    pub fn default_1200_22k() -> Self {
        Self {
            sample_rate: 22050,
            ..Self::default_1200()
        }
    }

    /// Configuration at 44100 Hz sample rate (CD quality).
    pub fn default_1200_44k() -> Self {
        Self {
            sample_rate: 44100,
            ..Self::default_1200()
        }
    }

    /// Number of audio samples per symbol period.
    pub fn samples_per_symbol(&self) -> u32 {
        self.sample_rate / self.baud_rate.max(1)
    }
}

/// Modulator configuration
#[derive(Clone, Debug)]
pub struct ModConfig {
    /// Audio sample rate in Hz
    pub sample_rate: u32,
    /// Mark frequency in Hz
    pub mark_freq: u32,
    /// Space frequency in Hz
    pub space_freq: u32,
    /// Baud rate
    pub baud_rate: u32,
    /// Output amplitude (0-32767 for i16)
    pub amplitude: i16,
}

impl ModConfig {
    /// Default modulator configuration at 11025 Hz.
    pub fn default_1200() -> Self {
        Self {
            sample_rate: 11025,
            mark_freq: MARK_FREQ,
            space_freq: SPACE_FREQ,
            baud_rate: BAUD_RATE,
            amplitude: 16000,
        }
    }

    /// Default modulator configuration for 300 baud at 11025 Hz.
    pub fn default_300() -> Self {
        Self {
            sample_rate: 11025,
            mark_freq: MARK_FREQ_300,
            space_freq: SPACE_FREQ_300,
            baud_rate: BAUD_RATE_300,
            amplitude: 16000,
        }
    }

    /// Number of audio samples per symbol period.
    pub fn samples_per_symbol(&self) -> u32 {
        self.sample_rate / self.baud_rate.max(1)
    }
}

/// 256-entry sine lookup table, Q15 format.
/// SIN_TABLE[i] = round(sin(2π·i/256) × 32767)
pub static SIN_TABLE_Q15: [i16; 256] = [
        0,   804,  1608,  2410,  3212,  4011,  4808,  5602,
     6393,  7179,  7962,  8739,  9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530,
    18204, 18868, 19519, 20159, 20787, 21403, 22005, 22594,
    23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790,
    27245, 27683, 28105, 28510, 28898, 29268, 29621, 29956,
    30273, 30571, 30852, 31113, 31356, 31580, 31785, 31971,
    32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757,
    32767, 32757, 32728, 32678, 32609, 32521, 32412, 32285,
    32137, 31971, 31785, 31580, 31356, 31113, 30852, 30571,
    30273, 29956, 29621, 29268, 28898, 28510, 28105, 27683,
    27245, 26790, 26319, 25832, 25329, 24811, 24279, 23731,
    23170, 22594, 22005, 21403, 20787, 20159, 19519, 18868,
    18204, 17530, 16846, 16151, 15446, 14732, 14010, 13279,
    12539, 11793, 11039, 10278,  9512,  8739,  7962,  7179,
     6393,  5602,  4808,  4011,  3212,  2410,  1608,   804,
        0,  -804, -1608, -2410, -3212, -4011, -4808, -5602,
    -6393, -7179, -7962, -8739, -9512,-10278,-11039,-11793,
   -12539,-13279,-14010,-14732,-15446,-16151,-16846,-17530,
   -18204,-18868,-19519,-20159,-20787,-21403,-22005,-22594,
   -23170,-23731,-24279,-24811,-25329,-25832,-26319,-26790,
   -27245,-27683,-28105,-28510,-28898,-29268,-29621,-29956,
   -30273,-30571,-30852,-31113,-31356,-31580,-31785,-31971,
   -32137,-32285,-32412,-32521,-32609,-32678,-32728,-32757,
   -32767,-32757,-32728,-32678,-32609,-32521,-32412,-32285,
   -32137,-31971,-31785,-31580,-31356,-31113,-30852,-30571,
   -30273,-29956,-29621,-29268,-28898,-28510,-28105,-27683,
   -27245,-26790,-26319,-25832,-25329,-24811,-24279,-23731,
   -23170,-22594,-22005,-21403,-20787,-20159,-19519,-18868,
   -18204,-17530,-16846,-16151,-15446,-14732,-14010,-13279,
   -12539,-11793,-11039,-10278, -9512, -8739, -7962, -7179,
    -6393, -5602, -4808, -4011, -3212, -2410, -1608,  -804,
];

// ─── Shared utilities for multi-decoder modules ────────────────────────

/// FNV-1a 32-bit hash for frame deduplication.
///
/// Used by `MultiDecoder`, `MiniDecoder`, `CorrSlicerDecoder`, and
/// `Multi9600Decoder` to detect duplicate frames across parallel decoders.
pub fn frame_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Result of checking a frame against the dedup ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupAction {
    /// No overlapping frame — emit this frame.
    New,
    /// An overlapping frame exists with equal or better cost — drop this one.
    Duplicate,
    /// An overlapping frame exists with worse cost — replace it at the given output slot.
    Replace(u8),
}

/// Dedup ring entry: hash, start sample, cost, and output buffer slot.
#[derive(Clone, Copy)]
struct DedupEntry {
    hash: u32,
    start_sample: u64,
    cost: u16,
    slot: u8,
}

impl Default for DedupEntry {
    fn default() -> Self {
        Self { hash: 0, start_sample: 0, cost: 0, slot: 0 }
    }
}

/// Time-window-based dedup ring buffer for multi-decoder frame deduplication.
///
/// Tracks frame start samples in a fixed-size ring. Frames whose start
/// samples overlap (within a duration window) are considered from the same
/// physical transmission. Among overlapping frames, only the one with the
/// lowest cost (best quality) is kept.
pub struct DedupRing<const N: usize> {
    entries: [DedupEntry; N],
    write_idx: usize,
    count: usize,
    /// Maximum sample distance for two frames to be considered overlapping.
    /// Set via `set_overlap_window()` or defaults to `MAX_OVERLAP_SAMPLES`.
    overlap_window: u64,
}

impl<const N: usize> Default for DedupRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Default overlap window in samples for time-based dedup.
/// ~80 symbols at 1200 baud/11025 Hz = 720 samples. Tight enough to avoid
/// merging consecutive packets, wide enough to catch same-TX decodes
/// with slightly different frame lengths from different decoders.
const DEFAULT_OVERLAP_SAMPLES: u64 = 800;

/// Number of symbol periods used to compute the overlap window.
/// overlap_window = OVERLAP_SYMBOLS * samples_per_symbol.
/// 89 symbols ≈ 801 samples at 1200 baud / 11025 Hz, matching the legacy default.
/// (samples_per_symbol = 11025/1200 = 9 via integer division, so 89 × 9 = 801)
const OVERLAP_SYMBOLS: u64 = 89;

/// Expiry window: hash-based entries older than this many samples are ignored.
/// ~0.45 seconds at 11025 Hz — matches the old 4-generation window (~4 × 1024 samples).
/// Must be short enough to allow repeated transmissions of identical content.
const EXPIRY_SAMPLES: u64 = 5000;

impl<const N: usize> DedupRing<N> {
    /// Create a new empty dedup ring.
    pub fn new() -> Self {
        Self {
            entries: [DedupEntry::default(); N],
            write_idx: 0,
            count: 0,
            overlap_window: DEFAULT_OVERLAP_SAMPLES,
        }
    }

    /// Create a dedup ring with overlap window scaled to samples-per-symbol.
    ///
    /// `overlap_window = OVERLAP_SYMBOLS * sps`, which adapts automatically
    /// to any sample_rate / baud_rate combination.
    pub fn with_overlap_from_config(sps: u32) -> Self {
        Self {
            entries: [DedupEntry::default(); N],
            write_idx: 0,
            count: 0,
            overlap_window: OVERLAP_SYMBOLS * sps as u64,
        }
    }

    /// Set the overlap window in samples.
    pub fn set_overlap_window(&mut self, window: u64) {
        self.overlap_window = window;
    }

    /// No-op for backward compatibility. Time-based dedup doesn't use generations.
    pub fn advance_generation(&mut self) {}

    /// Check if a hash was seen recently (backward-compatible wrapper).
    /// Prefer `check()` for new code.
    pub fn is_duplicate(&self, hash: u32) -> bool {
        // Legacy fallback: check by hash only, using most recent sample as reference
        let limit = self.count.min(N);
        for i in 0..limit {
            let e = &self.entries[i];
            if e.hash == hash {
                return true;
            }
        }
        false
    }

    /// Check a frame against existing entries using hash match and time-window overlap.
    ///
    /// Two matching strategies:
    /// 1. **Hash match** (within expiry window): same frame content from different decoders
    ///    or retransmissions. Compare costs to keep the best decode.
    /// 2. **Time overlap** (tight window): different decoders may produce slightly different
    ///    content for the same physical TX (e.g., soft recovery variants). Compare costs.
    ///
    /// Returns `New` if no match, `Duplicate` if an existing frame is
    /// equal/better quality, or `Replace(slot)` if the new frame is better.
    pub fn check(&self, hash: u32, start_sample: u64, cost: u16) -> DedupAction {
        let limit = self.count.min(N);
        for i in 0..limit {
            let e = &self.entries[i];
            // Compute time delta
            let delta = if start_sample >= e.start_sample {
                start_sample - e.start_sample
            } else {
                e.start_sample - start_sample
            };
            // Skip expired entries (neither hash nor time match matters)
            if delta > EXPIRY_SAMPLES {
                continue;
            }
            // Match on hash (same content from different decoders) or
            // time overlap (different soft-recovery variants of the same TX).
            let is_match = e.hash == hash || delta < self.overlap_window;
            if is_match {
                // Same physical TX — compare quality
                if cost < e.cost {
                    return DedupAction::Replace(e.slot);
                } else {
                    return DedupAction::Duplicate;
                }
            }
        }
        DedupAction::New
    }

    /// Record a frame in the ring.
    pub fn record(&mut self, hash: u32) {
        self.record_with_info(hash, 0, 0, 0);
    }

    /// Record a frame with full time-window info.
    pub fn record_with_info(&mut self, hash: u32, start_sample: u64, cost: u16, slot: u8) {
        self.entries[self.write_idx] = DedupEntry { hash, start_sample, cost, slot };
        self.write_idx = (self.write_idx + 1) % N;
        if self.count < N {
            self.count += 1;
        }
    }

    /// Update an existing entry's cost and slot (after a Replace).
    pub fn update_entry(&mut self, old_slot: u8, new_cost: u16, new_slot: u8) {
        let limit = self.count.min(N);
        for i in 0..limit {
            if self.entries[i].slot == old_slot {
                self.entries[i].cost = new_cost;
                self.entries[i].slot = new_slot;
                return;
            }
        }
    }

    /// Reset all state.
    pub fn reset(&mut self) {
        self.entries = [DedupEntry::default(); N];
        self.write_idx = 0;
        self.count = 0;
    }
}

