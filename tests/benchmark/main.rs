//! WA8LMF TNC Test CD Benchmark Runner
//!
//! Processes WAV files through both demodulator paths and reports packet
//! counts, decode rates, and comparative performance against Dire Wolf.
//!
//! Usage:
//!   cargo run --release -p benchmark -- --wav track1.wav
//!   cargo run --release -p benchmark -- --suite tests/wav/
//!   cargo run --release -p benchmark -- --compare-approaches track1.wav
//!   cargo run --release -p benchmark -- --synthetic

use std::time::{Duration, Instant};

use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::modem::demod::{CorrelationDemodulator, DemodSymbol, DmDemodulator, FastDemodulator, QualityDemodulator};
use packet_radio_core::modem::binary_xor::BinaryXorDemodulator;
use packet_radio_core::modem::corr_slicer::CorrSlicerDecoder;
use packet_radio_core::modem::multi::{MiniDecoder, MultiDecoder, TwistMiniDecoder};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};
use packet_radio_core::modem::DemodConfig;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "--wav" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --wav <file.wav>");
                return;
            }
            run_single_wav(&args[2]);
        }
        "--suite" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --suite <directory>");
                return;
            }
            run_suite(&args[2]);
        }
        "--compare-approaches" | "--compare" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --compare-approaches <file.wav>");
                return;
            }
            run_compare_approaches(&args[2]);
        }
        "--dm" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --dm <file.wav>");
                return;
            }
            run_dm_single(&args[2]);
        }
        "--synthetic" => {
            run_synthetic_benchmark();
        }
        "--dm-pll" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --dm-pll <file.wav>");
                return;
            }
            run_dm_pll(&args[2]);
        }
        "--dm-pll-sweep" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --dm-pll-sweep <file.wav>");
                return;
            }
            run_dm_pll_sweep(&args[2]);
        }
        "--dm-debug" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --dm-debug <file.wav>");
                return;
            }
            run_dm_debug(&args[2]);
        }
        "--dm-pll-tune" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --dm-pll-tune <file.wav>");
                return;
            }
            run_dm_pll_tune(&args[2]);
        }
        "--export" => {
            if args.len() < 4 {
                eprintln!("Usage: benchmark --export <file.wav> <output_dir>");
                return;
            }
            run_export(&args[2], &args[3]);
        }
        "--diff" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --diff <file.wav> [--reference <packets.txt>]");
                return;
            }
            let reference = if args.len() >= 5 && args[3] == "--reference" {
                Some(args[4].as_str())
            } else {
                None
            };
            run_diff(&args[2], reference);
        }
        "--attribution" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --attribution <file.wav>");
                return;
            }
            run_attribution(&args[2]);
        }
        "--smart3" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --smart3 <file.wav>");
                return;
            }
            run_smart3(&args[2]);
        }
        "--soft-diag" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --soft-diag <file.wav>");
                return;
            }
            run_soft_diag(&args[2]);
        }
        "--corr" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --corr <file.wav>");
                return;
            }
            run_corr(&args[2]);
        }
        "--corr-lpf-sweep" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --corr-lpf-sweep <file.wav>");
                return;
            }
            run_corr_lpf_sweep(&args[2]);
        }
        "--corr-slicer" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --corr-slicer <file.wav>");
                return;
            }
            run_corr_slicer(&args[2]);
        }
        "--corr-pll" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --corr-pll <file.wav>");
                return;
            }
            run_corr_pll(&args[2]);
        }
        "--corr-pll-sweep" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --corr-pll-sweep <file.wav>");
                return;
            }
            run_corr_pll_sweep(&args[2]);
        }
        "--xor" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --xor <file.wav>");
                return;
            }
            run_xor(&args[2]);
        }
        "--twist" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --twist <file.wav>");
                return;
            }
            run_twist_sweep(&args[2]);
        }
        "--twist-mini" => {
            if args.len() < 3 {
                eprintln!("Usage: benchmark --twist-mini <file.wav>");
                return;
            }
            run_twist_mini(&args[2]);
        }
        "--help" | "-h" => print_usage(),
        _ => {
            eprintln!("Unknown argument: {}", args[1]);
            print_usage();
        }
    }
}

fn print_usage() {
    println!("Packet Radio RS — Benchmark Runner");
    println!();
    println!("USAGE:");
    println!("  benchmark --wav <file.wav>             Decode a single WAV file");
    println!("  benchmark --dm <file.wav>              Decode using delay-multiply demodulator");
    println!("  benchmark --dm-pll <file.wav>          DM+PLL with all variant combinations");
    println!("  benchmark --dm-pll-sweep <file.wav>    Sweep PLL alpha/beta parameters");
    println!("  benchmark --dm-pll-tune <file.wav>     Two-stage parameter tuning (Gardner shift, smoothing, LLR)");
    println!("  benchmark --dm-debug <file.wav>        Dump DM discriminator diagnostics to CSV");
    println!("  benchmark --export <wav> <dir>         Export decoded frames to files");
    println!("  benchmark --diff <file.wav>            Frame-level diff against Dire Wolf reference");
    println!("  benchmark --attribution <file.wav>     Per-decoder attribution analysis (multi-decoder)");
    println!("  benchmark --smart3 <file.wav>          Decode using Smart3 mini-decoder (3 optimal decoders)");
    println!("  benchmark --soft-diag <file.wav>       Soft decode diagnostics (per-frame LLR analysis)");
    println!("  benchmark --corr <file.wav>            Decode using correlation (mixer) demodulator");
    println!("  benchmark --corr-slicer <file.wav>    Decode using correlation multi-slicer (8 gain levels)");
    println!("  benchmark --corr-lpf-sweep <file.wav>  Sweep correlation LPF cutoff (400-1000 Hz, 50 Hz steps)");
    println!("  benchmark --corr-pll <file.wav>        Correlation + Gardner PLL timing recovery");
    println!("  benchmark --corr-pll-sweep <file.wav>  Sweep Corr+PLL alpha/error_shift parameters");
    println!("  benchmark --xor <file.wav>             Decode using binary XOR correlator");
    println!("  benchmark --twist <file.wav>           Sweep twist-tuned decoder configurations");
    println!("  benchmark --suite <directory>           Decode all WAV files, compare with Dire Wolf");
    println!("  benchmark --compare-approaches <wav>    Compare fast vs. quality path frame-by-frame");
    println!("  benchmark --synthetic                   Run synthetic signal benchmark");
    println!();
    println!("The WAV files from the WA8LMF TNC Test CD are the standard benchmark.");
    println!("Download from: http://wa8lmf.net/TNCtest/");
}

// ─── Decode Engine ───────────────────────────────────────────────────────

/// Result of decoding a WAV file with one demodulator path.
struct DecodeResult {
    frames: Vec<Vec<u8>>,
    elapsed: Duration,
}

/// Decode audio samples using the fast demodulator + hard HDLC.
fn decode_fast(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = FastDemodulator::new(config).with_adaptive_gain();
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode audio samples using the quality demodulator + soft HDLC.
fn decode_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = QualityDemodulator::new(config).with_adaptive_gain();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    let soft_recovered = soft_hdlc.stats_total_soft_recovered();
    (
        DecodeResult {
            frames,
            elapsed: start.elapsed(),
        },
        soft_recovered,
    )
}

