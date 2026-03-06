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
use super::hilbert::{HilbertTransform, InstFreqDetector, hilbert_31};
use super::adaptive::AdaptiveTracker;

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

/// Number of candidate Bresenham phases to try during preamble.
const NUM_PHASE_CANDIDATES: usize = 3;

/// Minimum flags a phase candidate must see before committing.
const PHASE_COMMIT_MIN_FLAGS: u8 = 3;

/// Non-flag symbols after last flag before committing the best phase.
const PHASE_COMMIT_GAP: u8 = 8;

/// Adaptive state for preamble frequency retune (Hilbert + InstFreq + Tracker).
struct CorrAdaptiveState {
    hilbert: HilbertTransform<31>,
    inst_freq: InstFreqDetector,
    tracker: AdaptiveTracker,
    retuned: bool,
    sample_index: u32,
}

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
    /// Decimation accumulators for mixer outputs: [mark_i, mark_q, space_i, space_q].
    decim_acc: [i32; 4],
    /// Current count within decimation block.
    decim_count: u8,
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
    /// Preamble phase scoring: whether the best phase has been committed.
    phase_committed: bool,
    /// Bresenham counters for candidate timing phases.
    candidate_phases: [u32; NUM_PHASE_CANDIDATES],
    /// Shift registers for flag detection per candidate.
    candidate_shift_regs: [u8; NUM_PHASE_CANDIDATES],
    /// NRZI previous bit state per candidate.
    candidate_prev_nrzi: [bool; NUM_PHASE_CANDIDATES],
    /// Flags detected per candidate phase.
    candidate_flag_counts: [u8; NUM_PHASE_CANDIDATES],
    /// Consecutive non-flag symbols per candidate (for commit detection).
    candidate_nonflag_run: [u8; NUM_PHASE_CANDIDATES],
}

