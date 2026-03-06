//! Multi-Decoder for 9600 baud — parallel demodulators with LPF order, cutoff, and slicer diversity.
//!
//! Runs multiple 9600 baud demodulator instances with four diversity axes:
//! 1. **LPF order** — 2nd-order (single biquad) vs 4th-order (cascaded) for different noise profiles
//! 2. **LPF cutoff** — 5400/6000/6600/7200 Hz for noise rejection vs bandwidth tradeoff
//! 3. **Slicer threshold** — multi-slicer for DC offset / AGC error compensation (-660 to +330)
//! 4. **Timing phase** — PLL phase offset for timing alignment diversity (limited)
//!
//! Grid search tuning showed 2nd-order LPF dominates for best single-decoder performance,
//! while 4th-order cascaded adds complementary diversity in multi-decoder ensembles.
//!
//! # Configuration
//!
//! - **std**: ~34 decoders (24 DW + 10 Gardner) with LPF order/cutoff/threshold diversity
//! - **no_std** (ESP32): DW-style (3 slicers) + Gardner (3 slicers) = 6 decoders
//! - **Mini9600**: 6 attribution-optimal decoders for MCU (~1.5 KB RAM)

use super::demod::DemodSymbol;
use super::demod_9600::*;
use super::soft_hdlc::{FrameResult, SoftHdlcDecoder};

#[cfg(feature = "std")]
extern crate alloc;
#[cfg(feature = "std")]
use alloc::vec::Vec;
#[cfg(feature = "attribution")]
use alloc::boxed::Box;

#[cfg(not(feature = "std"))]
use crate::ax25::frame::HdlcDecoder;

/// Maximum number of 9600 baud decoders in the ensemble.
#[cfg(feature = "std")]
const MAX_9600_DECODERS: usize = 48;
#[cfg(not(feature = "std"))]
const MAX_9600_DECODERS: usize = 8;

/// Maximum output frames per process call.
const MAX_OUTPUT_FRAMES: usize = 16;

/// Dedup ring size.
const DEDUP_RING_SIZE: usize = 64;

/// Maximum symbols per process_samples call.
const MAX_SYMBOLS: usize = 512;

/// Default multi-slicer thresholds (AGC-normalized ±16384 scale).
const _SLICER_THRESHOLDS_3: [i16; 3] = [-330, 0, 330];

/// Negative-biased thresholds (grid search optimal for 9600 baud).
const SLICER_THRESHOLDS_NEG: [i16; 3] = [-660, -330, 0];

/// Positive-inclusive thresholds for 4th-order LPF diversity.
const SLICER_THRESHOLDS_POS: [i16; 3] = [-330, 0, 330];

/// A decoded frame with its content.
pub struct DecodedFrame9600 {
    pub data: [u8; 330],
    pub len: usize,
}

/// Multi-decoder output buffer for 9600 baud.
pub struct Multi9600Output {
    frames: [DecodedFrame9600; MAX_OUTPUT_FRAMES],
    count: usize,
}