/// Decode audio samples using the multi-decoder (9 parallel fast decoders).
fn decode_multi(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

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
fn decode_smart3(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

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
fn decode_twist_mini(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

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
fn decode_fast_adaptive(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = FastDemodulator::new(config).with_adaptive_retune().with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using the quality demodulator (with retune + hybrid LLR) + soft HDLC.
fn decode_quality_adaptive(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = QualityDemodulator::new(config);
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using the best single-decoder config from attribution analysis:
/// freq-50 Hz offset, timing phase t2 (2/3 symbol), narrow BPF,
/// adaptive retune + energy LLR + SoftHdlcDecoder.
fn decode_best_single(samples: &[i16], sample_rate: u32) -> DecodeResult {
    use packet_radio_core::modem::filter;

    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let freq_offset: i32 = -50;
    let phase_offset = 2 * sample_rate / 3; // t2
    let mark = (config.mark_freq as i32 + freq_offset) as u32;
    let space = (config.space_freq as i32 + freq_offset) as u32;

    // Use freq-shifted BPF to match the offset
    let center = (1700i32 + freq_offset) as f64;
    let bpf = filter::bandpass_coeffs(sample_rate, center, 2000.0);

    let mut demod = FastDemodulator::with_filter_freq_and_offset(
        config, bpf, phase_offset, mark, space,
    ).with_adaptive_retune().with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode with a custom single Goertzel decoder: shifted frequency + timing offset.
///
/// `freq_offset`: Hz shift applied to both mark/space frequencies and BPF center.
/// `timing_phase`: 0=t0, 1=t1 (1/3 symbol), 2=t2 (2/3 symbol).
/// `bpf_variant`: 0=narrow, 1=std, 2=wide. -1 = runtime BPF matching freq offset.
fn decode_custom_goertzel(
    samples: &[i16],
    sample_rate: u32,
    freq_offset: i32,
    timing_phase: u32,
    bpf_variant: i32,
) -> DecodeResult {
    use packet_radio_core::modem::filter;

    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

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

    let mut demod = FastDemodulator::with_filter_freq_and_offset(
        config, bpf, phase_offset, mark, space,
    );
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Resample audio to a target sample rate using linear interpolation.
/// Works for both upsampling and downsampling (e.g., 44100 → 13200).
fn resample_to(samples: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
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

/// Upsample audio 2x using linear interpolation.
fn upsample_2x(samples: &[i16]) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(samples.len() * 2);
    for i in 0..samples.len() {
        out.push(samples[i]);
        if i + 1 < samples.len() {
            // Linear interpolation between adjacent samples
            let mid = ((samples[i] as i32 + samples[i + 1] as i32) / 2) as i16;
            out.push(mid);
        } else {
            out.push(samples[i]);
        }
    }
    out
}

/// Decode audio samples using the delay-multiply demodulator + hard HDLC.
///
/// Uses BPF + LPF for real-world signals. Optionally upsamples to 22050 Hz.
fn decode_dm(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = DmDemodulator::with_bpf(config);
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode audio samples using the correlation (mixer) demodulator + hard HDLC.
fn decode_corr(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = CorrelationDemodulator::new(config).with_adaptive_gain();
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using correlation demod + energy LLR + soft HDLC.
fn decode_corr_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    let soft_recovered = soft_hdlc.stats_total_soft_recovered();
    (
        DecodeResult {
            frames,
            elapsed: start.elapsed(),
        },
        soft_recovered,
    )
}

/// Decode using correlation demod with 3 timing phases (mini multi-decoder).
///
/// Runs 3 correlation demodulators at different Bresenham phases (0, 1/3, 2/3)
/// and deduplicates results using FNV-1a hash. This is the "poor man's multi"
/// approach — 3× compute instead of 38× but captures timing diversity.
fn decode_corr_3phase(samples: &[i16], sample_rate: u32) -> DecodeResult {

    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let offsets = [0, sample_rate / 3, 2 * sample_rate / 3];

    // Phase 1: decode each timing phase independently
    let mut phase_frames: Vec<Vec<(u64, usize, Vec<u8>)>> = Vec::new(); // (hash, sample_pos, data)

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
            for i in 0..n {
                if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                    let hash = fnv1a_hash(frame);
                    frames.push((hash, sample_pos, frame.to_vec()));
                }
            }
            sample_pos += chunk.len();
        }
        phase_frames.push(frames);
    }

    // Phase 2: merge — phase[0] is primary, add unique frames from other phases
    // A frame is "duplicate" if same hash appears within ±2000 samples (~180ms)
    let mut all_frames: Vec<Vec<u8>> = Vec::new();
    let mut seen: Vec<(u64, usize)> = Vec::new(); // (hash, sample_pos)
    let dedup_window = sample_rate as usize * 2; // ±2 seconds (generous window)

    for phase in &phase_frames {
        for (hash, pos, data) in phase {
            let is_dup = seen.iter().any(|(h, p)| {
                *h == *hash && (*pos as i64 - *p as i64).unsigned_abs() < dedup_window as u64
            });
            if !is_dup {
                seen.push((*hash, *pos));
                all_frames.push(data.clone());
            }
        }
    }

    DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using correlation demod with 3 timing phases + energy LLR + soft HDLC.
/// Uses time-windowed dedup (hash + sample position) to handle repeated identical packets.
fn decode_corr_3phase_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let offsets = [0, sample_rate / 3, 2 * sample_rate / 3];

    // Phase 1: decode each timing phase independently
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
            for i in 0..n {
                if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
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

    // Phase 2: merge with time-windowed dedup
    let mut all_frames: Vec<Vec<u8>> = Vec::new();
    let mut seen: Vec<(u64, usize)> = Vec::new();
    let dedup_window = sample_rate as usize * 2;

    for phase in &phase_frames {
        for (hash, pos, data) in phase {
            let is_dup = seen.iter().any(|(h, p)| {
                *h == *hash && (*pos as i64 - *p as i64).unsigned_abs() < dedup_window as u64
            });
            if !is_dup {
                seen.push((*hash, *pos));
                all_frames.push(data.clone());
            }
        }
    }

    (DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }, total_soft)
}

/// FNV-1a hash for frame deduplication.
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Decode using fast demod with cascaded BPF.
fn decode_fast_cascade(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = FastDemodulator::new(config)
        .with_adaptive_gain()
        .with_cascade_bpf();
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    DecodeResult { frames, elapsed: start.elapsed() }
}

/// Decode using correlation demod with cascaded BPF.
fn decode_corr_cascade(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_cascade_bpf();
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    DecodeResult { frames, elapsed: start.elapsed() }
}

/// Decode using DM at 22050 Hz (upsampled if needed).
fn decode_dm_22k(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let (effective_rate, owned);
    let work_samples: &[i16] = if sample_rate <= 11025 {
        owned = upsample_2x(samples);
        effective_rate = sample_rate * 2;
        &owned
    } else {
        effective_rate = sample_rate;
        owned = Vec::new();
        let _ = &owned;
        samples
    };

    let mut config = DemodConfig::default_1200();
    config.sample_rate = effective_rate;

    let mut demod = DmDemodulator::with_bpf(config);
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in work_samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

// ─── Single WAV File Decode ────────────────────────────────────────────────

fn run_single_wav(path: &str) {
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs,
        samples.len(),
        sample_rate
    );
    println!();

    let fast = decode_fast(&samples, sample_rate);
    let (quality, qual_soft) = decode_quality(&samples, sample_rate);
    let (smart3, smart3_soft) = decode_smart3(&samples, sample_rate);
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);

    let fast_rt = duration_secs / fast.elapsed.as_secs_f64();
    let qual_rt = duration_secs / quality.elapsed.as_secs_f64();
    let smart3_rt = duration_secs / smart3.elapsed.as_secs_f64();
    let multi_rt = duration_secs / multi.elapsed.as_secs_f64();

    println!(
        "  Fast path:    {:>4} packets in {:.2}s ({:.0}x real-time)",
        fast.frames.len(),
        fast.elapsed.as_secs_f64(),
        fast_rt
    );
    println!(
        "  Quality path: {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        quality.frames.len(),
        quality.elapsed.as_secs_f64(),
        qual_rt,
        qual_soft
    );
    println!(
        "  Smart3 path:  {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        smart3.frames.len(),
        smart3.elapsed.as_secs_f64(),
        smart3_rt,
        smart3_soft
    );
    println!(
        "  Multi path:   {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        multi.frames.len(),
        multi.elapsed.as_secs_f64(),
        multi_rt,
        multi_soft
    );
    println!();
}

// ─── Smart3 Single WAV Decode ─────────────────────────────────────────────

fn run_smart3(path: &str) {
    println!("═══ Smart3 Mini-Decoder (3 attribution-optimal decoders) ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs,
        samples.len(),
        sample_rate
    );
    println!();

    let (smart3, smart3_soft) = decode_smart3(&samples, sample_rate);
    let fast = decode_fast(&samples, sample_rate);

    println!("  Smart3:  {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        smart3.frames.len(),
        smart3.elapsed.as_secs_f64(),
        duration_secs / smart3.elapsed.as_secs_f64(),
        smart3_soft);
    println!("  Fast:    {:>4} packets (baseline comparison)",
        fast.frames.len());
    let gain = smart3.frames.len() as i64 - fast.frames.len() as i64;
    println!("  Gain:    {:>+4} packets ({:.1}% improvement over fast)",
        gain,
        if fast.frames.len() > 0 { gain as f64 / fast.frames.len() as f64 * 100.0 } else { 0.0 });
    println!();
}

// ─── Soft Decode Diagnostics ─────────────────────────────────────────────

fn run_corr(path: &str) {
    println!("═══ Correlation (Mixer) Demodulator ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs, samples.len(), sample_rate
    );
    println!();

    let corr = decode_corr(&samples, sample_rate);
    let (corr_q, corr_soft) = decode_corr_quality(&samples, sample_rate);
    let corr_3p = decode_corr_3phase(&samples, sample_rate);
    let (corr_3pq, corr_3p_soft) = decode_corr_3phase_quality(&samples, sample_rate);
    let slicer = decode_corr_slicer(&samples, sample_rate);
    let slicer_3p = decode_corr_slicer_3phase(&samples, sample_rate);
    let fast = decode_fast(&samples, sample_rate);
    let (quality, qual_soft) = decode_quality(&samples, sample_rate);
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);

    println!("  Corr hard:    {:>4} packets in {:.2}s ({:.0}x real-time)",
        corr.frames.len(),
        corr.elapsed.as_secs_f64(),
        duration_secs / corr.elapsed.as_secs_f64());
    println!("  Corr quality: {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        corr_q.frames.len(),
        corr_q.elapsed.as_secs_f64(),
        duration_secs / corr_q.elapsed.as_secs_f64(),
        corr_soft);
    println!("  Corr×3 hard:  {:>4} packets in {:.2}s ({:.0}x real-time)",
        corr_3p.frames.len(),
        corr_3p.elapsed.as_secs_f64(),
        duration_secs / corr_3p.elapsed.as_secs_f64());
    println!("  Corr×3 qual:  {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        corr_3pq.frames.len(),
        corr_3pq.elapsed.as_secs_f64(),
        duration_secs / corr_3pq.elapsed.as_secs_f64(),
        corr_3p_soft);
    println!("  Slicer 8×:    {:>4} packets in {:.2}s ({:.0}x real-time)",
        slicer.frames.len(),
        slicer.elapsed.as_secs_f64(),
        duration_secs / slicer.elapsed.as_secs_f64());
    println!("  Slicer×3:     {:>4} packets in {:.2}s ({:.0}x real-time)",
        slicer_3p.frames.len(),
        slicer_3p.elapsed.as_secs_f64(),
        duration_secs / slicer_3p.elapsed.as_secs_f64());
    println!("  Fast:         {:>4} packets (Goertzel baseline)",
        fast.frames.len());
    println!("  Quality:      {:>4} packets ({} soft saves, Goertzel+Hilbert baseline)",
        quality.frames.len(), qual_soft);
    println!("  Multi (38×):  {:>4} packets ({} soft saves)",
        multi.frames.len(), multi_soft);
    let gain_hard = corr.frames.len() as i64 - fast.frames.len() as i64;
    let gain_3p = corr_3pq.frames.len() as i64 - quality.frames.len() as i64;
    let gain_slicer = slicer.frames.len() as i64 - corr.frames.len() as i64;
    println!("  Gain (single): {:>+4} packets vs fast", gain_hard);
    println!("  Gain (3-ph):   {:>+4} packets vs quality", gain_3p);
    println!("  Gain (slicer): {:>+4} packets vs corr single", gain_slicer);
    println!();
}

// ─── Correlation Multi-Slicer ───────────────────────────────────────────

/// Decode using correlation multi-slicer (single demod, N gain slicers).
fn decode_corr_slicer(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

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
fn decode_corr_slicer_3phase(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

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

    // Merge with time-windowed dedup
    let mut all_frames: Vec<Vec<u8>> = Vec::new();
    let mut seen: Vec<(u64, usize)> = Vec::new();
    let dedup_window = sample_rate as usize * 2;

    for phase in &phase_frames {
        for (hash, pos, data) in phase {
            let is_dup = seen.iter().any(|(h, p)| {
                *h == *hash && (*pos as i64 - *p as i64).unsigned_abs() < dedup_window as u64
            });
            if !is_dup {
                seen.push((*hash, *pos));
                all_frames.push(data.clone());
            }
        }
    }

    DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using corr slicer with phase scoring enabled.
fn decode_corr_slicer_phase(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut decoder = CorrSlicerDecoder::new(config)
        .with_adaptive_gain()
        .with_phase_scoring();
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

/// Decode using corr slicer with adaptive retune enabled.
fn decode_corr_slicer_retune(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut decoder = CorrSlicerDecoder::new(config)
        .with_adaptive_gain()
        .with_adaptive_retune();
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

/// Decode using corr slicer with both phase scoring and adaptive retune.
fn decode_corr_slicer_both(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut decoder = CorrSlicerDecoder::new(config)
        .with_adaptive_gain()
        .with_phase_scoring()
        .with_adaptive_retune();
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

fn run_corr_slicer(path: &str) {
    println!("═══ Correlation Multi-Slicer Demodulator ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs, samples.len(), sample_rate
    );
    println!();

    let slicer = decode_corr_slicer(&samples, sample_rate);
    let slicer_3p = decode_corr_slicer_3phase(&samples, sample_rate);
    let slicer_phase = decode_corr_slicer_phase(&samples, sample_rate);
    let slicer_retune = decode_corr_slicer_retune(&samples, sample_rate);
    let slicer_both = decode_corr_slicer_both(&samples, sample_rate);
    let fast = decode_fast(&samples, sample_rate);
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);

    println!("  Slicer 8× (base):    {:>4} packets in {:.2}s ({:.0}x RT)",
        slicer.frames.len(),
        slicer.elapsed.as_secs_f64(),
        duration_secs / slicer.elapsed.as_secs_f64());
    println!("  Slicer +phase:       {:>4} packets in {:.2}s ({:.0}x RT)  {:>+4}",
        slicer_phase.frames.len(),
        slicer_phase.elapsed.as_secs_f64(),
        duration_secs / slicer_phase.elapsed.as_secs_f64(),
        slicer_phase.frames.len() as i64 - slicer.frames.len() as i64);
    println!("  Slicer +retune:      {:>4} packets in {:.2}s ({:.0}x RT)  {:>+4}",
        slicer_retune.frames.len(),
        slicer_retune.elapsed.as_secs_f64(),
        duration_secs / slicer_retune.elapsed.as_secs_f64(),
        slicer_retune.frames.len() as i64 - slicer.frames.len() as i64);
    println!("  Slicer +both:        {:>4} packets in {:.2}s ({:.0}x RT)  {:>+4}",
        slicer_both.frames.len(),
        slicer_both.elapsed.as_secs_f64(),
        duration_secs / slicer_both.elapsed.as_secs_f64(),
        slicer_both.frames.len() as i64 - slicer.frames.len() as i64);
    println!("  Slicer 8×+3ph:       {:>4} packets in {:.2}s ({:.0}x RT)",
        slicer_3p.frames.len(),
        slicer_3p.elapsed.as_secs_f64(),
        duration_secs / slicer_3p.elapsed.as_secs_f64());
    println!("  Fast (Goertzel):     {:>4} packets",
        fast.frames.len());
    println!("  Multi (38×):         {:>4} packets ({} soft saves)",
        multi.frames.len(), multi_soft);
    println!();
}

// ─── Correlation LPF Sweep ──────────────────────────────────────────────

fn run_corr_lpf_sweep(path: &str) {
    use packet_radio_core::modem::filter::lowpass_coeffs;

    println!("═══ Correlation LPF Cutoff Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz", duration_secs, samples.len(), sample_rate);
    println!();

    // Baseline: default 500 Hz cutoff (tone_separation / 2)
    let corr_baseline = decode_corr(&samples, sample_rate);
    let (corr_q_baseline, soft_baseline) = decode_corr_quality(&samples, sample_rate);
    println!("Baseline (500 Hz): {} hard, {} quality ({} soft saves)",
        corr_baseline.frames.len(), corr_q_baseline.frames.len(), soft_baseline);
    println!();

    let cutoffs = [400.0, 450.0, 500.0, 550.0, 600.0, 650.0, 700.0, 750.0,
                   800.0, 850.0, 900.0, 950.0, 1000.0];

    println!("{:<10} {:>8} {:>8} {:>8}", "Cutoff", "Hard", "Quality", "Soft");
    println!("{}", "─".repeat(40));

    for &cutoff in &cutoffs {
        let lpf = lowpass_coeffs(sample_rate, cutoff, 0.707);

        // Hard decode
        let mut config = DemodConfig::default_1200();
        config.sample_rate = sample_rate;
        let mut demod = CorrelationDemodulator::new(config)
            .with_adaptive_gain()
            .with_corr_lpf(lpf);
        let mut hdlc = HdlcDecoder::new();
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                    frames.push(frame.to_vec());
                }
            }
        }
        let hard_count = frames.len();

        // Quality decode
        let lpf2 = lowpass_coeffs(sample_rate, cutoff, 0.707);
        let mut demod2 = CorrelationDemodulator::new(config)
            .with_adaptive_gain()
            .with_energy_llr()
            .with_corr_lpf(lpf2);
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut frames2: Vec<Vec<u8>> = Vec::new();
        let mut symbols2 = [DemodSymbol { bit: false, llr: 0 }; 1024];
        for chunk in samples.chunks(1024) {
            let n = demod2.process_samples(chunk, &mut symbols2);
            for i in 0..n {
                if let Some(result) = soft_hdlc.feed_soft_bit(symbols2[i].llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => d,
                        FrameResult::Recovered { data, .. } => data,
                    };
                    frames2.push(data.to_vec());
                }
            }
        }
        let quality_count = frames2.len();
        let soft_saves = soft_hdlc.stats_total_soft_recovered();

        println!("{:<10} {:>8} {:>8} {:>8}", format!("{:.0} Hz", cutoff), hard_count, quality_count, soft_saves);
    }

    println!();
}

// ─── Correlation + PLL ─────────────────────────────────────────────────

/// Decode using correlation demod + Gardner PLL timing recovery.
fn decode_corr_pll(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_pll();
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    DecodeResult { frames, elapsed: start.elapsed() }
}

/// Decode using correlation demod + Gardner PLL + energy LLR + soft HDLC.
fn decode_corr_pll_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr()
        .with_pll();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
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

/// Decode using correlation demod + PLL with custom alpha and error_shift.
fn decode_corr_pll_custom(samples: &[i16], sample_rate: u32, alpha: i16, error_shift: u8) -> DecodeResult {
    use packet_radio_core::modem::pll::ClockRecoveryPll;

    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let pll = ClockRecoveryPll::new_gardner(sample_rate, 1200, alpha, 0)
        .with_error_shift(error_shift);
    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_custom_pll(pll);
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    DecodeResult { frames, elapsed: start.elapsed() }
}

/// Decode using 2-phase correlation demod (two timing phases, dedup).
fn decode_corr_2phase(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let offsets = [0, sample_rate / 2];
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
            for i in 0..n {
                if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                    let hash = fnv1a_hash(frame);
                    frames.push((hash, sample_pos, frame.to_vec()));
                }
            }
            sample_pos += chunk.len();
        }
        phase_frames.push(frames);
    }

    let mut all_frames: Vec<Vec<u8>> = Vec::new();
    let mut seen: Vec<(u64, usize)> = Vec::new();
    let dedup_window = sample_rate as usize * 2;

    for phase in &phase_frames {
        for (hash, pos, data) in phase {
            let is_dup = seen.iter().any(|(h, p)| {
                *h == *hash && (*pos as i64 - *p as i64).unsigned_abs() < dedup_window as u64
            });
            if !is_dup {
                seen.push((*hash, *pos));
                all_frames.push(data.clone());
            }
        }
    }

    DecodeResult { frames: all_frames, elapsed: start.elapsed() }
}

/// Decode using 2-phase correlation demod + PLL per phase + dedup.
fn decode_corr_2phase_pll(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let offsets = [0, sample_rate / 2];
    let mut phase_frames: Vec<Vec<(u64, usize, Vec<u8>)>> = Vec::new();

    let start = Instant::now();

    for &offset in &offsets {
        let mut demod = CorrelationDemodulator::new(config)
            .with_adaptive_gain()
            .with_pll();
        demod.set_bit_phase(offset);
        let mut hdlc = HdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        let mut frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
        let mut sample_pos: usize = 0;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                    let hash = fnv1a_hash(frame);
                    frames.push((hash, sample_pos, frame.to_vec()));
                }
            }
            sample_pos += chunk.len();
        }
        phase_frames.push(frames);
    }

    let mut all_frames: Vec<Vec<u8>> = Vec::new();
    let mut seen: Vec<(u64, usize)> = Vec::new();
    let dedup_window = sample_rate as usize * 2;

    for phase in &phase_frames {
        for (hash, pos, data) in phase {
            let is_dup = seen.iter().any(|(h, p)| {
                *h == *hash && (*pos as i64 - *p as i64).unsigned_abs() < dedup_window as u64
            });
            if !is_dup {
                seen.push((*hash, *pos));
                all_frames.push(data.clone());
            }
        }
    }

    DecodeResult { frames: all_frames, elapsed: start.elapsed() }
}

