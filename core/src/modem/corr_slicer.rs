//! Multi-Slicer Correlation Demodulator — M freq offsets × N gain slicers.
//!
//! Shares a single BPF across all frequency channels. Each channel has its
//! own NCO + LPF + Bresenham timing + gain slicers.
//!
//! On `std`: 3 frequency channels (0, −50, +50 Hz) × 8 gain slicers = 24 total.
//! On `no_std`: 1 frequency channel (0 Hz) × 4 gain slicers = 4 total.
//!
//! ```text
//! Audio → BPF (shared)
//!          ├── NCO(1200/2200) → LPF×4 → Bresenham → [8 gain slicers] → HDLC×8
//!          ├── NCO(1150/2150) → LPF×4 → Bresenham → [8 gain slicers] → HDLC×8
//!          └── NCO(1250/2250) → LPF×4 → Bresenham → [8 gain slicers] → HDLC×8
//!                                                            ↓
//!                                              FNV-1a dedup → unique frames
//! ```

use super::filter::BiquadFilter;
use super::DemodConfig;
use super::SIN_TABLE_Q15;

#[cfg(feature = "std")]
use super::soft_hdlc::{FrameResult, SoftHdlcDecoder};
#[cfg(not(feature = "std"))]
use crate::ax25::frame::HdlcDecoder;

/// Maximum number of gain slicers per frequency channel.
#[cfg(feature = "std")]
const MAX_SLICERS: usize = 8;
#[cfg(not(feature = "std"))]
const MAX_SLICERS: usize = 4;

/// Maximum number of frequency channels.
#[cfg(feature = "std")]
const MAX_FREQ_CHANNELS: usize = 3;
#[cfg(not(feature = "std"))]
const MAX_FREQ_CHANNELS: usize = 1;

/// Frequency offsets (Hz) for each channel.
#[cfg(feature = "std")]
const FREQ_OFFSETS: [i32; MAX_FREQ_CHANNELS] = [0, -50, 50];
#[cfg(not(feature = "std"))]
const FREQ_OFFSETS: [i32; MAX_FREQ_CHANNELS] = [0];

/// Gain levels (Q8) for multi-slicer diversity.
/// Same as MultiDecoder: covers −12 dB to +12 dB range.
#[cfg(feature = "std")]
const SLICER_GAINS: [u16; MAX_SLICERS] = [64, 107, 181, 256, 511, 868, 1440, 4057];
#[cfg(not(feature = "std"))]
const SLICER_GAINS: [u16; MAX_SLICERS] = [107, 256, 868, 4057];

/// Maximum unique frames tracked for deduplication.
const DEDUP_RING_SIZE: usize = 64;

/// Maximum output frames per process_samples call.
const MAX_OUTPUT_FRAMES: usize = 16;

/// Right-shift for energies before preamble gain tracking.
const AGC_ENERGY_SHIFT: u32 = 8;

/// A decoded frame with its content.
pub struct DecodedFrame {
    pub data: [u8; 330],
    pub len: usize,
}

/// Output buffer for CorrSlicerDecoder.
pub struct SlicerOutput {
    frames: [DecodedFrame; MAX_OUTPUT_FRAMES],
    count: usize,
}

impl SlicerOutput {
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

/// Per-slicer state: gain level + NRZI + HDLC decoder.
struct CorrSlicer {
    /// Space energy gain in Q8.
    space_gain_q8: u16,
    /// NRZI decode state.
    prev_nrzi_bit: bool,
    /// HDLC decoder (soft on std, hard on no_std).
    #[cfg(feature = "std")]
    hdlc: SoftHdlcDecoder,
    #[cfg(not(feature = "std"))]
    hdlc: HdlcDecoder,
}

impl CorrSlicer {
    fn new(gain_q8: u16) -> Self {
        Self {
            space_gain_q8: gain_q8,
            prev_nrzi_bit: false,
            #[cfg(feature = "std")]
            hdlc: SoftHdlcDecoder::new(),
            #[cfg(not(feature = "std"))]
            hdlc: HdlcDecoder::new(),
        }
    }

