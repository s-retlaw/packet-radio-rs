//! 300 Baud PLL tuning benchmarks.

use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::modem::demod::{CorrelationDemodulator, DemodSymbol};

use crate::common::*;
use crate::dm::{decode_dm_pll_opts, decode_dm_pll_soft};

// ─── 300 Baud PLL Tuning ────────────────────────────────────────────────

pub fn run_pll_300(path: &str) {
    println!("═══ 300 Baud PLL Timing Recovery Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    let spb = sample_rate as f64 / get_baud() as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz (SPB={:.1}, baud={})",
        duration_secs, samples.len(), sample_rate, spb, get_baud());
    println!();

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (smart3, _) = decode_smart3(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);
    println!("  Baselines:");
    println!("    Fast (Goertzel):    {:>4}", fast.frames.len());
    println!("    Smart3:             {:>4}", smart3.frames.len());
    println!("    Multi (38×):        {:>4}", multi.frames.len());
    println!();

    // DM+PLL wide alpha sweep (300 baud needs much wider bandwidth)
    println!("  DM+PLL alpha sweep (beta=0, no preemph):");
    println!("  {:>6}  {:>5}  {:>5}", "Alpha", "Hard", "Soft");
    println!("  {}", "─".repeat(22));

    let alphas: &[i16] = &[
        200, 400, 600, 800, 936, 1200, 1500, 2000, 2500, 3000,
        4000, 5000, 6000, 8000, 10000, 12000, 15000, 20000,
    ];

    let mut best_hard = 0usize;
    let mut best_hard_alpha = 0i16;
    let mut best_soft = 0usize;
    let mut best_soft_alpha = 0i16;

    for &a in alphas {
        let hard = decode_dm_pll_opts(&samples, sample_rate, a, 0, false, 0, 0);
        let (soft, saves) = decode_dm_pll_soft(&samples, sample_rate, a, 0, false, 0, 0);
        let hard_count = hard.frames.len();
        let soft_count = soft.frames.len();
        let marker = if hard_count >= best_hard && hard_count > 0 { " ★" } else { "" };
        println!("  {:>6}  {:>5}  {:>5} ({} saves){}",
            a, hard_count, soft_count, saves, marker);
        if hard_count > best_hard {
            best_hard = hard_count;
            best_hard_alpha = a;
        }
        if soft_count > best_soft {
            best_soft = soft_count;
            best_soft_alpha = a;
        }
    }

    println!();
    println!("  Best hard: alpha={} → {} frames", best_hard_alpha, best_hard);
    println!("  Best soft: alpha={} → {} frames", best_soft_alpha, best_soft);

    // Beta sweep at best alpha
    if best_hard_alpha > 0 {
        println!();
        println!("  Beta sweep at alpha={}:", best_hard_alpha);
        let betas: &[i16] = &[0, 1, 2, 5, 10, 20, 50, 100, 200, 500];
        for &b in betas {
            let hard = decode_dm_pll_opts(&samples, sample_rate, best_hard_alpha, b, false, 0, 0);
            println!("    beta={:<5} → {:>5}", b, hard.frames.len());
        }
    }

    // Corr+PLL sweep
    println!();
    println!("  Corr+PLL alpha sweep:");
    println!("  {:>6}  {:>5}", "Alpha", "Pkts");
    println!("  {}", "─".repeat(14));

    for &a in &[200i16, 400, 600, 936, 1500, 2000, 3000, 5000, 8000, 12000, 20000] {
        let config = config_for_rate(sample_rate, get_baud());
        let pll = packet_radio_core::modem::pll::ClockRecoveryPll::new_gardner(
            sample_rate, get_baud(), a, 0,
        ).with_error_shift(8);
        let mut demod = CorrelationDemodulator::new(config)
            .with_adaptive_gain()
            .with_custom_pll(pll);
        let mut hdlc = HdlcDecoder::new();
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if let Some(frame) = hdlc.feed_bit(sym.bit) {
                    frames.push(frame.to_vec());
                }
            }
        }
        println!("  {:>6}  {:>5}", a, frames.len());
    }

    println!();
}