fn run_corr_pll(path: &str) {
    println!("═══ Correlation + PLL Demodulator ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz", duration_secs, samples.len(), sample_rate);
    println!();

    let corr = decode_corr(&samples, sample_rate);
    let (corr_q, corr_soft) = decode_corr_quality(&samples, sample_rate);
    let corr_pll = decode_corr_pll(&samples, sample_rate);
    let (corr_pll_q, corr_pll_soft) = decode_corr_pll_quality(&samples, sample_rate);
    let corr_2p = decode_corr_2phase(&samples, sample_rate);
    let corr_2p_pll = decode_corr_2phase_pll(&samples, sample_rate);
    let corr_3p = decode_corr_3phase(&samples, sample_rate);

    println!("  Corr hard:      {:>4} packets (Bresenham baseline)", corr.frames.len());
    println!("  Corr quality:   {:>4} packets ({} soft saves)", corr_q.frames.len(), corr_soft);
    println!("  Corr+PLL hard:  {:>4} packets (Gardner PLL)", corr_pll.frames.len());
    println!("  Corr+PLL qual:  {:>4} packets ({} soft saves)", corr_pll_q.frames.len(), corr_pll_soft);
    println!("  Corr×2 hard:    {:>4} packets (2-phase diversity)", corr_2p.frames.len());
    println!("  Corr×2+PLL:     {:>4} packets (2-phase + PLL)", corr_2p_pll.frames.len());
    println!("  Corr×3 hard:    {:>4} packets (3-phase diversity)", corr_3p.frames.len());
    println!();
    let pll_gain = corr_pll.frames.len() as i64 - corr.frames.len() as i64;
    let pll_q_gain = corr_pll_q.frames.len() as i64 - corr_q.frames.len() as i64;
    println!("  PLL gain (hard):    {:>+4}", pll_gain);
    println!("  PLL gain (quality): {:>+4}", pll_q_gain);
    println!("  3-phase gap closed: {:.0}%",
        if corr_3p.frames.len() > corr.frames.len() {
            pll_gain as f64 / (corr_3p.frames.len() as f64 - corr.frames.len() as f64) * 100.0
        } else { 0.0 });
    println!();
}

fn run_corr_pll_sweep(path: &str) {
    println!("═══ Correlation PLL Parameter Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("Duration: {:.1}s, {} samples at {} Hz",
        samples.len() as f64 / sample_rate as f64, samples.len(), sample_rate);
    println!();

    // Baseline
    let corr = decode_corr(&samples, sample_rate);
    println!("Baseline (Bresenham): {} packets", corr.frames.len());
    println!();

    let alphas: &[i16] = &[200, 400, 600, 800, 936, 1200, 1600];
    let error_shifts: &[u8] = &[6, 7, 8, 9, 10];

    println!("{:<8} {:>8} {:>8} {:>8} {:>8} {:>8}", "alpha", "es=6", "es=7", "es=8", "es=9", "es=10");
    println!("{}", "─".repeat(56));

    let mut best_count = 0usize;
    let mut best_alpha = 0i16;
    let mut best_shift = 0u8;

    for &alpha in alphas {
        print!("{:<8}", alpha);
        for &es in error_shifts {
            let result = decode_corr_pll_custom(&samples, sample_rate, alpha, es);
            let count = result.frames.len();
            print!(" {:>8}", count);
            if count > best_count {
                best_count = count;
                best_alpha = alpha;
                best_shift = es;
            }
        }
        println!();
    }

    println!("{}", "─".repeat(56));
    println!("Best: alpha={}, error_shift={} → {} packets ({:+} vs Bresenham)",
        best_alpha, best_shift, best_count,
        best_count as i64 - corr.frames.len() as i64);
    println!();
}

fn run_soft_diag(path: &str) {
    println!("═══ Soft Decode Diagnostics ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz", duration_secs, samples.len(), sample_rate);
    println!();

    // Run quality single-decoder with detailed stats
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;
    let mut demod = QualityDemodulator::new(config);
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    println!("=== Quality Single-Decoder Stats ===");
    println!("  Decoded frames:      {:>5}", frames.len());
    println!("  Hard decodes:        {:>5}", soft_hdlc.stats_hard_decode);
    println!("  CRC failures:        {:>5}", soft_hdlc.stats_crc_failures);
    println!("  Soft recovered:      {:>5} total", soft_hdlc.stats_total_soft_recovered());
    println!("    Syndrome 1-bit:    {:>5}", soft_hdlc.stats_syndrome);
    println!("    Single flip:       {:>5}", soft_hdlc.stats_single_flip);
    println!("    Pair flip:         {:>5}", soft_hdlc.stats_pair_flip);
    println!("    NRZI pair:         {:>5}", soft_hdlc.stats_nrzi_pair);
    println!("    Triple flip:       {:>5}", soft_hdlc.stats_triple_flip);
    println!("    NRZI triple:       {:>5}", soft_hdlc.stats_nrzi_triple);
    println!("  False positives:     {:>5}", soft_hdlc.stats_false_positives);
    println!();

    // Run multi-decoder with soft stats
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);
    let (smart3, smart3_soft) = decode_smart3(&samples, sample_rate);

    println!("=== Multi-Decoder Soft Stats ===");
    println!("  Multi decoded:       {:>5} ({} soft saves)", multi.frames.len(), multi_soft);
    println!("  Smart3 decoded:      {:>5} ({} soft saves)", smart3.frames.len(), smart3_soft);
    println!();

    // Run fast+adaptive single decoder with energy LLR
    let fast_adapt = decode_fast_adaptive(&samples, sample_rate);
    println!("=== Adaptive Goertzel ===");
    println!("  Fast+adapt decoded:  {:>5}", fast_adapt.frames.len());
    println!();

    // Load DW reference for comparison if available
    let dw_ref = discover_dw_reference(path);
    if let Some((pkt_path, _)) = dw_ref {
        if let Ok(dw_packets) = load_dw_packets(&pkt_path) {
            let dw_set: std::collections::HashSet<String> = dw_packets.into_iter().collect();
            let multi_tnc2 = frames_to_tnc2(&multi.frames);
            let multi_set: std::collections::HashSet<&str> = multi_tnc2.iter().map(|s| s.as_str()).collect();
            let dw_only = dw_set.iter().filter(|p| !multi_set.contains(p.as_str())).count();

            println!("=== vs Dire Wolf ===");
            println!("  DW unique:           {:>5}", dw_set.len());
            println!("  Multi overlap:       {:>5}", dw_set.iter().filter(|p| multi_set.contains(p.as_str())).count());
            println!("  DW-only (we miss):   {:>5}", dw_only);
            println!();
        }
    }
}

// ─── DM Single WAV Decode ────────────────────────────────────────────────

/// Decode DM without BPF/LPF (raw discriminator).
fn decode_dm_raw(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = DmDemodulator::new(config); // no BPF
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    DecodeResult { frames, elapsed: start.elapsed() }
}

/// Decode with DM using a specific delay value and optional BPF/LPF.
fn decode_dm_custom(samples: &[i16], sample_rate: u32, delay: usize, use_bpf: bool) -> DecodeResult {
    use packet_radio_core::modem::delay_multiply::DelayMultiplyDetector;
    use packet_radio_core::modem::filter::BiquadFilter;

    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let bpf = if use_bpf {
        Some(match sample_rate {
            22050 => packet_radio_core::modem::filter::afsk_bandpass_22050(),
            44100 => packet_radio_core::modem::filter::afsk_bandpass_44100(),
            _ => packet_radio_core::modem::filter::afsk_bandpass_11025(),
        })
    } else {
        None
    };

    let lpf = if use_bpf {
        packet_radio_core::modem::filter::post_detect_lpf(sample_rate)
    } else {
        BiquadFilter::passthrough()
    };

    let mut detector = DelayMultiplyDetector::with_delay(delay, lpf);
    let mut bpf_filt = bpf;
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let baud_rate = config.baud_rate;
    let mut bit_phase: u32 = 0;
    let mut accumulator: i64 = 0;
    let mut prev_nrzi_bit = false;

    // Determine polarity: delays giving τ≈363μs have mark→negative
    // Short delays have mark→positive. Use the cos formula to determine.
    let tau = delay as f64 / sample_rate as f64;
    let mark_cos = (2.0 * std::f64::consts::PI * 1200.0 * tau).cos();
    let mark_is_negative = mark_cos < 0.0;

    let start = Instant::now();

    for &sample in samples {
        let filtered = if let Some(ref mut f) = bpf_filt {
            f.process(sample)
        } else {
            sample
        };
        let disc_out = detector.process(filtered);
        accumulator += disc_out as i64;

        bit_phase += baud_rate;
        if bit_phase >= sample_rate {
            bit_phase -= sample_rate;

            let raw_bit = if mark_is_negative {
                accumulator < 0  // mark gives negative output
            } else {
                accumulator > 0  // mark gives positive output
            };

            let decoded_bit = raw_bit == prev_nrzi_bit;
            prev_nrzi_bit = raw_bit;

            if let Some(frame) = hdlc.feed_bit(decoded_bit) {
                frames.push(frame.to_vec());
            }

            accumulator = 0;
        }
    }

    DecodeResult { frames, elapsed: start.elapsed() }
}

fn run_dm_single(path: &str) {
    println!("═══ Delay-Multiply Demodulator — Delay Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs, samples.len(), sample_rate
    );
    println!();

    let fast = decode_fast(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);
    println!("  Fast:   {:>4}   Multi: {:>4}", fast.frames.len(), multi.frames.len());
    println!();

    // Sweep delays with BPF+LPF
    println!("  Delay sweep at {} Hz — BPF+LPF:", sample_rate);
    for delay in 1..16 {
        let tau_us = delay as f64 / sample_rate as f64 * 1e6;
        let mark_cos = (2.0 * std::f64::consts::PI * 1200.0 * delay as f64 / sample_rate as f64).cos();
        let space_cos = (2.0 * std::f64::consts::PI * 2200.0 * delay as f64 / sample_rate as f64).cos();
        let sep = (mark_cos - space_cos).abs();
        let polarity = if mark_cos < 0.0 { "M-" } else { "M+" };
        let result = decode_dm_custom(&samples, sample_rate, delay, true);
        println!("    d={:>2} τ={:>5.0}μs sep={:.2} {} → {:>4} packets",
            delay, tau_us, sep, polarity, result.frames.len());
    }
    println!();

    // Also sweep without BPF+LPF
    println!("  Delay sweep at {} Hz — no filters:", sample_rate);
    for delay in 1..16 {
        let tau_us = delay as f64 / sample_rate as f64 * 1e6;
        let mark_cos = (2.0 * std::f64::consts::PI * 1200.0 * delay as f64 / sample_rate as f64).cos();
        let space_cos = (2.0 * std::f64::consts::PI * 2200.0 * delay as f64 / sample_rate as f64).cos();
        let sep = (mark_cos - space_cos).abs();
        let polarity = if mark_cos < 0.0 { "M-" } else { "M+" };
        let result = decode_dm_custom(&samples, sample_rate, delay, false);
        println!("    d={:>2} τ={:>5.0}μs sep={:.2} {} → {:>4} packets",
            delay, tau_us, sep, polarity, result.frames.len());
    }
}

// ─── Benchmark Suite (All WAV Files) ───────────────────────────────────────

/// Entry parsed from Dire Wolf summary.csv
struct DireWolfEntry {
    track_file: String,
    decoded_packets: u32,
}