    fn reset(&mut self) {
        self.prev_nrzi_bit = false;
        self.hdlc.reset();
    }
}

/// Per-frequency-offset channel: NCO + LPF + Bresenham + slicers.
struct FreqChannel {
    /// NCO phase accumulators (Q24).
    mark_phase: u32,
    space_phase: u32,
    /// NCO phase increments (Q24).
    mark_phase_inc: u32,
    space_phase_inc: u32,
    /// Lowpass filters for the 4 mixer output channels.
    mark_i_lpf: BiquadFilter,
    mark_q_lpf: BiquadFilter,
    space_i_lpf: BiquadFilter,
    space_q_lpf: BiquadFilter,
    /// Bresenham fractional bit timing.
    bit_phase: u32,
    /// Parallel gain slicers.
    slicers: [CorrSlicer; MAX_SLICERS],
    /// Adaptive preamble gain state (only used for channel 0).
    demod_shift_reg: u8,
    preamble_mark_energy: i64,
    preamble_space_energy: i64,
    preamble_mark_count: u16,
    preamble_space_count: u16,
    preamble_flag_count: u8,
    symbols_since_last_flag: u8,
}

impl FreqChannel {
    fn new(mark_freq: u32, space_freq: u32, sample_rate: u32, lpf: BiquadFilter) -> Self {
        let mark_phase_inc = ((mark_freq as u64 * (1u64 << 24)) / sample_rate as u64) as u32;
        let space_phase_inc = ((space_freq as u64 * (1u64 << 24)) / sample_rate as u64) as u32;
        let slicers: [CorrSlicer; MAX_SLICERS] =
            core::array::from_fn(|i| CorrSlicer::new(SLICER_GAINS[i]));

        Self {
            mark_phase: 0,
            space_phase: 0,
            mark_phase_inc,
            space_phase_inc,
            mark_i_lpf: lpf,
            mark_q_lpf: lpf,
            space_i_lpf: lpf,
            space_q_lpf: lpf,
            bit_phase: 0,
            slicers,
            demod_shift_reg: 0,
            preamble_mark_energy: 0,
            preamble_space_energy: 0,
            preamble_mark_count: 0,
            preamble_space_count: 0,
            preamble_flag_count: 0,
            symbols_since_last_flag: 255,
        }
    }

    fn reset(&mut self) {
        self.mark_phase = 0;
        self.space_phase = 0;
        self.mark_i_lpf.reset();
        self.mark_q_lpf.reset();
        self.space_i_lpf.reset();
        self.space_q_lpf.reset();
        self.bit_phase = 0;
        for s in &mut self.slicers {
            s.reset();
        }
        self.demod_shift_reg = 0;
        self.preamble_mark_energy = 0;
        self.preamble_space_energy = 0;
        self.preamble_mark_count = 0;
        self.preamble_space_count = 0;
        self.preamble_flag_count = 0;
        self.symbols_since_last_flag = 255;
    }
}

/// Multi-slicer correlation demodulator with frequency diversity.
///
/// Shared BPF with M frequency channels, each containing its own
/// NCO → LPF → Bresenham → N gain slicers. Deduplicates output
/// frames using FNV-1a hash ring.
pub struct CorrSlicerDecoder {
    config: DemodConfig,
    bpf: BiquadFilter,
    channels: [FreqChannel; MAX_FREQ_CHANNELS],
    num_channels: usize,
    num_slicers: usize,
    samples_processed: u64,
    /// Adaptive preamble gain measurement (applied to channel 0 slicer 0).
    adaptive_gain_enabled: bool,
    /// Whether to produce energy-based LLR.
    energy_llr: bool,
    /// Ring buffer of (hash, generation) for deduplication.
    recent_hashes: [(u32, u32); DEDUP_RING_SIZE],
    recent_write: usize,
    recent_count: usize,
    generation: u32,
    /// Total frames decoded (including duplicates).
    pub total_decoded: u64,
    /// Total unique frames output.
    pub total_unique: u64,
}

impl CorrSlicerDecoder {
    /// Create a new multi-slicer correlation decoder with default gains and frequency offsets.
    pub fn new(config: DemodConfig) -> Self {
        let bpf = match config.sample_rate {
            22050 => super::filter::afsk_bandpass_22050(),
            44100 => super::filter::afsk_bandpass_44100(),
            _ => super::filter::afsk_bandpass_11025(),
        };
        let lpf = super::filter::corr_lpf_for_config(
            config.mark_freq,
            config.space_freq,
            config.baud_rate,
            config.sample_rate,
        );

        let channels: [FreqChannel; MAX_FREQ_CHANNELS] = core::array::from_fn(|i| {
            let offset = FREQ_OFFSETS[i];
            let mark = (config.mark_freq as i32 + offset) as u32;
            let space = (config.space_freq as i32 + offset) as u32;
            FreqChannel::new(mark, space, config.sample_rate, lpf)
        });

        Self {
            config,
            bpf,
            channels,
            num_channels: MAX_FREQ_CHANNELS,
            num_slicers: MAX_SLICERS,
            samples_processed: 0,
            adaptive_gain_enabled: false,
            energy_llr: true,
            recent_hashes: [(0u32, 0u32); DEDUP_RING_SIZE],
            recent_write: 0,
            recent_count: 0,
            generation: 0,
            total_decoded: 0,
            total_unique: 0,
        }
    }

