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
use super::fixed_vec::FixedVec;
use super::frame_output::FrameOutputBuffer;
use super::hdlc_bank::{AnyHdlc, HdlcBank};
use super::{DedupAction, DedupRing};

#[cfg(feature = "attribution")]
extern crate alloc;
#[cfg(feature = "attribution")]
use alloc::boxed::Box;

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
#[cfg(not(feature = "std"))]
const SLICER_THRESHOLDS_3: [i16; 3] = [-330, 0, 330];

/// Negative-biased thresholds (grid search optimal for 9600 baud).
#[cfg(feature = "std")]
const SLICER_THRESHOLDS_NEG: [i16; 3] = [-660, -330, 0];

/// Positive-inclusive thresholds for 4th-order LPF diversity.
#[cfg(feature = "std")]
const SLICER_THRESHOLDS_POS: [i16; 3] = [-330, 0, 330];

/// Multi-decoder output buffer for 9600 baud.
pub type Multi9600Output = FrameOutputBuffer<MAX_OUTPUT_FRAMES>;

/// Owning enum for 9600 baud algorithm instances (Direwolf or Gardner).
///
/// Both variants have identical field layouts and `process_samples` signatures.
/// Storing them in an enum eliminates the need for separate typed arrays and
/// an `algo_map` dispatch table.
enum Algo9600 {
    Direwolf(Demod9600Direwolf),
    Gardner(Demod9600Gardner),
}

impl Algo9600 {
    /// Process audio samples through the wrapped demodulator.
    fn process_samples(&mut self, samples: &[i16], out: &mut [DemodSymbol]) -> usize {
        match self {
            Self::Direwolf(d) => d.process_samples(samples, out),
            Self::Gardner(d) => d.process_samples(samples, out),
        }
    }
}

/// Multi-decoder for 9600 baud G3RUH.
///
/// Combines multiple algorithms × LPF orders × cutoffs × slicer thresholds.
/// Uses both 2nd-order (single biquad) and 4th-order (cascaded) LPF for diversity.
pub struct Multi9600Decoder {
    // Unified algorithm storage — replaces separate direwolf/gardner arrays + algo_map
    decoders: FixedVec<Algo9600, MAX_9600_DECODERS>,

    // HDLC decoders (one per algorithm instance)
    hdlc: HdlcBank<MAX_9600_DECODERS>,