/// Load Dire Wolf reference data from summary.csv.
fn load_direwolf_csv(dir: &str) -> Vec<DireWolfEntry> {
    // Try both relative to suite dir and well-known path
    let candidates = [
        format!("{}/../../iso/direwolf_review/summary.csv", dir),
        "iso/direwolf_review/summary.csv".to_string(),
    ];

    for path in &candidates {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let mut entries = Vec::new();
            for line in contents.lines().skip(1) {
                // CSV: track_file,decoded_packets,decode_time_s,realtime_x
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

/// Extract track name from filename for display and matching.
fn track_display_name(path: &str) -> String {
    let fname = std::path::Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    // Truncate to fit table column (30 chars)
    if fname.len() > 30 {
        fname[..30].to_string()
    } else {
        fname
    }
}

/// Extract WAV filename from path for matching with DW CSV.
fn wav_filename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

// ─── TwistMini Decoder ─────────────────────────────────────────────────

fn run_twist_mini(path: &str) {
    println!("═══ TwistMini Decoder (Smart3 + 3 twist-compensated = 6 decoders) ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz\n",
        duration_secs, samples.len(), sample_rate
    );

    // --- Native sample rate ---
    let fast = decode_fast(&samples, sample_rate);
    let (smart3, _) = decode_smart3(&samples, sample_rate);
    let (twist_mini, _) = decode_twist_mini(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);

    println!("At {} Hz:", sample_rate);
    println!("  Fast (1×):       {:>4} packets in {:.2}s ({:.0}x real-time)",
        fast.frames.len(), fast.elapsed.as_secs_f64(),
        duration_secs / fast.elapsed.as_secs_f64());
    println!("  Smart3 (3×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
        smart3.frames.len(), smart3.elapsed.as_secs_f64(),
        duration_secs / smart3.elapsed.as_secs_f64());
    println!("  TwistMini (6×):  {:>4} packets in {:.2}s ({:.0}x real-time)",
        twist_mini.frames.len(), twist_mini.elapsed.as_secs_f64(),
        duration_secs / twist_mini.elapsed.as_secs_f64());
    println!("  Multi (38×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
        multi.frames.len(), multi.elapsed.as_secs_f64(),
        duration_secs / multi.elapsed.as_secs_f64());

    let gain_vs_smart3 = twist_mini.frames.len() as i64 - smart3.frames.len() as i64;
    let gain_vs_multi = twist_mini.frames.len() as i64 - multi.frames.len() as i64;
    println!("  TwistMini vs Smart3: {:>+4} packets", gain_vs_smart3);
    println!("  TwistMini vs Multi:  {:>+4} packets", gain_vs_multi);

    // --- Try 13200 Hz variant (pre-resampled WAV or runtime resample) ---
    let target_rate = 13200u32;
    if sample_rate != target_rate {
        println!();
        // Try to find a pre-resampled WAV file (e.g., foo_13200.wav)
        let path_13k = path.replace(".wav", "_13200.wav");
        let (rate_13k, samples_13k) = if let Ok(v) = read_wav_file(&path_13k) {
            println!("At {} Hz (from {}):", target_rate, path_13k);
            v
        } else {
            println!("At {} Hz (resampled from {}):", target_rate, sample_rate);
            (target_rate, resample_to(&samples, sample_rate, target_rate))
        };

        let fast_13k = decode_fast(&samples_13k, rate_13k);
        let (smart3_13k, _) = decode_smart3(&samples_13k, rate_13k);
        let (twist_mini_13k, _) = decode_twist_mini(&samples_13k, rate_13k);
        let (multi_13k, _) = decode_multi(&samples_13k, rate_13k);

        let dur_13k = samples_13k.len() as f64 / rate_13k as f64;
        println!("  Fast (1×):       {:>4} packets in {:.2}s ({:.0}x real-time)",
            fast_13k.frames.len(), fast_13k.elapsed.as_secs_f64(),
            dur_13k / fast_13k.elapsed.as_secs_f64());
        println!("  Smart3 (3×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
            smart3_13k.frames.len(), smart3_13k.elapsed.as_secs_f64(),
            dur_13k / smart3_13k.elapsed.as_secs_f64());
        println!("  TwistMini (6×):  {:>4} packets in {:.2}s ({:.0}x real-time)",
            twist_mini_13k.frames.len(), twist_mini_13k.elapsed.as_secs_f64(),
            dur_13k / twist_mini_13k.elapsed.as_secs_f64());
        println!("  Multi (38×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
            multi_13k.frames.len(), multi_13k.elapsed.as_secs_f64(),
            dur_13k / multi_13k.elapsed.as_secs_f64());

        let gain_13k = twist_mini_13k.frames.len() as i64 - smart3_13k.frames.len() as i64;
        let gain_vs_native = twist_mini_13k.frames.len() as i64 - twist_mini.frames.len() as i64;
        println!("  TwistMini@13k vs Smart3@13k: {:>+4} packets", gain_13k);
        println!("  TwistMini@13k vs TwistMini@native: {:>+4} packets", gain_vs_native);
    }

    // --- Try 48000 Hz variant ---
    {
        let target_48k = 48000u32;
        let path_48k = path.replace(".wav", "_48000.wav");
        let loaded = read_wav_file(&path_48k);
        if let Ok((rate_48k, samples_48k)) = loaded {
            println!();
            println!("At {} Hz (from {}):", rate_48k, path_48k);

            let fast_48k = decode_fast(&samples_48k, rate_48k);
            let (smart3_48k, _) = decode_smart3(&samples_48k, rate_48k);
            let (twist_mini_48k, _) = decode_twist_mini(&samples_48k, rate_48k);
            let (multi_48k, _) = decode_multi(&samples_48k, rate_48k);

            let dur_48k = samples_48k.len() as f64 / rate_48k as f64;
            println!("  Fast (1×):       {:>4} packets in {:.2}s ({:.0}x real-time)",
                fast_48k.frames.len(), fast_48k.elapsed.as_secs_f64(),
                dur_48k / fast_48k.elapsed.as_secs_f64());
            println!("  Smart3 (3×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
                smart3_48k.frames.len(), smart3_48k.elapsed.as_secs_f64(),
                dur_48k / smart3_48k.elapsed.as_secs_f64());
            println!("  TwistMini (6×):  {:>4} packets in {:.2}s ({:.0}x real-time)",
                twist_mini_48k.frames.len(), twist_mini_48k.elapsed.as_secs_f64(),
                dur_48k / twist_mini_48k.elapsed.as_secs_f64());
            println!("  Multi (38×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
                multi_48k.frames.len(), multi_48k.elapsed.as_secs_f64(),
                dur_48k / multi_48k.elapsed.as_secs_f64());

            let gain_48k = twist_mini_48k.frames.len() as i64 - smart3_48k.frames.len() as i64;
            let gain_vs_native = twist_mini_48k.frames.len() as i64 - twist_mini.frames.len() as i64;
            println!("  TwistMini@48k vs Smart3@48k: {:>+4} packets", gain_48k);
            println!("  TwistMini@48k vs TwistMini@native: {:>+4} packets", gain_vs_native);
        }
    }
    println!();
}

// ─── Twist-Tuned Decoder Sweep ──────────────────────────────────────────

/// Decode with a twist-tuned single Goertzel decoder.
///
/// `bpf_center_offset`: Hz shift of BPF center relative to 1700 Hz midpoint.
///   Positive = favor space (compensate de-emphasis), negative = favor mark.
/// `space_gain_q8`: Static space energy gain (256 = 0 dB).
/// `timing_phase`: 0/1/2 (0, 1/3, 2/3 symbol offset).
fn decode_twist(
    samples: &[i16],
    sample_rate: u32,
    bpf_center_offset: i32,
    space_gain_q8: u16,
    timing_phase: u32,
) -> DecodeResult {
    use packet_radio_core::modem::filter;

    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let phase_offset = timing_phase * sample_rate / 3;

    // Shift the BPF center to favor one tone over the other
    let bpf = if bpf_center_offset != 0 {
        let center = (1700i32 + bpf_center_offset) as f64;
        filter::bandpass_coeffs(sample_rate, center, 2000.0)
    } else {
        filter::afsk_bandpass_11025()
    };

    let mut demod = FastDemodulator::with_filter_and_offset(config, bpf, phase_offset)
        .with_space_gain(space_gain_q8)
        .with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

fn run_twist_sweep(path: &str) {
    println!("═══ Twist-Tuned Decoder Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz\n",
        duration_secs, samples.len(), sample_rate
    );

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (smart3, _) = decode_smart3(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);

    println!("  Baselines:");
    println!("    Fast:     {:>4}", fast.frames.len());
    println!("    Smart3:   {:>4}", smart3.frames.len());
    println!("    Multi:    {:>4}\n", multi.frames.len());

    // Build Smart3 frame set for exclusive analysis
    use std::collections::HashSet;
    let smart3_set: HashSet<Vec<u8>> = smart3.frames.iter().cloned().collect();

    // Sweep parameters:
    // BPF center offsets: -200, -100, 0, +100, +200 Hz
    // Space gains (Q8): 128 (-3dB), 181 (-1.5dB), 256 (0dB), 362 (+1.5dB), 512 (+3dB), 868 (+5.3dB)
    // Timing phases: 0, 1, 2
    let bpf_offsets = [-200i32, -100, 0, 100, 200, 300];
    let gains: [(u16, &str); 6] = [
        (128, "-3dB"), (181, "-1.5dB"), (256, "0dB"),
        (362, "+1.5dB"), (512, "+3dB"), (868, "+5.3dB"),
    ];
    let timing_phases = [0u32, 1, 2];

    println!("  {:>8} {:>7} {:>3}  {:>5}  {:>5}  not-in-S3", "BPF_off", "Gain", "T", "Pkts", "Uniq");
    println!("  {}", "─".repeat(52));

    // Collect results for ranking
    struct TwistResult {
        bpf_off: i32,
        gain_label: String,
        timing: u32,
        count: usize,
        unique: usize,
        exclusive: usize,
    }
    let mut results: Vec<TwistResult> = Vec::new();

    for &bpf_off in &bpf_offsets {
        for &(gain_q8, gain_label) in &gains {
            for &t in &timing_phases {
                let r = decode_twist(&samples, sample_rate, bpf_off, gain_q8, t);
                let r_set: HashSet<Vec<u8>> = r.frames.iter().cloned().collect();
                let exclusive = r_set.difference(&smart3_set).count();

                results.push(TwistResult {
                    bpf_off,
                    gain_label: gain_label.to_string(),
                    timing: t,
                    count: r.frames.len(),
                    unique: r_set.len(),
                    exclusive,
                });
            }
        }
    }

    // Sort by exclusive frames (descending), then by total count
    results.sort_by(|a, b| b.exclusive.cmp(&a.exclusive).then(b.count.cmp(&a.count)));

    for r in &results {
        let marker = if r.exclusive > 0 { " ★" } else { "" };
        println!("  {:>+5} Hz {:>7} t{}  {:>5}  {:>5}  {:>4}{}",
            r.bpf_off, r.gain_label, r.timing, r.count, r.unique, r.exclusive, marker);
    }

    // Top candidates summary
    println!();
    let top: Vec<&TwistResult> = results.iter().filter(|r| r.exclusive > 0).collect();
    if top.is_empty() {
        println!("  No twist configuration found exclusive frames vs Smart3.");
    } else {
        println!("  Top twist configurations with exclusive frames vs Smart3:");
        for r in top.iter().take(10) {
            println!("    BPF{:>+4}Hz gain={} t{}: {} exclusive ({} total)",
                r.bpf_off, r.gain_label, r.timing, r.exclusive, r.count);
        }
    }

    // Also test: what if we add best twist decoders to Smart3?
    println!();
    if !top.is_empty() {
        // Combine Smart3 + top 3 twist configs
        let mut combined_set = smart3_set.clone();
        let mut added = 0;
        for r in top.iter().take(3) {
            let tr = decode_twist(&samples, sample_rate, r.bpf_off,
                gains.iter().find(|(_, l)| l == &r.gain_label).unwrap().0, r.timing);
            for f in &tr.frames {
                combined_set.insert(f.clone());
            }
            added += 1;
        }
        println!("  Smart3 + top {} twist decoders: {} unique (Smart3 alone: {})",
            added, combined_set.len(), smart3.frames.iter().cloned().collect::<HashSet<_>>().len());
    }
    println!();
}

// ─── Binary XOR Correlator ──────────────────────────────────────────────

/// Decode using Binary XOR correlator + hard HDLC.
fn decode_xor(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = BinaryXorDemodulator::new(config);
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using Binary XOR correlator + energy LLR + soft HDLC.
fn decode_xor_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = BinaryXorDemodulator::new(config).with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
    let mut soft_saves: u32 = 0;

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => {
                        soft_saves += 1;
                        data
                    }
                };
                frames.push(data.to_vec());
            }
        }
    }

    (DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }, soft_saves)
}

fn run_xor(path: &str) {
    println!("═══ Binary XOR Correlator ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs, samples.len(), sample_rate
    );
    println!();

    let xor = decode_xor(&samples, sample_rate);
    let (xor_q, xor_soft) = decode_xor_quality(&samples, sample_rate);
    let fast = decode_fast(&samples, sample_rate);
    let dm = decode_dm(&samples, sample_rate);

    println!("  XOR hard:     {:>4} packets in {:.2}s ({:.0}x real-time)",
        xor.frames.len(),
        xor.elapsed.as_secs_f64(),
        duration_secs / xor.elapsed.as_secs_f64());
    println!("  XOR quality:  {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        xor_q.frames.len(),
        xor_q.elapsed.as_secs_f64(),
        duration_secs / xor_q.elapsed.as_secs_f64(),
        xor_soft);
    println!("  Fast:         {:>4} packets (Goertzel baseline)",
        fast.frames.len());
    println!("  DM:           {:>4} packets (delay-multiply baseline)",
        dm.frames.len());
    let gain_hard = xor.frames.len() as i64 - fast.frames.len() as i64;
    let gain_dm = xor.frames.len() as i64 - dm.frames.len() as i64;
    println!("  Gain vs fast: {:>+4} packets", gain_hard);
    println!("  Gain vs DM:   {:>+4} packets", gain_dm);

    // Exclusive frame analysis: compare XOR vs MCU-feasible decoders
    let (smart3, _) = decode_smart3(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);

    use std::collections::HashSet;
    let xor_set: HashSet<Vec<u8>> = xor.frames.iter().cloned().collect();
    let xor_q_set: HashSet<Vec<u8>> = xor_q.frames.iter().cloned().collect();
    let smart3_set: HashSet<Vec<u8>> = smart3.frames.iter().cloned().collect();
    let multi_set: HashSet<Vec<u8>> = multi.frames.iter().cloned().collect();
    let fast_set: HashSet<Vec<u8>> = fast.frames.iter().cloned().collect();
    let dm_set: HashSet<Vec<u8>> = dm.frames.iter().cloned().collect();

    let xor_not_in_smart3 = xor_set.difference(&smart3_set).count();
    let xor_q_not_in_smart3 = xor_q_set.difference(&smart3_set).count();
    let xor_not_in_fast = xor_set.difference(&fast_set).count();
    let xor_not_in_multi = xor_set.difference(&multi_set).count();
    // XOR frames not in any MCU-feasible single decoder
    let mcu_union: HashSet<Vec<u8>> = fast_set.union(&dm_set).cloned().collect();
    let xor_not_in_mcu_singles = xor_set.difference(&mcu_union).count();
    // Smart3 + XOR combined
    let smart3_xor: HashSet<Vec<u8>> = smart3_set.union(&xor_set).cloned().collect();

    println!();
    println!("  Exclusive frame analysis:");
    println!("    XOR unique frames:        {:>4}", xor_set.len());
    println!("    Smart3 unique frames:     {:>4}", smart3_set.len());
    println!("    Multi unique frames:      {:>4}", multi_set.len());
    println!("    XOR not in Smart3:        {:>4}  ← MCU-relevant exclusives", xor_not_in_smart3);
    println!("    XOR qual not in Smart3:   {:>4}", xor_q_not_in_smart3);
    println!("    XOR not in Fast+DM:       {:>4}", xor_not_in_mcu_singles);
    println!("    XOR not in Multi:         {:>4}", xor_not_in_multi);
    println!("    Smart3+XOR combined:      {:>4}  (Smart3 alone: {})", smart3_xor.len(), smart3_set.len());
    println!();
}

