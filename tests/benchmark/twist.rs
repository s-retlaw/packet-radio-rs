//! Twist-tuned decoder benchmarks.

use std::time::Instant;

use packet_radio_core::modem::demod::{DemodSymbol, FastDemodulator};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};

use crate::common::*;

// ─── TwistMini Decoder ─────────────────────────────────────────────────

pub fn run_twist_mini(path: &str) {
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

    // --- Try 12000 Hz variant (48k ÷ 4 decimation target) ---
    {
        let path_12k = path.replace(".wav", "_12000.wav");
        if let Ok((rate_12k, samples_12k)) = read_wav_file(&path_12k) {
            println!();
            println!("At {} Hz (from {}):", rate_12k, path_12k);

            let fast_12k = decode_fast(&samples_12k, rate_12k);
            let (smart3_12k, _) = decode_smart3(&samples_12k, rate_12k);
            let (twist_mini_12k, _) = decode_twist_mini(&samples_12k, rate_12k);
            let (multi_12k, _) = decode_multi(&samples_12k, rate_12k);

            let dur_12k = samples_12k.len() as f64 / rate_12k as f64;
            println!("  Fast (1×):       {:>4} packets in {:.2}s ({:.0}x real-time)",
                fast_12k.frames.len(), fast_12k.elapsed.as_secs_f64(),
                dur_12k / fast_12k.elapsed.as_secs_f64());
            println!("  Smart3 (3×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
                smart3_12k.frames.len(), smart3_12k.elapsed.as_secs_f64(),
                dur_12k / smart3_12k.elapsed.as_secs_f64());
            println!("  TwistMini (6×):  {:>4} packets in {:.2}s ({:.0}x real-time)",
                twist_mini_12k.frames.len(), twist_mini_12k.elapsed.as_secs_f64(),
                dur_12k / twist_mini_12k.elapsed.as_secs_f64());
            println!("  Multi (38×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
                multi_12k.frames.len(), multi_12k.elapsed.as_secs_f64(),
                dur_12k / multi_12k.elapsed.as_secs_f64());

            let gain_12k = twist_mini_12k.frames.len() as i64 - smart3_12k.frames.len() as i64;
            let gain_vs_native = twist_mini_12k.frames.len() as i64 - twist_mini.frames.len() as i64;
            println!("  TwistMini@12k vs Smart3@12k: {:>+4} packets", gain_12k);
            println!("  TwistMini@12k vs TwistMini@native: {:>+4} packets", gain_vs_native);
        }
    }

    // --- Try 48000 Hz variant ---
    {
        let _target_48k = 48000u32;
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

    // --- Try 26400 Hz variant (48k ÷ 2 decimation target, or 2× oversampled) ---
    {
        let path_26k = path.replace(".wav", "_26400.wav");
        if let Ok((rate_26k, samples_26k)) = read_wav_file(&path_26k) {
            println!();
            println!("At {} Hz (from {}):", rate_26k, path_26k);

            let fast_26k = decode_fast(&samples_26k, rate_26k);
            let (smart3_26k, _) = decode_smart3(&samples_26k, rate_26k);
            let (twist_mini_26k, _) = decode_twist_mini(&samples_26k, rate_26k);
            let (multi_26k, _) = decode_multi(&samples_26k, rate_26k);

            let dur_26k = samples_26k.len() as f64 / rate_26k as f64;
            println!("  Fast (1×):       {:>4} packets in {:.2}s ({:.0}x real-time)",
                fast_26k.frames.len(), fast_26k.elapsed.as_secs_f64(),
                dur_26k / fast_26k.elapsed.as_secs_f64());
            println!("  Smart3 (3×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
                smart3_26k.frames.len(), smart3_26k.elapsed.as_secs_f64(),
                dur_26k / smart3_26k.elapsed.as_secs_f64());
            println!("  TwistMini (6×):  {:>4} packets in {:.2}s ({:.0}x real-time)",
                twist_mini_26k.frames.len(), twist_mini_26k.elapsed.as_secs_f64(),
                dur_26k / twist_mini_26k.elapsed.as_secs_f64());
            println!("  Multi (38×):     {:>4} packets in {:.2}s ({:.0}x real-time)",
                multi_26k.frames.len(), multi_26k.elapsed.as_secs_f64(),
                dur_26k / multi_26k.elapsed.as_secs_f64());

            let gain_26k = twist_mini_26k.frames.len() as i64 - smart3_26k.frames.len() as i64;
            let gain_vs_native = twist_mini_26k.frames.len() as i64 - twist_mini.frames.len() as i64;
            println!("  TwistMini@26k vs Smart3@26k: {:>+4} packets", gain_26k);
            println!("  TwistMini@26k vs TwistMini@native: {:>+4} packets", gain_vs_native);
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

    let config = config_for_rate(sample_rate, get_baud());

    let phase_offset = timing_phase * sample_rate / 3;

    // Shift the BPF center to favor one tone over the other
    let bpf = if bpf_center_offset != 0 {
        let center = (1700i32 + bpf_center_offset) as f64;
        filter::bandpass_coeffs(sample_rate, center, 2000.0)
    } else {
        filter::afsk_bandpass_11025()
    };

    let mut demod = FastDemodulator::new(config).filter(bpf).phase_offset(phase_offset)
        .with_space_gain(space_gain_q8)
        .with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0, sample_idx: 0, raw_bit: false }; 1024];

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
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

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

pub fn run_twist_sweep(path: &str) {
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
    // BPF center offsets: -200 to +300 Hz in 100 Hz steps
    // Space gains (Q8): 0.75 dB steps from -3dB to +6dB (13 values)
    // Timing phases: 0, 1, 2
    let bpf_offsets = [-200i32, -100, 0, 100, 200, 300];
    let gains: [(u16, &str); 13] = [
        (128, "-3.0dB"), (152, "-2.25dB"), (181, "-1.5dB"), (215, "-0.75dB"),
        (256, " 0.0dB"), (304, "+0.75dB"), (362, "+1.5dB"), (430, "+2.25dB"),
        (512, "+3.0dB"), (608, "+3.75dB"), (724, "+4.5dB"), (868, "+5.3dB"),
        (1024, "+6.0dB"),
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
