//! Shared types, decode helpers, WAV reader, and utility functions.

use std::cell::Cell;
use std::time::{Duration, Instant};

use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::modem::demod::{
    CorrelationDemodulator, DemodSymbol, DmDemodulator, FastDemodulator, QualityDemodulator,
};
use packet_radio_core::modem::corr_slicer::CorrSlicerDecoder;
use packet_radio_core::modem::multi::{MiniDecoder, MultiDecoder, TwistMiniDecoder};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};
use packet_radio_core::modem::DemodConfig;

thread_local! {
    pub static BAUD_RATE: Cell<u32> = const { Cell::new(1200) };
}

pub fn get_baud() -> u32 {
    BAUD_RATE.with(|b| b.get())
}

/// Build a DemodConfig for the given sample rate and baud rate.
pub fn config_for_rate(sample_rate: u32, baud: u32) -> DemodConfig {
    match baud {
        300 => {
            let mut c = DemodConfig::default_300();
            c.sample_rate = sample_rate;
            c
        }
        _ => {
            let mut c = DemodConfig::default_1200();
            c.sample_rate = sample_rate;
            c
        }
    }
}

// ─── Decode Engine ───────────────────────────────────────────────────────

/// Result of decoding a WAV file with one demodulator path.
pub struct DecodeResult {
    pub frames: Vec<Vec<u8>>,
    pub elapsed: Duration,
}

// ─── Decode Helpers ─────────────────────────────────────────────────────

/// Chunk size for all decode loops.
pub const DECODE_CHUNK: usize = 1024;

/// Run a symbol-producing demodulator through hard HDLC.
pub fn run_hard_decode(
    samples: &[i16],
    mut process: impl FnMut(&[i16], &mut [DemodSymbol]) -> usize,
) -> DecodeResult {
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; DECODE_CHUNK];

    let start = Instant::now();
    for chunk in samples.chunks(DECODE_CHUNK) {
        let n = process(chunk, &mut symbols);
        for sym in &symbols[..n] {
            if let Some(frame) = hdlc.feed_bit(sym.bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    DecodeResult { frames, elapsed: start.elapsed() }
}

/// Run a symbol-producing demodulator through soft HDLC (LLR bit-flip recovery).
pub fn run_soft_decode(
    samples: &[i16],
    mut process: impl FnMut(&[i16], &mut [DemodSymbol]) -> usize,
) -> (DecodeResult, u32) {
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; DECODE_CHUNK];

    let start = Instant::now();
    for chunk in samples.chunks(DECODE_CHUNK) {
        let n = process(chunk, &mut symbols);
        for sym in &symbols[..n] {
            if let Some(result) = soft_hdlc.feed_soft_bit(sym.llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }
    let soft_recovered = soft_hdlc.stats_total_soft_recovered();
    (DecodeResult { frames, elapsed: start.elapsed() }, soft_recovered)
}

/// Merge multiple lists of hashed+positioned frames, deduplicating by
/// content hash within a time window of `window_samples`.
pub fn dedup_merge(
    phase_frames: &[Vec<(u64, usize, Vec<u8>)>],
    window_samples: usize,
) -> Vec<Vec<u8>> {
    let mut all_frames: Vec<Vec<u8>> = Vec::new();
    let mut seen: Vec<(u64, usize)> = Vec::new();
    for phase in phase_frames {
        for (hash, pos, data) in phase {
            let is_dup = seen.iter().any(|(h, p)| {
                *h == *hash && (*pos as i64 - *p as i64).unsigned_abs() < window_samples as u64
            });
            if !is_dup {
                seen.push((*hash, *pos));
                all_frames.push(data.clone());
            }
        }
    }
    all_frames
}

/// FNV-1a hash for frame deduplication.
pub fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// ─── Standard Decode Functions ──────────────────────────────────────────

/// Decode audio samples using the fast demodulator + hard HDLC.
pub fn decode_fast(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = FastDemodulator::new(config).with_adaptive_gain();
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode audio samples using the quality demodulator + soft HDLC.
pub fn decode_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = QualityDemodulator::new(config).with_adaptive_gain();
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode audio samples using the multi-decoder (9 parallel fast decoders).
pub fn decode_multi(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let mut multi = MultiDecoder::new(config);
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let output = multi.process_samples(chunk);
        for i in 0..output.len() {
            frames.push(output.frame(i).to_vec());
        }
    }

    let soft = multi.total_soft_recovered();
    (DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }, soft)
}

/// Decode audio samples using the MiniDecoder (3 attribution-optimal decoders).
pub fn decode_smart3(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let mut mini = MiniDecoder::new(config);
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let output = mini.process_samples(chunk);
        for i in 0..output.len() {
            frames.push(output.frame(i).to_vec());
        }
    }

    let soft = mini.total_soft_recovered();
    (DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }, soft)
}

