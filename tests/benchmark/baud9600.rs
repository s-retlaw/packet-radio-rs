//! 9600 Baud G3RUH benchmarks.

use std::time::{Duration, Instant};

use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::modem::demod::DemodSymbol;
use packet_radio_core::modem::demod_9600::{
    Demod9600Config, Demod9600Direwolf, Demod9600Gardner,
    Demod9600EarlyLate, Demod9600MuellerMuller, Demod9600Rrc,
    select_9600_lpf,
};
use packet_radio_core::modem::multi_9600::{Mini9600Decoder, Multi9600Decoder, Single9600Decoder};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};

use crate::common::*;

// ─── 9600 Baud G3RUH Benchmarks ──────────────────────────────────────────

/// Decode a 9600 baud WAV file using all 5 algorithms.
pub fn run_9600_single(path: &str) {
    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("9600 Baud Decode: {}", path);
    println!("  Sample rate: {} Hz, {} samples ({:.1}s)",
        sample_rate, samples.len(), samples.len() as f64 / sample_rate as f64);
    println!();

    let config = Demod9600Config::with_sample_rate(sample_rate);

    #[allow(clippy::type_complexity)]
    let algos: [(&str, fn(Demod9600Config) -> Single9600Decoder); 5] = [
        ("DireWolf-style", Single9600Decoder::direwolf),
        ("Gardner PLL",    Single9600Decoder::gardner),
        ("Early-Late",     Single9600Decoder::early_late),
        ("Mueller-Muller", Single9600Decoder::mueller_muller),
        ("RRC Matched",    Single9600Decoder::rrc),
    ];

    println!("{:<18} {:>8} {:>10}", "Algorithm", "Frames", "Time");
    println!("{}", "-".repeat(40));

    for (name, ctor) in &algos {
        let mut decoder = ctor(config);
        let mut frame_count = 0u32;
        let start = Instant::now();

        for chunk in samples.chunks(1024) {
            frame_count += decoder.process_samples(chunk).len() as u32;
        }

        let elapsed = start.elapsed();
        println!("{:<18} {:>8} {:>8.1}ms", name, frame_count, elapsed.as_secs_f64() * 1000.0);
    }
}

/// Compare all 5 algorithms side by side.
pub fn run_9600_compare(path: &str) {
    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("9600 Baud Algorithm Comparison: {}", path);
    println!("  Sample rate: {} Hz, {:.1}s",
        sample_rate, samples.len() as f64 / sample_rate as f64);
    println!();

    let config = Demod9600Config::with_sample_rate(sample_rate);

    let mut results: Vec<(&str, u32, Duration)> = Vec::new();

    #[allow(clippy::type_complexity)]
    let algos: [(&str, fn(Demod9600Config) -> Single9600Decoder); 5] = [
        ("DireWolf-style", Single9600Decoder::direwolf),
        ("Gardner PLL",    Single9600Decoder::gardner),
        ("Early-Late",     Single9600Decoder::early_late),
        ("Mueller-Muller", Single9600Decoder::mueller_muller),
        ("RRC Matched",    Single9600Decoder::rrc),
    ];

    for (name, ctor) in &algos {
        let mut decoder = ctor(config);
        let mut frame_count = 0u32;
        let start = Instant::now();

        for chunk in samples.chunks(1024) {
            frame_count += decoder.process_samples(chunk).len() as u32;
        }

        results.push((name, frame_count, start.elapsed()));
    }

    let best = results.iter().map(|r| r.1).max().unwrap_or(0);

    println!("{:<18} {:>8} {:>8} {:>10}", "Algorithm", "Frames", "%Best", "Time");
    println!("{}", "-".repeat(50));

    for (name, frames, elapsed) in &results {
        let pct = if best > 0 { *frames as f64 / best as f64 * 100.0 } else { 0.0 };
        println!("{:<18} {:>8} {:>7.1}% {:>8.1}ms",
            name, frames, pct, elapsed.as_secs_f64() * 1000.0);
    }
}

