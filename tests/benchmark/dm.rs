//! Delay-multiply demodulator benchmarks.

use std::time::Instant;

use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::modem::demod::{DemodSymbol, DmDemodulator};
use packet_radio_core::modem::soft_hdlc::SoftHdlcDecoder;

use crate::common::*;

/// Decode with DM using a specific delay value and optional BPF/LPF.
fn decode_dm_custom(
    samples: &[i16],
    sample_rate: u32,
    delay: usize,
    use_bpf: bool,
) -> DecodeResult {
    use packet_radio_core::modem::delay_multiply::DelayMultiplyDetector;
    use packet_radio_core::modem::filter::BiquadFilter;

    let config = config_for_rate(sample_rate, get_baud());

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
                accumulator < 0 // mark gives negative output
            } else {
                accumulator > 0 // mark gives positive output
            };

            let decoded_bit = raw_bit == prev_nrzi_bit;
            prev_nrzi_bit = raw_bit;

            if let Some(frame) = hdlc.feed_bit(decoded_bit) {
                frames.push(frame.to_vec());
            }

            accumulator = 0;
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

pub fn run_dm_single(path: &str) {
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
        duration_secs,
        samples.len(),
        sample_rate
    );
    println!();

    let fast = decode_fast(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);
    println!(
        "  Fast:   {:>4}   Multi: {:>4}",
        fast.frames.len(),
        multi.frames.len()
    );
    println!();

    // Sweep delays with BPF+LPF
    println!("  Delay sweep at {} Hz — BPF+LPF:", sample_rate);
    for delay in 1..16 {
        let tau_us = delay as f64 / sample_rate as f64 * 1e6;
        let mark_cos =
            (2.0 * std::f64::consts::PI * 1200.0 * delay as f64 / sample_rate as f64).cos();
        let space_cos =
            (2.0 * std::f64::consts::PI * 2200.0 * delay as f64 / sample_rate as f64).cos();
        let sep = (mark_cos - space_cos).abs();
        let polarity = if mark_cos < 0.0 { "M-" } else { "M+" };
        let result = decode_dm_custom(&samples, sample_rate, delay, true);
        println!(
            "    d={:>2} τ={:>5.0}μs sep={:.2} {} → {:>4} packets",
            delay,
            tau_us,
            sep,
            polarity,
            result.frames.len()
        );
    }
    println!();

    // Also sweep without BPF+LPF
    println!("  Delay sweep at {} Hz — no filters:", sample_rate);
    for delay in 1..16 {
        let tau_us = delay as f64 / sample_rate as f64 * 1e6;
        let mark_cos =
            (2.0 * std::f64::consts::PI * 1200.0 * delay as f64 / sample_rate as f64).cos();
        let space_cos =
            (2.0 * std::f64::consts::PI * 2200.0 * delay as f64 / sample_rate as f64).cos();
        let sep = (mark_cos - space_cos).abs();
        let polarity = if mark_cos < 0.0 { "M-" } else { "M+" };
        let result = decode_dm_custom(&samples, sample_rate, delay, false);
        println!(
            "    d={:>2} τ={:>5.0}μs sep={:.2} {} → {:>4} packets",
            delay,
            tau_us,
            sep,
            polarity,
            result.frames.len()
        );
    }
}

// ─── DM+PLL Decode Engine ─────────────────────────────────────────────────

/// Decode using DM+PLL with configurable options.
pub fn decode_dm_pll_opts(
    samples: &[i16],
    sample_rate: u32,
    alpha: i16,
    beta: i16,
    adaptive: bool,
    preemph: i16,
    hysteresis: i16,
) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

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

    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using DM+PLL with SoftHdlcDecoder for bit-flip recovery.
pub fn decode_dm_pll_soft(
    samples: &[i16],
    sample_rate: u32,
    alpha: i16,
    beta: i16,
    adaptive: bool,
    preemph: i16,
    hysteresis: i16,
) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

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

    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Simple DM+PLL decode with default gains.
pub fn decode_dm_pll(samples: &[i16], sample_rate: u32) -> DecodeResult {
    decode_dm_pll_opts(samples, sample_rate, 400, 30, false, 0, 0)
}

/// Decode DM+PLL with symbol counting for diagnostics.
fn decode_dm_pll_counted(
    samples: &[i16],
    sample_rate: u32,
    alpha: i16,
    beta: i16,
) -> (DecodeResult, usize, u32) {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = DmDemodulator::with_bpf_pll_custom(config, alpha, beta);
    let mut hdlc = HdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut symbols = [DemodSymbol {
        bit: false,
        llr: 0,
        sample_idx: 0,
        raw_bit: false,
    }; 1024];
    let mut total_syms = 0usize;
    let mut flags = 0u32;
    let mut shift_reg: u8 = 0;

    let start = Instant::now();
    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        total_syms += n;
        for sym in &symbols[..n] {
            shift_reg = (shift_reg >> 1) | if sym.bit { 0x80 } else { 0 };
            if shift_reg == 0x7E {
                flags += 1;
            }
            if let Some(frame) = hdlc.feed_bit(sym.bit) {
                frames.push(frame.to_vec());
            }
        }
    }
    (
        DecodeResult {
            frames,
            elapsed: start.elapsed(),
        },
        total_syms,
        flags,
    )
}