/// Decode using TwistMiniDecoder (Smart3 + 3 twist-compensated decoders).
pub fn decode_twist_mini(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let mut decoder = TwistMiniDecoder::new(config);
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let output = decoder.process_samples(chunk);
        for i in 0..output.len() {
            frames.push(output.frame(i).to_vec());
        }
    }

    (DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }, 0) // TwistMiniDecoder doesn't expose soft stats yet
}

/// Decode using the fast demodulator with adaptive Goertzel re-tuning.
pub fn decode_fast_adaptive(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = FastDemodulator::new(config).with_adaptive_retune().with_energy_llr();
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms)).0
}

/// Decode using the quality demodulator (with retune + hybrid LLR) + soft HDLC.
pub fn decode_quality_adaptive(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = QualityDemodulator::new(config);
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms)).0
}

/// Decode using the best single-decoder config from attribution analysis.
pub fn decode_best_single(samples: &[i16], sample_rate: u32) -> DecodeResult {
    use packet_radio_core::modem::filter;

    let config = config_for_rate(sample_rate, get_baud());

    let freq_offset: i32 = -50;
    let phase_offset = 2 * sample_rate / 3; // t2
    let mark = (config.mark_freq as i32 + freq_offset) as u32;
    let space = (config.space_freq as i32 + freq_offset) as u32;

    let center = (1700i32 + freq_offset) as f64;
    let bpf = filter::bandpass_coeffs(sample_rate, center, 2000.0);

    let mut demod = FastDemodulator::new(config).filter(bpf).phase_offset(phase_offset)
        .frequencies(mark, space).with_adaptive_retune().with_energy_llr();
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms)).0
}

/// Decode with a custom single Goertzel decoder.
pub fn decode_custom_goertzel(
    samples: &[i16],
    sample_rate: u32,
    freq_offset: i32,
    timing_phase: u32,
    bpf_variant: i32,
) -> DecodeResult {
    use packet_radio_core::modem::filter;

    let config = config_for_rate(sample_rate, get_baud());

    let phase_offset = timing_phase * sample_rate / 3;
    let mark = (config.mark_freq as i32 + freq_offset) as u32;
    let space = (config.space_freq as i32 + freq_offset) as u32;

    let bpf = match bpf_variant {
        0 => filter::afsk_bandpass_narrow_11025(),
        2 => filter::afsk_bandpass_wide_11025(),
        _ if freq_offset != 0 => {
            let center = (1700i32 + freq_offset) as f64;
            filter::bandpass_coeffs(sample_rate, center, 2000.0)
        }
        _ => filter::afsk_bandpass_11025(),
    };

    let mut demod = FastDemodulator::new(config).filter(bpf).phase_offset(phase_offset)
        .frequencies(mark, space);
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Resample audio to a target sample rate using linear interpolation.
pub fn resample_to(samples: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let new_len = (samples.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;
        if src_idx + 1 < samples.len() {
            let s = samples[src_idx] as f64 * (1.0 - frac)
                + samples[src_idx + 1] as f64 * frac;
            out.push(s.clamp(-32768.0, 32767.0) as i16);
        } else if src_idx < samples.len() {
            out.push(samples[src_idx]);
        }
    }
    out
}

/// Decode audio samples using the delay-multiply demodulator + hard HDLC.
pub fn decode_dm(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = DmDemodulator::with_bpf(config);
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode audio samples using the correlation (mixer) demodulator + hard HDLC.
pub fn decode_corr(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = CorrelationDemodulator::new(config).with_adaptive_gain();
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using correlation demod + energy LLR + soft HDLC.
pub fn decode_corr_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr();
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using correlation demod with 3 timing phases.
pub fn decode_corr_3phase(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let offsets = [0, sample_rate / 3, 2 * sample_rate / 3];

    let mut phase_frames: Vec<Vec<(u64, usize, Vec<u8>)>> = Vec::new();

    let start = Instant::now();

    for &offset in &offsets {
        let mut demod = CorrelationDemodulator::new(config).with_adaptive_gain();
        demod.set_bit_phase(offset);
        let mut hdlc = HdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        let mut frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
        let mut sample_pos: usize = 0;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if let Some(frame) = hdlc.feed_bit(sym.bit) {
                    let hash = fnv1a_hash(frame);
                    frames.push((hash, sample_pos, frame.to_vec()));
                }
            }
            sample_pos += chunk.len();
        }
        phase_frames.push(frames);
    }

    let dedup_window = sample_rate as usize * 2;
    let all_frames = dedup_merge(&phase_frames, dedup_window);

    DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using correlation demod with 3 timing phases + energy LLR + soft HDLC.
pub fn decode_corr_3phase_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let offsets = [0, sample_rate / 3, 2 * sample_rate / 3];

    let mut phase_frames: Vec<Vec<(u64, usize, Vec<u8>)>> = Vec::new();
    let mut total_soft: u32 = 0;

    let start = Instant::now();

    for &offset in &offsets {
        let mut demod = CorrelationDemodulator::with_filter_and_offset(
            config,
            packet_radio_core::modem::filter::afsk_bandpass_11025(),
            offset,
        ).with_adaptive_gain().with_energy_llr();
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        let mut frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
        let mut sample_pos: usize = 0;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if let Some(result) = soft_hdlc.feed_soft_bit(sym.llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => d,
                        FrameResult::Recovered { data, .. } => data,
                    };
                    let hash = fnv1a_hash(data);
                    frames.push((hash, sample_pos, data.to_vec()));
                }
            }
            sample_pos += chunk.len();
        }
        total_soft += soft_hdlc.stats_total_soft_recovered();
        phase_frames.push(frames);
    }

    let dedup_window = sample_rate as usize * 2;
    let all_frames = dedup_merge(&phase_frames, dedup_window);

    (DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }, total_soft)
}

