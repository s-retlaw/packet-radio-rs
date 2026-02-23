//! Multi-Slicer Correlation Demodulator — single demod, N gain slicers.
//!
//! Shares the expensive per-sample DSP path (BPF → NCO × 4 → LPF × 4) and
//! only duplicates the cheap per-symbol decision (gain compare + NRZI + HDLC).
//!
//! This gives DireWolf-style multi-slicer at ~1.3× CPU vs 38× for multi-decoder.
//!
//! ```text
//! Audio → BPF → NCO(mark/space I/Q) → LPF×4 → [shared per-sample path]
//!                                               ↓
//!                               Bresenham timing (1 boundary signal)
//!                                               ↓
//!                               mark_energy = I²+Q², space_energy = I²+Q²
//!                                               ↓
//!                     ┌──────────┬──────────┬──── ··· ────┐
//!                     │ Slicer 0 │ Slicer 1 │             │ Slicer N
//!                     │ gain=64  │ gain=107 │             │ gain=4057
//!                     │ NRZI     │ NRZI     │             │ NRZI
//!                     │ HDLC     │ HDLC     │             │ HDLC
//!                     └──────────┴──────────┴──── ··· ────┘
//!                                               ↓
//!                               FNV-1a dedup → unique frames out
//! ```

use super::filter::BiquadFilter;
use super::DemodConfig;
use super::SIN_TABLE_Q15;

#[cfg(feature = "std")]
use super::soft_hdlc::{FrameResult, SoftHdlcDecoder};
#[cfg(not(feature = "std"))]
use crate::ax25::frame::HdlcDecoder;

/// Maximum number of slicers.
#[cfg(feature = "std")]
const MAX_SLICERS: usize = 8;
#[cfg(not(feature = "std"))]
const MAX_SLICERS: usize = 4;

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

/// Multi-slicer correlation demodulator.
///
/// Single demodulator (BPF → NCO → LPF → Bresenham) with N parallel
/// gain slicers, each with independent NRZI and HDLC state. Deduplicates
/// output frames using FNV-1a hash ring.
pub struct CorrSlicerDecoder {
    config: DemodConfig,
    bpf: BiquadFilter,
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
    samples_processed: u64,
    /// Parallel gain slicers.
    slicers: [CorrSlicer; MAX_SLICERS],
    num_slicers: usize,
    /// Adaptive preamble gain measurement (applied to slicer 0).
    adaptive_gain_enabled: bool,
    /// Shift register for flag detection (shared across slicers for preamble).
    demod_shift_reg: u8,
    preamble_mark_energy: i64,
    preamble_space_energy: i64,
    preamble_mark_count: u16,
    preamble_space_count: u16,
    preamble_flag_count: u8,
    symbols_since_last_flag: u8,
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
    /// Compute NCO phase increment for a given frequency.
    fn phase_inc(freq: u32, sample_rate: u32) -> u32 {
        ((freq as u64 * (1u64 << 24)) / sample_rate as u64) as u32
    }

    /// Create a new multi-slicer correlation decoder with default gains.
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

        let slicers: [CorrSlicer; MAX_SLICERS] =
            core::array::from_fn(|i| CorrSlicer::new(SLICER_GAINS[i]));