    // Decoder labels for attribution
    #[cfg(feature = "attribution")]
    labels: [&'static str; MAX_9600_DECODERS],

    // Per-decoder frame bitmask for attribution (heap-allocated)
    #[cfg(feature = "attribution")]
    pub frame_sources: Box<[[bool; MAX_9600_DECODERS]; 256]>,
    #[cfg(feature = "attribution")]
    pub frame_count: usize,

    // Deduplication
    dedup: DedupRing<DEDUP_RING_SIZE>,

    // Time tracking
    samples_processed: u64,

    // Stats
    pub total_decoded: u64,
    pub total_unique: u64,
}

impl Multi9600Decoder {
    /// Create a multi-decoder with default diversity for the given config.
    pub fn new(config: Demod9600Config) -> Self {
        let mut decoder = Self {
            decoders: FixedVec::new(),
            hdlc: HdlcBank::new(),
            #[cfg(feature = "attribution")]
            labels: [""; MAX_9600_DECODERS],
            #[cfg(feature = "attribution")]
            frame_sources: Box::new([[false; MAX_9600_DECODERS]; 256]),
            #[cfg(feature = "attribution")]
            frame_count: 0,
            dedup: DedupRing::with_overlap_from_config(config.samples_per_symbol()),
            samples_processed: 0,
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

    /// Push a Direwolf decoder and record its attribution label.
    fn push_dw(
        &mut self,
        dw: Demod9600Direwolf,
        #[cfg(feature = "attribution")] label: &'static str,
    ) {
        let idx = self.decoders.len();
        self.decoders.push(Algo9600::Direwolf(dw));
        #[cfg(feature = "attribution")]
        {
            self.labels[idx] = label;
        }
        let _ = idx;
    }

    /// Push a Gardner decoder and record its attribution label.
    fn push_gardner(
        &mut self,
        g: Demod9600Gardner,
        #[cfg(feature = "attribution")] label: &'static str,
    ) {
        let idx = self.decoders.len();
        self.decoders.push(Algo9600::Gardner(g));
        #[cfg(feature = "attribution")]
        {
            self.labels[idx] = label;
        }
        let _ = idx;
    }

    #[cfg(feature = "std")]
    fn build_std_ensemble(&mut self, config: Demod9600Config) {
        let phases = Self::timing_phases(&config);

        // === DW 2nd-order LPF: 4 cutoffs × 3 negative-biased thresholds = 12 ===
        let cutoffs_2nd: [u32; 4] = [5400, 6000, 6600, 7200];
        for &cutoff in &cutoffs_2nd {
            for &threshold in &SLICER_THRESHOLDS_NEG {
                self.push_dw(
                    Demod9600Direwolf::new(config)
                        .with_lpf_cutoff(cutoff)
                        .with_threshold(threshold),
                    #[cfg(feature = "attribution")]
                    Self::dw_label_2nd(cutoff, threshold),
                );
            }
        }

        // === DW 4th-order cascaded LPF: 3 cutoffs × 3 thresholds = 9 ===
        let cutoffs_4th: [u32; 3] = [5400, 6600, 7200];
        for &cutoff in &cutoffs_4th {
            for &threshold in &SLICER_THRESHOLDS_POS {
                self.push_dw(
                    Demod9600Direwolf::new(config)
                        .with_cascaded_lpf_cutoff(cutoff)
                        .with_threshold(threshold),
                    #[cfg(feature = "attribution")]
                    Self::dw_label_4th(cutoff, threshold),
                );
            }
        }

        // === DW timing offset: 3 strong configs at T/3 ===
        let timing_configs: [(u32, bool, i16); 3] = [
            (6000, false, -330),
            (6600, false, -330),
            (6000, false, -660),
        ];
        for &(cutoff, cascaded, threshold) in &timing_configs {
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
            self.push_dw(
                dw,
                #[cfg(feature = "attribution")]
                Self::dw_label_timing(cutoff, cascaded, threshold),
            );
        }

        // === Gardner 2nd-order: 2 inertias × 3 thresholds = 6 ===
        let inertias: [(i32, i32); 2] = [(228, 171), (180, 100)];
        for &(locked, searching) in &inertias {
            for &threshold in &SLICER_THRESHOLDS_NEG {
                self.push_gardner(
                    Demod9600Gardner::new(config)
                        .with_inertia(locked, searching)
                        .with_threshold(threshold),
                    #[cfg(feature = "attribution")]
                    Self::gardner_label_2nd(locked, threshold),
                );
            }
        }

        // === Gardner 4th-order: 2 inertias × 2 thresholds at 4800 Hz = 4 ===
        for &(locked, searching) in &inertias {
            for &threshold in &[-660i16, -330] {
                self.push_gardner(
                    Demod9600Gardner::new(config)
                        .with_cascaded_lpf_cutoff(4800)
                        .with_inertia(locked, searching)
                        .with_threshold(threshold),
                    #[cfg(feature = "attribution")]
                    Self::gardner_label_4th(locked, threshold),
                );
            }
        }
    }

    // Attribution label helpers
    #[cfg(feature = "attribution")]
    fn dw_label_2nd(cutoff: u32, threshold: i16) -> &'static str {
        match (cutoff, threshold) {
            (5400, -660) => "DW:5400/2nd/th-660",
            (5400, -330) => "DW:5400/2nd/th-330",
            (5400, _) => "DW:5400/2nd/th0",
            (6000, -660) => "DW:6000/2nd/th-660",
            (6000, -330) => "DW:6000/2nd/th-330",
            (6000, _) => "DW:6000/2nd/th0",
            (6600, -660) => "DW:6600/2nd/th-660",
            (6600, -330) => "DW:6600/2nd/th-330",
            (6600, _) => "DW:6600/2nd/th0",
            (_, -660) => "DW:7200/2nd/th-660",
            (_, -330) => "DW:7200/2nd/th-330",
            _ => "DW:7200/2nd/th0",
        }
    }

    #[cfg(feature = "attribution")]
    fn dw_label_4th(cutoff: u32, threshold: i16) -> &'static str {
        match (cutoff, threshold) {
            (5400, -330) => "DW:5400/4th/th-330",
            (5400, 0) => "DW:5400/4th/th0",
            (5400, _) => "DW:5400/4th/th+330",
            (6600, -330) => "DW:6600/4th/th-330",
            (6600, 0) => "DW:6600/4th/th0",
            (6600, _) => "DW:6600/4th/th+330",
            (_, -330) => "DW:7200/4th/th-330",
            (_, 0) => "DW:7200/4th/th0",
            _ => "DW:7200/4th/th+330",
        }
    }