// ─── DM+PLL Single File Analysis ────────────────────────────────────────

pub fn run_dm_pll(path: &str) {
    println!("═══ DM+PLL Demodulator Variants ═══");
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
        duration_secs,
        samples.len(),
        sample_rate
    );

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);
    let dm_bres = decode_dm(&samples, sample_rate);
    // DM+Bresenham with adaptive
    let dm_bres_adapt = {
        let config = config_for_rate(sample_rate, get_baud());
        let demod = DmDemodulator::with_bpf(config).with_adaptive();
        let mut hdlc = HdlcDecoder::new();
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut symbols = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; 1024];
        let mut dm = demod;
        for chunk in samples.chunks(1024) {
            let n = dm.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if let Some(frame) = hdlc.feed_bit(sym.bit) {
                    frames.push(frame.to_vec());
                }
            }
        }
        frames.len()
    };

    println!("  Baselines:");
    println!("    Fast (Goertzel+Bresenham):  {:>5}", fast.frames.len());
    println!(
        "    DM+Bresenham:               {:>5}",
        dm_bres.frames.len()
    );
    println!("    DM+Bres+adaptive:           {:>5}", dm_bres_adapt);
    println!("    Multi (38 decoders):         {:>5}", multi.frames.len());
    println!();

    // Diagnostic: symbol count and flag detection
    let (r_diag, sym_count, flag_count) = decode_dm_pll_counted(&samples, sample_rate, 400, 30);
    let expected_syms = (samples.len() as u64 * 1200 / sample_rate as u64) as usize;
    println!("  PLL diagnostics (a=400, b=30):");
    println!(
        "    Symbols produced: {} (expected ~{})",
        sym_count, expected_syms
    );
    println!("    Flags detected:   {}", flag_count);
    println!("    Frames decoded:   {}", r_diag.frames.len());

    // Also compare with Bresenham
    {
        let config = config_for_rate(sample_rate, get_baud());
        let mut demod = DmDemodulator::with_bpf(config);
        let mut symbols_buf = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; 1024];
        let mut bres_syms = 0usize;
        let mut bres_flags = 0u32;
        let mut shift: u8 = 0;
        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols_buf);
            bres_syms += n;
            for sym in &symbols_buf[..n] {
                shift = (shift >> 1) | if sym.bit { 0x80 } else { 0 };
                if shift == 0x7E {
                    bres_flags += 1;
                }
            }
        }
        println!("    Bresenham syms:   {} flags: {}", bres_syms, bres_flags);
    }
    println!();

    // DM+PLL variants with best alpha, testing beta and features
    println!("  DM+PLL variants (alpha=936):");
    let variants: &[(&str, i16, bool, i16, i16)] = &[
        // (name, beta, adaptive, preemph, hysteresis)
        ("DM+PLL b=0", 0, false, 0, 0),
        ("DM+PLL b=10", 10, false, 0, 0),
        ("DM+PLL b=30", 30, false, 0, 0),
        ("DM+PLL b=0 +adaptive", 0, true, 0, 0),
        ("DM+PLL b=0 +preemph(0.90)", 0, false, 29491, 0),
        ("DM+PLL b=0 +preemph(0.95)", 0, false, 31130, 0),
        ("DM+PLL b=0 +adapt+preemph(0.90)", 0, true, 29491, 0),
        ("DM+PLL b=0 +adapt+preemph(0.95)", 0, true, 31130, 0),
        ("DM+PLL b=10 +adapt+preemph(0.95)", 10, true, 31130, 0),
        // With hysteresis
        ("DM+PLL b=10 hyst=50", 10, false, 0, 50),
        ("DM+PLL b=30 hyst=50", 30, false, 0, 50),
        ("DM+PLL b=10 hyst=100", 10, false, 0, 100),
        ("DM+PLL b=30 hyst=100", 30, false, 0, 100),
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
        ("DM+PLL+Soft b=0", 0, false, 0, 0),
        ("DM+PLL+Soft b=10", 10, false, 0, 0),
        ("DM+PLL+Soft b=30", 30, false, 0, 0),
        ("DM+PLL+Soft b=74", 74, false, 0, 0),
        ("DM+PLL+Soft b=74 +adaptive", 74, true, 0, 0),
    ];

    for &(name, beta, adaptive, preemph, hyst) in soft_variants {
        let (r, saves) =
            decode_dm_pll_soft(&samples, sample_rate, 936, beta, adaptive, preemph, hyst);
        println!(
            "    {:<38} {:>5} ({} soft saves)",
            name,
            r.frames.len(),
            saves
        );
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

pub fn run_dm_pll_sweep(path: &str) {
    println!("═══ DM+PLL Alpha/Beta Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s at {} Hz\n", duration_secs, sample_rate);

    let alphas = [100i16, 200, 300, 400, 500, 600, 800, 936, 1200, 1500];
    let betas = [10i16, 20, 30, 40, 50, 60, 74, 80, 100, 120];

    // Header
    print!("  {:>6}", "a\\b");
    for &b in &betas {
        print!(" {:>5}", b);
    }
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

    println!(
        "\n  Best: alpha={}, beta={} → {} frames",
        best_a, best_b, best_count
    );

    // Now sweep with adaptive + best alpha/beta
    println!("\n  Best alpha/beta with adaptive + pre-emphasis:");
    let preemphs = [0i16, 26214, 29491, 31130, 32440];
    let preemph_names = ["none", "0.80", "0.90", "0.95", "0.99"];
    for (i, &pe) in preemphs.iter().enumerate() {
        let r_plain = decode_dm_pll_opts(&samples, sample_rate, best_a, best_b, false, pe, 0);
        let r_adapt = decode_dm_pll_opts(&samples, sample_rate, best_a, best_b, true, pe, 0);
        println!(
            "    preemph={:<5}  plain={:>5}  adaptive={:>5}",
            preemph_names[i],
            r_plain.frames.len(),
            r_adapt.frames.len()
        );
    }
}

// ─── DM+PLL Parameter Tune (Two-Stage Sweep) ─────────────────────────

/// Decode DM+PLL with all tunable parameters.
#[allow(clippy::too_many_arguments)]
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
    let config = config_for_rate(sample_rate, get_baud());

    let mut demod = DmDemodulator::with_bpf_pll_custom(config, alpha, beta)
        .with_pll_error_shift(error_shift)
        .with_pll_smoothing(smooth_shift)
        .with_llr_shift(llr_shift);

    let mut symbols = [DemodSymbol {
        bit: false,
        llr: 0,
        sample_idx: 0,
        raw_bit: false,
    }; 1024];

    if use_soft {
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut frame_count = 0usize;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if soft_hdlc.feed_soft_bit(sym.llr).is_some() {
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
            for sym in &symbols[..n] {
                if hdlc.feed_bit(sym.bit).is_some() {
                    frame_count += 1;
                }
            }
        }
        (frame_count, 0)
    }
}