/// Decode using the Multi9600 ensemble decoder.
pub fn run_9600_multi(path: &str) {
    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("9600 Baud Multi-Decoder: {}", path);
    println!("  Sample rate: {} Hz, {:.1}s",
        sample_rate, samples.len() as f64 / sample_rate as f64);

    let config = Demod9600Config::with_sample_rate(sample_rate);
    let mut decoder = Multi9600Decoder::new(config);

    println!("  Decoders: {}", decoder.num_decoders());
    println!();

    let start = Instant::now();
    let mut total_frames = 0u32;

    for chunk in samples.chunks(1024) {
        let output = decoder.process_samples(chunk);
        total_frames += output.len() as u32;
    }

    let elapsed = start.elapsed();
    println!("  Unique frames: {}", total_frames);
    println!("  Total decoded: {} (before dedup)", decoder.total_decoded);
    println!("  Time: {:.1}ms", elapsed.as_secs_f64() * 1000.0);
}

/// Helper: decode a 9600 WAV with a single-algo decoder, return frame count.
pub fn decode_9600_single_count(samples: &[i16], config: Demod9600Config, ctor: fn(Demod9600Config) -> Single9600Decoder) -> u32 {
    let mut decoder = ctor(config);
    let mut count = 0u32;
    for chunk in samples.chunks(1024) {
        count += decoder.process_samples(chunk).len() as u32;
    }
    count
}

/// Helper: decode a 9600 WAV with Multi9600Decoder, return unique frame count.
pub fn decode_9600_multi_count(samples: &[i16], config: Demod9600Config) -> u32 {
    let mut decoder = Multi9600Decoder::new(config);
    let mut count = 0u32;
    for chunk in samples.chunks(1024) {
        count += decoder.process_samples(chunk).len() as u32;
    }
    count
}

/// Run all 9600 algorithms × all 9600 WAV files in a directory, producing a grid.
pub fn run_9600_suite(dir: &str) {
    // Find all 9600 WAV files, sorted by name
    let mut wav_files: Vec<(String, String)> = Vec::new(); // (path, display_name)
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.contains("9600") && name.ends_with(".wav") {
                    wav_files.push((path.to_string_lossy().to_string(), name.to_string()));
                }
            }
        }
    }
    wav_files.sort_by(|a, b| a.1.cmp(&b.1));

    if wav_files.is_empty() {
        eprintln!("No 9600 baud WAV files found in {}", dir);
        eprintln!("Generate them with: ./tests/generate_9600_tests.sh");
        return;
    }

    println!("9600 Baud G3RUH — Algorithm × Sample Rate Grid");
    println!("================================================");
    println!();

    // Algorithm names and constructors
    // All use DwPll with different front-ends:
    // DW-style: LPF(6000Hz) + DwPll(0.89/0.67)
    // FastTrack: LPF(6000Hz) + DwPll(0.80/0.50) — faster tracking
    // Narrow: LPF(4800Hz) + DwPll(0.89/0.67) — tighter filter
    // Wide: LPF(7200Hz) + DwPll(0.89/0.67) — wider bandwidth
    // RRC: RRC matched filter + DwPll(0.89/0.67)
    #[allow(clippy::type_complexity)]
    let algos: Vec<(&str, fn(Demod9600Config) -> Single9600Decoder)> = vec![
        ("DW-style", Single9600Decoder::direwolf),
        ("FastTrk",  Single9600Decoder::gardner),
        ("Narrow",   Single9600Decoder::early_late),
        ("Wide",     Single9600Decoder::mueller_muller),
        ("RRC",      Single9600Decoder::rrc),
    ];

    // Load all WAV files
    struct WavData {
        display_name: String,
        sample_rate: u32,
        samples: Vec<i16>,
    }
    let mut wavs: Vec<WavData> = Vec::new();
    for (path, display) in &wav_files {
        match read_wav_file(path) {
            Ok((sr, samples)) => wavs.push(WavData {
                display_name: display.clone(),
                sample_rate: sr,
                samples,
            }),
            Err(e) => eprintln!("  Skipping {}: {}", display, e),
        }
    }

    if wavs.is_empty() {
        return;
    }

    // Print header
    let name_width = wavs.iter().map(|w| w.display_name.len()).max().unwrap_or(20).max(20);
    print!("{:<width$}", "File (rate, sps)", width = name_width + 2);
    for (algo_name, _) in &algos {
        print!(" {:>9}", algo_name);
    }
    println!(" {:>9}", "Multi");
    println!("{}", "-".repeat(name_width + 2 + (algos.len() + 1) * 10));

    // Run each WAV through each algorithm
    for wav in &wavs {
        let config = Demod9600Config::with_sample_rate(wav.sample_rate);
        let sps = wav.sample_rate / 9600;
        let label = format!("{} ({}Hz, {}sps)", wav.display_name, wav.sample_rate, sps);
        print!("{:<width$}", label, width = name_width + 2);

        for (_algo_name, ctor) in &algos {
            let count = decode_9600_single_count(&wav.samples, config, *ctor);
            print!(" {:>9}", count);
        }

        // Multi-decoder
        let multi_count = decode_9600_multi_count(&wav.samples, config);
        print!(" {:>9}", multi_count);

        println!();
    }

    println!();
    println!("DireWolf baselines (atest -B 9600, single decoder):");
    println!("  Run: for f in {}/*9600*.wav; do echo \"$(basename $f): $(atest -B 9600 $f 2>&1 | grep -c '^[A-Z0-9]')\"; done", dir);
}

