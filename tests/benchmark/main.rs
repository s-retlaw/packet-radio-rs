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
use packet_radio_core::modem::demod::{DemodSymbol, DmDemodulator, FastDemodulator, QualityDemodulator};
use packet_radio_core::modem::multi::MultiDecoder;
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
        "--export" => {
            if args.len() < 4 {
                eprintln!("Usage: benchmark --export <file.wav> <output_dir>");
                return;
            }
            run_export(&args[2], &args[3]);
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
    println!("  benchmark --dm-debug <file.wav>        Dump DM discriminator diagnostics to CSV");
    println!("  benchmark --export <wav> <dir>         Export decoded frames to files");
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

    let mut demod = FastDemodulator::new(config);
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

    let soft_recovered = soft_hdlc.stats_soft_recovered;
    (
        DecodeResult {
            frames,
            elapsed: start.elapsed(),
        },
        soft_recovered,
    )
}

/// Decode audio samples using the multi-decoder (9 parallel fast decoders).
fn decode_multi(samples: &[i16], sample_rate: u32) -> DecodeResult {
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

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
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

/// Decode audio samples using DM at 22050 Hz (upsampled if needed).
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
    let (quality, soft_saves) = decode_quality(&samples, sample_rate);
    let multi = decode_multi(&samples, sample_rate);

    let fast_rt = duration_secs / fast.elapsed.as_secs_f64();
    let qual_rt = duration_secs / quality.elapsed.as_secs_f64();
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
        soft_saves
    );
    println!(
        "  Multi path:   {:>4} packets in {:.2}s ({:.0}x real-time)",
        multi.frames.len(),
        multi.elapsed.as_secs_f64(),
        multi_rt
    );
    println!();
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
    let multi = decode_multi(&samples, sample_rate);
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
        multi_count: usize,
        dm_count: usize,
        soft_saves: u32,
        dw_count: Option<u32>,
        fast_elapsed: Duration,
        qual_elapsed: Duration,
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
        let (quality, soft_saves) = decode_quality(&samples, sample_rate);
        let multi = decode_multi(&samples, sample_rate);
        let dm = decode_dm(&samples, sample_rate);

        eprintln!(
            "fast={}, quality={}, multi={}, dm={}",
            fast.frames.len(),
            quality.frames.len(),
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
            multi_count: multi.frames.len(),
            dm_count: dm.frames.len(),
            soft_saves,
            dw_count,
            fast_elapsed: fast.elapsed,
            qual_elapsed: quality.elapsed,
            multi_elapsed: multi.elapsed,
            dm_elapsed: dm.elapsed,
            duration_secs,
        });
    }

    println!();

    // Print comparison table
    if have_dw {
        println!(
            "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
            "Track", "DireWolf", "Fast", "Quality", "Multi", "DM", "Fast%", "Multi%", "DM%"
        );
        println!("{}", "─".repeat(105));
    } else {
        println!(
            "{:<30} {:>7} {:>7} {:>7} {:>7} {:>5}",
            "Track", "Fast", "Quality", "Multi", "DM", "Saves"
        );
        println!("{}", "─".repeat(67));
    }

    let mut total_fast = 0usize;
    let mut total_quality = 0usize;
    let mut total_multi = 0usize;
    let mut total_dm = 0usize;
    let mut total_dw = 0u32;
    let mut total_saves = 0u32;

    for r in &results {
        total_fast += r.fast_count;
        total_quality += r.quality_count;
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
                "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
                r.display_name, dw, r.fast_count, r.quality_count, r.multi_count, r.dm_count,
                pct(r.fast_count), pct(r.multi_count), pct(r.dm_count)
            );
        } else {
            println!(
                "{:<30} {:>7} {:>7} {:>7} {:>7} {:>5}",
                r.display_name, r.fast_count, r.quality_count, r.multi_count, r.dm_count, r.soft_saves
            );
        }
    }

    // Totals
    if have_dw {
        println!("{}", "─".repeat(105));
        let pct = |count: usize| -> String {
            if total_dw > 0 {
                format!("{:.1}%", count as f64 / total_dw as f64 * 100.0)
            } else {
                "---".to_string()
            }
        };
        println!(
            "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
            "TOTAL", total_dw, total_fast, total_quality, total_multi, total_dm,
            pct(total_fast), pct(total_multi), pct(total_dm)
        );
    } else {
        println!("{}", "─".repeat(67));
        println!(
            "{:<30} {:>7} {:>7} {:>7} {:>7} {:>5}",
            "TOTAL", total_fast, total_quality, total_multi, total_dm, total_saves
        );
    }

    // Timing summary
    println!();
    println!("Timing:");
    for r in &results {
        let fast_rt = r.duration_secs / r.fast_elapsed.as_secs_f64();
        let qual_rt = r.duration_secs / r.qual_elapsed.as_secs_f64();
        let multi_rt = r.duration_secs / r.multi_elapsed.as_secs_f64();
        let dm_rt = r.duration_secs / r.dm_elapsed.as_secs_f64();
        println!(
            "  {:<30}  fast {:.2}s ({:.0}x)  quality {:.2}s ({:.0}x)  multi {:.2}s ({:.0}x)  dm {:.2}s ({:.0}x)",
            r.display_name,
            r.fast_elapsed.as_secs_f64(), fast_rt,
            r.qual_elapsed.as_secs_f64(), qual_rt,
            r.multi_elapsed.as_secs_f64(), multi_rt,
            r.dm_elapsed.as_secs_f64(), dm_rt
        );
    }
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
        let multi = decode_multi(&signal, sample_rate);

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
    let multi = decode_multi(&samples, sample_rate);
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
    println!("    Multi (36 decoders):         {:>5}", multi.frames.len());
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
            decode_multi(s, sr)
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
    let multi = decode_multi(&samples, sample_rate);

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
