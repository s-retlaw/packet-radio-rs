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
use packet_radio_core::modem::demod::{DemodSymbol, FastDemodulator, QualityDemodulator};
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
        "--synthetic" => {
            run_synthetic_benchmark();
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
        soft_saves: u32,
        dw_count: Option<u32>,
        fast_elapsed: Duration,
        qual_elapsed: Duration,
        multi_elapsed: Duration,
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

        eprintln!(
            "fast={}, quality={}, multi={}",
            fast.frames.len(),
            quality.frames.len(),
            multi.frames.len()
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
            soft_saves,
            dw_count,
            fast_elapsed: fast.elapsed,
            qual_elapsed: quality.elapsed,
            multi_elapsed: multi.elapsed,
            duration_secs,
        });
    }

    println!();

    // Print comparison table
    if have_dw {
        println!(
            "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
            "Track", "DireWolf", "Fast", "Quality", "Multi", "Fast%", "Qual%", "Multi%"
        );
        println!("{}", "─".repeat(91));
    } else {
        println!(
            "{:<30} {:>7} {:>7} {:>7} {:>5}",
            "Track", "Fast", "Quality", "Multi", "Saves"
        );
        println!("{}", "─".repeat(60));
    }

    let mut total_fast = 0usize;
    let mut total_quality = 0usize;
    let mut total_multi = 0usize;
    let mut total_dw = 0u32;
    let mut total_saves = 0u32;

    for r in &results {
        total_fast += r.fast_count;
        total_quality += r.quality_count;
        total_multi += r.multi_count;
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
                "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
                r.display_name, dw, r.fast_count, r.quality_count, r.multi_count,
                pct(r.fast_count), pct(r.quality_count), pct(r.multi_count)
            );
        } else {
            println!(
                "{:<30} {:>7} {:>7} {:>7} {:>5}",
                r.display_name, r.fast_count, r.quality_count, r.multi_count, r.soft_saves
            );
        }
    }

    // Totals
    if have_dw {
        println!("{}", "─".repeat(91));
        let pct = |count: usize| -> String {
            if total_dw > 0 {
                format!("{:.1}%", count as f64 / total_dw as f64 * 100.0)
            } else {
                "---".to_string()
            }
        };
        println!(
            "{:<30} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
            "TOTAL", total_dw, total_fast, total_quality, total_multi,
            pct(total_fast), pct(total_quality), pct(total_multi)
        );
    } else {
        println!("{}", "─".repeat(60));
        println!(
            "{:<30} {:>7} {:>7} {:>7} {:>5}",
            "TOTAL", total_fast, total_quality, total_multi, total_saves
        );
    }

    // Timing summary
    println!();
    println!("Timing:");
    for r in &results {
        let fast_rt = r.duration_secs / r.fast_elapsed.as_secs_f64();
        let qual_rt = r.duration_secs / r.qual_elapsed.as_secs_f64();
        let multi_rt = r.duration_secs / r.multi_elapsed.as_secs_f64();
        println!(
            "  {:<30}  fast {:.2}s ({:.0}x RT)  quality {:.2}s ({:.0}x RT)  multi {:.2}s ({:.0}x RT)",
            r.display_name,
            r.fast_elapsed.as_secs_f64(),
            fast_rt,
            r.qual_elapsed.as_secs_f64(),
            qual_rt,
            r.multi_elapsed.as_secs_f64(),
            multi_rt
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