impl Multi9600Output {
    fn new() -> Self {
        Self {
            frames: core::array::from_fn(|_| DecodedFrame9600 {
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

/// Which 9600 baud algorithm a decoder slot uses.
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum Algo9600 {
    Direwolf(usize),
    Gardner(usize),
    EarlyLate(usize),
    MuellerMuller(usize),
    Rrc(usize),
}

/// Multi-decoder for 9600 baud G3RUH.
///
/// Combines multiple algorithms × LPF orders × cutoffs × slicer thresholds.
/// Uses both 2nd-order (single biquad) and 4th-order (cascaded) LPF for diversity.
pub struct Multi9600Decoder {
    // Algorithm instances (heap-allocated on std to avoid stack overflow)
    #[cfg(feature = "std")]
    direwolf: Vec<Demod9600Direwolf>,
    #[cfg(not(feature = "std"))]
    direwolf: [Demod9600Direwolf; 4],
    dw_count: usize,

    #[cfg(feature = "std")]
    gardner: Vec<Demod9600Gardner>,
    #[cfg(not(feature = "std"))]
    gardner: [Demod9600Gardner; 4],
    gardner_count: usize,

    // HDLC decoders (one per algorithm instance)
    #[cfg(feature = "std")]
    hdlc: Vec<SoftHdlcDecoder>,
    #[cfg(not(feature = "std"))]
    hdlc: [HdlcDecoder; MAX_9600_DECODERS],

    // Mapping from decoder index to algorithm
    algo_map: [Algo9600; MAX_9600_DECODERS],
    num_active: usize,

    // Decoder labels for attribution
    #[cfg(feature = "attribution")]
    labels: [&'static str; MAX_9600_DECODERS],

    // Per-decoder frame bitmask for attribution (heap-allocated)
    #[cfg(feature = "attribution")]
    pub frame_sources: Box<[[bool; MAX_9600_DECODERS]; 256]>,
    #[cfg(feature = "attribution")]
    pub frame_count: usize,

    // Deduplication
    recent_hashes: [(u32, u32); DEDUP_RING_SIZE],
    recent_write: usize,
    recent_count: usize,
    generation: u32,

    // Stats
    pub total_decoded: u64,
    pub total_unique: u64,
}

impl Multi9600Decoder {
    /// Create a multi-decoder with default diversity for the given config.
    pub fn new(config: Demod9600Config) -> Self {
        let mut decoder = Self {
            #[cfg(feature = "std")]
            direwolf: Vec::new(),
            #[cfg(not(feature = "std"))]
            direwolf: core::array::from_fn(|_| Demod9600Direwolf::new(config)),
            dw_count: 0,
            #[cfg(feature = "std")]
            gardner: Vec::new(),
            #[cfg(not(feature = "std"))]
            gardner: core::array::from_fn(|_| Demod9600Gardner::new(config)),
            gardner_count: 0,
            #[cfg(feature = "std")]
            hdlc: Vec::new(),
            #[cfg(not(feature = "std"))]
            hdlc: core::array::from_fn(|_| HdlcDecoder::new()),
            algo_map: [Algo9600::Direwolf(0); MAX_9600_DECODERS],
            num_active: 0,
            #[cfg(feature = "attribution")]
            labels: [""; MAX_9600_DECODERS],
            #[cfg(feature = "attribution")]
            frame_sources: Box::new([[false; MAX_9600_DECODERS]; 256]),
            #[cfg(feature = "attribution")]
            frame_count: 0,
            recent_hashes: [(0, 0); DEDUP_RING_SIZE],
            recent_write: 0,
            recent_count: 0,
            generation: 0,
            total_decoded: 0,
            total_unique: 0,
        };

        #[cfg(feature = "std")]
        decoder.build_std_ensemble(config);
        #[cfg(not(feature = "std"))]
        decoder.build_nostd_ensemble(config);

        decoder
    }

    /// Compute timing phase offsets for the symbol period.
    /// Returns [0, period/3, 2*period/3] in Q8 phase units.
    fn timing_phases(config: &Demod9600Config) -> [i32; 3] {
        let period = (config.sample_rate as i64 * 256 / config.baud_rate as i64) as i32;
        [0, period / 3, period * 2 / 3]
    }

    #[cfg(feature = "std")]
    fn build_std_ensemble(&mut self, config: Demod9600Config) {
        let mut idx = 0;
        let phases = Self::timing_phases(&config);

        // === DW 2nd-order LPF: 4 cutoffs × 3 negative-biased thresholds = 12 ===
        // Grid search shows 2nd-order dominates for best single-decoder performance.
        let cutoffs_2nd: [u32; 4] = [5400, 6000, 6600, 7200];
        for &cutoff in &cutoffs_2nd {
            for &threshold in &SLICER_THRESHOLDS_NEG {
                if idx < MAX_9600_DECODERS {
                    self.direwolf.push(
                        Demod9600Direwolf::new(config)
                            .with_lpf_cutoff(cutoff)
                            .with_threshold(threshold),
                    );
                    self.hdlc.push(SoftHdlcDecoder::new());
                    self.algo_map[idx] = Algo9600::Direwolf(self.dw_count);
                    #[cfg(feature = "attribution")]
                    {
                        self.labels[idx] = Self::dw_label_2nd(cutoff, threshold);
                    }
                    self.dw_count += 1;
                    idx += 1;
                }
            }
        }

        // === DW 4th-order cascaded LPF: 3 cutoffs × 3 thresholds = 9 ===
        // 4th-order adds complementary diversity (appears in set-cover positions 3-5).
        let cutoffs_4th: [u32; 3] = [5400, 6600, 7200];
        for &cutoff in &cutoffs_4th {
            for &threshold in &SLICER_THRESHOLDS_POS {
                if idx < MAX_9600_DECODERS {
                    self.direwolf.push(
                        Demod9600Direwolf::new(config)
                            .with_cascaded_lpf_cutoff(cutoff)
                            .with_threshold(threshold),
                    );
                    self.hdlc.push(SoftHdlcDecoder::new());
                    self.algo_map[idx] = Algo9600::Direwolf(self.dw_count);
                    #[cfg(feature = "attribution")]
                    {
                        self.labels[idx] = Self::dw_label_4th(cutoff, threshold);
                    }
                    self.dw_count += 1;
                    idx += 1;
                }
            }
        }

        // === DW timing offset: 3 strong configs at T/3 ===
        // Timing phase is minor but catches a few extra frames.
        let timing_configs: [(u32, bool, i16); 3] = [
            (6000, false, -330),  // 2nd-order
            (6600, false, -330),  // 2nd-order
            (6000, false, -660),  // 2nd-order
        ];
        for &(cutoff, cascaded, threshold) in &timing_configs {
            if idx < MAX_9600_DECODERS {
                let dw = if cascaded {
                    Demod9600Direwolf::new(config)
                        .with_cascaded_lpf_cutoff(cutoff)
                        .with_timing_offset(phases[1])
                        .with_threshold(threshold)
                } else {
                    Demod9600Direwolf::new(config)
                        .with_lpf_cutoff(cutoff)
                        .with_timing_offset(phases[1])
                        .with_threshold(threshold)
                };
                self.direwolf.push(dw);
                self.hdlc.push(SoftHdlcDecoder::new());
                self.algo_map[idx] = Algo9600::Direwolf(self.dw_count);
                #[cfg(feature = "attribution")]
                {
                    self.labels[idx] = Self::dw_label_timing(cutoff, cascaded, threshold);
                }
                self.dw_count += 1;
                idx += 1;
            }
        }

        // === Gardner 2nd-order: 2 inertias × 3 thresholds = 6 ===
        let inertias: [(i32, i32); 2] = [(228, 171), (180, 100)];
        for &(locked, searching) in &inertias {
            for &threshold in &SLICER_THRESHOLDS_NEG {
                if idx < MAX_9600_DECODERS {
                    self.gardner.push(
                        Demod9600Gardner::new(config)
                            .with_inertia(locked, searching)
                            .with_threshold(threshold),
                    );
                    self.hdlc.push(SoftHdlcDecoder::new());
                    self.algo_map[idx] = Algo9600::Gardner(self.gardner_count);
                    #[cfg(feature = "attribution")]
                    {
                        self.labels[idx] = Self::gardner_label_2nd(locked, threshold);
                    }
                    self.gardner_count += 1;
                    idx += 1;
                }
            }
        }

        // === Gardner 4th-order: 2 inertias × 2 thresholds at 4800 Hz = 4 ===
        // Grid search showed G:4800/i180/th-660 appears in set-cover.
        for &(locked, searching) in &inertias {
            for &threshold in &[-660i16, -330] {
                if idx < MAX_9600_DECODERS {
                    self.gardner.push(
                        Demod9600Gardner::new(config)
                            .with_cascaded_lpf_cutoff(4800)
                            .with_inertia(locked, searching)
                            .with_threshold(threshold),
                    );
                    self.hdlc.push(SoftHdlcDecoder::new());
                    self.algo_map[idx] = Algo9600::Gardner(self.gardner_count);
                    #[cfg(feature = "attribution")]
                    {
                        self.labels[idx] = Self::gardner_label_4th(locked, threshold);
                    }
                    self.gardner_count += 1;
                    idx += 1;
                }
            }
        }

        self.num_active = idx;
    }

    // Attribution label helpers
    #[cfg(feature = "attribution")]
    fn dw_label_2nd(cutoff: u32, threshold: i16) -> &'static str {
        match (cutoff, threshold) {
            (5400, -660) => "DW:5400/2nd/th-660",
            (5400, -330) => "DW:5400/2nd/th-330",
            (5400, _)    => "DW:5400/2nd/th0",
            (6000, -660) => "DW:6000/2nd/th-660",
            (6000, -330) => "DW:6000/2nd/th-330",
            (6000, _)    => "DW:6000/2nd/th0",
            (6600, -660) => "DW:6600/2nd/th-660",
            (6600, -330) => "DW:6600/2nd/th-330",
            (6600, _)    => "DW:6600/2nd/th0",
            (_, -660)    => "DW:7200/2nd/th-660",
            (_, -330)    => "DW:7200/2nd/th-330",
            _            => "DW:7200/2nd/th0",
        }
    }

    #[cfg(feature = "attribution")]
    fn dw_label_4th(cutoff: u32, threshold: i16) -> &'static str {
        match (cutoff, threshold) {
            (5400, -330) => "DW:5400/4th/th-330",
            (5400, 0)    => "DW:5400/4th/th0",
            (5400, _)    => "DW:5400/4th/th+330",
            (6600, -330) => "DW:6600/4th/th-330",
            (6600, 0)    => "DW:6600/4th/th0",
            (6600, _)    => "DW:6600/4th/th+330",
            (_, -330)    => "DW:7200/4th/th-330",
            (_, 0)       => "DW:7200/4th/th0",
            _            => "DW:7200/4th/th+330",
        }
    }

    #[cfg(feature = "attribution")]
    fn dw_label_timing(cutoff: u32, _cascaded: bool, threshold: i16) -> &'static str {
        match (cutoff, threshold) {
            (6000, -660) => "DW:6000/2nd/t1/th-660",
            (6000, _)    => "DW:6000/2nd/t1/th-330",
            (_, _)       => "DW:6600/2nd/t1/th-330",
        }
    }

    #[cfg(feature = "attribution")]
    fn gardner_label_2nd(locked: i32, threshold: i16) -> &'static str {
        match (locked, threshold) {
            (228, -660) => "G:i228/2nd/th-660",
            (228, -330) => "G:i228/2nd/th-330",
            (228, _)    => "G:i228/2nd/th0",
            (_, -660)   => "G:i180/2nd/th-660",
            (_, -330)   => "G:i180/2nd/th-330",
            _           => "G:i180/2nd/th0",
        }
    }

    #[cfg(feature = "attribution")]
    fn gardner_label_4th(locked: i32, threshold: i16) -> &'static str {
        match (locked, threshold) {
            (228, -660) => "G:i228/4th/4800/th-660",
            (228, _)    => "G:i228/4th/4800/th-330",
            (_, -660)   => "G:i180/4th/4800/th-660",
            _           => "G:i180/4th/4800/th-330",
        }
    }

    #[cfg(not(feature = "std"))]
    fn build_nostd_ensemble(&mut self, config: Demod9600Config) {
        let mut idx = 0;

        // DW-style: 3 slicer thresholds
        for &threshold in &SLICER_THRESHOLDS_3 {
            if self.dw_count < 4 && idx < MAX_9600_DECODERS {
                self.direwolf[self.dw_count] = Demod9600Direwolf::new(config)
                    .with_threshold(threshold);
                self.algo_map[idx] = Algo9600::Direwolf(self.dw_count);
                self.dw_count += 1;
                idx += 1;
            }
        }

        // Gardner: 3 slicer thresholds
        for &threshold in &SLICER_THRESHOLDS_3 {
            if self.gardner_count < 4 && idx < MAX_9600_DECODERS {
                self.gardner[self.gardner_count] = Demod9600Gardner::new(config)
                    .with_threshold(threshold);
                self.algo_map[idx] = Algo9600::Gardner(self.gardner_count);
                self.gardner_count += 1;
                idx += 1;
            }
        }

        self.num_active = idx;
    }

    /// Process a buffer of audio samples through all decoders.
    pub fn process_samples(&mut self, samples: &[i16]) -> Multi9600Output {
        let mut output = Multi9600Output::new();
        self.generation = self.generation.wrapping_add(1);

        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; MAX_SYMBOLS];

        for decoder_idx in 0..self.num_active {
            let n_syms = match self.algo_map[decoder_idx] {
                Algo9600::Direwolf(i) => self.direwolf[i].process_samples(samples, &mut symbols),
                Algo9600::Gardner(i) => self.gardner[i].process_samples(samples, &mut symbols),
                _ => 0, // EarlyLate/MuellerMuller/Rrc not used in ensemble
            };

            for sym in &symbols[..n_syms] {
                let mut frame_buf = [0u8; 330];
                let mut frame_len = 0usize;
                let mut got_frame = false;

                {
                    let frame_opt = {
                        #[cfg(feature = "std")]
                        { self.hdlc[decoder_idx].feed_soft_bit(sym.llr) }
                        #[cfg(not(feature = "std"))]
                        { self.hdlc[decoder_idx].feed_bit(sym.bit) }
                    };

                    let frame_data = match frame_opt {
                        #[cfg(feature = "std")]
                        Some(FrameResult::Valid(data)) | Some(FrameResult::Recovered { data, .. }) => Some(data),
                        #[cfg(not(feature = "std"))]
                        Some(data) => Some(data),
                        _ => None,
                    };

                    if let Some(data) = frame_data {
                        frame_len = data.len().min(330);
                        frame_buf[..frame_len].copy_from_slice(&data[..frame_len]);
                        got_frame = true;
                    }
                }

                if got_frame {
                    self.total_decoded += 1;
                    let hash = super::frame_hash(&frame_buf[..frame_len]);
                    let is_dup = self.is_duplicate(hash);

                    #[cfg(feature = "attribution")]
                    {
                        // Find or create frame entry for attribution
                        let frame_idx = self.find_or_add_frame_hash(hash);
                        if frame_idx < 256 {
                            self.frame_sources[frame_idx][decoder_idx] = true;
                        }
                    }

                    if !is_dup {
                        self.add_hash(hash);
                        self.total_unique += 1;

                        if output.count < MAX_OUTPUT_FRAMES {
                            output.frames[output.count].data[..frame_len]
                                .copy_from_slice(&frame_buf[..frame_len]);
                            output.frames[output.count].len = frame_len;
                            output.count += 1;
                        }
                    }
                }
            }
        }

        output
    }