    /// Set the initial Bresenham timing phase for all channels.
    pub fn set_bit_phase(&mut self, phase: u32) {
        for ch in &mut self.channels[..self.num_channels] {
            ch.bit_phase = phase;
        }
    }

    /// Enable adaptive mark/space gain from preamble measurement (channel 0 slicer 0).
    pub fn with_adaptive_gain(mut self) -> Self {
        self.adaptive_gain_enabled = true;
        self
    }

    /// Number of active gain slicers per channel.
    pub fn num_slicers(&self) -> usize {
        self.num_slicers
    }

    /// Number of active frequency channels.
    pub fn num_channels(&self) -> usize {
        self.num_channels
    }

    /// Reset all state for a new audio stream.
    pub fn reset(&mut self) {
        self.bpf.reset();
        for ch in &mut self.channels[..self.num_channels] {
            ch.reset();
        }
        self.samples_processed = 0;
        self.recent_hashes = [(0u32, 0u32); DEDUP_RING_SIZE];
        self.recent_write = 0;
        self.recent_count = 0;
        self.generation = 0;
    }

    /// Total soft-recovered frames across all channels and slicers (std only).
    #[cfg(feature = "std")]
    pub fn total_soft_recovered(&self) -> u32 {
        let mut total = 0u32;
        for ch in &self.channels[..self.num_channels] {
            for s in &ch.slicers[..self.num_slicers] {
                total += s.hdlc.stats_total_soft_recovered();
            }
        }
        total
    }