        Self {
            config,
            bpf,
            mark_phase: 0,
            space_phase: 0,
            mark_phase_inc: Self::phase_inc(config.mark_freq, config.sample_rate),
            space_phase_inc: Self::phase_inc(config.space_freq, config.sample_rate),
            mark_i_lpf: lpf,
            mark_q_lpf: lpf,
            space_i_lpf: lpf,
            space_q_lpf: lpf,
            bit_phase: 0,
            samples_processed: 0,
            slicers,
            num_slicers: MAX_SLICERS,
            adaptive_gain_enabled: false,
            demod_shift_reg: 0,
            preamble_mark_energy: 0,
            preamble_space_energy: 0,
            preamble_mark_count: 0,
            preamble_space_count: 0,
            preamble_flag_count: 0,
            symbols_since_last_flag: 255,
            energy_llr: true, // default to energy LLR for soft decode
            recent_hashes: [(0u32, 0u32); DEDUP_RING_SIZE],
            recent_write: 0,
            recent_count: 0,
            generation: 0,
            total_decoded: 0,
            total_unique: 0,
        }
    }

    /// Set the initial Bresenham timing phase.
    pub fn set_bit_phase(&mut self, phase: u32) {
        self.bit_phase = phase;
    }

    /// Enable adaptive mark/space gain from preamble measurement (slicer 0).
    pub fn with_adaptive_gain(mut self) -> Self {
        self.adaptive_gain_enabled = true;
        self
    }

    /// Number of active slicers.
    pub fn num_slicers(&self) -> usize {
        self.num_slicers
    }

    /// Reset all state for a new audio stream.
    pub fn reset(&mut self) {
        self.bpf.reset();
        self.mark_phase = 0;
        self.space_phase = 0;
        self.mark_i_lpf.reset();
        self.mark_q_lpf.reset();
        self.space_i_lpf.reset();
        self.space_q_lpf.reset();
        self.bit_phase = 0;
        self.samples_processed = 0;
        for s in &mut self.slicers[..self.num_slicers] {
            s.reset();
        }
        self.demod_shift_reg = 0;
        self.preamble_mark_energy = 0;
        self.preamble_space_energy = 0;
        self.preamble_mark_count = 0;
        self.preamble_space_count = 0;
        self.preamble_flag_count = 0;
        self.symbols_since_last_flag = 255;
        self.recent_hashes = [(0u32, 0u32); DEDUP_RING_SIZE];
        self.recent_write = 0;
        self.recent_count = 0;
        self.generation = 0;
    }

    /// Total soft-recovered frames across all slicers (std only).
    #[cfg(feature = "std")]
    pub fn total_soft_recovered(&self) -> u32 {
        let mut total = 0u32;
        for s in &self.slicers[..self.num_slicers] {
            total += s.hdlc.stats_total_soft_recovered();
        }
        total
    }

    /// Process audio samples through the shared demod path and all slicers.
    ///
    /// Returns a `SlicerOutput` containing unique decoded frames.
    pub fn process_samples(&mut self, samples: &[i16]) -> SlicerOutput {
        self.generation = self.generation.wrapping_add(1);
        let mut output = SlicerOutput::new();
        let sample_rate = self.config.sample_rate;
        let baud_rate = self.config.baud_rate;

        for &sample in samples {
            self.samples_processed += 1;

            // 1. Bandpass filter
            let filtered = self.bpf.process(sample);
            let x = filtered as i32;

            // 2. Mix with local oscillators (NCO)
            let mark_sin_idx = (self.mark_phase >> 16) as u8;
            let mark_cos_idx = mark_sin_idx.wrapping_add(64);
            let space_sin_idx = (self.space_phase >> 16) as u8;
            let space_cos_idx = space_sin_idx.wrapping_add(64);

            let mark_i_raw = (x * SIN_TABLE_Q15[mark_sin_idx as usize] as i32) >> 15;
            let mark_q_raw = (x * SIN_TABLE_Q15[mark_cos_idx as usize] as i32) >> 15;
            let space_i_raw = (x * SIN_TABLE_Q15[space_sin_idx as usize] as i32) >> 15;
            let space_q_raw = (x * SIN_TABLE_Q15[space_cos_idx as usize] as i32) >> 15;

            self.mark_phase = self.mark_phase.wrapping_add(self.mark_phase_inc);
            self.space_phase = self.space_phase.wrapping_add(self.space_phase_inc);

            // 3. Lowpass filter each channel
            let mark_i = self.mark_i_lpf.process(mark_i_raw as i16);
            let mark_q = self.mark_q_lpf.process(mark_q_raw as i16);
            let space_i = self.space_i_lpf.process(space_i_raw as i16);
            let space_q = self.space_q_lpf.process(space_q_raw as i16);

            // 4. Bresenham symbol timing
            self.bit_phase += baud_rate;
            if self.bit_phase < sample_rate {
                continue;
            }
            self.bit_phase -= sample_rate;

            // ── Symbol boundary: compute energies once, feed all slicers ──

            let mark_energy = (mark_i as i64) * (mark_i as i64)
                + (mark_q as i64) * (mark_q as i64);
            let space_energy = (space_i as i64) * (space_i as i64)
                + (space_q as i64) * (space_q as i64);

            // Adaptive preamble gain: use slicer 0's NRZI-decoded bit for flag detection
            if self.adaptive_gain_enabled {
                let raw_bit_0 = mark_energy * 256 > space_energy * (self.slicers[0].space_gain_q8 as i64);
                let decoded_bit_0 = raw_bit_0 == self.slicers[0].prev_nrzi_bit;
                self.demod_shift_reg = (self.demod_shift_reg << 1) | (decoded_bit_0 as u8);

                if self.demod_shift_reg == 0x7E {
                    self.preamble_flag_count = self.preamble_flag_count.saturating_add(1);
                    self.symbols_since_last_flag = 0;
                } else {
                    self.symbols_since_last_flag = self.symbols_since_last_flag.saturating_add(1);
                }

                if self.preamble_flag_count >= 1 && self.symbols_since_last_flag <= 8 {
                    if mark_energy > space_energy {
                        self.preamble_mark_energy += mark_energy >> AGC_ENERGY_SHIFT;
                        self.preamble_mark_count += 1;
                    } else {
                        self.preamble_space_energy += space_energy >> AGC_ENERGY_SHIFT;
                        self.preamble_space_count += 1;
                    }
                }

                if self.symbols_since_last_flag > 8
                    && self.preamble_flag_count >= 2
                    && self.preamble_mark_count > 0
                    && self.preamble_space_count > 0
                {
                    let mark_avg = self.preamble_mark_energy / self.preamble_mark_count as i64;
                    let space_avg = self.preamble_space_energy / self.preamble_space_count as i64;
                    if space_avg > 0 {
                        let measured = (mark_avg * 256) / space_avg;
                        let excess = (measured - 256).max(0);
                        let gain = 256 + (excess >> 2);
                        self.slicers[0].space_gain_q8 = (gain as u16).min(512);
                    }
                    self.preamble_mark_energy = 0;
                    self.preamble_space_energy = 0;
                    self.preamble_mark_count = 0;
                    self.preamble_space_count = 0;
                    self.preamble_flag_count = 0;
                }
            }

            // Feed each slicer with the shared energies
            for s_idx in 0..self.num_slicers {
                let slicer = &mut self.slicers[s_idx];

                // Gain-adjusted bit decision
                let raw_bit = mark_energy * 256 > space_energy * (slicer.space_gain_q8 as i64);

                // NRZI decode
                let decoded_bit = raw_bit == slicer.prev_nrzi_bit;
                slicer.prev_nrzi_bit = raw_bit;

                // Feed HDLC decoder
                #[cfg(feature = "std")]
                {
                    // LLR for soft decode
                    let llr = if self.energy_llr {
                        let total = mark_energy + space_energy;
                        if total > 0 {
                            let energy_ratio = ((mark_energy - space_energy) * 127) / total;
                            let mut confidence = energy_ratio.unsigned_abs().max(1).min(127) as i8;
                            if !decoded_bit {
                                confidence >>= 1;
                                if confidence == 0 { confidence = 1; }
                            }
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
                        self.total_decoded += 1;
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
                #[cfg(not(feature = "std"))]
                {
                    if let Some(frame_bytes) = slicer.hdlc.feed_bit(decoded_bit) {
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
        }

        output
    }

    /// Check if a hash was seen recently.
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
}