    /// Number of active decoders.
    pub fn num_decoders(&self) -> usize {
        self.num_active
    }

    /// Get decoder labels (for attribution).
    #[cfg(feature = "attribution")]
    pub fn labels(&self) -> &[&'static str] {
        &self.labels[..self.num_active]
    }

    fn is_duplicate(&self, hash: u32) -> bool {
        let min_gen = self.generation.saturating_sub(3);
        for i in 0..self.recent_count.min(DEDUP_RING_SIZE) {
            if self.recent_hashes[i].0 == hash && self.recent_hashes[i].1 >= min_gen {
                return true;
            }
        }
        false
    }

    fn add_hash(&mut self, hash: u32) {
        self.recent_hashes[self.recent_write] = (hash, self.generation);
        self.recent_write = (self.recent_write + 1) % DEDUP_RING_SIZE;
        self.recent_count = (self.recent_count + 1).min(DEDUP_RING_SIZE);
    }

    #[cfg(feature = "attribution")]
    fn find_or_add_frame_hash(&mut self, hash: u32) -> usize {
        // Linear scan (frame_count is small — typically <100)
        for i in 0..self.frame_count {
            if self.recent_hashes.get(i).map(|h| h.0) == Some(hash) {
                return i;
            }
        }
        // Check in the dedup ring as a secondary lookup
        // Actually, we need a separate hash-to-index mapping for attribution.
        // Use frame_count as the index for new frames.
        let idx = self.frame_count;
        if idx < 256 {
            self.frame_count += 1;
        }
        idx
    }
}