/// Diagnostic: count symbols and analyze bit patterns for each algorithm.
pub fn run_9600_diag(path: &str) {
    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("9600 Baud Diagnostic: {}", path);
    println!("  Sample rate: {} Hz, {} samples ({:.1}s)",
        sample_rate, samples.len(), samples.len() as f64 / sample_rate as f64);
    println!();

    let config = Demod9600Config::with_sample_rate(sample_rate);

    // Test each algorithm: count symbols, ones ratio, and check HDLC flags
    let algo_names = ["DW-style", "Gardner", "Early-Late", "M&M", "RRC"];

    for (idx, name) in algo_names.iter().enumerate() {
        let mut sym_buf = [DemodSymbol { bit: false, llr: 0 }; 2048];
        let mut total_syms = 0usize;
        let mut ones = 0usize;
        let mut flag_count = 0usize;
        let mut consec_ones = 0u8;
        let mut max_llr = 0i8;
        let mut min_llr = 0i8;

        for chunk in samples.chunks(1024) {
            let n = match idx {
                0 => { let mut d = Demod9600Direwolf::new(config); d.process_samples(chunk, &mut sym_buf) }
                1 => { let mut d = Demod9600Gardner::new(config); d.process_samples(chunk, &mut sym_buf) }
                2 => { let mut d = Demod9600EarlyLate::new(config); d.process_samples(chunk, &mut sym_buf) }
                3 => { let mut d = Demod9600MuellerMuller::new(config); d.process_samples(chunk, &mut sym_buf) }
                4 => { let mut d = Demod9600Rrc::new(config); d.process_samples(chunk, &mut sym_buf) }
                _ => 0,
            };
            total_syms += n;
            for sym in &sym_buf[..n] {
                if sym.bit { ones += 1; }
                if sym.llr > max_llr { max_llr = sym.llr; }
                if sym.llr < min_llr { min_llr = sym.llr; }
                // Check for HDLC flag pattern 01111110
                if sym.bit {
                    consec_ones += 1;
                    if consec_ones == 6 {
                        // Might be in a flag - next bit should be 0
                    }
                } else {
                    if consec_ones == 6 {
                        flag_count += 1;
                    }
                    consec_ones = 0;
                }
            }
        }

        let ratio = if total_syms > 0 { ones as f64 / total_syms as f64 } else { 0.0 };
        println!("{:<12} syms={:<8} ones={:.3} flags={:<5} llr=[{},{}]",
                 name, total_syms, ratio, flag_count, min_llr, max_llr);
    }

    println!();
    println!("Note: each chunk creates a fresh demod (no state across chunks).");
    println!("Now testing with persistent state across all samples:");
    println!();

    // Persistent state test - this is the real test
    for (idx, name) in algo_names.iter().enumerate() {
        let mut sym_buf = [DemodSymbol { bit: false, llr: 0 }; 2048];
        let mut total_syms = 0usize;
        let mut ones = 0usize;
        let mut flag_count = 0usize;
        let mut consec_ones = 0u8;
        let mut hdlc_frames = 0usize;
        let mut max_llr = 0i8;
        let mut min_llr = 0i8;

        // Create decoder ONCE for all chunks
        let mut dw = Demod9600Direwolf::new(config);
        let mut gd = Demod9600Gardner::new(config);
        let mut el = Demod9600EarlyLate::new(config);
        let mut mm = Demod9600MuellerMuller::new(config);
        let mut rrc = Demod9600Rrc::new(config);
        let mut hdlc = HdlcDecoder::new();

        for chunk in samples.chunks(1024) {
            let n = match idx {
                0 => dw.process_samples(chunk, &mut sym_buf),
                1 => gd.process_samples(chunk, &mut sym_buf),
                2 => el.process_samples(chunk, &mut sym_buf),
                3 => mm.process_samples(chunk, &mut sym_buf),
                4 => rrc.process_samples(chunk, &mut sym_buf),
                _ => 0,
            };
            total_syms += n;
            for sym in &sym_buf[..n] {
                if sym.bit { ones += 1; }
                if sym.llr > max_llr { max_llr = sym.llr; }
                if sym.llr < min_llr { min_llr = sym.llr; }
                if sym.bit {
                    consec_ones += 1;
                } else {
                    if consec_ones == 6 {
                        flag_count += 1;
                    }
                    consec_ones = 0;
                }
                if hdlc.feed_bit(sym.bit).is_some() {
                    hdlc_frames += 1;
                }
            }
        }

        let ratio = if total_syms > 0 { ones as f64 / total_syms as f64 } else { 0.0 };
        println!("{:<12} syms={:<8} ones={:.3} flags={:<5} hdlc={:<5} llr=[{},{}]",
                 name, total_syms, ratio, flag_count, hdlc_frames, min_llr, max_llr);
    }
}

