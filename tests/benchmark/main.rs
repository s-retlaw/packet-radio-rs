//! Benchmark Runner — TNC Test CD & Demodulator Comparison
//!
//! This binary processes WAV files through both demodulator paths and
//! reports packet counts, decode rates, and comparative performance.
//!
//! Usage:
//!   cargo run --release -p benchmark -- --wav track1.wav
//!   cargo run --release -p benchmark -- --suite tests/wav/
//!   cargo run --release -p benchmark -- --compare-approaches track1.wav
//!   cargo run --release -p benchmark -- --synthetic --scenarios
//!
//! The --compare-approaches mode runs both demodulators on the same audio
//! and reports which packets each one decoded, providing a direct A/B
//! comparison of the fast path vs. quality path.

use std::time::Instant;

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
    println!("  benchmark --suite <directory>           Decode all WAV files in a directory");
    println!("  benchmark --compare-approaches <wav>    Compare fast vs. quality path");
    println!("  benchmark --synthetic                   Run synthetic signal benchmark");
    println!();
    println!("The WAV files from the WA8LMF TNC Test CD are the standard benchmark.");
    println!("Download from: http://wa8lmf.net/TNCtest/");
}

// ─── Single WAV File Decode ────────────────────────────────────────────────

fn run_single_wav(path: &str) {
    println!("═══ Packet Radio RS Benchmark ═══");
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
    println!("Sample rate: {} Hz", sample_rate);
    println!("Duration: {:.1} seconds", duration_secs);
    println!("Samples: {}", samples.len());
    println!();

    // TODO: Once demodulators are complete, run both paths and count packets:
    //
    // let start = Instant::now();
    // let fast_results = decode_with_fast_path(&samples, sample_rate);
    // let fast_time = start.elapsed();
    //
    // let start = Instant::now();
    // let quality_results = decode_with_quality_path(&samples, sample_rate);
    // let quality_time = start.elapsed();
    //
    // println!("Fast path:    {} packets in {:?} ({:.0}x real-time)",
    //     fast_results.len(), fast_time,
    //     duration_secs / fast_time.as_secs_f64());
    // println!("Quality path: {} packets in {:?} ({:.0}x real-time)",
    //     quality_results.len(), quality_time,
    //     duration_secs / quality_time.as_secs_f64());

    println!("[Demodulator not yet implemented — scaffold only]");
    println!();
    println!("Once the demodulator is complete, this will report:");
    println!("  - Total packets decoded (per path)");
    println!("  - Unique packets vs. duplicates");
    println!("  - CRC failures");
    println!("  - Soft-recovery saves (quality path)");
    println!("  - Processing speed (× real-time)");
    println!("  - Comparison with Dire Wolf reference (if available)");
}

// ─── Benchmark Suite (All WAV Files) ───────────────────────────────────────

fn run_suite(dir: &str) {
    println!("═══ Benchmark Suite: {} ═══", dir);
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
        println!("and place them in the tests/wav/ directory.");
        return;
    }

    println!("Found {} WAV files:", wav_files.len());
    for f in &wav_files {
        println!("  {}", f);
    }
    println!();

    for f in &wav_files {
        run_single_wav(f);
        println!("───────────────────────────────────────────");
    }
}

// ─── Compare Approaches (A/B Test) ────────────────────────────────────────