// fnv1a_hash is now centralized as super::frame_hash

/// Output buffer for Single9600Decoder — holds up to 4 frames per process call.
pub struct Single9600Output {
    pub frames: [([u8; 330], usize); 4],
    pub count: usize,
}

impl Single9600Output {
    fn new() -> Self {
        Self {
            frames: [([0u8; 330], 0); 4],
            count: 0,
        }
    }

    /// Number of frames decoded in this batch.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether no frames were decoded.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Get a decoded frame by index as (data, len).
    pub fn frame(&self, index: usize) -> &([u8; 330], usize) {
        &self.frames[index]
    }
}

/// Convenience alias for a single-algorithm 9600 baud decoder (no multi-slicer).
/// Wraps any algorithm with an HDLC decoder.
pub struct Single9600Decoder {
    algo: SingleAlgo,
    #[cfg(feature = "std")]
    hdlc: SoftHdlcDecoder,
    #[cfg(not(feature = "std"))]
    hdlc: HdlcDecoder,
}

enum SingleAlgo {
    Direwolf(Demod9600Direwolf),
    Gardner(Demod9600Gardner),
    EarlyLate(Demod9600EarlyLate),
    MuellerMuller(Demod9600MuellerMuller),
    Rrc(Demod9600Rrc),
}