impl FreqChannel {
    fn new(mark_freq: u32, space_freq: u32, sample_rate: u32, lpf: BiquadFilter, effective_rate: u32) -> Self {
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
            decim_acc: [0; 4],
            decim_count: 0,
            bit_phase: 0,
            slicers,
            demod_shift_reg: 0,
            preamble_mark_energy: 0,
            preamble_space_energy: 0,
            preamble_mark_count: 0,
            preamble_space_count: 0,
            preamble_flag_count: 0,
            symbols_since_last_flag: 255,
            phase_committed: true, // Default: disabled; enabled by with_phase_scoring()
            candidate_phases: [0, effective_rate / 3, effective_rate * 2 / 3],
            candidate_shift_regs: [0; NUM_PHASE_CANDIDATES],
            candidate_prev_nrzi: [false; NUM_PHASE_CANDIDATES],
            candidate_flag_counts: [0; NUM_PHASE_CANDIDATES],
            candidate_nonflag_run: [0; NUM_PHASE_CANDIDATES],
        }
    }

    fn reset(&mut self, effective_rate: u32, phase_scoring: bool) {
        self.mark_phase = 0;
        self.space_phase = 0;
        self.mark_i_lpf.reset();
        self.mark_q_lpf.reset();
        self.space_i_lpf.reset();
        self.space_q_lpf.reset();
        self.decim_acc = [0; 4];
        self.decim_count = 0;
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
        self.phase_committed = !phase_scoring;
        self.candidate_phases = [0, effective_rate / 3, effective_rate * 2 / 3];
        self.candidate_shift_regs = [0; NUM_PHASE_CANDIDATES];
        self.candidate_prev_nrzi = [false; NUM_PHASE_CANDIDATES];
        self.candidate_flag_counts = [0; NUM_PHASE_CANDIDATES];
        self.candidate_nonflag_run = [0; NUM_PHASE_CANDIDATES];
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
    /// Decimation factor for mixer outputs (1 = none, 2 or 4).
    corr_decim_factor: u8,
    /// Effective sample rate after decimation.
    effective_sample_rate: u32,
    /// Adaptive preamble gain measurement (applied to channel 0 slicer 0).
    adaptive_gain_enabled: bool,
    /// Whether to produce energy-based LLR.
    energy_llr: bool,
    /// Optional adaptive frequency retune from preamble measurements.
    adaptive: Option<CorrAdaptiveState>,
    /// Whether preamble phase scoring is enabled.
    phase_scoring: bool,
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
        let bpf = super::filter::select_std_bpf(config.baud_rate, config.sample_rate);

        // For 300 baud at high sample rates, decimate mixer outputs so the LPF
        // runs at ~11025 Hz where Q15 coefficients have adequate precision.
        let (decim_factor, effective_rate) = if config.baud_rate == 300 {
            if config.sample_rate >= 44100 {
                (4u8, config.sample_rate / 4)
            } else if config.sample_rate >= 22050 {
                (2u8, config.sample_rate / 2)
            } else {
                (1u8, config.sample_rate)
            }
        } else {
            (1u8, config.sample_rate)
        };

        // Select LPF for effective (decimated) sample rate
        let lpf = super::filter::corr_lpf_for_config(
            config.mark_freq, config.space_freq, config.baud_rate, effective_rate,
        );

        // Narrower frequency offsets for 300 baud (200 Hz tone separation vs 1000 Hz)
        let freq_offsets_300: [i32; MAX_FREQ_CHANNELS] = {
            #[cfg(feature = "std")]
            { [0, -10, 10] }
            #[cfg(not(feature = "std"))]
            { [0] }
        };

        let offsets = if config.baud_rate == 300 { &freq_offsets_300 } else { &FREQ_OFFSETS };

        let channels: [FreqChannel; MAX_FREQ_CHANNELS] = core::array::from_fn(|i| {
            let offset = offsets[i];
            let mark = (config.mark_freq as i32 + offset) as u32;
            let space = (config.space_freq as i32 + offset) as u32;
            FreqChannel::new(mark, space, config.sample_rate, lpf, effective_rate)
        });

        Self {
            config,
            bpf,
            channels,
            num_channels: MAX_FREQ_CHANNELS,
            num_slicers: MAX_SLICERS,
            samples_processed: 0,
            corr_decim_factor: decim_factor,
            effective_sample_rate: effective_rate,
            adaptive_gain_enabled: false,
            energy_llr: true,
            adaptive: None,
            phase_scoring: false,
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

    /// Enable preamble phase scoring.
    ///
    /// During the preamble, runs 3 candidate Bresenham phases in parallel.
    /// Commits the best phase (most flag detections) when data starts.
    /// Cost: ~30 ops/sample during preamble only; zero after commit.
    pub fn with_phase_scoring(mut self) -> Self {
        self.phase_scoring = true;
        for ch in &mut self.channels[..self.num_channels] {
            ch.phase_committed = false;
        }
        self
    }

    /// Enable adaptive NCO frequency retune from preamble measurements.
    ///
    /// Runs Hilbert transform + instantaneous frequency estimation on the
    /// shared BPF output during preamble. When the AdaptiveTracker locks,
    /// retunes all channels' NCO phase increments to match the transmitter's
    /// actual mark/space frequencies.
    ///
    /// Cost: ~46 ops/sample during preamble; zero after lock.
    pub fn with_adaptive_retune(mut self) -> Self {
        self.adaptive = Some(CorrAdaptiveState {
            hilbert: hilbert_31(),
            inst_freq: InstFreqDetector::new(self.config.sample_rate),
            tracker: AdaptiveTracker::new_for_config(&self.config),
            retuned: false,
            sample_index: 0,
        });
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
        let effective_rate = self.effective_sample_rate;
        let phase_scoring = self.phase_scoring;
        for ch in &mut self.channels[..self.num_channels] {
            ch.reset(effective_rate, phase_scoring);
        }
        self.samples_processed = 0;
        self.recent_hashes = [(0u32, 0u32); DEDUP_RING_SIZE];
        self.recent_write = 0;
        self.recent_count = 0;
        self.generation = 0;
        if let Some(ref mut adaptive) = self.adaptive {
            adaptive.hilbert.reset();
            adaptive.inst_freq.reset();
            adaptive.tracker.reset();
            adaptive.retuned = false;
            adaptive.sample_index = 0;
        }
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
        let phase_scoring = self.phase_scoring;
        let mark_freq = self.config.mark_freq;
        let space_freq = self.config.space_freq;
        let decim_factor = self.corr_decim_factor;
        let effective_rate = self.effective_sample_rate;

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

            // 1b. Adaptive retune: Hilbert → InstFreq → Tracker (only until lock)
            let retune_freqs = if let Some(ref mut adaptive) = self.adaptive {
                if !adaptive.retuned {
                    adaptive.sample_index = adaptive.sample_index.wrapping_add(1);
                    let (real, imag) = adaptive.hilbert.process(filtered);
                    let freq_fp = adaptive.inst_freq.process(real, imag);
                    adaptive.tracker.feed(freq_fp, adaptive.sample_index);

                    if adaptive.tracker.is_locked() {
                        let mhz = (adaptive.tracker.mark_freq_est >> 8) as u32;
                        let shz = (adaptive.tracker.space_freq_est >> 8) as u32;
                        adaptive.retuned = true;
                        Some((mhz, shz))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // Apply frequency retune to all channels' NCO phase increments
            if let Some((mark_hz, space_hz)) = retune_freqs {
                let mark_delta = (mark_hz as i32 - mark_freq as i32).unsigned_abs();
                let space_delta = (space_hz as i32 - space_freq as i32).unsigned_abs();
                if mark_delta < 200 && space_delta < 200 && mark_hz > 0 && space_hz > 0 {
                    for (ch, &offset) in self.channels[..num_channels].iter_mut().zip(FREQ_OFFSETS.iter()) {
                        let ch_mark = (mark_hz as i32 + offset) as u32;
                        let ch_space = (space_hz as i32 + offset) as u32;
                        ch.mark_phase_inc =
                            ((ch_mark as u64 * (1u64 << 24)) / sample_rate as u64) as u32;
                        ch.space_phase_inc =
                            ((ch_space as u64 * (1u64 << 24)) / sample_rate as u64) as u32;
                    }
                }
            }

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

                // 2b. Decimation + Lowpass filter
                ch.decim_acc[0] += mark_i_raw;
                ch.decim_acc[1] += mark_q_raw;
                ch.decim_acc[2] += space_i_raw;
                ch.decim_acc[3] += space_q_raw;
                ch.decim_count += 1;

                if ch.decim_count < decim_factor {
                    continue;
                }

                let df = decim_factor as i32;
                let dec_mi = (ch.decim_acc[0] / df) as i16;
                let dec_mq = (ch.decim_acc[1] / df) as i16;
                let dec_si = (ch.decim_acc[2] / df) as i16;
                let dec_sq = (ch.decim_acc[3] / df) as i16;
                ch.decim_acc = [0; 4];
                ch.decim_count = 0;

                let mark_i = ch.mark_i_lpf.process(dec_mi);
                let mark_q = ch.mark_q_lpf.process(dec_mq);
                let space_i = ch.space_i_lpf.process(dec_si);
                let space_q = ch.space_q_lpf.process(dec_sq);

                // 2c. Preamble phase scoring (3 candidate Bresenham timings)
                if phase_scoring && !ch.phase_committed {
                    for c in 0..NUM_PHASE_CANDIDATES {
                        ch.candidate_phases[c] += baud_rate;
                        if ch.candidate_phases[c] >= effective_rate {
                            ch.candidate_phases[c] -= effective_rate;
                            // Symbol boundary for candidate c — compute energy
                            let me = (mark_i as i64) * (mark_i as i64)
                                + (mark_q as i64) * (mark_q as i64);
                            let se = (space_i as i64) * (space_i as i64)
                                + (space_q as i64) * (space_q as i64);
                            let raw_bit = me > se;
                            let decoded_bit = raw_bit == ch.candidate_prev_nrzi[c];
                            ch.candidate_prev_nrzi[c] = raw_bit;
                            ch.candidate_shift_regs[c] =
                                (ch.candidate_shift_regs[c] << 1) | (decoded_bit as u8);
                            if ch.candidate_shift_regs[c] == 0x7E {
                                ch.candidate_flag_counts[c] =
                                    ch.candidate_flag_counts[c].saturating_add(1);
                                ch.candidate_nonflag_run[c] = 0;
                            } else {
                                ch.candidate_nonflag_run[c] =
                                    ch.candidate_nonflag_run[c].saturating_add(1);
                            }
                        }
                    }
                    // Check if any candidate is ready to commit
                    let mut best_c: usize = 0;
                    let mut best_flags: u8 = 0;
                    let mut ready = false;
                    for c in 0..NUM_PHASE_CANDIDATES {
                        if ch.candidate_flag_counts[c] >= PHASE_COMMIT_MIN_FLAGS
                            && ch.candidate_nonflag_run[c] > PHASE_COMMIT_GAP
                        {
                            if ch.candidate_flag_counts[c] > best_flags {
                                best_flags = ch.candidate_flag_counts[c];
                                best_c = c;
                            }
                            ready = true;
                        }
                    }
                    if ready {
                        ch.bit_phase = ch.candidate_phases[best_c];
                        // Sync all slicer NRZI states from the winning candidate
                        for s in ch.slicers[..num_slicers].iter_mut() {
                            s.prev_nrzi_bit = ch.candidate_prev_nrzi[best_c];
                        }
                        ch.phase_committed = true;
                    }
                }

                // 2d. Bresenham symbol timing (per-channel, at effective rate)
                ch.bit_phase += baud_rate;
                if ch.bit_phase < effective_rate {
                    continue;
                }
                ch.bit_phase -= effective_rate;

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
                                let confidence = energy_ratio.unsigned_abs().clamp(1, 127) as i8;
                                if decoded_bit { confidence } else { -confidence }
                            } else {
                                0
                            }
                        } else if decoded_bit {
                            64
                        } else {
                            -64
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
                            let hash = super::frame_hash(&frame_copy[..len]);
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
                            let hash = super::frame_hash(&frame_copy[..len]);
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
    for &(h, gen) in &recent_hashes[..limit] {
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

// frame_hash is now centralized in super::frame_hash

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

    #[test]
    fn test_phase_scoring_builder() {
        let config = DemodConfig::default_1200();
        let decoder = CorrSlicerDecoder::new(config).with_phase_scoring();
        assert!(decoder.phase_scoring);
        // All channels should have phase_committed = false (scoring enabled)
        for ch in &decoder.channels[..decoder.num_channels()] {
            assert!(!ch.phase_committed);
            assert_eq!(ch.candidate_flag_counts, [0; NUM_PHASE_CANDIDATES]);
            // Candidate phases should be spaced at 0, SR/3, 2*SR/3
            assert_eq!(ch.candidate_phases[0], 0);
            assert_eq!(ch.candidate_phases[1], config.sample_rate / 3);
            assert_eq!(ch.candidate_phases[2], config.sample_rate * 2 / 3);
        }
    }

    #[test]
    fn test_phase_scoring_disabled_by_default() {
        let config = DemodConfig::default_1200();
        let decoder = CorrSlicerDecoder::new(config);
        assert!(!decoder.phase_scoring);
        for ch in &decoder.channels[..decoder.num_channels()] {
            assert!(ch.phase_committed); // Scoring disabled
        }
    }

    #[test]
    fn test_phase_scoring_processes_silence() {
        let config = DemodConfig::default_1200();
        let mut decoder = CorrSlicerDecoder::new(config).with_phase_scoring();
        let silence = [0i16; 1000];
        let output = decoder.process_samples(&silence);
        assert!(output.is_empty());
        // Phase should NOT be committed with silence (no flags)
        assert!(!decoder.channels[0].phase_committed);
    }

    #[test]
    fn test_adaptive_retune_builder() {
        let config = DemodConfig::default_1200();
        let decoder = CorrSlicerDecoder::new(config).with_adaptive_retune();
        assert!(decoder.adaptive.is_some());
        let adaptive = decoder.adaptive.as_ref().unwrap();
        assert!(!adaptive.retuned);
        assert_eq!(adaptive.sample_index, 0);
    }

    #[test]
    fn test_adaptive_retune_reset() {
        let config = DemodConfig::default_1200();
        let mut decoder = CorrSlicerDecoder::new(config).with_adaptive_retune();
        let noise = [1000i16; 100];
        decoder.process_samples(&noise);
        decoder.reset();
        let adaptive = decoder.adaptive.as_ref().unwrap();
        assert!(!adaptive.retuned);
        assert_eq!(adaptive.sample_index, 0);
    }

    #[test]
    fn test_phase_scoring_reset() {
        let config = DemodConfig::default_1200();
        let mut decoder = CorrSlicerDecoder::new(config).with_phase_scoring();
        let noise = [1000i16; 500];
        decoder.process_samples(&noise);
        decoder.reset();
        // Phase scoring should be re-enabled after reset
        for ch in &decoder.channels[..decoder.num_channels()] {
            assert!(!ch.phase_committed);
            assert_eq!(ch.candidate_flag_counts, [0; NUM_PHASE_CANDIDATES]);
            assert_eq!(ch.candidate_phases[0], 0);
        }
    }

    #[test]
    fn test_combined_builders() {
        let config = DemodConfig::default_1200();
        let decoder = CorrSlicerDecoder::new(config)
            .with_phase_scoring()
            .with_adaptive_retune()
            .with_adaptive_gain();
        assert!(decoder.phase_scoring);
        assert!(decoder.adaptive.is_some());
        assert!(decoder.adaptive_gain_enabled);
    }
}