fn run_suite(dir: &str) {
    println!("═══ WA8LMF TNC Test CD Benchmark Suite ═══");
    println!();

    let mut wav_files: Vec<String> = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "wav").unwrap_or(false) {
                    wav_files.push(path.to_string_lossy().to_string());
                }
            }
        }
        Err(e) => {
            eprintln!("Error reading directory {}: {}", dir, e);
            return;
        }
    }

    wav_files.sort();

    if wav_files.is_empty() {
        println!("No WAV files found in {}", dir);
        println!("Download test files from http://wa8lmf.net/TNCtest/");
        return;
    }

    // Load Dire Wolf baseline
    let dw_entries = load_direwolf_csv(dir);
    let have_dw = !dw_entries.is_empty();

    // Decode all tracks
    struct TrackResult {
        display_name: String,
        fast_count: usize,
        quality_count: usize,
        smart3_count: usize,
        multi_count: usize,
        dm_count: usize,
        soft_saves: u32,
        smart3_soft: u32,
        multi_soft: u32,
        dw_count: Option<u32>,
        fast_elapsed: Duration,
        qual_elapsed: Duration,
        smart3_elapsed: Duration,
        multi_elapsed: Duration,
        dm_elapsed: Duration,
        duration_secs: f64,
    }

    let mut results: Vec<TrackResult> = Vec::new();

    for wav_path in &wav_files {
        let (sample_rate, samples) = match read_wav_file(wav_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  Error reading {}: {}", wav_path, e);
                continue;
            }
        };

        let duration_secs = samples.len() as f64 / sample_rate as f64;
        let display = track_display_name(wav_path);
        eprint!("  Decoding {}... ", display);

        let fast = decode_fast(&samples, sample_rate);
        let (quality, qual_soft) = decode_quality(&samples, sample_rate);
        let (smart3, smart3_soft) = decode_smart3(&samples, sample_rate);
        let (multi, multi_soft) = decode_multi(&samples, sample_rate);
        let dm = decode_dm(&samples, sample_rate);

        eprintln!(
            "fast={}, quality={}, smart3={}, multi={}, dm={}",
            fast.frames.len(),
            quality.frames.len(),
            smart3.frames.len(),
            multi.frames.len(),
            dm.frames.len()
        );

        // Match against Dire Wolf data
        let fname = wav_filename(wav_path);
        let dw_count = dw_entries
            .iter()
            .find(|e| e.track_file == fname)
            .map(|e| e.decoded_packets);

        results.push(TrackResult {
            display_name: display,
            fast_count: fast.frames.len(),
            quality_count: quality.frames.len(),
            smart3_count: smart3.frames.len(),
            multi_count: multi.frames.len(),
            dm_count: dm.frames.len(),
            soft_saves: qual_soft,
            smart3_soft,
            multi_soft,
            dw_count,
            fast_elapsed: fast.elapsed,
            qual_elapsed: quality.elapsed,
            smart3_elapsed: smart3.elapsed,
            multi_elapsed: multi.elapsed,
            dm_elapsed: dm.elapsed,
            duration_secs,
        });
    }

    println!();

    // Print comparison table
    if have_dw {
        println!(
            "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
            "Track", "DireWolf", "Fast", "Quality", "Smart3", "Multi", "DM", "Fast%", "Smart3%", "Multi%"
        );
        println!("{}", "─".repeat(118));
    } else {
        println!(
            "{:<30} {:>7} {:>7} {:>7} {:>7} {:>7} {:>5}",
            "Track", "Fast", "Quality", "Smart3", "Multi", "DM", "Saves"
        );
        println!("{}", "─".repeat(75));
    }

    let mut total_fast = 0usize;
    let mut total_quality = 0usize;
    let mut total_smart3 = 0usize;
    let mut total_multi = 0usize;
    let mut total_dm = 0usize;
    let mut total_dw = 0u32;
    let mut total_saves = 0u32;

    for r in &results {
        total_fast += r.fast_count;
        total_quality += r.quality_count;
        total_smart3 += r.smart3_count;
        total_multi += r.multi_count;
        total_dm += r.dm_count;
        total_saves += r.soft_saves;

        if have_dw {
            let dw = r.dw_count.unwrap_or(0);
            total_dw += dw;
            let pct = |count: usize| -> String {
                if dw > 0 {
                    format!("{:.1}%", count as f64 / dw as f64 * 100.0)
                } else {
                    "---".to_string()
                }
            };
            println!(
                "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
                r.display_name, dw, r.fast_count, r.quality_count, r.smart3_count,
                r.multi_count, r.dm_count,
                pct(r.fast_count), pct(r.smart3_count), pct(r.multi_count)
            );
        } else {
            println!(
                "{:<30} {:>7} {:>7} {:>7} {:>7} {:>7} {:>5}",
                r.display_name, r.fast_count, r.quality_count, r.smart3_count,
                r.multi_count, r.dm_count, r.soft_saves
            );
        }
    }

    // Totals
    if have_dw {
        println!("{}", "─".repeat(118));
        let pct = |count: usize| -> String {
            if total_dw > 0 {
                format!("{:.1}%", count as f64 / total_dw as f64 * 100.0)
            } else {
                "---".to_string()
            }
        };
        println!(
            "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
            "TOTAL", total_dw, total_fast, total_quality, total_smart3,
            total_multi, total_dm,
            pct(total_fast), pct(total_smart3), pct(total_multi)
        );
    } else {
        println!("{}", "─".repeat(75));
        println!(
            "{:<30} {:>7} {:>7} {:>7} {:>7} {:>7} {:>5}",
            "TOTAL", total_fast, total_quality, total_smart3, total_multi, total_dm, total_saves
        );
    }

    // Timing summary
    println!();
    println!("Timing:");
    for r in &results {
        let fast_rt = r.duration_secs / r.fast_elapsed.as_secs_f64();
        let qual_rt = r.duration_secs / r.qual_elapsed.as_secs_f64();
        let smart3_rt = r.duration_secs / r.smart3_elapsed.as_secs_f64();
        let multi_rt = r.duration_secs / r.multi_elapsed.as_secs_f64();
        let dm_rt = r.duration_secs / r.dm_elapsed.as_secs_f64();
        println!(
            "  {:<28}  fast {:.2}s ({:.0}x)  quality {:.2}s ({:.0}x)  smart3 {:.2}s ({:.0}x)  multi {:.2}s ({:.0}x)  dm {:.2}s ({:.0}x)",
            r.display_name,
            r.fast_elapsed.as_secs_f64(), fast_rt,
            r.qual_elapsed.as_secs_f64(), qual_rt,
            r.smart3_elapsed.as_secs_f64(), smart3_rt,
            r.multi_elapsed.as_secs_f64(), multi_rt,
            r.dm_elapsed.as_secs_f64(), dm_rt
        );
    }

    // Soft recovery summary
    let total_smart3_soft: u32 = results.iter().map(|r| r.smart3_soft).sum();
    let total_multi_soft: u32 = results.iter().map(|r| r.multi_soft).sum();
    println!();
    println!("Soft recovery saves:");
    println!("  {:<28}  {:>6} {:>6} {:>6}", "Track", "Qual", "Smart3", "Multi");
    println!("  {}", "─".repeat(52));
    for r in &results {
        println!("  {:<28}  {:>6} {:>6} {:>6}",
            r.display_name, r.soft_saves, r.smart3_soft, r.multi_soft);
    }
    println!("  {}", "─".repeat(52));
    println!("  {:<28}  {:>6} {:>6} {:>6}",
        "TOTAL", total_saves, total_smart3_soft, total_multi_soft);
}

// ─── Compare Approaches (A/B Test) ────────────────────────────────────────

fn run_compare_approaches(path: &str) {
    println!("═══ Approach Comparison ═══");
    println!("File: {}", path);
    println!();

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs,
        samples.len(),
        sample_rate
    );
    println!();

    let fast = decode_fast(&samples, sample_rate);
    let (quality, soft_saves) = decode_quality(&samples, sample_rate);

    // Compare unique frames by raw bytes
    let quality_set: std::collections::HashSet<Vec<u8>> =
        quality.frames.iter().cloned().collect();
    let fast_set: std::collections::HashSet<Vec<u8>> = fast.frames.iter().cloned().collect();

    let fast_unique = fast_set.len();
    let quality_unique = quality_set.len();
    let both_unique = fast_set.intersection(&quality_set).count();
    let fast_only_unique = fast_unique - both_unique;
    let quality_only_unique = quality_unique - both_unique;

    let fast_rt = duration_secs / fast.elapsed.as_secs_f64();
    let qual_rt = duration_secs / quality.elapsed.as_secs_f64();

    println!("  ┌──────────────────────────┬────────────┬─────────────┐");
    println!("  │ Metric                   │ Fast Path  │ Quality Path│");
    println!("  ├──────────────────────────┼────────────┼─────────────┤");
    println!(
        "  │ Total frames decoded     │ {:>10} │ {:>11} │",
        fast.frames.len(),
        quality.frames.len()
    );
    println!(
        "  │ Unique frames            │ {:>10} │ {:>11} │",
        fast_unique, quality_unique
    );
    println!(
        "  │ Soft-recovery saves      │        N/A │ {:>11} │",
        soft_saves
    );
    println!(
        "  │ Processing time          │ {:>8.2}s  │ {:>9.2}s  │",
        fast.elapsed.as_secs_f64(),
        quality.elapsed.as_secs_f64()
    );
    println!(
        "  │ Speed (x real-time)      │ {:>9.0}x │ {:>10.0}x │",
        fast_rt, qual_rt
    );
    println!("  └──────────────────────────┴────────────┴─────────────┘");
    println!();
    println!(
        "  Unique frames decoded by both: {:>6}",
        both_unique
    );
    println!(
        "  Fast only (quality missed):    {:>6}",
        fast_only_unique
    );
    println!(
        "  Quality only (fast missed):    {:>6}",
        quality_only_unique
    );
    println!();

    // Show a few example fast-only and quality-only frames
    if fast_only_unique > 0 {
        println!(
            "  First {} fast-only frame(s):",
            fast_only_unique.min(3)
        );
        let mut shown = 0;
        for frame in &fast.frames {
            if !quality_set.contains(frame) && shown < 3 {
                println!("    [{} bytes] {:02X?}", frame.len(), &frame[..frame.len().min(20)]);
                shown += 1;
            }
        }
    }
    if quality_only_unique > 0 {
        println!(
            "  First {} quality-only frame(s):",
            quality_only_unique.min(3)
        );
        let mut shown = 0;
        for frame in &quality.frames {
            if !fast_set.contains(frame) && shown < 3 {
                println!("    [{} bytes] {:02X?}", frame.len(), &frame[..frame.len().min(20)]);
                shown += 1;
            }
        }
    }
}

// ─── Synthetic Signal Benchmark ────────────────────────────────────────────