pub fn run_dm_pll_tune(path: &str) {
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
    println!(
        "  Baselines: fast={}, multi={}",
        fast.frames.len(),
        multi.frames.len()
    );
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
                let (frames, _) =
                    decode_dm_pll_tuned(&samples, sample_rate, 936, b, es, ss, 6, false);
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
            i + 1,
            r.error_shift,
            r.smooth_shift,
            r.beta,
            r.frames
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
                &samples,
                sample_rate,
                a,
                best_beta,
                best_es,
                best_ss,
                ls,
                false,
            );
            let (soft, soft_saves) = decode_dm_pll_tuned(
                &samples,
                sample_rate,
                a,
                best_beta,
                best_es,
                best_ss,
                ls,
                true,
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
    stage2_results.sort_by(|a, b| b.soft.cmp(&a.soft).then(b.hard.cmp(&a.hard)));

    println!(
        "  {:>3}  {:>5}  {:>9}  {:>5}  {:>5}  {:>10}",
        "#", "alpha", "llr_shift", "hard", "soft", "soft_saves"
    );
    println!("  {}", "─".repeat(45));

    let show_s2 = stage2_results.len().min(20);
    for (i, r) in stage2_results.iter().take(show_s2).enumerate() {
        println!(
            "  {:>3}  {:>5}  {:>9}  {:>5}  {:>5}  {:>10}",
            i + 1,
            r.alpha,
            r.llr_shift,
            r.hard,
            r.soft,
            r.soft_saves
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
    println!(
        "  soft frames:      {} ({} soft saves)",
        best_s2.soft, best_s2.soft_saves
    );
}

// ─── DM Debug Diagnostics ──────────────────────────────────────────────

pub fn run_dm_debug(path: &str) {
    use packet_radio_core::modem::delay_multiply::DelayMultiplyDetector;

    println!("═══ DM Discriminator Diagnostics ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
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

    let mut pll = packet_radio_core::modem::pll::ClockRecoveryPll::new(sample_rate, 1200, 400, 30);

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

        csv.push_str(&format!(
            "{},{},{},{},{},{}\n",
            i,
            disc_out,
            leaky,
            pll.phase,
            if pll.locked { 1 } else { 0 },
            if sym.is_some() { 1 } else { 0 },
        ));
    }

    match std::fs::write(&csv_path, &csv) {
        Ok(_) => println!("Wrote {} samples to {}", work_samples.len(), csv_path),
        Err(e) => eprintln!("Error writing {}: {}", csv_path, e),
    }
}