/// Decode using correlation multi-slicer (single demod, N gain slicers).
pub fn decode_corr_slicer(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let mut decoder = CorrSlicerDecoder::new(config).with_adaptive_gain();
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let output = decoder.process_samples(chunk);
        for i in 0..output.len() {
            frames.push(output.frame(i).to_vec());
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using correlation multi-slicer with 3 timing phases.
pub fn decode_corr_slicer_3phase(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let offsets = [0, sample_rate / 3, 2 * sample_rate / 3];

    let mut phase_frames: Vec<Vec<(u64, usize, Vec<u8>)>> = Vec::new();

    let start = Instant::now();

    for &offset in &offsets {
        let mut decoder = CorrSlicerDecoder::new(config).with_adaptive_gain();
        decoder.set_bit_phase(offset);
        let mut frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
        let mut sample_pos: usize = 0;

        for chunk in samples.chunks(1024) {
            let output = decoder.process_samples(chunk);
            for i in 0..output.len() {
                let data = output.frame(i).to_vec();
                let hash = fnv1a_hash(&data);
                frames.push((hash, sample_pos, data));
            }
            sample_pos += chunk.len();
        }
        phase_frames.push(frames);
    }

    let dedup_window = sample_rate as usize * 2;
    let all_frames = dedup_merge(&phase_frames, dedup_window);

    DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using MultiDecoder + CorrSlicerDecoder (3-phase) combined, with dedup merge.
pub fn decode_combined(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let start = Instant::now();

    // Run MultiDecoder
    let mut multi = MultiDecoder::new(config);
    let mut multi_frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
    let mut sample_pos: usize = 0;
    for chunk in samples.chunks(1024) {
        let output = multi.process_samples(chunk);
        for i in 0..output.len() {
            let data = output.frame(i).to_vec();
            let hash = fnv1a_hash(&data);
            multi_frames.push((hash, sample_pos, data));
        }
        sample_pos += chunk.len();
    }
    let multi_soft = multi.total_soft_recovered();

    // Run CorrSlicerDecoder with 3 timing phases
    let offsets = [0, sample_rate / 3, 2 * sample_rate / 3];
    let mut corr_frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
    for &offset in &offsets {
        let mut decoder = CorrSlicerDecoder::new(config).with_adaptive_gain();
        decoder.set_bit_phase(offset);
        let mut sp: usize = 0;
        for chunk in samples.chunks(1024) {
            let output = decoder.process_samples(chunk);
            for i in 0..output.len() {
                let data = output.frame(i).to_vec();
                let hash = fnv1a_hash(&data);
                corr_frames.push((hash, sp, data));
            }
            sp += chunk.len();
        }
    }

    let dedup_window = sample_rate as usize * 2;
    let combined_phases = vec![multi_frames, corr_frames];
    let all_frames = dedup_merge(&combined_phases, dedup_window);

    (DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }, multi_soft)
}

// ─── Unified Result & Grid Printer ────────────────────────────────────────

/// Result for one WAV file across all decoder types.
pub struct UnifiedResult {
    pub display_name: String,
    pub dw_count: Option<u32>,
    pub duration_secs: f64,
    pub fast: Option<(usize, Duration)>,
    pub quality: Option<(usize, Duration)>,
    pub smart3: Option<(usize, Duration)>,
    pub twist_mini: Option<(usize, Duration)>,
    pub dm: Option<(usize, Duration)>,
    pub multi: Option<(usize, Duration)>,
    pub corr3: Option<(usize, Duration)>,
    pub slicer3: Option<(usize, Duration)>,
    pub combined: Option<(usize, Duration)>,
}

impl UnifiedResult {
    pub fn count(&self, col: usize) -> Option<usize> {
        match col {
            0 => self.fast.map(|(c, _)| c),
            1 => self.quality.map(|(c, _)| c),
            2 => self.smart3.map(|(c, _)| c),
            3 => self.twist_mini.map(|(c, _)| c),
            4 => self.dm.map(|(c, _)| c),
            5 => self.multi.map(|(c, _)| c),
            6 => self.corr3.map(|(c, _)| c),
            7 => self.slicer3.map(|(c, _)| c),
            8 => self.combined.map(|(c, _)| c),
            _ => None,
        }
    }

    pub fn elapsed(&self, col: usize) -> Option<Duration> {
        match col {
            0 => self.fast.map(|(_, e)| e),
            1 => self.quality.map(|(_, e)| e),
            2 => self.smart3.map(|(_, e)| e),
            3 => self.twist_mini.map(|(_, e)| e),
            4 => self.dm.map(|(_, e)| e),
            5 => self.multi.map(|(_, e)| e),
            6 => self.corr3.map(|(_, e)| e),
            7 => self.slicer3.map(|(_, e)| e),
            8 => self.combined.map(|(_, e)| e),
            _ => None,
        }
    }
}

pub const MCU_COLS: usize = 5;
pub const ALL_COLS: usize = 9;
pub const COL_NAMES: [&str; 9] = ["Fst", "Qlt", "S3", "TM", "DM", "Mlt", "C3", "Sl3", "Cmb"];

pub fn num_cols(mcu_only: bool) -> usize {
    if mcu_only { MCU_COLS } else { ALL_COLS }
}

/// Print the unified comparison grid.
pub fn print_unified_grid(title: &str, results: &[UnifiedResult], mcu_only: bool) {
    let cols = num_cols(mcu_only);
    let have_dw = results.iter().any(|r| r.dw_count.is_some());

    println!("{}", title);
    println!();

    let dw_hdr = if have_dw { format!("{:>5}", "DW") } else { String::new() };
    let mut hdr = format!("{:<30} {}", "Track", dw_hdr);
    for (i, col_name) in COL_NAMES.iter().enumerate().take(cols) {
        if i == MCU_COLS && !mcu_only {
            hdr.push_str(" \u{2502}");
        }
        hdr.push_str(&format!(" {:>5}", col_name));
    }
    println!("{}", hdr);

    let dw_sep_w = if have_dw { 6 } else { 0 };
    let mcu_w = 30 + dw_sep_w + MCU_COLS * 6;
    let desk_w = if mcu_only { 0 } else { (ALL_COLS - MCU_COLS) * 6 + 2 };
    let sep_mcu = "\u{2500}".repeat(mcu_w);
    if mcu_only {
        println!("{}", sep_mcu);
    } else {
        println!("{}\u{253c}{}", sep_mcu, "\u{2500}".repeat(desk_w));
    }

    let mut totals = vec![0usize; cols];
    let mut total_dw = 0u32;

    for r in results {
        let dw_str = if have_dw {
            format!(" {:>5}", r.dw_count.map_or("?".to_string(), |d| d.to_string()))
        } else {
            String::new()
        };
        let mut row = format!("{:<30}{}", r.display_name, dw_str);
        for (i, total) in totals.iter_mut().enumerate() {
            if i == MCU_COLS && !mcu_only {
                row.push_str(" \u{2502}");
            }
            match r.count(i) {
                Some(c) => {
                    row.push_str(&format!(" {:>5}", c));
                    *total += c;
                }
                None => row.push_str("     -"),
            }
        }
        println!("{}", row);

        if let Some(dw) = r.dw_count {
            total_dw += dw;
        }
    }

    if mcu_only {
        println!("{}", sep_mcu);
    } else {
        println!("{}\u{253c}{}", sep_mcu, "\u{2500}".repeat(desk_w));
    }

    let dw_total_str = if have_dw {
        format!(" {:>5}", total_dw)
    } else {
        String::new()
    };
    let mut total_row = format!("{:<30}{}", "TOTAL", dw_total_str);
    for (i, total) in totals.iter().enumerate() {
        if i == MCU_COLS && !mcu_only {
            total_row.push_str(" \u{2502}");
        }
        if results.iter().any(|r| r.count(i).is_some()) {
            total_row.push_str(&format!(" {:>5}", total));
        } else {
            total_row.push_str("     -");
        }
    }
    println!("{}", total_row);

    if have_dw && total_dw > 0 {
        let mut pct_row = format!("{:<30}{}", "%DW", if have_dw { "      " } else { "" });
        for (i, total) in totals.iter().enumerate() {
            if i == MCU_COLS && !mcu_only {
                pct_row.push_str(" \u{2502}");
            }
            if results.iter().any(|r| r.count(i).is_some()) {
                let pct = *total as f64 / total_dw as f64 * 100.0;
                pct_row.push_str(&format!(" {:>5.1}", pct));
            } else {
                pct_row.push_str("     -");
            }
        }
        println!("{}", pct_row);
    }

    println!();
}

/// Print timing summary for all results.
pub fn print_timing_summary(results: &[UnifiedResult], mcu_only: bool) {
    let cols = num_cols(mcu_only);
    println!("Timing (x real-time):");
    let mut hdr = format!("  {:<28}", "Track");
    for (i, col_name) in COL_NAMES.iter().enumerate().take(cols) {
        if i == MCU_COLS && !mcu_only {
            hdr.push_str("  \u{2502}");
        }
        hdr.push_str(&format!("  {:>5}", col_name));
    }
    println!("{}", hdr);
    println!("  {}", "\u{2500}".repeat(28 + cols * 7 + if mcu_only { 0 } else { 3 }));

    for r in results {
        let mut row = format!("  {:<28}", r.display_name);
        for i in 0..cols {
            if i == MCU_COLS && !mcu_only {
                row.push_str("  \u{2502}");
            }
            match r.elapsed(i) {
                Some(e) if e.as_secs_f64() > 0.0 => {
                    let rt = r.duration_secs / e.as_secs_f64();
                    row.push_str(&format!("  {:>5.0}", rt));
                }
                _ => row.push_str("      -"),
            }
        }
        println!("{}", row);
    }
    println!();
}

/// Run all decoders on a given set of samples, returning a UnifiedResult.
pub fn decode_all_unified(
    display_name: &str,
    samples: &[i16],
    sample_rate: u32,
    dw_count: Option<u32>,
    mcu_only: bool,
) -> UnifiedResult {
    let duration_secs = samples.len() as f64 / sample_rate as f64;
    let baud = get_baud();

    let fast = decode_fast(samples, sample_rate);
    let (quality, _) = decode_quality(samples, sample_rate);
    let (smart3, _) = decode_smart3(samples, sample_rate);
    let (twist_mini, _) = if baud != 300 {
        decode_twist_mini(samples, sample_rate)
    } else {
        (DecodeResult { frames: vec![], elapsed: Duration::ZERO }, 0)
    };
    let dm = decode_dm(samples, sample_rate);

    let (multi, corr3, slicer3, combined) = if !mcu_only {
        let (m, _) = decode_multi(samples, sample_rate);
        let c3 = decode_corr_3phase(samples, sample_rate);
        let s3 = decode_corr_slicer_3phase(samples, sample_rate);
        let (cmb, _) = decode_combined(samples, sample_rate);
        (
            Some((m.frames.len(), m.elapsed)),
            Some((c3.frames.len(), c3.elapsed)),
            Some((s3.frames.len(), s3.elapsed)),
            Some((cmb.frames.len(), cmb.elapsed)),
        )
    } else {
        (None, None, None, None)
    };

    UnifiedResult {
        display_name: display_name.to_string(),
        dw_count,
        duration_secs,
        fast: Some((fast.frames.len(), fast.elapsed)),
        quality: Some((quality.frames.len(), quality.elapsed)),
        smart3: Some((smart3.frames.len(), smart3.elapsed)),
        twist_mini: if baud != 300 {
            Some((twist_mini.frames.len(), twist_mini.elapsed))
        } else {
            None
        },
        dm: Some((dm.frames.len(), dm.elapsed)),
        multi,
        corr3,
        slicer3,
        combined,
    }
}

// ─── WAV File Reader ───────────────────────────────────────────────────────

pub fn read_wav_file(path: &str) -> Result<(u32, Vec<i16>), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| format!("{}", e))?;

    if buf.len() < 44 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err("Not a valid WAV file".to_string());
    }

    let audio_format = u16::from_le_bytes([buf[20], buf[21]]);
    if audio_format != 1 {
        return Err(format!(
            "Unsupported WAV format code {} (only PCM=1 supported)",
            audio_format
        ));
    }

    let sample_rate = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let channels = u16::from_le_bytes([buf[22], buf[23]]);
    let bits_per_sample = u16::from_le_bytes([buf[34], buf[35]]);

    if bits_per_sample != 16 {
        return Err(format!(
            "Unsupported bit depth: {} (need 16-bit)",
            bits_per_sample
        ));
    }

    let mut pos = 12;
    while pos + 8 < buf.len() {
        let chunk_id = &buf[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]])
            as usize;

        if chunk_id == b"data" {
            let data_start = pos + 8;
            let data_end = (data_start + chunk_size).min(buf.len());

            let samples: Vec<i16> = buf[data_start..data_end]
                .chunks_exact(2 * channels as usize)
                .map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
                .collect();

            return Ok((sample_rate, samples));
        }

        pos += 8 + chunk_size;
        if !chunk_size.is_multiple_of(2) {
            pos += 1;
        }
    }

    Err("No data chunk found in WAV file".to_string())
}