// ─── Mini9600 Benchmark ────────────────────────────────────────────────────

/// Decode using the Mini9600 ensemble decoder (6-decoder MCU-optimized).
pub fn run_9600_mini(path: &str) {
    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("9600 Baud Mini9600 Decoder: {}", path);
    println!("  Sample rate: {} Hz, {:.1}s",
        sample_rate, samples.len() as f64 / sample_rate as f64);

    let config = Demod9600Config::with_sample_rate(sample_rate);
    let mut decoder = Mini9600Decoder::new(config);

    println!("  Decoders: {}", decoder.num_decoders());
    println!();

    let start = Instant::now();
    let mut total_frames = 0u32;

    for chunk in samples.chunks(1024) {
        let output = decoder.process_samples(chunk);
        total_frames += output.len() as u32;
    }

    let elapsed = start.elapsed();
    println!("  Unique frames: {}", total_frames);
    println!("  Total decoded: {} (before dedup)", decoder.total_decoded);
    println!("  Time: {:.1}ms", elapsed.as_secs_f64() * 1000.0);
}

// ─── 9600 Grid Search Tuning ──────────────────────────────────────────────

/// Grid search across LPF cutoff, LPF order, PLL inertia, slicer threshold, and timing phase.
/// Reports top-50 single-decoder configs and greedy set-cover for top 3/6/10 combos.
pub fn run_9600_tune(path: &str) {
    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("9600 Baud Grid Search Tuning: {}", path);
    println!("  Sample rate: {} Hz, {:.1}s",
        sample_rate, samples.len() as f64 / sample_rate as f64);

    let config = Demod9600Config::with_sample_rate(sample_rate);
    let period = (sample_rate as i64 * 256 / 9600) as i32;

    // Parameter grid
    let cutoffs: [u32; 5] = [4800, 5400, 6000, 6600, 7200];
    let lpf_orders: [bool; 2] = [false, true]; // false=2nd, true=4th (cascaded)
    let inertias: [(i32, i32); 3] = [(228, 171), (205, 128), (180, 100)];
    let thresholds: [i16; 5] = [-660, -330, 0, 330, 660];
    let timing_phases: [i32; 4] = [0, period / 4, period / 2, period * 3 / 4];

    println!("  Grid: {} cutoffs × {} orders × {} inertias × {} thresholds × {} phases",
        cutoffs.len(), lpf_orders.len(), inertias.len(), thresholds.len(), timing_phases.len());

    let total_combos = cutoffs.len() * lpf_orders.len() * inertias.len() * thresholds.len() * timing_phases.len();
    println!("  Total combos: {}", total_combos);
    println!();

    // Collect results: (label, frame_count, frame_hashes)
    struct TuneResult {
        label: String,
        frames: u32,
        hashes: Vec<u32>,
    }

    let mut results: Vec<TuneResult> = Vec::with_capacity(total_combos);
    let start = Instant::now();

    for &cutoff in &cutoffs {
        for &cascaded in &lpf_orders {
            for &(locked, _searching) in &inertias {
                for &threshold in &thresholds {
                    for &phase in &timing_phases {
                        let lpf_tag = if cascaded { "4th" } else { "2nd" };
                        let label = format!("DW:{}Hz/{}/i{}/th{}/p{}",
                            cutoff, lpf_tag, locked, threshold, phase);

                        let mut demod = Demod9600Direwolf::new(config)
                            .with_threshold(threshold)
                            .with_timing_offset(phase);

                        if cascaded {
                            demod = demod.with_cascaded_lpf_cutoff(cutoff);
                        } else {
                            demod = demod.with_lpf(select_9600_lpf(sample_rate, cutoff));
                        }

                        let mut hdlc = SoftHdlcDecoder::new();
                        let mut sym_buf = [DemodSymbol { bit: false, llr: 0 }; 512];
                        let mut hashes: Vec<u32> = Vec::new();

                        for chunk in samples.chunks(1024) {
                            let n = demod.process_samples(chunk, &mut sym_buf);
                            for sym in &sym_buf[..n] {
                                if let Some(FrameResult::Valid(data)) | Some(FrameResult::Recovered { data, .. }) = hdlc.feed_soft_bit(sym.llr) {
                                    let mut h: u32 = 0x811c9dc5;
                                    for &b in data {
                                        h ^= b as u32;
                                        h = h.wrapping_mul(0x01000193);
                                    }
                                    if !hashes.contains(&h) {
                                        hashes.push(h);
                                    }
                                }
                            }
                        }

                        results.push(TuneResult { label, frames: hashes.len() as u32, hashes });
                    }
                }
            }
        }
    }

    // Also sweep Gardner with different inertias
    for &cutoff in &cutoffs {
        for &cascaded in &lpf_orders {
            for &(locked, searching) in &inertias {
                for &threshold in &thresholds {
                    for &phase in &timing_phases {
                        let lpf_tag = if cascaded { "4th" } else { "2nd" };
                        let label = format!("G:{}Hz/{}/i{}-{}/th{}/p{}",
                            cutoff, lpf_tag, locked, searching, threshold, phase);

                        let mut demod = Demod9600Gardner::new(config)
                            .with_inertia(locked, searching)
                            .with_threshold(threshold)
                            .with_timing_offset(phase);

                        if cascaded {
                            demod = demod.with_cascaded_lpf();
                        }
                        // Note: Gardner always uses its default 6000 Hz LPF;
                        // cutoff variation is less relevant for Gardner

                        let mut hdlc = SoftHdlcDecoder::new();
                        let mut sym_buf = [DemodSymbol { bit: false, llr: 0 }; 512];
                        let mut hashes: Vec<u32> = Vec::new();

                        for chunk in samples.chunks(1024) {
                            let n = demod.process_samples(chunk, &mut sym_buf);
                            for sym in &sym_buf[..n] {
                                if let Some(FrameResult::Valid(data)) | Some(FrameResult::Recovered { data, .. }) = hdlc.feed_soft_bit(sym.llr) {
                                    let mut h: u32 = 0x811c9dc5;
                                    for &b in data {
                                        h ^= b as u32;
                                        h = h.wrapping_mul(0x01000193);
                                    }
                                    if !hashes.contains(&h) {
                                        hashes.push(h);
                                    }
                                }
                            }
                        }

                        results.push(TuneResult { label, frames: hashes.len() as u32, hashes });
                    }
                }
            }
        }
    }

    let elapsed = start.elapsed();
    println!("  Sweep completed in {:.1}s ({} configs)", elapsed.as_secs_f64(), results.len());
    println!();

    // Sort by frame count descending
    results.sort_by(|a, b| b.frames.cmp(&a.frames));

    // Top 50 single-decoder configs
    println!("Top 50 Single-Decoder Configs:");
    println!("{:<50} {:>8}", "Config", "Frames");
    println!("{}", "-".repeat(60));
    for (i, r) in results.iter().take(50).enumerate() {
        println!("{:>3}. {:<46} {:>8}", i + 1, r.label, r.frames);
    }

    // Greedy set-cover for top N combos
    println!();
    println!("Greedy Set-Cover (optimal N-decoder subsets):");
    println!("{}", "-".repeat(70));

    // Collect all unique hashes across all configs
    let mut all_hashes: Vec<u32> = Vec::new();
    for r in &results {
        for &h in &r.hashes {
            if !all_hashes.contains(&h) {
                all_hashes.push(h);
            }
        }
    }

    let mut covered: Vec<bool> = vec![false; all_hashes.len()];
    let mut selected: Vec<usize> = Vec::new();
    let mut cumulative = 0u32;

    for step in 0..10 {
        let mut best_idx = 0;
        let mut best_gain = 0u32;

        for (ri, r) in results.iter().enumerate() {
            if selected.contains(&ri) { continue; }
            let gain = r.hashes.iter().filter(|h| {
                if let Some(pos) = all_hashes.iter().position(|ah| ah == *h) {
                    !covered[pos]
                } else { false }
            }).count() as u32;
            if gain > best_gain {
                best_gain = gain;
                best_idx = ri;
            }
        }

        if best_gain == 0 { break; }

        // Mark covered
        for &h in &results[best_idx].hashes {
            if let Some(pos) = all_hashes.iter().position(|ah| *ah == h) {
                covered[pos] = true;
            }
        }
        cumulative += best_gain;
        selected.push(best_idx);

        println!("  #{}: +{:>3} = {:>4} total  {}", step + 1, best_gain, cumulative, results[best_idx].label);
    }

    println!();
    println!("Total unique frames across all configs: {}", all_hashes.len());
}