fn run_compare_approaches(path: &str) {
    println!("═══ Approach Comparison: {} ═══", path);
    println!();

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz", duration_secs, samples.len(), sample_rate);
    println!();

    // TODO: Implement when demodulators are complete.
    // This will:
    // 1. Run both fast and quality paths on the same audio
    // 2. Collect all decoded packets from each
    // 3. Compare packet-by-packet:
    //    - Packets decoded by BOTH paths
    //    - Packets decoded ONLY by fast path
    //    - Packets decoded ONLY by quality path
    //    - Packets recovered by soft-decision bit-flipping
    // 4. Report the specific advantage of each technique

    println!("Approach comparison structure:");
    println!();
    println!("  ┌──────────────────────┬────────────┬─────────────┐");
    println!("  │ Metric               │ Fast Path  │ Quality Path│");
    println!("  ├──────────────────────┼────────────┼─────────────┤");
    println!("  │ Total packets        │     -      │      -      │");
    println!("  │ Hard-decision only   │     -      │      -      │");
    println!("  │ Soft-recovery saves  │    N/A     │      -      │");
    println!("  │ Processing speed     │     -      │      -      │");
    println!("  │ Peak memory (KB)     │     -      │      -      │");
    println!("  └──────────────────────┴────────────┴─────────────┘");
    println!();
    println!("  Packets decoded by both:        -");
    println!("  Fast only (quality missed):     -");
    println!("  Quality only (fast missed):     -");
    println!("  Recovered by bit-flipping:      -");
    println!();
    println!("  [Not yet implemented — scaffold only]");
}

// ─── Synthetic Signal Benchmark ────────────────────────────────────────────

fn run_synthetic_benchmark() {
    println!("═══ Synthetic Signal Benchmark ═══");
    println!();
    println!("Generating test packets under controlled conditions...");
    println!();

    let scenarios = vec![
        ("Clean signal", None, None, None),
        ("20 dB SNR", Some(20.0f64), None, None),
        ("10 dB SNR", Some(10.0), None, None),
        ("6 dB SNR", Some(6.0), None, None),
        ("3 dB SNR", Some(3.0), None, None),
        ("+50 Hz offset", None, Some(50.0f64), None),
        ("+100 Hz offset", None, Some(100.0), None),
        ("1% clock drift", None, None, Some(1.01f64)),
        ("2% clock drift", None, None, Some(1.02)),
        ("10dB + 50Hz + 1%", Some(10.0), Some(50.0), Some(1.01)),
        ("6dB + 100Hz + 2%", Some(6.0), Some(100.0), Some(1.02)),
    ];

    println!("  ┌──────────────────────────────────┬────────────┬─────────────┬────────────┐");
    println!("  │ Scenario                         │ Fast Path  │ Quality Path│ Soft Saves │");
    println!("  ├──────────────────────────────────┼────────────┼─────────────┼────────────┤");

    for (name, snr, freq_off, clock) in &scenarios {
        // TODO: Generate test signal, apply impairments, decode with both paths
        println!("  │ {:<32} │    --/100  │    --/100   │     --     │", name);
    }

    println!("  └──────────────────────────────────┴────────────┴─────────────┴────────────┘");
    println!();
    println!("  [Not yet implemented — scaffold only]");
    println!();
    println!("  When complete, this benchmark will:");
    println!("  1. Generate 100 random APRS packets");
    println!("  2. Modulate to audio with ideal AFSK");
    println!("  3. Apply each impairment scenario");
    println!("  4. Decode with fast path and quality path");
    println!("  5. Report packets decoded by each approach");
    println!("  6. Report soft-decision recovery saves");
    println!("  7. Identify the crossover point where quality path wins");
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

    let sample_rate = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let channels = u16::from_le_bytes([buf[22], buf[23]]);
    let bits_per_sample = u16::from_le_bytes([buf[34], buf[35]]);

    if bits_per_sample != 16 {
        return Err(format!("Unsupported bit depth: {} (need 16-bit)", bits_per_sample));
    }

    // Find data chunk
    let mut pos = 12;
    while pos + 8 < buf.len() {
        let chunk_id = &buf[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7],
        ]) as usize;

        if chunk_id == b"data" {
            let data_start = pos + 8;
            let data_end = (data_start + chunk_size).min(buf.len());

            let step = channels as usize; // Skip extra channels (take left only)
            let samples: Vec<i16> = buf[data_start..data_end]
                .chunks_exact(2 * channels as usize)
                .map(|frame| i16::from_le_bytes([frame[0], frame[1]])) // Left channel
                .collect();

            return Ok((sample_rate, samples));
        }

        pos += 8 + chunk_size;
        if chunk_size % 2 != 0 { pos += 1; }
    }

    Err("No data chunk found in WAV file".to_string())
}