// ─── Signal Impairment Utilities ─────────────────────────────────────────

/// Add white Gaussian noise at the specified SNR (dB).
pub fn add_white_noise(samples: &[i16], snr_db: f64, seed: u64) -> Vec<i16> {
    let signal_power: f64 = samples
        .iter()
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>()
        / samples.len() as f64;

    let noise_power = signal_power / f64::powf(10.0, snr_db / 10.0);
    let noise_stddev = f64::sqrt(noise_power);

    let mut rng_state = seed;
    let mut next_random = move || -> f64 {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        let u1 = (rng_state & 0xFFFFFFFF) as f64 / 4294967296.0 + 0.0001;
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        let u2 = (rng_state & 0xFFFFFFFF) as f64 / 4294967296.0;
        f64::sqrt(-2.0 * f64::ln(u1)) * f64::cos(std::f64::consts::TAU * u2)
    };

    samples
        .iter()
        .map(|&s| {
            let noise = next_random() * noise_stddev;
            (s as f64 + noise).clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// Apply a frequency offset (Hz) to simulate transmitter crystal drift.
pub fn apply_frequency_offset(samples: &[i16], offset_hz: f64, sample_rate: u32) -> Vec<i16> {
    use std::f64::consts::TAU;

    const HALF_LEN: usize = 15;
    const HILBERT_LEN: usize = 2 * HALF_LEN + 1;
    let mut hilbert_coeffs = [0.0f64; HILBERT_LEN];
    for (i, coeff) in hilbert_coeffs.iter_mut().enumerate() {
        let n = i as isize - HALF_LEN as isize;
        if n == 0 {
            *coeff = 0.0;
        } else if n % 2 != 0 {
            let hamming = 0.54 - 0.46 * f64::cos(TAU * i as f64 / (HILBERT_LEN - 1) as f64);
            *coeff = (2.0 / (std::f64::consts::PI * n as f64)) * hamming;
        }
    }

    let phase_step = TAU * offset_hz / sample_rate as f64;
    let mut delay_line = vec![0.0f64; HILBERT_LEN];
    let mut write_idx = 0;
    let mut phase = 0.0;

    samples
        .iter()
        .map(|&s| {
            let x = s as f64;
            delay_line[write_idx] = x;
            write_idx = (write_idx + 1) % HILBERT_LEN;

            let mut q = 0.0;
            for (k, &hc) in hilbert_coeffs.iter().enumerate() {
                let idx = (write_idx + k) % HILBERT_LEN;
                q += delay_line[idx] * hc;
            }

            let i_delayed = delay_line[(write_idx + HALF_LEN) % HILBERT_LEN];

            let cos_p = f64::cos(phase);
            let sin_p = f64::sin(phase);
            let shifted = i_delayed * cos_p - q * sin_p;

            phase += phase_step;
            if phase > TAU {
                phase -= TAU;
            }

            shifted.clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// Resample to simulate clock drift (ratio > 1.0 = transmitter clock fast).
pub fn apply_clock_drift(samples: &[i16], ratio: f64) -> Vec<i16> {
    if samples.is_empty() || ratio <= 0.0 {
        return Vec::new();
    }
    let new_len = (samples.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;

        let s = if src_idx + 1 < samples.len() {
            samples[src_idx] as f64 * (1.0 - frac) + samples[src_idx + 1] as f64 * frac
        } else {
            samples[src_idx.min(samples.len() - 1)] as f64
        };
        output.push(s.clamp(-32768.0, 32767.0) as i16);
    }

    output
}

// ─── Utility ───────────────────────────────────────────────────────────────

/// Truncate a string to at most `max_bytes`, respecting char boundaries.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ─── TNC2 Frame Formatter ──────────────────────────────────────────────────

/// Convert raw AX.25 frame bytes to TNC2 monitor format matching Dire Wolf output.
pub fn frame_to_tnc2(frame: &[u8]) -> Option<String> {
    if frame.len() < 16 {
        return None;
    }

    let dst = parse_callsign_tnc2(&frame[0..7], false);
    let src = parse_callsign_tnc2(&frame[7..14], false);

    let mut result = format!("{}>{}",  src, dst);

    struct ViaEntry {
        callsign: String,
        h_bit: bool,
    }
    let mut vias = Vec::new();
    let mut pos = 14;
    let mut addr_end = (frame[13] & 0x01) != 0;

    while !addr_end && pos + 7 <= frame.len() {
        let h_bit = (frame[pos + 6] & 0x80) != 0;
        let callsign = parse_callsign_tnc2(&frame[pos..pos + 7], false);
        vias.push(ViaEntry { callsign, h_bit });
        addr_end = (frame[pos + 6] & 0x01) != 0;
        pos += 7;
    }

    let last_h = vias.iter().rposition(|v| v.h_bit);
    for (i, via) in vias.iter().enumerate() {
        result.push(',');
        result.push_str(&via.callsign);
        if Some(i) == last_h {
            result.push('*');
        }
    }

    if pos + 2 > frame.len() {
        return None;
    }
    pos += 2;

    result.push(':');

    let info = &frame[pos..];
    let cleaned: Vec<u8> = info.iter().copied()
        .filter(|&b| b >= 0x20 || b == 0x09)
        .collect();
    let info_str = String::from_utf8_lossy(&cleaned);
    result.push_str(&info_str);
    Some(result)
}

/// Parse callsign for TNC2 display, with optional H-bit marker.
pub fn parse_callsign_tnc2(data: &[u8], h_bit: bool) -> String {
    if data.len() < 7 {
        return "???".to_string();
    }
    let mut call = String::with_capacity(10);
    for &b in &data[..6] {
        let c = (b >> 1) & 0x7F;
        if c > 0x20 {
            call.push(c as char);
        }
    }
    let ssid = (data[6] >> 1) & 0x0F;
    if ssid > 0 {
        call.push_str(&format!("-{}",  ssid));
    }
    if h_bit {
        call.push('*');
    }
    call
}

/// Convert a batch of raw AX.25 frames to TNC2 strings.
pub fn frames_to_tnc2(frames: &[Vec<u8>]) -> Vec<String> {
    frames.iter().filter_map(|f| frame_to_tnc2(f)).collect()
}

// ─── Dire Wolf Reference ────────────────────────────────────────────────

/// Metadata for a DW-decoded frame from the clean log.
#[derive(Clone, Debug)]
pub struct DwFrameInfo {
    pub seq: u32,
    pub timestamp: String,
    pub audio_level: u32,
    pub mark_space: String,
    pub mark: u32,
    pub space: u32,
}

/// Load Dire Wolf .packets.txt file.
pub fn load_dw_packets(path: &str) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}", e))?;
    let contents = String::from_utf8_lossy(&bytes);
    Ok(contents.lines().filter(|l| !l.is_empty()).map(String::from).collect())
}

/// Parse DW .clean.log to extract per-frame metadata.
pub fn parse_dw_clean_log(path: &str) -> Result<Vec<(String, DwFrameInfo)>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}", e))?;
    let contents = String::from_utf8_lossy(&bytes);
    let mut results = Vec::new();

    let lines: Vec<&str> = contents.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("DECODED[") {
            if let Some(bracket_end) = rest.find(']') {
                let seq: u32 = rest[..bracket_end].parse().unwrap_or(0);
                let after_bracket = &rest[bracket_end + 1..].trim_start();

                let parts: Vec<&str> = after_bracket.splitn(2, ' ').collect();
                let timestamp = parts.first().unwrap_or(&"").to_string();

                let (audio_level, mark, space, mark_space) =
                    if let Some(al_pos) = line.find("audio level = ") {
                        let al_rest = &line[al_pos + 14..];
                        let level_str: String = al_rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                        let audio_level: u32 = level_str.parse().unwrap_or(0);

                        let (mark, space, ms_str) = if let Some(paren_start) = al_rest.find('(') {
                            let paren_end = al_rest.find(')').unwrap_or(al_rest.len());
                            let ratio = &al_rest[paren_start + 1..paren_end];
                            let ms_str = ratio.to_string();
                            let parts: Vec<&str> = ratio.split('/').collect();
                            let m: u32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                            let s: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                            (m, s, ms_str)
                        } else {
                            (0, 0, String::new())
                        };

                        (audio_level, mark, space, ms_str)
                    } else {
                        (0, 0, 0, String::new())
                    };

                i += 1;
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                if i < lines.len() {
                    let pkt_line = lines[i].trim();
                    let packet = pkt_line.strip_prefix("[0] ").unwrap_or(pkt_line);

                    let cleaned = packet
                        .replace("<0x0d>", "")
                        .replace("<0x0a>", "")
                        .replace("<0x09>", "\t");

                    results.push((cleaned, DwFrameInfo {
                        seq,
                        timestamp,
                        audio_level,
                        mark_space,
                        mark,
                        space,
                    }));
                }
            }
        }
        i += 1;
    }

    Ok(results)
}

/// Auto-discover DW reference files from WAV path.
pub fn discover_dw_reference(wav_path: &str) -> Option<(String, String)> {
    let stem = std::path::Path::new(wav_path)
        .file_stem()?
        .to_string_lossy()
        .to_string();

    let candidates = [
        ("iso/direwolf_review/packets", "iso/direwolf_review/raw_logs"),
    ];

    for &(pkt_dir, log_dir) in &candidates {
        let pkt_path = format!("{}/{}.packets.txt", pkt_dir, stem);
        let log_path = format!("{}/{}.clean.log", log_dir, stem);
        if std::path::Path::new(&pkt_path).exists() {
            return Some((pkt_path, log_path));
        }
    }
    None
}

/// Extract track name from filename for display and matching.
pub fn track_display_name(path: &str) -> String {
    let fname = std::path::Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if fname.len() > 30 {
        fname[..30].to_string()
    } else {
        fname
    }
}

/// Extract WAV filename from path for matching with DW CSV.
pub fn wav_filename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Known sample rates for rate suffix extraction.
pub const KNOWN_RATES: [u32; 8] = [8000, 11025, 12000, 13200, 22050, 26400, 44100, 48000];

/// Extract rate suffix from a filename stem.
pub fn extract_rate_suffix(stem: &str) -> Option<(&str, u32)> {
    if let Some(pos) = stem.rfind('_') {
        let suffix = &stem[pos + 1..];
        if let Ok(rate) = suffix.parse::<u32>() {
            if KNOWN_RATES.contains(&rate) {
                return Some((&stem[..pos], rate));
            }
        }
    }
    None
}

/// Abbreviate sample rate for display.
pub fn rate_abbrev(rate: u32) -> &'static str {
    match rate {
        8000 => "8k",
        11025 => "11k",
        12000 => "12k",
        13200 => "13k",
        22050 => "22k",
        26400 => "26k",
        44100 => "44k",
        48000 => "48k",
        _ => "?",
    }
}