    #[cfg(feature = "attribution")]
    fn dw_label_timing(cutoff: u32, _cascaded: bool, threshold: i16) -> &'static str {
        match (cutoff, threshold) {
            (6000, -660) => "DW:6000/2nd/t1/th-660",
            (6000, _) => "DW:6000/2nd/t1/th-330",
            (_, _) => "DW:6600/2nd/t1/th-330",
        }
    }

    #[cfg(feature = "attribution")]
    fn gardner_label_2nd(locked: i32, threshold: i16) -> &'static str {
        match (locked, threshold) {
            (228, -660) => "G:i228/2nd/th-660",
            (228, -330) => "G:i228/2nd/th-330",
            (228, _) => "G:i228/2nd/th0",
            (_, -660) => "G:i180/2nd/th-660",
            (_, -330) => "G:i180/2nd/th-330",
            _ => "G:i180/2nd/th0",
        }
    }

    #[cfg(feature = "attribution")]
    fn gardner_label_4th(locked: i32, threshold: i16) -> &'static str {
        match (locked, threshold) {
            (228, -660) => "G:i228/4th/4800/th-660",
            (228, _) => "G:i228/4th/4800/th-330",
            (_, -660) => "G:i180/4th/4800/th-660",
            _ => "G:i180/4th/4800/th-330",
        }
    }

    #[cfg(not(feature = "std"))]
    fn build_nostd_ensemble(&mut self, config: Demod9600Config) {
        // DW-style: 3 slicer thresholds
        for &threshold in &SLICER_THRESHOLDS_3 {
            self.decoders.push(Algo9600::Direwolf(
                Demod9600Direwolf::new(config).with_threshold(threshold),
            ));
        }

        // Gardner: 3 slicer thresholds
        for &threshold in &SLICER_THRESHOLDS_3 {
            self.decoders.push(Algo9600::Gardner(
                Demod9600Gardner::new(config).with_threshold(threshold),
            ));
        }
    }

    /// Process a buffer of audio samples through all decoders.
    pub fn process_samples(&mut self, samples: &[i16]) -> Multi9600Output {
        self.samples_processed += samples.len() as u64;
        let mut output = Multi9600Output::new();

        let mut symbols = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; MAX_SYMBOLS];

        for decoder_idx in 0..self.decoders.len() {
            let n_syms = self.decoders[decoder_idx].process_samples(samples, &mut symbols);

            for sym in &symbols[..n_syms] {
                let Some((data, cost)) = self.hdlc.feed(decoder_idx, sym.bit, sym.llr) else {
                    continue;
                };
                let mut frame_buf = [0u8; 330];
                let frame_len = data.len().min(330);
                frame_buf[..frame_len].copy_from_slice(&data[..frame_len]);

                {
                    self.total_decoded += 1;
                    let hash = super::frame_hash(&frame_buf[..frame_len]);

                    #[cfg(feature = "attribution")]
                    {
                        let frame_idx = self.find_or_add_frame_hash(hash);
                        if frame_idx < 256 {
                            self.frame_sources[frame_idx][decoder_idx] = true;
                        }
                    }

                    let start = sym.sample_idx as u64;
                    match self.dedup.check(hash, start, cost) {
                        DedupAction::New => {
                            if let Some(slot) = output.push_with_cost(&frame_buf[..frame_len], cost)
                            {
                                self.dedup.record_with_info(hash, start, cost, slot);
                                self.total_unique += 1;
                            }
                        }
                        DedupAction::Replace(old_slot) => {
                            output.replace(old_slot, &frame_buf[..frame_len], cost);
                            self.dedup.update_entry(old_slot, cost, old_slot);
                        }
                        DedupAction::Duplicate => {}
                    }
                }
            }
        }

        output
    }

    /// Number of active decoders.
    pub fn num_decoders(&self) -> usize {
        self.decoders.len()
    }

    /// Get decoder labels (for attribution).
    #[cfg(feature = "attribution")]
    pub fn labels(&self) -> &[&'static str] {
        &self.labels[..self.decoders.len()]
    }

    #[cfg(feature = "attribution")]
    fn find_or_add_frame_hash(&mut self, _hash: u32) -> usize {
        let idx = self.frame_count;
        if idx < 256 {
            self.frame_count += 1;
        }
        idx
    }
}

