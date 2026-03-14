//! Smart3 mini-decoder benchmarks.

use packet_radio_core::modem::demod::{DemodSymbol, FastDemodulator};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};
use packet_radio_core::modem::DemodConfig;

use crate::common::*;

// ─── Smart3 Single WAV Decode ─────────────────────────────────────────────

pub fn run_smart3(path: &str) {
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
        if !fast.frames.is_empty() { gain as f64 / fast.frames.len() as f64 * 100.0 } else { 0.0 });
    println!();
}

// ─── Smart3 Parameter Sweep ─────────────────────────────────────────────

/// Sweep Smart3 base decoder parameters to find optimal configs at current sample rate.
/// Smart3 = D1(freq-50/wide/t2) + D2(narrow/t0) + D3(narrow/t1).
/// This sweeps freq offsets, BPF types, and timing phases for each slot.
pub fn run_smart3_sweep(path: &str) {
    use std::collections::HashSet;
    use packet_radio_core::modem::filter;

    println!("═══ Smart3 Parameter Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz\n", duration_secs, samples.len(), sample_rate);

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (smart3, _) = decode_smart3(&samples, sample_rate);
    println!("  Baselines:  Fast={}, Smart3={}\n", fast.frames.len(), smart3.frames.len());

    let smart3_set: HashSet<Vec<u8>> = smart3.frames.iter().cloned().collect();

    // Parameters to sweep
    let freq_offsets = [-125i32, -100, -75, -50, -25, 0, 25, 50, 75, 100, 125];
    #[allow(clippy::type_complexity)]
    let bpf_types: [(&str, Box<dyn Fn(u32, f64) -> packet_radio_core::modem::filter::BiquadFilter>); 3] = [
        ("narrow", Box::new(|sr, center| filter::bandpass_coeffs(sr, center, 1200.0))),
        ("std",    Box::new(|sr, center| filter::bandpass_coeffs(sr, center, 1600.0))),
        ("wide",   Box::new(|sr, center| filter::bandpass_coeffs(sr, center, 2000.0))),
    ];
    let timing_phases = [0u32, 1, 2];

    struct SweepResult {
        freq_off: i32,
        bpf_type: String,
        timing: u32,
        count: usize,
        exclusive: usize,
    }
    let mut results: Vec<SweepResult> = Vec::new();

    println!("  {:>6} {:>6} {:>2}  {:>5}  not-in-S3", "Freq", "BPF", "T", "Pkts");
    println!("  {}", "─".repeat(40));

    // Sweep single decoders across all parameter combos
    for &freq_off in &freq_offsets {
        for (bpf_name, bpf_fn) in &bpf_types {
            for &t in &timing_phases {
                let mut config = DemodConfig::default_1200();
                config.sample_rate = sample_rate;

                let mark = (config.mark_freq as i32 + freq_off) as u32;
                let space = (config.space_freq as i32 + freq_off) as u32;
                let center = (1700i32 + freq_off) as f64;
                let bpf = bpf_fn(sample_rate, center);
                let phase_offset = t * sample_rate / 3;

                let mut demod = FastDemodulator::new(config).filter(bpf).phase_offset(phase_offset)
                    .frequencies(mark, space).with_energy_llr();
                let mut soft_hdlc = SoftHdlcDecoder::new();
                let mut frames: Vec<Vec<u8>> = Vec::new();
                let mut symbols = [DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 1024];

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

                let frame_set: HashSet<Vec<u8>> = frames.iter().cloned().collect();
                let exclusive = frame_set.difference(&smart3_set).count();

                results.push(SweepResult {
                    freq_off,
                    bpf_type: bpf_name.to_string(),
                    timing: t,
                    count: frames.len(),
                    exclusive,
                });
            }
        }
    }

    // Sort by packet count descending
    results.sort_by(|a, b| b.count.cmp(&a.count).then(b.exclusive.cmp(&a.exclusive)));

    for r in &results {
        let marker = if r.exclusive > 0 { " ★" } else { "" };
        println!("  {:>+4} Hz {:>6} t{}  {:>5}  {:>4}{}",
            r.freq_off, r.bpf_type, r.timing, r.count, r.exclusive, marker);
    }

    // Show top 15 by packet count
    println!("\n  Top 15 single decoders by packet count:");
    for (i, r) in results.iter().take(15).enumerate() {
        println!("    {:>2}. freq={:>+4} bpf={:>6} t{}: {} pkts ({} exclusive)",
            i + 1, r.freq_off, r.bpf_type, r.timing, r.count, r.exclusive);
    }

    // Now test 3-decoder ensembles: sweep D1 freq offset and timing, keep D2/D3 as best narrow pair
    println!("\n  ─── 3-Decoder Ensemble Sweep ───");
    println!("  Sweep D1 (freq+bpf+timing), D2/D3 as narrow at best timing pair\n");

    // Find best narrow timing pair for D2+D3
    let narrow_results: Vec<&SweepResult> = results.iter()
        .filter(|r| r.bpf_type == "narrow" && r.freq_off == 0)
        .collect();
    println!("  Narrow decoders (freq=0):");
    for r in &narrow_results {
        println!("    t{}: {} pkts", r.timing, r.count);
    }

    // Test all 3-decoder combos with dedup
    struct EnsembleResult {
        d1_freq: i32,
        d1_bpf: String,
        d1_timing: u32,
        d2_timing: u32,
        d3_timing: u32,
        total: usize,
    }
    let mut ensembles: Vec<EnsembleResult> = Vec::new();

    // Sweep D1 across top configs, D2/D3 as narrow with timing diversity
    let d1_freq_offsets = [-125i32, -100, -75, -50, -25, 0, 25, 50, 75, 100];
    #[allow(clippy::type_complexity)]
    let d1_bpf_types: [(&str, Box<dyn Fn(u32, f64) -> filter::BiquadFilter>); 2] = [
        ("std",  Box::new(|sr, center| filter::bandpass_coeffs(sr, center, 1600.0))),
        ("wide", Box::new(|sr, center| filter::bandpass_coeffs(sr, center, 2000.0))),
    ];
    // D2/D3 timing pairs (distinct phases)
    let d23_pairs = [(0u32, 1u32), (0, 2), (1, 2)];

    for &d1_freq in &d1_freq_offsets {
        for (d1_bpf_name, d1_bpf_fn) in &d1_bpf_types {
            for &d1_t in &timing_phases {
                for &(d2_t, d3_t) in &d23_pairs {
                    // Skip if D1 timing overlaps D2 or D3
                    if d1_t == d2_t || d1_t == d3_t { continue; }

                    let mut config = DemodConfig::default_1200();
                    config.sample_rate = sample_rate;

                    // D1: freq-shifted + chosen BPF
                    let mark1 = (config.mark_freq as i32 + d1_freq) as u32;
                    let space1 = (config.space_freq as i32 + d1_freq) as u32;
                    let center1 = (1700i32 + d1_freq) as f64;
                    let bpf1 = d1_bpf_fn(sample_rate, center1);
                    let off1 = d1_t * sample_rate / 3;

                    // D2, D3: narrow, freq=0
                    let narrow = filter::bandpass_coeffs(sample_rate, 1700.0, 1200.0);
                    let off2 = d2_t * sample_rate / 3;
                    let off3 = d3_t * sample_rate / 3;

                    let mut d1 = FastDemodulator::new(config).filter(bpf1).phase_offset(off1).frequencies(mark1, space1).with_energy_llr();
                    let mut d2 = FastDemodulator::new(config).filter(narrow).phase_offset(off2).with_energy_llr();
                    let mut d3 = FastDemodulator::new(config).filter(narrow).phase_offset(off3).with_energy_llr();
                    let mut h1 = SoftHdlcDecoder::new();
                    let mut h2 = SoftHdlcDecoder::new();
                    let mut h3 = SoftHdlcDecoder::new();

                    let mut all_frames: HashSet<Vec<u8>> = HashSet::new();
                    let mut symbols = [DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 1024];

                    for chunk in samples.chunks(1024) {
                        let n1 = d1.process_samples(chunk, &mut symbols);
                        for sym in &symbols[..n1] {
                            if let Some(result) = h1.feed_soft_bit(sym.llr) {
                                let data = match &result {
                                    FrameResult::Valid(d) => d,
                                    FrameResult::Recovered { data, .. } => data,
                                };
                                all_frames.insert(data.to_vec());
                            }
                        }
                        let n2 = d2.process_samples(chunk, &mut symbols);
                        for sym in &symbols[..n2] {
                            if let Some(result) = h2.feed_soft_bit(sym.llr) {
                                let data = match &result {
                                    FrameResult::Valid(d) => d,
                                    FrameResult::Recovered { data, .. } => data,
                                };
                                all_frames.insert(data.to_vec());
                            }
                        }
                        let n3 = d3.process_samples(chunk, &mut symbols);
                        for sym in &symbols[..n3] {
                            if let Some(result) = h3.feed_soft_bit(sym.llr) {
                                let data = match &result {
                                    FrameResult::Valid(d) => d,
                                    FrameResult::Recovered { data, .. } => data,
                                };
                                all_frames.insert(data.to_vec());
                            }
                        }
                    }

                    ensembles.push(EnsembleResult {
                        d1_freq,
                        d1_bpf: d1_bpf_name.to_string(),
                        d1_timing: d1_t,
                        d2_timing: d2_t,
                        d3_timing: d3_t,
                        total: all_frames.len(),
                    });
                }
            }
        }
    }

    ensembles.sort_by(|a, b| b.total.cmp(&a.total));

    println!("\n  Top 20 ensembles (3 decoders, deduped):");
    println!("  {:>6} {:>5} {:>8} {:>8}  {:>5}", "D1freq", "D1bpf", "D1/D2/D3", "timings", "Total");
    println!("  {}", "─".repeat(45));
    for (i, e) in ensembles.iter().take(20).enumerate() {
        println!("  {:>+4}Hz {:>5} t{}/t{}/t{}            {:>5}{}",
            e.d1_freq, e.d1_bpf, e.d1_timing, e.d2_timing, e.d3_timing,
            e.total,
            if i == 0 { " ★" } else { "" });
    }

    println!("\n  Current Smart3: freq-50/wide/t2 + narrow/t0 + narrow/t1 = {}", smart3_set.len());
    println!();
}