/// Entry parsed from Dire Wolf summary.csv or baselines file.
pub struct DireWolfEntry {
    pub track_file: String,
    pub decoded_packets: u32,
}

/// Load Dire Wolf reference data from summary.csv.
pub fn load_direwolf_csv(dir: &str) -> Vec<DireWolfEntry> {
    let candidates = [
        format!("{}/../../iso/direwolf_review/summary.csv", dir),
        "iso/direwolf_review/summary.csv".to_string(),
    ];

    for path in &candidates {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let mut entries = Vec::new();
            for line in contents.lines().skip(1) {
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() >= 2 {
                    if let Ok(count) = fields[1].trim().parse::<u32>() {
                        entries.push(DireWolfEntry {
                            track_file: fields[0].trim().to_string(),
                            decoded_packets: count,
                        });
                    }
                }
            }
            return entries;
        }
    }

    Vec::new()
}

/// Load DW baselines from direwolf_baselines.txt.
pub fn load_direwolf_baselines(dir: &str) -> Vec<DireWolfEntry> {
    let path = format!("{}/direwolf_baselines.txt", dir);
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut entries = Vec::new();
    let mut current_file: Option<String> = None;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("---") && trimmed.ends_with("---") {
            let inner = trimmed.trim_start_matches('-').trim_end_matches('-').trim();
            if inner.ends_with(".wav") {
                current_file = Some(inner.to_string());
            }
            continue;
        }
        let fname = match current_file.as_ref() {
            Some(f) => f.clone(),
            None => continue,
        };
        let mut matched = false;
        if trimmed.contains("packets decoded") {
            if let Some(count_str) = trimmed.split_whitespace().next() {
                if let Ok(count) = count_str.parse::<u32>() {
                    entries.push(DireWolfEntry {
                        track_file: fname.clone(),
                        decoded_packets: count,
                    });
                    matched = true;
                }
            }
        }
        if !matched && trimmed.contains(" from ") {
            if let Some(count_str) = trimmed.split_whitespace().next() {
                if let Ok(count) = count_str.parse::<u32>() {
                    entries.push(DireWolfEntry {
                        track_file: fname,
                        decoded_packets: count,
                    });
                    matched = true;
                }
            }
        }
        if matched {
            current_file = None;
        }
    }

    entries
}

/// Load DW data from either direwolf_baselines.txt or summary.csv.
pub fn load_dw_data(dir: &str) -> Vec<DireWolfEntry> {
    let baseline_entries = load_direwolf_baselines(dir);
    if !baseline_entries.is_empty() {
        return baseline_entries;
    }
    load_direwolf_csv(dir)
}

/// Convert a frame to hex string.
pub fn frame_to_hex(frame: &[u8]) -> String {
    frame.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join("")
}