impl Single9600Decoder {
    /// Create a single DW-style 9600 decoder.
    pub fn direwolf(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::Direwolf(Demod9600Direwolf::new(config)),
            #[cfg(feature = "std")]
            hdlc: SoftHdlcDecoder::new(),
            #[cfg(not(feature = "std"))]
            hdlc: HdlcDecoder::new(),
        }
    }

    /// Create a single Gardner PLL 9600 decoder.
    pub fn gardner(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::Gardner(Demod9600Gardner::new(config)),
            #[cfg(feature = "std")]
            hdlc: SoftHdlcDecoder::new(),
            #[cfg(not(feature = "std"))]
            hdlc: HdlcDecoder::new(),
        }
    }

    /// Create a single Early-Late 9600 decoder.
    pub fn early_late(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::EarlyLate(Demod9600EarlyLate::new(config)),
            #[cfg(feature = "std")]
            hdlc: SoftHdlcDecoder::new(),
            #[cfg(not(feature = "std"))]
            hdlc: HdlcDecoder::new(),
        }
    }

    /// Create a single Mueller-Muller 9600 decoder.
    pub fn mueller_muller(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::MuellerMuller(Demod9600MuellerMuller::new(config)),
            #[cfg(feature = "std")]
            hdlc: SoftHdlcDecoder::new(),
            #[cfg(not(feature = "std"))]
            hdlc: HdlcDecoder::new(),
        }
    }

    /// Create a single RRC 9600 decoder.
    pub fn rrc(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::Rrc(Demod9600Rrc::new(config)),
            #[cfg(feature = "std")]
            hdlc: SoftHdlcDecoder::new(),
            #[cfg(not(feature = "std"))]
            hdlc: HdlcDecoder::new(),
        }
    }

    /// Process a buffer of audio samples.
    /// Returns all decoded frames (up to 4) found in this batch.
    pub fn process_samples(&mut self, samples: &[i16]) -> Single9600Output {
        let mut output = Single9600Output::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; MAX_SYMBOLS];

        let n_syms = match &mut self.algo {
            SingleAlgo::Direwolf(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::Gardner(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::EarlyLate(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::MuellerMuller(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::Rrc(d) => d.process_samples(samples, &mut symbols),
        };

        for sym in &symbols[..n_syms] {
            let frame_opt = {
                #[cfg(feature = "std")]
                { self.hdlc.feed_soft_bit(sym.llr) }
                #[cfg(not(feature = "std"))]
                { self.hdlc.feed_bit(sym.bit) }
            };

            let frame_data = match frame_opt {
                #[cfg(feature = "std")]
                Some(FrameResult::Valid(data)) | Some(FrameResult::Recovered { data, .. }) => Some(data),
                #[cfg(not(feature = "std"))]
                Some(data) => Some(data),
                _ => None,
            };

            if let Some(data) = frame_data {
                if output.count < 4 {
                    let len = data.len().min(330);
                    output.frames[output.count].0[..len].copy_from_slice(&data[..len]);
                    output.frames[output.count].1 = len;
                    output.count += 1;
                }
            }
        }

        output
    }
}

// ─── Mini9600Decoder — MCU-optimized 6-decoder ensemble ───

/// Number of decoders in the Mini9600 ensemble.
const MINI_9600_DECODERS: usize = 6;

/// Mini9600Decoder — grid-search-optimal 6-decoder ensemble for MCU targets.
///
/// Selected from greedy set-cover analysis across 38400/44100/48000 Hz:
///
/// 1. DW: 2nd-order 6000Hz, threshold=-330 (best single at 48k: 64 frames)
/// 2. DW: 2nd-order 6600Hz, threshold=-330 (best single at 44k: 56 frames)
/// 3. DW: 2nd-order 6600Hz, threshold=0 (+4 at 48k set-cover, +3 at 44k)
/// 4. DW: 2nd-order 5400Hz, threshold=0 (+3 at 44k set-cover)
/// 5. DW: 4th-order 5400Hz, threshold=330 (LPF order diversity, +1 at 38k)
/// 6. Gardner: 2nd-order 4800Hz, inertia 180/100, threshold=-660 (algorithm diversity)
///
/// RAM: ~1.5 KB (6 × ~250 bytes per decoder state). MCU-feasible.
pub struct Mini9600Decoder {
    dw: [Demod9600Direwolf; 5],     // slots 0-4
    gardner: [Demod9600Gardner; 1],  // slot 5
    #[cfg(feature = "std")]
    hdlc: [SoftHdlcDecoder; MINI_9600_DECODERS],
    #[cfg(not(feature = "std"))]
    hdlc: [HdlcDecoder; MINI_9600_DECODERS],
    // dedup
    recent_hashes: [(u32, u32); 32],
    recent_write: usize,
    recent_count: usize,
    generation: u32,
    pub total_decoded: u64,
    pub total_unique: u64,
}

impl Mini9600Decoder {
    /// Create a Mini9600Decoder tuned for the given sample rate.
    ///
    /// Branches on samples-per-symbol to select rate-optimal decoder combos:
    /// - ≤4.3 sps (38400 Hz): low-sps-optimized combo
    /// - 4.3-4.7 sps (44100 Hz): mid-sps-optimized combo
    /// - >4.7 sps (48000+ Hz): original cross-rate-optimal combo
    pub fn new(config: Demod9600Config) -> Self {
        let sps_x10 = config.sample_rate * 10 / config.baud_rate;

        let (dw, gardner) = if sps_x10 <= 43 {
            // 38400 Hz (4.0 sps): wider LPF + negative thresholds dominate.
            Self::build_low_sps(config)
        } else if sps_x10 <= 47 {
            // 44100 Hz (4.59 sps): grid-search-optimal combo.
            Self::build_mid_sps(config)
        } else if sps_x10 >= 90 {
            // 96000+ Hz (≥9.4 sps): 4th-order LPF dominates at high oversampling.
            Self::build_high_sps(config)
        } else {
            // 48000 Hz (5.0 sps): grid-search-optimal combo.
            Self::build_default(config)
        };

        Self {
            dw,
            gardner,
            #[cfg(feature = "std")]
            hdlc: core::array::from_fn(|_| SoftHdlcDecoder::new()),
            #[cfg(not(feature = "std"))]
            hdlc: core::array::from_fn(|_| HdlcDecoder::new()),
            recent_hashes: [(0, 0); 32],
            recent_write: 0,
            recent_count: 0,
            generation: 0,
            total_decoded: 0,
            total_unique: 0,
        }
    }

    /// Build decoder combo for ≤4.3 sps (38400 Hz).
    ///
    /// Grid-search-optimal set-cover at 38400 Hz (with PLL hysteresis):
    /// #1 DW:6000/2nd/th-330 (60), #2 DW:6600/2nd/th-660 (+2=62),
    /// #3 DW:6600/4th/th0 (+1=63), #4 G:4800/2nd/i180-100/th-660 (+1=64)
    fn build_low_sps(config: Demod9600Config) -> ([Demod9600Direwolf; 5], [Demod9600Gardner; 1]) {
        // At 4.0 sps: bad_threshold=5, with good_threshold diversity (8 vs 16).
        // good_count>8 catches faster-locking frames that good_count>16 misses.
        let dw = [
            // #1: best single at 38k (bad=5, good=16)
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-330)
                .with_bad_threshold(5),
            // #2: set-cover +2 (bad=5, good=16)
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(5),
            // #3: 4th-order diversity (bad=5, good=16)
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(0)
                .with_bad_threshold(5),
            // Fast-lock diversity: #1 config with good_threshold=8
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-330)
                .with_bad_threshold(5)
                .with_good_threshold(8),
            // Fast-lock diversity: #2 config with good_threshold=8
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(5)
                .with_good_threshold(8),
        ];
        let gardner = [
            // #4: algorithm diversity, set-cover +1
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660)
                .with_bad_threshold(5),
        ];
        (dw, gardner)
    }

    /// Build decoder combo for 4.3-4.7 sps (44100 Hz).
    ///
    /// Grid-search-optimal set-cover at 44100 Hz (with PLL hysteresis):
    /// #1 DW:6600/2nd/th-660 (62), #2 DW:6600/4th/th-330 (+4=66),
    /// #3 DW:5400/2nd/th330 (+1=67)
    fn build_mid_sps(config: Demod9600Config) -> ([Demod9600Direwolf; 5], [Demod9600Gardner; 1]) {
        // bad_threshold=3 optimal at 44k; add good_threshold diversity (8 vs 16)
        let dw = [
            // #1: best single at 44k (62 frames)
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660),
            // #2: 4th-order diversity, set-cover +4
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(-330),
            // #3: set-cover +1
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(5400)
                .with_threshold(330),
            // Extra diversity: DW:6000/2nd/th-660 (strong at 48k too)
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-660),
            // Extra diversity: DW:7200/2nd/th-660
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(7200)
                .with_threshold(-660),
        ];
        let gardner = [
            // Algorithm diversity
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660),
        ];
        (dw, gardner)
    }

    /// Build default decoder combo for >4.7 sps (48000+ Hz).
    ///
    /// Grid-search-optimal set-cover at 48000 Hz (with PLL hysteresis):
    /// #1 DW:6000/2nd/th-660 (65), #2 DW:6600/4th/th330 (+4=69),
    /// #3 DW:6600/2nd/th-660 (+1=70), #4 G:4800/2nd/i180-100/th-660 (+1=71),
    /// #5 DW:5400/4th/th-660 (+1=72), #6 DW:4800/4th/th-330 (+1=73)
    fn build_default(config: Demod9600Config) -> ([Demod9600Direwolf; 5], [Demod9600Gardner; 1]) {
        let dw = [
            // #1: best single at 48k (65 frames)
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-660),
            // #2: 4th-order diversity, set-cover +4
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(330),
            // #3: set-cover +1
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660),
            // #5: 4th-order diversity, set-cover +1
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(5400)
                .with_threshold(-660),
            // #6: 4th-order diversity, set-cover +1
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(4800)
                .with_threshold(-330),
        ];
        let gardner = [
            // #4: algorithm diversity, set-cover +1
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660),
        ];
        (dw, gardner)
    }

    /// Build decoder combo for ≥9.4 sps (96000+ Hz).
    ///
    /// Grid-search-optimal set-cover at 96000 Hz (with PLL hysteresis):
    /// #1 DW:6600/4th/th-660 (86), #2 DW:6600/4th/th0 (+3=89),
    /// #3 DW:6000/2nd/th-330 (+2=91), #4 DW:7200/4th/th330 (+1=92),
    /// #5 G:4800/2nd/i180-100/th-660 (+1=93)
    fn build_high_sps(config: Demod9600Config) -> ([Demod9600Direwolf; 5], [Demod9600Gardner; 1]) {
        // At ≥10 sps, bad_threshold=4 is optimal
        let dw = [
            // #1: best single at 96k (86 frames) — 4th-order dominates at high oversampling
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(4),
            // #2: set-cover +3
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(0)
                .with_bad_threshold(4),
            // #3: 2nd-order diversity, set-cover +2
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-330)
                .with_bad_threshold(4),
            // #4: set-cover +1
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(7200)
                .with_threshold(330)
                .with_bad_threshold(4),
            // Extra diversity: 2nd-order 6600
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(4),
        ];
        let gardner = [
            // #5: algorithm diversity, set-cover +1
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660)
                .with_bad_threshold(4),
        ];
        (dw, gardner)
    }

    /// Process a buffer of audio samples through all 6 decoders.
    pub fn process_samples(&mut self, samples: &[i16]) -> Multi9600Output {
        let mut output = Multi9600Output::new();
        self.generation = self.generation.wrapping_add(1);

        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; MAX_SYMBOLS];

        // Process all 6 decoder slots (5 DW + 1 Gardner)
        for slot in 0..MINI_9600_DECODERS {
            let n_syms = match slot {
                0..=4 => self.dw[slot].process_samples(samples, &mut symbols),
                5 => self.gardner[0].process_samples(samples, &mut symbols),
                _ => 0,
            };

            for sym in &symbols[..n_syms] {
                let mut frame_buf = [0u8; 330];
                let mut frame_len = 0usize;
                let mut got_frame = false;

                {
                    let frame_opt = {
                        #[cfg(feature = "std")]
                        { self.hdlc[slot].feed_soft_bit(sym.llr) }
                        #[cfg(not(feature = "std"))]
                        { self.hdlc[slot].feed_bit(sym.bit) }
                    };

                    let frame_data = match frame_opt {
                        #[cfg(feature = "std")]
                        Some(FrameResult::Valid(data)) | Some(FrameResult::Recovered { data, .. }) => Some(data),
                        #[cfg(not(feature = "std"))]
                        Some(data) => Some(data),
                        _ => None,
                    };

                    if let Some(data) = frame_data {
                        frame_len = data.len().min(330);
                        frame_buf[..frame_len].copy_from_slice(&data[..frame_len]);
                        got_frame = true;
                    }
                }

                if got_frame {
                    self.total_decoded += 1;
                    let hash = super::frame_hash(&frame_buf[..frame_len]);
                    if !self.is_duplicate(hash) {
                        self.add_hash(hash);
                        self.total_unique += 1;

                        if output.count < MAX_OUTPUT_FRAMES {
                            output.frames[output.count].data[..frame_len]
                                .copy_from_slice(&frame_buf[..frame_len]);
                            output.frames[output.count].len = frame_len;
                            output.count += 1;
                        }
                    }
                }
            }
        }

        output
    }

    /// Number of active decoders (always 6).
    pub fn num_decoders(&self) -> usize {
        MINI_9600_DECODERS
    }

    fn is_duplicate(&self, hash: u32) -> bool {
        let min_gen = self.generation.saturating_sub(3);
        for i in 0..self.recent_count.min(32) {
            if self.recent_hashes[i].0 == hash && self.recent_hashes[i].1 >= min_gen {
                return true;
            }
        }
        false
    }

    fn add_hash(&mut self, hash: u32) {
        self.recent_hashes[self.recent_write] = (hash, self.generation);
        self.recent_write = (self.recent_write + 1) % 32;
        self.recent_count = (self.recent_count + 1).min(32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi9600_creation() {
        let config = Demod9600Config::default_48k();
        let decoder = Multi9600Decoder::new(config);

        assert!(decoder.num_decoders() > 0,
            "Should have active decoders, got {}", decoder.num_decoders());

        #[cfg(feature = "std")]
        assert_eq!(decoder.num_decoders(), 34,
            "std should have 34 decoders (24 DW + 10 Gardner), got {}", decoder.num_decoders());

        #[cfg(not(feature = "std"))]
        assert_eq!(decoder.num_decoders(), 6,
            "no_std should have 6 decoders");
    }

    #[test]
    fn test_multi9600_44k_creation() {
        let config = Demod9600Config::default_44k();
        let decoder = Multi9600Decoder::new(config);

        #[cfg(feature = "std")]
        assert_eq!(decoder.num_decoders(), 34);
    }

    #[test]
    fn test_mini9600_creation() {
        let config = Demod9600Config::default_48k();
        let decoder = Mini9600Decoder::new(config);
        assert_eq!(decoder.num_decoders(), 6);
    }

    #[test]
    fn test_mini9600_44k_creation() {
        let config = Demod9600Config::default_44k();
        let decoder = Mini9600Decoder::new(config);
        assert_eq!(decoder.num_decoders(), 6);
    }

    #[test]
    fn test_frame_hash() {
        let data1 = b"hello world";
        let data2 = b"hello world";
        let data3 = b"different data";

        assert_eq!(crate::modem::frame_hash(data1), crate::modem::frame_hash(data2));
        assert_ne!(crate::modem::frame_hash(data1), crate::modem::frame_hash(data3));
    }

    #[test]
    fn test_single_decoder_creation() {
        let config = Demod9600Config::default_48k();
        let _dw = Single9600Decoder::direwolf(config);
        let _g = Single9600Decoder::gardner(config);
        let _el = Single9600Decoder::early_late(config);
        let _mm = Single9600Decoder::mueller_muller(config);
        let _rrc = Single9600Decoder::rrc(config);
    }

    #[test]
    fn test_multi9600_empty_input() {
        let config = Demod9600Config::default_48k();
        let mut decoder = Multi9600Decoder::new(config);
        let output = decoder.process_samples(&[]);
        assert!(output.is_empty());
    }

    #[test]
    fn test_mini9600_empty_input() {
        let config = Demod9600Config::default_48k();
        let mut decoder = Mini9600Decoder::new(config);
        let output = decoder.process_samples(&[]);
        assert!(output.is_empty());
    }
}