fn run_synthetic_benchmark() {
    use packet_radio_core::ax25::frame::{build_test_frame, hdlc_encode};
    use packet_radio_core::modem::afsk::AfskModulator;
    use packet_radio_core::modem::ModConfig;

    println!("═══ Synthetic Signal Benchmark ═══");
    println!();

    let sample_rate: u32 = 11025;
    let num_packets = 100;

    // Generate test packets
    println!("Generating {} test packets...", num_packets);
    let mut rng: u64 = 42;
    let next_rng = |state: &mut u64| -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    };

    // Build diverse test payloads
    let callsigns = [
        "N0CALL", "WA1ABC", "VE3XYZ", "K4DEF", "W5GHI",
        "KA6JKL", "N7MNO", "W8PQR", "K9STU", "WB0VWX",
    ];

    let mut clean_audio: Vec<i16> = Vec::new();
    let mut modulator = AfskModulator::new(ModConfig::default_1200());

    for i in 0..num_packets {
        // Generate varied payload
        let src = callsigns[i % callsigns.len()];
        let payload = format!(
            "!{:04}.{:02}N/{:05}.{:02}W-Packet {}",
            3000 + (next_rng(&mut rng) % 6000),
            next_rng(&mut rng) % 100,
            7000 + (next_rng(&mut rng) % 12000),
            next_rng(&mut rng) % 100,
            i
        );

        let (frame_data, frame_len) = build_test_frame(src, "APRS", payload.as_bytes());
        let encoded = hdlc_encode(&frame_data[..frame_len]);

        // Inter-packet gap (silence)
        let gap = vec![0i16; 1000];
        clean_audio.extend_from_slice(&gap);

        // Preamble flags
        for _ in 0..25 {
            let mut buf = [0i16; 128];
            let n = modulator.modulate_flag(&mut buf);
            clean_audio.extend_from_slice(&buf[..n]);
        }

        // Frame data
        for bit_idx in 0..encoded.bit_count {
            let bit = encoded.bits[bit_idx] != 0;
            let mut buf = [0i16; 128];
            let n = modulator.modulate_bit(bit, &mut buf);
            clean_audio.extend_from_slice(&buf[..n]);
        }

        // Trailing silence
        clean_audio.extend_from_slice(&[0i16; 20]);
    }

    let duration_secs = clean_audio.len() as f64 / sample_rate as f64;
    println!(
        "Generated {:.1}s of audio ({} samples)",
        duration_secs,
        clean_audio.len()
    );
    println!();

    // Define scenarios
    struct Scenario {
        name: &'static str,
        snr_db: Option<f64>,
        freq_offset_hz: Option<f64>,
        clock_drift: Option<f64>,
    }

    let scenarios = [
        Scenario { name: "Clean signal", snr_db: None, freq_offset_hz: None, clock_drift: None },
        Scenario { name: "20 dB SNR", snr_db: Some(20.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "10 dB SNR", snr_db: Some(10.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "6 dB SNR", snr_db: Some(6.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "3 dB SNR", snr_db: Some(3.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "+50 Hz offset", snr_db: None, freq_offset_hz: Some(50.0), clock_drift: None },
        Scenario { name: "+100 Hz offset", snr_db: None, freq_offset_hz: Some(100.0), clock_drift: None },
        Scenario { name: "1% clock drift", snr_db: None, freq_offset_hz: None, clock_drift: Some(1.01) },
        Scenario { name: "2% clock drift", snr_db: None, freq_offset_hz: None, clock_drift: Some(1.02) },
        Scenario { name: "10dB + 50Hz + 1%", snr_db: Some(10.0), freq_offset_hz: Some(50.0), clock_drift: Some(1.01) },
        Scenario { name: "6dB + 100Hz + 2%", snr_db: Some(6.0), freq_offset_hz: Some(100.0), clock_drift: Some(1.02) },
    ];

    println!(
        "  {:<32}  {:>10}  {:>10}  {:>10}  {:>10}",
        "Scenario", "Fast", "Quality", "Multi", "Soft Saves"
    );
    println!("  {}", "─".repeat(32 + 10 + 10 + 10 + 10 + 8));

    for scenario in &scenarios {
        // Apply impairments
        let mut signal = clean_audio.clone();

        if let Some(offset) = scenario.freq_offset_hz {
            signal = apply_frequency_offset(&signal, offset, sample_rate);
        }
        if let Some(drift) = scenario.clock_drift {
            signal = apply_clock_drift(&signal, drift);
        }
        if let Some(snr) = scenario.snr_db {
            signal = add_white_noise(&signal, snr, 42);
        }

        let fast = decode_fast(&signal, sample_rate);
        let (quality, soft_saves) = decode_quality(&signal, sample_rate);
        let (multi, _) = decode_multi(&signal, sample_rate);

        println!(
            "  {:<32}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}  {:>10}",
            scenario.name,
            fast.frames.len(),
            num_packets,
            quality.frames.len(),
            num_packets,
            multi.frames.len(),
            num_packets,
            soft_saves
        );
    }

    println!("  {}", "─".repeat(32 + 10 + 10 + 10 + 10 + 8));
}

// ─── DM+PLL Decode Engine ─────────────────────────────────────────────────

/// Decode using DM+PLL with configurable options.
fn decode_dm_pll_opts(
    samples: &[i16],
    sample_rate: u32,
    alpha: i16,
    beta: i16,
    adaptive: bool,
    preemph: i16,
    hysteresis: i16,
) -> DecodeResult {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = DmDemodulator::with_bpf_pll_custom(config, alpha, beta);
    if hysteresis > 0 {
        demod = demod.with_pll_hysteresis(hysteresis);
    }
    if adaptive {
        demod = demod.with_adaptive();
    }
    if preemph != 0 {
        demod = demod.with_preemph(preemph);
    }

    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }

    DecodeResult { frames, elapsed: start.elapsed() }
}

/// Decode using DM+PLL with SoftHdlcDecoder for bit-flip recovery.
fn decode_dm_pll_soft(
    samples: &[i16],
    sample_rate: u32,
    alpha: i16,
    beta: i16,
    adaptive: bool,
    preemph: i16,
    hysteresis: i16,
) -> (DecodeResult, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = DmDemodulator::with_bpf_pll_custom(config, alpha, beta);
    if hysteresis > 0 {
        demod = demod.with_pll_hysteresis(hysteresis);
    }
    if adaptive {
        demod = demod.with_adaptive();
    }
    if preemph != 0 {
        demod = demod.with_preemph(preemph);
    }

    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for i in 0..n {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
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

/// Simple DM+PLL decode with default gains.
fn decode_dm_pll(samples: &[i16], sample_rate: u32) -> DecodeResult {
    decode_dm_pll_opts(samples, sample_rate, 400, 30, false, 0, 0)
}

/// Decode DM+PLL with symbol counting for diagnostics.
fn decode_dm_pll_counted(samples: &[i16], sample_rate: u32, alpha: i16, beta: i16) -> (DecodeResult, usize, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;
    let mut demod = DmDemodulator::with_bpf_pll_custom(config, alpha, beta);
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
    let mut total_syms = 0usize;
    let mut flags = 0u32;
    let mut shift_reg: u8 = 0;

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        total_syms += n;
        for i in 0..n {
            shift_reg = (shift_reg >> 1) | if symbols[i].bit { 0x80 } else { 0 };
            if shift_reg == 0x7E { flags += 1; }
            if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    (DecodeResult { frames, elapsed: start.elapsed() }, total_syms, flags)
}

// ─── DM+PLL Single File Analysis ────────────────────────────────────────

fn run_dm_pll(path: &str) {
    println!("═══ DM+PLL Demodulator Variants ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz\n", duration_secs, samples.len(), sample_rate);

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);
    let dm_bres = decode_dm(&samples, sample_rate);
    // DM+Bresenham with adaptive
    let dm_bres_adapt = {
        let mut config = DemodConfig::default_1200();
        config.sample_rate = sample_rate;
        let demod = DmDemodulator::with_bpf(config).with_adaptive();
        let mut hdlc = HdlcDecoder::new();
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        let mut dm = demod;
        for chunk in samples.chunks(1024) {
            let n = dm.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if let Some(frame) = hdlc.feed_bit(symbols[i].bit) {
                    frames.push(frame.to_vec());
                }
            }
        }
        frames.len()
    };

    println!("  Baselines:");
    println!("    Fast (Goertzel+Bresenham):  {:>5}", fast.frames.len());
    println!("    DM+Bresenham:               {:>5}", dm_bres.frames.len());
    println!("    DM+Bres+adaptive:           {:>5}", dm_bres_adapt);
    println!("    Multi (38 decoders):         {:>5}", multi.frames.len());
    println!();

    // Diagnostic: symbol count and flag detection
    let (r_diag, sym_count, flag_count) = decode_dm_pll_counted(&samples, sample_rate, 400, 30);
    let expected_syms = (samples.len() as u64 * 1200 / sample_rate as u64) as usize;
    println!("  PLL diagnostics (a=400, b=30):");
    println!("    Symbols produced: {} (expected ~{})", sym_count, expected_syms);
    println!("    Flags detected:   {}", flag_count);
    println!("    Frames decoded:   {}", r_diag.frames.len());

    // Also compare with Bresenham
    {
        let mut config = DemodConfig::default_1200();
        config.sample_rate = sample_rate;
        let mut demod = DmDemodulator::with_bpf(config);
        let mut symbols_buf = [DemodSymbol { bit: false, llr: 0 }; 1024];
        let mut bres_syms = 0usize;
        let mut bres_flags = 0u32;
        let mut shift: u8 = 0;
        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols_buf);
            bres_syms += n;
            for i in 0..n {
                shift = (shift >> 1) | if symbols_buf[i].bit { 0x80 } else { 0 };
                if shift == 0x7E { bres_flags += 1; }
            }
        }
        println!("    Bresenham syms:   {} flags: {}", bres_syms, bres_flags);
    }
    println!();

    // DM+PLL variants with best alpha, testing beta and features
    println!("  DM+PLL variants (alpha=936):");
    let variants: &[(&str, i16, bool, i16, i16)] = &[
        // (name, beta, adaptive, preemph, hysteresis)
        ("DM+PLL b=0",                         0,  false, 0,     0),
        ("DM+PLL b=10",                        10, false, 0,     0),
        ("DM+PLL b=30",                        30, false, 0,     0),
        ("DM+PLL b=0 +adaptive",               0,  true,  0,     0),
        ("DM+PLL b=0 +preemph(0.90)",          0,  false, 29491, 0),
        ("DM+PLL b=0 +preemph(0.95)",          0,  false, 31130, 0),
        ("DM+PLL b=0 +adapt+preemph(0.90)",    0,  true,  29491, 0),
        ("DM+PLL b=0 +adapt+preemph(0.95)",    0,  true,  31130, 0),
        ("DM+PLL b=10 +adapt+preemph(0.95)",   10, true,  31130, 0),
        // With hysteresis
        ("DM+PLL b=10 hyst=50",                10, false, 0,     50),
        ("DM+PLL b=30 hyst=50",                30, false, 0,     50),
        ("DM+PLL b=10 hyst=100",               10, false, 0,     100),
        ("DM+PLL b=30 hyst=100",               30, false, 0,     100),
    ];

    for &(name, beta, adaptive, preemph, hyst) in variants {
        let r = decode_dm_pll_opts(&samples, sample_rate, 936, beta, adaptive, preemph, hyst);
        println!("    {:<38} {:>5}", name, r.frames.len());
    }
    println!();

    // DM+PLL+Soft variants (Gardner TED + SoftHdlcDecoder bit-flip recovery)
    println!("  DM+PLL+Soft (Gardner + SoftHdlcDecoder, alpha=936):");
    let soft_variants: &[(&str, i16, bool, i16, i16)] = &[
        // (name, beta, adaptive, preemph, hysteresis)
        ("DM+PLL+Soft b=0",              0,  false, 0, 0),
        ("DM+PLL+Soft b=10",             10, false, 0, 0),
        ("DM+PLL+Soft b=30",             30, false, 0, 0),
        ("DM+PLL+Soft b=74",             74, false, 0, 0),
        ("DM+PLL+Soft b=74 +adaptive",   74, true,  0, 0),
    ];

    for &(name, beta, adaptive, preemph, hyst) in soft_variants {
        let (r, saves) = decode_dm_pll_soft(&samples, sample_rate, 936, beta, adaptive, preemph, hyst);
        println!("    {:<38} {:>5} ({} soft saves)", name, r.frames.len(), saves);
    }
    println!();

    // Also try different alpha/beta
    println!("  Alpha/beta sweep (no adaptive/preemph):");
    let alphas = [0i16, 200, 400, 600, 936, 1200, 1500, 2000, 3000];
    let betas = [0i16, 1, 2, 5, 10, 20];
    for &a in &alphas {
        for &b in &betas {
            let r = decode_dm_pll_opts(&samples, sample_rate, a, b, false, 0, 0);
            println!("    a={:>4} b={:>3} → {:>5}", a, b, r.frames.len());
        }
    }
}

// ─── DM+PLL Parameter Sweep ────────────────────────────────────────────

fn run_dm_pll_sweep(path: &str) {
    println!("═══ DM+PLL Alpha/Beta Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s at {} Hz\n", duration_secs, sample_rate);

    let alphas = [100i16, 200, 300, 400, 500, 600, 800, 936, 1200, 1500];
    let betas = [10i16, 20, 30, 40, 50, 60, 74, 80, 100, 120];

    // Header
    print!("  {:>6}", "a\\b");
    for &b in &betas { print!(" {:>5}", b); }
    println!();
    println!("  {}", "─".repeat(6 + betas.len() * 6));

    let mut best_count = 0usize;
    let mut best_a = 0i16;
    let mut best_b = 0i16;

    for &a in &alphas {
        print!("  {:>6}", a);
        for &b in &betas {
            let r = decode_dm_pll_opts(&samples, sample_rate, a, b, false, 0, 0);
            let count = r.frames.len();
            print!(" {:>5}", count);
            if count > best_count {
                best_count = count;
                best_a = a;
                best_b = b;
            }
        }
        println!();
    }

    println!("\n  Best: alpha={}, beta={} → {} frames", best_a, best_b, best_count);

    // Now sweep with adaptive + best alpha/beta
    println!("\n  Best alpha/beta with adaptive + pre-emphasis:");
    let preemphs = [0i16, 26214, 29491, 31130, 32440];
    let preemph_names = ["none", "0.80", "0.90", "0.95", "0.99"];
    for (i, &pe) in preemphs.iter().enumerate() {
        let r_plain = decode_dm_pll_opts(&samples, sample_rate, best_a, best_b, false, pe, 0);
        let r_adapt = decode_dm_pll_opts(&samples, sample_rate, best_a, best_b, true, pe, 0);
        println!("    preemph={:<5}  plain={:>5}  adaptive={:>5}",
            preemph_names[i], r_plain.frames.len(), r_adapt.frames.len());
    }
}

// ─── DM+PLL Parameter Tune (Two-Stage Sweep) ─────────────────────────

/// Decode DM+PLL with all tunable parameters.
fn decode_dm_pll_tuned(
    samples: &[i16],
    sample_rate: u32,
    alpha: i16,
    beta: i16,
    error_shift: u8,
    smooth_shift: u8,
    llr_shift: u8,
    use_soft: bool,
) -> (usize, u32) {
    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut demod = DmDemodulator::with_bpf_pll_custom(config, alpha, beta)
        .with_pll_error_shift(error_shift)
        .with_pll_smoothing(smooth_shift)
        .with_llr_shift(llr_shift);

    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

    if use_soft {
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut frame_count = 0usize;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if soft_hdlc.feed_soft_bit(symbols[i].llr).is_some() {
                    frame_count += 1;
                }
            }
        }
        let soft_saves = soft_hdlc.stats_total_soft_recovered();
        (frame_count, soft_saves)
    } else {
        let mut hdlc = HdlcDecoder::new();
        let mut frame_count = 0usize;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if hdlc.feed_bit(symbols[i].bit).is_some() {
                    frame_count += 1;
                }
            }
        }
        (frame_count, 0)
    }
}

fn run_dm_pll_tune(path: &str) {
    println!("═══ DM+PLL Parameter Tune (Two-Stage Sweep) ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        duration_secs,
        samples.len(),
        sample_rate
    );

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);
    println!("  Baselines: fast={}, multi={}", fast.frames.len(), multi.frames.len());
    println!();

    // ── Stage 1: Gardner error shift × smoothing × beta ──
    println!("=== Stage 1: Gardner shift × smoothing × beta (alpha=936, llr_shift=6) ===");

    let error_shifts: &[u8] = &[6, 8, 10, 12, 14];
    let smooth_shifts: &[u8] = &[0, 2, 3, 4, 5];
    let betas: &[i16] = &[0, 1, 5, 10, 30, 74];

    struct Stage1Result {
        error_shift: u8,
        smooth_shift: u8,
        beta: i16,
        frames: usize,
    }

    let mut stage1_results: Vec<Stage1Result> = Vec::new();

    let total_combos = error_shifts.len() * smooth_shifts.len() * betas.len();
    eprint!("  Sweeping {} combinations...", total_combos);

    for &es in error_shifts {
        for &ss in smooth_shifts {
            for &b in betas {
                let (frames, _) = decode_dm_pll_tuned(
                    &samples, sample_rate, 936, b, es, ss, 6, false,
                );
                stage1_results.push(Stage1Result {
                    error_shift: es,
                    smooth_shift: ss,
                    beta: b,
                    frames,
                });
            }
        }
    }
    eprintln!(" done.");

    // Sort descending by frame count
    stage1_results.sort_by(|a, b| b.frames.cmp(&a.frames));

    println!(
        "  {:>3}  {:>9}  {:>6}  {:>4}  {:>6}",
        "#", "err_shift", "smooth", "beta", "frames"
    );
    println!("  {}", "─".repeat(35));

    let show_top = stage1_results.len().min(20);
    for (i, r) in stage1_results.iter().take(show_top).enumerate() {
        println!(
            "  {:>3}  {:>9}  {:>6}  {:>4}  {:>6}",
            i + 1, r.error_shift, r.smooth_shift, r.beta, r.frames
        );
    }
    println!("  Top {} shown (of {}).", show_top, total_combos);
    println!();

    // Use best params from stage 1
    let best = &stage1_results[0];
    let best_es = best.error_shift;
    let best_ss = best.smooth_shift;
    let best_beta = best.beta;

    println!(
        "  Best stage 1: err_shift={}, smooth={}, beta={} → {} frames",
        best_es, best_ss, best_beta, best.frames
    );
    println!();

    // ── Stage 2: Alpha × LLR shift ──
    println!(
        "=== Stage 2: Alpha × LLR shift (err={}, smooth={}, beta={}) ===",
        best_es, best_ss, best_beta
    );

    let alphas: &[i16] = &[400, 600, 800, 936, 1200];
    let llr_shifts: &[u8] = &[4, 5, 6, 7, 8, 9, 10];

    struct Stage2Result {
        alpha: i16,
        llr_shift: u8,
        hard: usize,
        soft: usize,
        soft_saves: u32,
    }

    let mut stage2_results: Vec<Stage2Result> = Vec::new();

    let total_s2 = alphas.len() * llr_shifts.len();
    eprint!("  Sweeping {} combinations (hard + soft)...", total_s2);

    for &a in alphas {
        for &ls in llr_shifts {
            let (hard, _) = decode_dm_pll_tuned(
                &samples, sample_rate, a, best_beta, best_es, best_ss, ls, false,
            );
            let (soft, soft_saves) = decode_dm_pll_tuned(
                &samples, sample_rate, a, best_beta, best_es, best_ss, ls, true,
            );
            stage2_results.push(Stage2Result {
                alpha: a,
                llr_shift: ls,
                hard,
                soft,
                soft_saves,
            });
        }
    }
    eprintln!(" done.");

    // Sort descending by soft frame count (primary), then hard (tiebreak)
    stage2_results.sort_by(|a, b| {
        b.soft.cmp(&a.soft).then(b.hard.cmp(&a.hard))
    });

    println!(
        "  {:>3}  {:>5}  {:>9}  {:>5}  {:>5}  {:>10}",
        "#", "alpha", "llr_shift", "hard", "soft", "soft_saves"
    );
    println!("  {}", "─".repeat(45));

    let show_s2 = stage2_results.len().min(20);
    for (i, r) in stage2_results.iter().take(show_s2).enumerate() {
        println!(
            "  {:>3}  {:>5}  {:>9}  {:>5}  {:>5}  {:>10}",
            i + 1, r.alpha, r.llr_shift, r.hard, r.soft, r.soft_saves
        );
    }
    println!("  Top {} shown (of {}).", show_s2, total_s2);
    println!();

    // Summary
    let best_s2 = &stage2_results[0];
    println!("=== Optimal Parameters ===");
    println!("  error_shift:      {}", best_es);
    println!("  pll_smooth_shift: {}", best_ss);
    println!("  beta:             {}", best_beta);
    println!("  alpha:            {}", best_s2.alpha);
    println!("  llr_shift:        {}", best_s2.llr_shift);
    println!("  hard frames:      {}", best_s2.hard);
    println!("  soft frames:      {} ({} soft saves)", best_s2.soft, best_s2.soft_saves);
}