// ─── 9600 Attribution Analysis ────────────────────────────────────────────

/// Per-decoder attribution analysis for Multi9600.
/// Shows which decoders find unique frames, plus greedy set-cover for optimal subsets.
pub fn run_9600_attribution(path: &str) {
    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", path, e); return; }
    };

    println!("9600 Baud Attribution Analysis: {}", path);
    println!("  Sample rate: {} Hz, {:.1}s",
        sample_rate, samples.len() as f64 / sample_rate as f64);

    let config = Demod9600Config::with_sample_rate(sample_rate);

    // Run Multi9600 to get total unique count
    let mut multi = Multi9600Decoder::new(config);
    let mut total_frames = 0u32;
    for chunk in samples.chunks(1024) {
        total_frames += multi.process_samples(chunk).len() as u32;
    }

    println!("  Multi9600: {} decoders, {} unique frames", multi.num_decoders(), total_frames);
    println!();

    // We need to run each decoder slot individually to get per-decoder hashes.
    // Rebuild individual decoders matching the Multi9600 ensemble configuration.
    let period = (sample_rate as i64 * 256 / 9600) as i32;
    let phases = [0i32, period / 3, period * 2 / 3];
    let cutoffs: [u32; 3] = [4800, 6000, 7200];
    let thresholds: [i16; 3] = [-330, 0, 330];
    let inertias: [(i32, i32); 2] = [(205, 128), (180, 100)];

    struct DecoderResult {
        label: String,
        hashes: Vec<u32>,
    }
    let mut dec_results: Vec<DecoderResult> = Vec::new();

    // DW decoders: 3 phases × 3 cutoffs × 3 thresholds = 27
    for (pi, &phase) in phases.iter().enumerate() {
        for &cutoff in &cutoffs {
            for &threshold in &thresholds {
                let label = format!("DW:{}Hz/t{}/th{}", cutoff, pi, threshold);
                let mut demod = Demod9600Direwolf::new(config)
                    .with_cascaded_lpf_cutoff(cutoff)
                    .with_timing_offset(phase)
                    .with_threshold(threshold);
                let mut hdlc = SoftHdlcDecoder::new();
                let mut sym_buf = [DemodSymbol { bit: false, llr: 0 }; 512];
                let mut hashes: Vec<u32> = Vec::new();

                for chunk in samples.chunks(1024) {
                    let n = demod.process_samples(chunk, &mut sym_buf);
                    for sym in &sym_buf[..n] {
                        if let Some(FrameResult::Valid(data)) | Some(FrameResult::Recovered { data, .. }) = hdlc.feed_soft_bit(sym.llr) {
                            let mut h: u32 = 0x811c9dc5;
                            for &b in data { h ^= b as u32; h = h.wrapping_mul(0x01000193); }
                            if !hashes.contains(&h) { hashes.push(h); }
                        }
                    }
                }
                dec_results.push(DecoderResult { label, hashes });
            }
        }
    }

    // Gardner decoders: 3 phases × 2 inertias × 3 thresholds = 18
    for (pi, &phase) in phases.iter().enumerate() {
        for &(locked, searching) in &inertias {
            for &threshold in &thresholds {
                let label = format!("G:i{}/t{}/th{}", locked, pi, threshold);
                let mut demod = Demod9600Gardner::new(config)
                    .with_cascaded_lpf()
                    .with_inertia(locked, searching)
                    .with_timing_offset(phase)
                    .with_threshold(threshold);
                let mut hdlc = SoftHdlcDecoder::new();
                let mut sym_buf = [DemodSymbol { bit: false, llr: 0 }; 512];
                let mut hashes: Vec<u32> = Vec::new();

                for chunk in samples.chunks(1024) {
                    let n = demod.process_samples(chunk, &mut sym_buf);
                    for sym in &sym_buf[..n] {
                        if let Some(FrameResult::Valid(data)) | Some(FrameResult::Recovered { data, .. }) = hdlc.feed_soft_bit(sym.llr) {
                            let mut h: u32 = 0x811c9dc5;
                            for &b in data { h ^= b as u32; h = h.wrapping_mul(0x01000193); }
                            if !hashes.contains(&h) { hashes.push(h); }
                        }
                    }
                }
                dec_results.push(DecoderResult { label, hashes });
            }
        }
    }

    // Collect all unique hashes
    let mut all_hashes: Vec<u32> = Vec::new();
    for r in &dec_results {
        for &h in &r.hashes {
            if !all_hashes.contains(&h) {
                all_hashes.push(h);
            }
        }
    }

    // Per-decoder stats
    println!("Per-Decoder Frame Counts:");
    println!("{:<30} {:>8} {:>8}", "Decoder", "Frames", "Unique");
    println!("{}", "-".repeat(48));

    // Calculate unique-to-this-decoder count
    for (i, r) in dec_results.iter().enumerate() {
        let unique_count = r.hashes.iter().filter(|h| {
            dec_results.iter().enumerate().filter(|(j, _)| *j != i).all(|(_j, other)| !other.hashes.contains(h))
        }).count();
        println!("{:<30} {:>8} {:>8}", r.label, r.hashes.len(), unique_count);
    }

    // Greedy set-cover
    println!();
    println!("Greedy Set-Cover (optimal N-decoder subsets):");
    println!("{}", "-".repeat(70));

    let mut covered: Vec<bool> = vec![false; all_hashes.len()];
    let mut selected: Vec<usize> = Vec::new();
    let mut cumulative = 0u32;

    for step in 0..10 {
        let mut best_idx = 0;
        let mut best_gain = 0u32;

        for (ri, r) in dec_results.iter().enumerate() {
            if selected.contains(&ri) { continue; }
            let gain = r.hashes.iter().filter(|h| {
                if let Some(pos) = all_hashes.iter().position(|ah| ah == *h) {
                    !covered[pos]
                } else { false }
            }).count() as u32;
            if gain > best_gain {
                best_gain = gain;
                best_idx = ri;
            }
        }

        if best_gain == 0 { break; }

        for &h in &dec_results[best_idx].hashes {
            if let Some(pos) = all_hashes.iter().position(|ah| *ah == h) {
                covered[pos] = true;
            }
        }
        cumulative += best_gain;
        selected.push(best_idx);

        let pct = cumulative as f64 / all_hashes.len().max(1) as f64 * 100.0;
        println!("  #{}: +{:>3} = {:>4} ({:>5.1}%)  {}", step + 1, best_gain, cumulative, pct, dec_results[best_idx].label);
    }

    println!();
    println!("Total unique frames: {}", all_hashes.len());
    println!("Multi9600 total: {}", total_frames);

    // Also run Mini9600 for comparison
    let mut mini = Mini9600Decoder::new(config);
    let mut mini_frames = 0u32;
    for chunk in samples.chunks(1024) {
        mini_frames += mini.process_samples(chunk).len() as u32;
    }
    println!("Mini9600 total: {}", mini_frames);
}