// ─── Single9600Decoder ─────────────────────────────────────────────────

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
    hdlc: AnyHdlc,
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
            hdlc: AnyHdlc::new(),
        }
    }

    /// Create a single Gardner PLL 9600 decoder.
    pub fn gardner(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::Gardner(Demod9600Gardner::new(config)),
            hdlc: AnyHdlc::new(),
        }
    }

    /// Create a single Early-Late 9600 decoder.
    pub fn early_late(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::EarlyLate(Demod9600EarlyLate::new(config)),
            hdlc: AnyHdlc::new(),
        }
    }

    /// Create a single Mueller-Muller 9600 decoder.
    pub fn mueller_muller(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::MuellerMuller(Demod9600MuellerMuller::new(config)),
            hdlc: AnyHdlc::new(),
        }
    }

    /// Create a single RRC 9600 decoder.
    pub fn rrc(config: Demod9600Config) -> Self {
        Self {
            algo: SingleAlgo::Rrc(Demod9600Rrc::new(config)),
            hdlc: AnyHdlc::new(),
        }
    }

    /// Process a buffer of audio samples.
    /// Returns all decoded frames (up to 4) found in this batch.
    pub fn process_samples(&mut self, samples: &[i16]) -> Single9600Output {
        let mut output = Single9600Output::new();
        let mut symbols = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; MAX_SYMBOLS];

        let n_syms = match &mut self.algo {
            SingleAlgo::Direwolf(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::Gardner(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::EarlyLate(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::MuellerMuller(d) => d.process_samples(samples, &mut symbols),
            SingleAlgo::Rrc(d) => d.process_samples(samples, &mut symbols),
        };

        for sym in &symbols[..n_syms] {
            if let Some((data, _cost)) = self.hdlc.feed(sym.bit, sym.llr) {
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
    decoders: FixedVec<Algo9600, MINI_9600_DECODERS>,
    hdlc: HdlcBank<MINI_9600_DECODERS>,
    dedup: DedupRing<32>,
    samples_processed: u64,
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

        let mut decoders = FixedVec::new();

        if sps_x10 <= 43 {
            Self::build_low_sps(config, &mut decoders);
        } else if sps_x10 <= 47 {
            Self::build_mid_sps(config, &mut decoders);
        } else if sps_x10 >= 90 {
            Self::build_high_sps(config, &mut decoders);
        } else {
            Self::build_default(config, &mut decoders);
        }

        Self {
            decoders,
            hdlc: HdlcBank::new(),
            dedup: DedupRing::with_overlap_from_config(config.samples_per_symbol()),
            samples_processed: 0,
            total_decoded: 0,
            total_unique: 0,
        }
    }

    /// Build decoder combo for ≤4.3 sps (38400 Hz).
    fn build_low_sps(config: Demod9600Config, d: &mut FixedVec<Algo9600, MINI_9600_DECODERS>) {
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-330)
                .with_bad_threshold(5),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(5),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(0)
                .with_bad_threshold(5),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-330)
                .with_bad_threshold(5)
                .with_good_threshold(8),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(5)
                .with_good_threshold(8),
        ));
        d.push(Algo9600::Gardner(
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660)
                .with_bad_threshold(5),
        ));
    }

    /// Build decoder combo for 4.3-4.7 sps (44100 Hz).
    fn build_mid_sps(config: Demod9600Config, d: &mut FixedVec<Algo9600, MINI_9600_DECODERS>) {
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(-330),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(5400)
                .with_threshold(330),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-660),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(7200)
                .with_threshold(-660),
        ));
        d.push(Algo9600::Gardner(
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660),
        ));
    }

    /// Build default decoder combo for >4.7 sps (48000+ Hz).
    fn build_default(config: Demod9600Config, d: &mut FixedVec<Algo9600, MINI_9600_DECODERS>) {
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-660),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(330),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(5400)
                .with_threshold(-660),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(4800)
                .with_threshold(-330),
        ));
        d.push(Algo9600::Gardner(
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660),
        ));
    }

    /// Build decoder combo for ≥9.4 sps (96000+ Hz).
    fn build_high_sps(config: Demod9600Config, d: &mut FixedVec<Algo9600, MINI_9600_DECODERS>) {
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(4),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(6600)
                .with_threshold(0)
                .with_bad_threshold(4),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6000)
                .with_threshold(-330)
                .with_bad_threshold(4),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_cascaded_lpf_cutoff(7200)
                .with_threshold(330)
                .with_bad_threshold(4),
        ));
        d.push(Algo9600::Direwolf(
            Demod9600Direwolf::new(config)
                .with_lpf_cutoff(6600)
                .with_threshold(-660)
                .with_bad_threshold(4),
        ));
        d.push(Algo9600::Gardner(
            Demod9600Gardner::new(config)
                .with_lpf_cutoff(4800)
                .with_inertia(180, 100)
                .with_threshold(-660)
                .with_bad_threshold(4),
        ));
    }

    /// Process a buffer of audio samples through all 6 decoders.
    pub fn process_samples(&mut self, samples: &[i16]) -> Multi9600Output {
        self.samples_processed += samples.len() as u64;
        let mut output = Multi9600Output::new();

        let mut symbols = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; MAX_SYMBOLS];

        for (slot, decoder) in self.decoders.iter_mut().enumerate() {
            let n_syms = decoder.process_samples(samples, &mut symbols);

            for sym in &symbols[..n_syms] {
                if let Some((data, cost)) = self.hdlc.feed(slot, sym.bit, sym.llr) {
                    let frame_len = data.len().min(330);
                    let mut frame_buf = [0u8; 330];
                    frame_buf[..frame_len].copy_from_slice(&data[..frame_len]);

                    self.total_decoded += 1;
                    let hash = super::frame_hash(&frame_buf[..frame_len]);
                    let start = sym.sample_idx as u64;
                    match self.dedup.check(hash, start, cost) {
                        DedupAction::New => {
                            if let Some(out_slot) =
                                output.push_with_cost(&frame_buf[..frame_len], cost)
                            {
                                self.dedup.record_with_info(hash, start, cost, out_slot);
                                self.total_unique += 1;
                            }
                        }
                        DedupAction::Replace(old_slot) => {
                            output.replace(old_slot, &frame_buf[..frame_len], cost);
                            self.dedup.update_entry(old_slot, cost, old_slot);
                        }
                        DedupAction::Duplicate => {}
                    }
                }
            }
        }

        output
    }

    /// Number of active decoders (always 6).
    pub fn num_decoders(&self) -> usize {
        self.decoders.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi9600_creation() {
        let config = Demod9600Config::default_48k();
        let decoder = Multi9600Decoder::new(config);

        assert!(
            decoder.num_decoders() > 0,
            "Should have active decoders, got {}",
            decoder.num_decoders()
        );

        #[cfg(feature = "std")]
        assert_eq!(
            decoder.num_decoders(),
            34,
            "std should have 34 decoders (24 DW + 10 Gardner), got {}",
            decoder.num_decoders()
        );

        #[cfg(not(feature = "std"))]
        assert_eq!(decoder.num_decoders(), 6, "no_std should have 6 decoders");
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

        assert_eq!(
            crate::modem::frame_hash(data1),
            crate::modem::frame_hash(data2)
        );
        assert_ne!(
            crate::modem::frame_hash(data1),
            crate::modem::frame_hash(data3)
        );
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