// ─── DM Debug Diagnostics ──────────────────────────────────────────────

fn run_dm_debug(path: &str) {
    use packet_radio_core::modem::delay_multiply::DelayMultiplyDetector;

    println!("═══ DM Discriminator Diagnostics ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    // Limit to first 5 seconds for manageable output
    let max_samples = (sample_rate * 5) as usize;
    let work_samples = &samples[..samples.len().min(max_samples)];

    let delay = 8usize; // Standard filtered delay for 11025 Hz
    let lpf = packet_radio_core::modem::filter::post_detect_lpf(sample_rate);
    let mut detector = DelayMultiplyDetector::with_delay(delay, lpf);
    let mut bpf = match sample_rate {
        22050 => packet_radio_core::modem::filter::afsk_bandpass_22050(),
        44100 => packet_radio_core::modem::filter::afsk_bandpass_44100(),
        _ => packet_radio_core::modem::filter::afsk_bandpass_11025(),
    };

    let mut pll = packet_radio_core::modem::pll::ClockRecoveryPll::new(
        sample_rate, 1200, 400, 30,
    );

    let csv_path = format!("{}.dm_debug.csv", path);
    let mut csv = String::from("sample,disc_out,leaky,pll_phase,pll_locked,symbol_boundary\n");

    let mut leaky: i64 = 0;
    for (i, &s) in work_samples.iter().enumerate() {
        let filtered = bpf.process(s);
        let disc_out = detector.process(filtered);

        // Replicate the leaky integrator from DmDemodulator
        leaky -= leaky >> 3;
        leaky += disc_out as i64;
        let pll_input = leaky.clamp(-32000, 32000) as i16;
        let sym = pll.update(pll_input);

        csv.push_str(&format!("{},{},{},{},{},{}\n",
            i, disc_out, leaky, pll.phase,
            if pll.locked { 1 } else { 0 },
            if sym.is_some() { 1 } else { 0 },
        ));
    }

    match std::fs::write(&csv_path, &csv) {
        Ok(_) => println!("Wrote {} samples to {}", work_samples.len(), csv_path),
        Err(e) => eprintln!("Error writing {}: {}", csv_path, e),
    }
}

// ─── Frame Export ──────────────────────────────────────────────────────

fn frame_to_hex(frame: &[u8]) -> String {
    frame.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join("")
}

fn run_export(wav_path: &str, output_dir: &str) {
    println!("═══ Frame Export ═══");
    println!("File: {}", wav_path);

    let (sample_rate, samples) = match read_wav_file(wav_path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", wav_path, e); return; }
    };

    // Create output directory
    if let Err(e) = std::fs::create_dir_all(output_dir) {
        eprintln!("Error creating {}: {}", output_dir, e);
        return;
    }

    let paths: &[(&str, Box<dyn Fn(&[i16], u32) -> DecodeResult>)] = &[
        ("fast", Box::new(|s, sr| decode_fast(s, sr))),
        ("dm", Box::new(|s, sr| decode_dm(s, sr))),
        ("dm_pll", Box::new(|s, sr| decode_dm_pll(s, sr))),
        ("multi", Box::new(|s, sr| {
            decode_multi(s, sr).0
        })),
    ];

    for &(name, ref decode_fn) in paths {
        let result = decode_fn(&samples, sample_rate);
        let out_path = format!("{}/{}.txt", output_dir, name);
        let mut content = String::new();
        for frame in &result.frames {
            content.push_str(&frame_to_hex(frame));
            // Try to parse AX.25 callsigns
            if frame.len() >= 14 {
                let dst = parse_callsign(&frame[0..7]);
                let src = parse_callsign(&frame[7..14]);
                content.push_str(&format!(" {}>{}",  src, dst));
            }
            content.push('\n');
        }
        match std::fs::write(&out_path, &content) {
            Ok(_) => println!("  {} → {} ({} frames)", name, out_path, result.frames.len()),
            Err(e) => eprintln!("  Error writing {}: {}", out_path, e),
        }
    }

    // Frame comparison: show overlap between paths
    println!();
    let fast = decode_fast(&samples, sample_rate);
    let dm = decode_dm(&samples, sample_rate);
    let dm_pll = decode_dm_pll(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);

    let sets: Vec<(&str, std::collections::HashSet<Vec<u8>>)> = vec![
        ("fast", fast.frames.into_iter().collect()),
        ("dm", dm.frames.into_iter().collect()),
        ("dm_pll", dm_pll.frames.into_iter().collect()),
        ("multi", multi.frames.into_iter().collect()),
    ];

    println!("  Frame overlap matrix:");
    print!("  {:>8}", "");
    for &(name, _) in &sets { print!(" {:>8}", name); }
    println!();

    for &(name_a, ref set_a) in &sets {
        print!("  {:>8}", name_a);
        for &(_, ref set_b) in &sets {
            let overlap = set_a.intersection(set_b).count();
            print!(" {:>8}", overlap);
        }
        println!();
    }
}

/// Parse AX.25 callsign from 7 bytes (6 chars + SSID byte).
fn parse_callsign(data: &[u8]) -> String {
    if data.len() < 7 { return "???".to_string(); }
    let mut call = String::with_capacity(9);
    for i in 0..6 {
        let c = (data[i] >> 1) & 0x7F;
        if c > 0x20 { call.push(c as char); }
    }
    let ssid = (data[6] >> 1) & 0x0F;
    if ssid > 0 {
        call.push_str(&format!("-{}", ssid));
    }
    call
}

// ─── Signal Impairment Utilities ─────────────────────────────────────────

/// Add white Gaussian noise at the specified SNR (dB).
fn add_white_noise(samples: &[i16], snr_db: f64, seed: u64) -> Vec<i16> {
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
///
/// Uses a proper SSB (single-sideband) shift via Hilbert transform so each
/// tone shifts to a single new frequency without creating image artifacts.
/// This accurately models real transmitter crystal offset where mark shifts
/// from 1200→1250 Hz (not to both 1150 and 1250 Hz).
fn apply_frequency_offset(samples: &[i16], offset_hz: f64, sample_rate: u32) -> Vec<i16> {
    use std::f64::consts::TAU;

    // Hilbert transform via FFT-like FIR (length 31)
    const HALF_LEN: usize = 15;
    const HILBERT_LEN: usize = 2 * HALF_LEN + 1;
    let mut hilbert_coeffs = [0.0f64; HILBERT_LEN];
    for i in 0..HILBERT_LEN {
        let n = i as isize - HALF_LEN as isize;
        if n == 0 {
            hilbert_coeffs[i] = 0.0;
        } else if n % 2 != 0 {
            // h[n] = 2/(π·n) for odd n, windowed with Hamming
            let hamming = 0.54 - 0.46 * f64::cos(TAU * i as f64 / (HILBERT_LEN - 1) as f64);
            hilbert_coeffs[i] = (2.0 / (std::f64::consts::PI * n as f64)) * hamming;
        }
    }

    // Apply Hilbert transform (FIR convolution) and SSB frequency shift
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

            // Compute Hilbert transform output
            let mut q = 0.0;
            for k in 0..HILBERT_LEN {
                let idx = (write_idx + k) % HILBERT_LEN;
                q += delay_line[idx] * hilbert_coeffs[k];
            }

            // Delayed direct signal (aligned with Hilbert group delay)
            let i_delayed = delay_line[(write_idx + HALF_LEN) % HILBERT_LEN];

            // SSB frequency shift: real*cos - imag*sin
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
fn apply_clock_drift(samples: &[i16], ratio: f64) -> Vec<i16> {
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
fn truncate_str(s: &str, max_bytes: usize) -> &str {
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
///
/// Format: `SRC>DST,VIA1*,VIA2:payload`
///
/// The H-bit (high bit of SSID byte, bit 7) marks digipeaters that have
/// processed the frame — displayed as `*` after the callsign.
fn frame_to_tnc2(frame: &[u8]) -> Option<String> {
    // Minimum: 14 (dst+src) + 2 (ctrl+pid) = 16 bytes
    if frame.len() < 16 {
        return None;
    }

    let dst = parse_callsign_tnc2(&frame[0..7], false);
    let src = parse_callsign_tnc2(&frame[7..14], false);

    let mut result = format!("{}>{}",  src, dst);

    // First pass: collect digipeater addresses and find last H-bit
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

    // TNC2 format: `*` only on the last digipeater with H-bit set
    let last_h = vias.iter().rposition(|v| v.h_bit);
    for (i, via) in vias.iter().enumerate() {
        result.push(',');
        result.push_str(&via.callsign);
        if Some(i) == last_h {
            result.push('*');
        }
    }

    // Skip control byte (usually 0x03 = UI) and PID byte (usually 0xF0 = no L3)
    if pos + 2 > frame.len() {
        return None;
    }
    pos += 2;

    result.push(':');

    // Info field: render using lossy UTF-8, stripping control characters to
    // match Dire Wolf's packets.txt format (which strips CR, LF, and other
    // non-printable control bytes below 0x20 except space and tab).
    let info = &frame[pos..];
    let cleaned: Vec<u8> = info.iter().copied()
        .filter(|&b| b >= 0x20 || b == 0x09) // keep printable + tab
        .collect();
    let info_str = String::from_utf8_lossy(&cleaned);
    result.push_str(&info_str);
    Some(result)
}

/// Parse callsign for TNC2 display, with optional H-bit marker.
fn parse_callsign_tnc2(data: &[u8], h_bit: bool) -> String {
    if data.len() < 7 {
        return "???".to_string();
    }
    let mut call = String::with_capacity(10);
    for i in 0..6 {
        let c = (data[i] >> 1) & 0x7F;
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

// ─── Dire Wolf Reference Loader ───────────────────────────────────────────

/// Metadata for a DW-decoded frame from the clean log.
#[derive(Clone, Debug)]
struct DwFrameInfo {
    /// Decode sequence number.
    seq: u32,
    /// Timestamp string (e.g. "0:42.318").
    timestamp: String,
    /// Audio level (0-100).
    audio_level: u32,
    /// Mark/space ratio as raw string (e.g. "3/1").
    mark_space: String,
    /// Mark value from ratio.
    mark: u32,
    /// Space value from ratio.
    space: u32,
}

/// Load Dire Wolf .packets.txt file — one TNC2 line per decoded frame.
/// Uses lossy UTF-8 conversion since Mic-E packets contain high bytes.
fn load_dw_packets(path: &str) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}", e))?;
    let contents = String::from_utf8_lossy(&bytes);
    Ok(contents.lines().filter(|l| !l.is_empty()).map(String::from).collect())
}

/// Parse DW .clean.log to extract per-frame metadata.
///
/// Returns a map from TNC2 packet string → DwFrameInfo, plus a Vec preserving order.
fn parse_dw_clean_log(path: &str) -> Result<Vec<(String, DwFrameInfo)>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}", e))?;
    let contents = String::from_utf8_lossy(&bytes);
    let mut results = Vec::new();

    let lines: Vec<&str> = contents.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Look for DECODED[N] lines
        if let Some(rest) = line.strip_prefix("DECODED[") {
            // Parse: DECODED[N] M:SS.mmm <info> audio level = NN(mark/space)
            if let Some(bracket_end) = rest.find(']') {
                let seq: u32 = rest[..bracket_end].parse().unwrap_or(0);
                let after_bracket = &rest[bracket_end + 1..].trim_start();

                // Extract timestamp (first space-delimited token)
                let parts: Vec<&str> = after_bracket.splitn(2, ' ').collect();
                let timestamp = parts.first().unwrap_or(&"").to_string();

                // Extract audio level
                let (audio_level, mark, space, mark_space) =
                    if let Some(al_pos) = line.find("audio level = ") {
                        let al_rest = &line[al_pos + 14..];
                        let level_str: String = al_rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                        let audio_level: u32 = level_str.parse().unwrap_or(0);

                        // Parse (mark/space) ratio
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

                // Next non-empty line should be [0] PACKET_CONTENT
                i += 1;
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                if i < lines.len() {
                    let pkt_line = lines[i].trim();
                    // Strip "[0] " prefix
                    let packet = if pkt_line.starts_with("[0] ") {
                        &pkt_line[4..]
                    } else {
                        pkt_line
                    };

                    // Clean DW's <0x0d><0x0a> markers
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
///
/// Maps: tests/wav/02_100-mic-e-bursts-de-emphasized.wav
///   → iso/direwolf_review/packets/02_100-mic-e-bursts-de-emphasized.packets.txt
///   → iso/direwolf_review/raw_logs/02_100-mic-e-bursts-de-emphasized.clean.log
fn discover_dw_reference(wav_path: &str) -> Option<(String, String)> {
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

// ─── Frame Diff: Dire Wolf Comparison ─────────────────────────────────────

fn run_diff(wav_path: &str, reference: Option<&str>) {
    println!("═══ Frame-Level Diff vs Dire Wolf ═══");
    println!("File: {}", wav_path);

    let (sample_rate, samples) = match read_wav_file(wav_path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", wav_path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz", duration_secs, samples.len(), sample_rate);

    // Load DW reference
    let (pkt_path, log_path) = if let Some(ref_path) = reference {
        (ref_path.to_string(), String::new())
    } else {
        match discover_dw_reference(wav_path) {
            Some((p, l)) => (p, l),
            None => {
                eprintln!("Cannot find Dire Wolf reference for {}", wav_path);
                eprintln!("Use --reference <file> to specify explicitly");
                return;
            }
        }
    };

    println!("Reference: {}", pkt_path);

    let dw_packets = match load_dw_packets(&pkt_path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error loading reference: {}", e); return; }
    };

    // Load clean log for enrichment (optional — may not exist)
    let dw_log = if !log_path.is_empty() {
        parse_dw_clean_log(&log_path).unwrap_or_default()
    } else {
        Vec::new()
    };

    // Build DW lookup: TNC2 string → DwFrameInfo (first occurrence)
    let dw_info: std::collections::HashMap<String, DwFrameInfo> = dw_log.iter()
        .map(|(pkt, info)| (pkt.clone(), info.clone()))
        .collect();

    let dw_set: std::collections::HashSet<String> = dw_packets.iter().cloned().collect();

    println!("DW total: {} frames ({} unique)", dw_packets.len(), dw_set.len());
    println!();

    // Run all decoder modes including best single-decoder configs from attribution
    struct ModeResult {
        name: &'static str,
        tnc2_frames: Vec<String>,
    }

    let fast = decode_fast(&samples, sample_rate);
    let (quality, qual_soft) = decode_quality(&samples, sample_rate);
    let fast_adapt = decode_fast_adaptive(&samples, sample_rate);
    let qual_adapt = decode_quality_adaptive(&samples, sample_rate);
    let best_single = decode_best_single(&samples, sample_rate);
    let (smart3, smart3_soft) = decode_smart3(&samples, sample_rate);
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);
    let dm = decode_dm(&samples, sample_rate);
    // Best single decoders from attribution coverage curve
    let best1 = decode_custom_goertzel(&samples, sample_rate, -50, 2, -1); // G:freq-50/t2
    let best2 = decode_custom_goertzel(&samples, sample_rate, 0, 0, 0);    // G:narrow/t0
    let best3 = decode_custom_goertzel(&samples, sample_rate, 0, 1, 0);    // G:narrow/t1

    let modes = vec![
        ModeResult { name: "fast", tnc2_frames: frames_to_tnc2(&fast.frames) },
        ModeResult { name: "quality", tnc2_frames: frames_to_tnc2(&quality.frames) },
        ModeResult { name: "fast+adapt", tnc2_frames: frames_to_tnc2(&fast_adapt.frames) },
        ModeResult { name: "qual+adapt", tnc2_frames: frames_to_tnc2(&qual_adapt.frames) },
        ModeResult { name: "best-single", tnc2_frames: frames_to_tnc2(&best_single.frames) },
        ModeResult { name: "dm", tnc2_frames: frames_to_tnc2(&dm.frames) },
        ModeResult { name: "freq-50/t2", tnc2_frames: frames_to_tnc2(&best1.frames) },
        ModeResult { name: "narrow/t0", tnc2_frames: frames_to_tnc2(&best2.frames) },
        ModeResult { name: "narrow/t1", tnc2_frames: frames_to_tnc2(&best3.frames) },
        ModeResult { name: "smart3", tnc2_frames: frames_to_tnc2(&smart3.frames) },
        ModeResult { name: "multi", tnc2_frames: frames_to_tnc2(&multi.frames) },
    ];

    // Summary table
    println!("=== Decoder Mode Comparison vs Dire Wolf ===");
    println!("{:<14} {:>8} {:>8} {:>8} {:>8} {:>6}", "Mode", "Decoded", "Overlap", "DW-only", "Us-only", "Soft");
    println!("{}", "─".repeat(62));

    // Map mode names to their soft recovery counts
    let soft_map: std::collections::HashMap<&str, u32> = [
        ("quality", qual_soft),
        ("smart3", smart3_soft),
        ("multi", multi_soft),
    ].iter().copied().collect();

    for mode in &modes {
        let us_set: std::collections::HashSet<&str> = mode.tnc2_frames.iter().map(|s| s.as_str()).collect();
        let overlap = dw_set.iter().filter(|p| us_set.contains(p.as_str())).count();
        let dw_only = dw_set.len() - overlap;
        let us_only = us_set.len() - overlap;
        let soft_str = match soft_map.get(mode.name) {
            Some(&n) if n > 0 => format!("{}", n),
            _ => "-".to_string(),
        };
        println!("{:<14} {:>8} {:>8} {:>8} {:>8} {:>6}",
            mode.name, mode.tnc2_frames.len(), overlap, dw_only, us_only, soft_str);
    }
    println!();

    // Detailed analysis for multi-decoder (our best — last in modes list)
    let multi_tnc2 = &modes[modes.len() - 1].tnc2_frames;
    let multi_set: std::collections::HashSet<&str> = multi_tnc2.iter().map(|s| s.as_str()).collect();
    let overlap: Vec<&String> = dw_set.iter().filter(|p| multi_set.contains(p.as_str())).collect();
    let dw_only: Vec<&String> = dw_set.iter().filter(|p| !multi_set.contains(p.as_str())).collect();
    let us_only: Vec<String> = multi_set.iter().filter(|&&p| !dw_set.contains(p)).map(|&s| s.to_string()).collect();

    println!("=== Detailed: Multi-Decoder vs Dire Wolf ===");
    println!("DW decoded:   {:>5}    Us decoded: {:>5}", dw_set.len(), multi_set.len());
    println!("Overlap:      {:>5}    DW-only:    {:>5}    Us-only: {:>5}", overlap.len(), dw_only.len(), us_only.len());
    println!();

    // DW-only frames with enrichment
    if !dw_only.is_empty() {
        println!("--- DW-only frames (we miss, multi-decoder) ---");
        println!("  {:>3}  {:<10} {:>5} {:>5}  {}", "#", "Time", "Audio", "Mk/Sp", "Packet");
        let mut sorted_dw_only: Vec<(&String, Option<&DwFrameInfo>)> = dw_only.iter()
            .map(|p| (*p, dw_info.get(*p)))
            .collect();
        sorted_dw_only.sort_by_key(|(_, info)| info.map(|i| i.seq).unwrap_or(9999));

        for (i, (pkt, info)) in sorted_dw_only.iter().enumerate() {
            let (time, audio, ms) = match info {
                Some(inf) => (inf.timestamp.as_str(), format!("{}", inf.audio_level), inf.mark_space.clone()),
                None => ("?", "?".to_string(), "?".to_string()),
            };
            let display = truncate_str(pkt, 80);
            println!("  {:>3}  {:<10} {:>5} {:>5}  {}", i + 1, time, audio, ms, display);
        }
        println!();

        // Audio level distribution
        let mut level_bins = [0u32; 5]; // 0-19, 20-39, 40-59, 60-79, 80+
        let mut ratio_bins = [0u32; 3]; // <=2, 3-5, 6+
        let mut enriched = 0u32;

        for (_, info) in &sorted_dw_only {
            if let Some(inf) = info {
                enriched += 1;
                let bin = match inf.audio_level {
                    0..=19 => 0,
                    20..=39 => 1,
                    40..=59 => 2,
                    60..=79 => 3,
                    _ => 4,
                };
                level_bins[bin] += 1;

                let ratio = if inf.space > 0 { inf.mark / inf.space } else { 0 };
                let r_bin = match ratio {
                    0..=2 => 0,
                    3..=5 => 1,
                    _ => 2,
                };
                ratio_bins[r_bin] += 1;
            }
        }

        if enriched > 0 {
            println!("--- DW-only by audio level distribution ---");
            let level_labels = ["Level  0-19", "Level 20-39", "Level 40-59", "Level 60-79", "Level 80+  "];
            for (i, &label) in level_labels.iter().enumerate() {
                let count = level_bins[i];
                if count > 0 || i >= 1 {
                    let pct = count as f64 / enriched as f64 * 100.0;
                    println!("  {}: {:>3} frames ({:.0}%)", label, count, pct);
                }
            }
            println!();

            println!("--- DW-only by mark/space ratio ---");
            let ratio_labels = ["Ratio <=2 (flat)     ", "Ratio  3-5 (moderate)", "Ratio  6+  (severe) "];
            for (i, &label) in ratio_labels.iter().enumerate() {
                let count = ratio_bins[i];
                let pct = count as f64 / enriched as f64 * 100.0;
                println!("  {}: {:>3} frames ({:.0}%)", label, count, pct);
            }
            println!();
        }
    }

    // Us-only frames
    if !us_only.is_empty() {
        println!("--- Us-only frames (we find, DW misses) ---");
        for (i, pkt) in us_only.iter().enumerate().take(20) {
            let display = truncate_str(pkt, 80);
            println!("  {:>3}  {}", i + 1, display);
        }
        if us_only.len() > 20 {
            println!("  ... and {} more", us_only.len() - 20);
        }
        println!();
    }

    // Per-mode DW-only analysis: which modes find which DW-only frames?
    if !dw_only.is_empty() && modes.len() > 1 {
        println!("--- DW-only recovery by mode ---");
        println!("  Of {} DW-only frames (vs multi), how many does each mode find?", dw_only.len());
        for mode in &modes {
            let mode_set: std::collections::HashSet<&str> = mode.tnc2_frames.iter().map(|s| s.as_str()).collect();
            let recovered = dw_only.iter().filter(|p| mode_set.contains(p.as_str())).count();
            println!("    {:<14}: {} of {}", mode.name, recovered, dw_only.len());
        }
        println!();
    }
}

/// Convert a batch of raw AX.25 frames to TNC2 strings.
fn frames_to_tnc2(frames: &[Vec<u8>]) -> Vec<String> {
    frames.iter().filter_map(|f| frame_to_tnc2(f)).collect()
}

// ─── Attribution Mode ─────────────────────────────────────────────────────

fn run_attribution(wav_path: &str) {
    use packet_radio_core::modem::multi::AttributionReport;

    println!("═══ Per-Decoder Attribution Analysis ═══");
    println!("File: {}", wav_path);

    let (sample_rate, samples) = match read_wav_file(wav_path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", wav_path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz", duration_secs, samples.len(), sample_rate);
    println!();

    let mut config = DemodConfig::default_1200();
    config.sample_rate = sample_rate;

    let mut multi = MultiDecoder::new(config);
    let configs = multi.decoder_configs();

    println!("Active decoders: {} ({} Goertzel + {} DM)",
        configs.len(),
        configs.iter().filter(|c| c.algorithm == "goertzel").count(),
        configs.iter().filter(|c| c.algorithm == "dm").count());
    println!();

    let mut report = AttributionReport::new(configs.clone());
    let mut total_frames = 0usize;

    let start = std::time::Instant::now();
    for chunk in samples.chunks(1024) {
        let attributed = multi.process_samples_attributed(chunk);
        total_frames += attributed.output.len();
        report.merge(&attributed);
    }
    let elapsed = start.elapsed();
    report.finalize();

    println!("Decoded {} unique frames in {:.2}s", total_frames, elapsed.as_secs_f64());
    println!();

    // Per-decoder table
    println!("=== Per-Decoder Statistics ===");
    println!("  {:>3}  {:<28} {:>6} {:>6} {:>9}", "#", "Decoder", "Total", "First", "Exclusive");
    println!("  {}", "─".repeat(60));

    for (i, cfg) in configs.iter().enumerate() {
        let stat = &report.stats[i];
        let exc_str = if stat.exclusive > 0 {
            format!("{}", stat.exclusive)
        } else {
            "-".to_string()
        };
        println!("  {:>3}  {:<28} {:>6} {:>6} {:>9}",
            i, cfg.label, stat.total, stat.first, exc_str);
    }
    println!();

    // By-tag aggregation
    println!("=== Stats by Dimension ===");
    let tag_stats = report.stats_by_tag();
    println!("  {:<16} {:>8} {:>9} {:>9}", "Tag", "Frames", "Exclusive", "RawHits");
    println!("  {}", "─".repeat(48));
    for (tag, stat) in &tag_stats {
        println!("  {:<16} {:>8} {:>9} {:>9}", tag, stat.first, stat.exclusive, stat.total);
    }
    println!();

    // Coverage curve
    println!("=== Coverage Curve (Greedy Set Cover) ===");
    let curve = report.coverage_curve();
    let total_unique = report.total_unique();
    println!("  {:>3}  {:<28} {:>6} {:>7}", "#", "Decoder", "Cumul.", "% Total");
    println!("  {}", "─".repeat(50));

    for (step, &(dec_idx, cumulative)) in curve.iter().enumerate() {
        let label = if dec_idx < configs.len() {
            configs[dec_idx].label.as_str()
        } else {
            "?"
        };
        let pct = if total_unique > 0 {
            cumulative as f64 / total_unique as f64 * 100.0
        } else {
            0.0
        };
        println!("  {:>3}  {:<28} {:>6} {:>6.1}%", step + 1, label, cumulative, pct);
        // Stop printing after 100% or 15 entries
        if cumulative >= total_unique || step >= 14 {
            if step < curve.len() - 1 {
                println!("  ... ({} more decoders needed for remaining frames)", curve.len() - step - 1);
            }
            break;
        }
    }
    println!();

    // ESP32 recommendation
    if curve.len() >= 3 {
        println!("=== ESP32 Recommendation (top 3 decoders) ===");
        for &(dec_idx, cumulative) in curve.iter().take(3) {
            let label = if dec_idx < configs.len() {
                configs[dec_idx].label.as_str()
            } else {
                "?"
            };
            let pct = if total_unique > 0 {
                cumulative as f64 / total_unique as f64 * 100.0
            } else {
                0.0
            };
            println!("  {} → {} frames ({:.1}%)", label, cumulative, pct);
        }
        println!();
    }
}

// ─── WAV File Reader ───────────────────────────────────────────────────────

fn read_wav_file(path: &str) -> Result<(u32, Vec<i16>), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| format!("{}", e))?;

    if buf.len() < 44 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err("Not a valid WAV file".to_string());
    }

    // NOTE: Assumes standard PCM WAV (format code 1) with no extra fmt fields.
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

    // Find data chunk
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
                .map(|frame| i16::from_le_bytes([frame[0], frame[1]])) // Left channel
                .collect();

            return Ok((sample_rate, samples));
        }

        pos += 8 + chunk_size;
        if chunk_size % 2 != 0 {
            pos += 1;
        }
    }

    Err("No data chunk found in WAV file".to_string())
}