    /// Process audio samples through the shared BPF, then per-channel NCO+LPF+slicers.
    ///
    /// Returns a `SlicerOutput` containing unique decoded frames.
    pub fn process_samples(&mut self, samples: &[i16]) -> SlicerOutput {
        self.generation = self.generation.wrapping_add(1);
        let mut output = SlicerOutput::new();
        let sample_rate = self.config.sample_rate;
        let baud_rate = self.config.baud_rate;
        let num_channels = self.num_channels;
        let num_slicers = self.num_slicers;
        let _energy_llr = self.energy_llr;
        #[cfg(feature = "std")]
        let energy_llr = _energy_llr;
        let adaptive_gain = self.adaptive_gain_enabled;

        // Dedup state borrowed separately to avoid borrow conflicts with channels.
        let recent_hashes = &mut self.recent_hashes;
        let recent_write = &mut self.recent_write;
        let recent_count = &mut self.recent_count;
        let generation = self.generation;
        let total_decoded = &mut self.total_decoded;
        let total_unique = &mut self.total_unique;

        for &sample in samples {
            self.samples_processed += 1;

            // 1. Shared bandpass filter
            let filtered = self.bpf.process(sample);
            let x = filtered as i32;

            // 2. Process each frequency channel
            for ch_idx in 0..num_channels {
                let ch = &mut self.channels[ch_idx];

                // 2a. Mix with this channel's NCO
                let mark_sin_idx = (ch.mark_phase >> 16) as u8;
                let mark_cos_idx = mark_sin_idx.wrapping_add(64);
                let space_sin_idx = (ch.space_phase >> 16) as u8;
                let space_cos_idx = space_sin_idx.wrapping_add(64);

                let mark_i_raw = (x * SIN_TABLE_Q15[mark_sin_idx as usize] as i32) >> 15;
                let mark_q_raw = (x * SIN_TABLE_Q15[mark_cos_idx as usize] as i32) >> 15;
                let space_i_raw = (x * SIN_TABLE_Q15[space_sin_idx as usize] as i32) >> 15;
                let space_q_raw = (x * SIN_TABLE_Q15[space_cos_idx as usize] as i32) >> 15;

                ch.mark_phase = ch.mark_phase.wrapping_add(ch.mark_phase_inc);
                ch.space_phase = ch.space_phase.wrapping_add(ch.space_phase_inc);

                // 2b. Lowpass filter each mixer output
                let mark_i = ch.mark_i_lpf.process(mark_i_raw as i16);
                let mark_q = ch.mark_q_lpf.process(mark_q_raw as i16);
                let space_i = ch.space_i_lpf.process(space_i_raw as i16);
                let space_q = ch.space_q_lpf.process(space_q_raw as i16);

                // 2c. Bresenham symbol timing (per-channel)
                ch.bit_phase += baud_rate;
                if ch.bit_phase < sample_rate {
                    continue;
                }
                ch.bit_phase -= sample_rate;

                // ── Symbol boundary ──

                let mark_energy = (mark_i as i64) * (mark_i as i64)
                    + (mark_q as i64) * (mark_q as i64);
                let space_energy = (space_i as i64) * (space_i as i64)
                    + (space_q as i64) * (space_q as i64);

                // Adaptive preamble gain (channel 0 slicer 0 only)
                if adaptive_gain && ch_idx == 0 {
                    let raw_bit_0 = mark_energy * 256 > space_energy * (ch.slicers[0].space_gain_q8 as i64);
                    let decoded_bit_0 = raw_bit_0 == ch.slicers[0].prev_nrzi_bit;
                    ch.demod_shift_reg = (ch.demod_shift_reg << 1) | (decoded_bit_0 as u8);

                    if ch.demod_shift_reg == 0x7E {
                        ch.preamble_flag_count = ch.preamble_flag_count.saturating_add(1);
                        ch.symbols_since_last_flag = 0;
                    } else {
                        ch.symbols_since_last_flag = ch.symbols_since_last_flag.saturating_add(1);
                    }

                    if ch.preamble_flag_count >= 1 && ch.symbols_since_last_flag <= 8 {
                        if mark_energy > space_energy {
                            ch.preamble_mark_energy += mark_energy >> AGC_ENERGY_SHIFT;
                            ch.preamble_mark_count += 1;
                        } else {
                            ch.preamble_space_energy += space_energy >> AGC_ENERGY_SHIFT;
                            ch.preamble_space_count += 1;
                        }
                    }

                    if ch.symbols_since_last_flag > 8
                        && ch.preamble_flag_count >= 2
                        && ch.preamble_mark_count > 0
                        && ch.preamble_space_count > 0
                    {
                        let mark_avg = ch.preamble_mark_energy / ch.preamble_mark_count as i64;
                        let space_avg = ch.preamble_space_energy / ch.preamble_space_count as i64;
                        if space_avg > 0 {
                            let measured = (mark_avg * 256) / space_avg;
                            let excess = (measured - 256).max(0);
                            let gain = 256 + (excess >> 2);
                            ch.slicers[0].space_gain_q8 = (gain as u16).min(512);
                        }
                        ch.preamble_mark_energy = 0;
                        ch.preamble_space_energy = 0;
                        ch.preamble_mark_count = 0;
                        ch.preamble_space_count = 0;
                        ch.preamble_flag_count = 0;
                    }
                }

                // Feed each slicer
                for s_idx in 0..num_slicers {
                    let slicer = &mut ch.slicers[s_idx];

                    let raw_bit = mark_energy * 256 > space_energy * (slicer.space_gain_q8 as i64);
                    let decoded_bit = raw_bit == slicer.prev_nrzi_bit;
                    slicer.prev_nrzi_bit = raw_bit;

                    #[cfg(feature = "std")]
                    {
                        let llr = if energy_llr {
                            let total = mark_energy + space_energy;
                            if total > 0 {
                                let energy_ratio = ((mark_energy - space_energy) * 127) / total;
                                let confidence = energy_ratio.unsigned_abs().max(1).min(127) as i8;
                                if decoded_bit { confidence } else { -confidence }
                            } else {
                                0
                            }
                        } else {
                            if decoded_bit { 64 } else { -64 }
                        };
                        if let Some(result) = slicer.hdlc.feed_soft_bit(llr) {
                            let frame_bytes = match &result {
                                FrameResult::Valid(d) => *d,
                                FrameResult::Recovered { data, .. } => *data,
                            };
                            let len = frame_bytes.len().min(330);
                            let mut frame_copy = [0u8; 330];
                            frame_copy[..len].copy_from_slice(&frame_bytes[..len]);
                            *total_decoded += 1;
                            let hash = frame_hash(&frame_copy[..len]);
                            if !is_dup(recent_hashes, *recent_count, generation, hash) {
                                record(recent_hashes, recent_write, recent_count, generation, hash);
                                *total_unique += 1;
                                if output.count < MAX_OUTPUT_FRAMES {
                                    output.frames[output.count].data[..len]
                                        .copy_from_slice(&frame_copy[..len]);
                                    output.frames[output.count].len = len;
                                    output.count += 1;
                                }
                            }
                        }
                    }
                    #[cfg(not(feature = "std"))]
                    {
                        if let Some(frame_bytes) = slicer.hdlc.feed_bit(decoded_bit) {
                            *total_decoded += 1;
                            let len = frame_bytes.len().min(330);
                            let mut frame_copy = [0u8; 330];
                            frame_copy[..len].copy_from_slice(&frame_bytes[..len]);
                            let hash = frame_hash(&frame_copy[..len]);
                            if !is_dup(recent_hashes, *recent_count, generation, hash) {
                                record(recent_hashes, recent_write, recent_count, generation, hash);
                                *total_unique += 1;
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
            }
        }

        output
    }

}

/// Check if a hash was seen recently (free function to avoid borrow conflicts).
fn is_dup(
    recent_hashes: &[(u32, u32); DEDUP_RING_SIZE],
    recent_count: usize,
    generation: u32,
    hash: u32,
) -> bool {
    const DEDUP_WINDOW: u32 = 4;
    let limit = recent_count.min(DEDUP_RING_SIZE);
    for i in 0..limit {
        let (h, gen) = recent_hashes[i];
        if h == hash && generation.wrapping_sub(gen) <= DEDUP_WINDOW {
            return true;
        }
    }
    false
}

/// Record a hash in the dedup ring (free function to avoid borrow conflicts).
fn record(
    recent_hashes: &mut [(u32, u32); DEDUP_RING_SIZE],
    recent_write: &mut usize,
    recent_count: &mut usize,
    generation: u32,
    hash: u32,
) {
    recent_hashes[*recent_write] = (hash, generation);
    *recent_write = (*recent_write + 1) % DEDUP_RING_SIZE;
    if *recent_count < DEDUP_RING_SIZE {
        *recent_count += 1;
    }
}

/// FNV-1a 32-bit hash for frame deduplication.
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
    fn test_corr_slicer_creation() {
        let config = DemodConfig::default_1200();
        let decoder = CorrSlicerDecoder::new(config);
        assert_eq!(decoder.samples_processed, 0);
        assert_eq!(decoder.num_slicers(), MAX_SLICERS);
        assert_eq!(decoder.num_channels(), MAX_FREQ_CHANNELS);
    }

    #[test]
    fn test_corr_slicer_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut decoder = CorrSlicerDecoder::new(config);
        let silence = [0i16; 1000];

        let output = decoder.process_samples(&silence);
        assert!(output.is_empty());
    }

    #[test]
    fn test_corr_slicer_reset() {
        let config = DemodConfig::default_1200();
        let mut decoder = CorrSlicerDecoder::new(config);
        let noise = [1000i16; 100];

        decoder.process_samples(&noise);
        decoder.reset();
        assert_eq!(decoder.samples_processed, 0);
        assert_eq!(decoder.total_decoded, 0);
        assert_eq!(decoder.total_unique, 0);
    }

    #[test]
    fn test_corr_slicer_gains() {
        // Verify slicer gains are monotonically increasing
        for i in 1..SLICER_GAINS.len() {
            assert!(SLICER_GAINS[i] > SLICER_GAINS[i - 1],
                "Gains must be monotonically increasing: [{}]={} <= [{}]={}",
                i, SLICER_GAINS[i], i - 1, SLICER_GAINS[i - 1]);
        }
    }

    #[test]
    fn test_corr_slicer_freq_channels() {
        // Verify frequency offsets are configured
        let config = DemodConfig::default_1200();
        let decoder = CorrSlicerDecoder::new(config);
        assert_eq!(decoder.num_channels(), MAX_FREQ_CHANNELS);

        // Channel 0 should have nominal frequencies
        let ch0 = &decoder.channels[0];
        let expected_mark_inc = ((config.mark_freq as u64 * (1u64 << 24)) / config.sample_rate as u64) as u32;
        assert_eq!(ch0.mark_phase_inc, expected_mark_inc);
    }
}
