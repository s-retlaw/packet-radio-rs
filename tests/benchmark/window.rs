//! Windowed Goertzel sweep benchmarks.

use packet_radio_core::modem::demod::{FastDemodulator, GoertzelWindow};

use crate::common::*;

// ─── Windowed Goertzel Sweep ────────────────────────────────────────────

/// Decode using FastDemodulator with a specific window type.
fn decode_fast_windowed(samples: &[i16], sample_rate: u32, window: GoertzelWindow) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = FastDemodulator::new(config).with_adaptive_gain().with_window(window);
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using FastDemodulator with window + energy LLR + soft HDLC.
fn decode_fast_windowed_soft(samples: &[i16], sample_rate: u32, window: GoertzelWindow) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = FastDemodulator::new(config).with_adaptive_gain().with_energy_llr().with_window(window);
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

pub fn run_window_sweep(path: &str) {
    println!("═══ Goertzel Window Sweep (ISI Reduction) ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    let spb = sample_rate / get_baud();
    println!("Duration: {:.1}s, {} samples at {} Hz (SPB={})", duration_secs, samples.len(), sample_rate, spb);
    println!();

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (quality, qual_soft) = decode_quality(&samples, sample_rate);
    println!("  Baselines:  Fast={}, Quality={} (soft={})", fast.frames.len(), quality.frames.len(), qual_soft);
    println!();

    let windows = [
        (GoertzelWindow::Rectangular, "Rectangular"),
        (GoertzelWindow::Hann, "Hann"),
        (GoertzelWindow::Hamming, "Hamming"),
        (GoertzelWindow::Blackman, "Blackman"),
        (GoertzelWindow::EdgeTaper, "EdgeTaper"),
    ];

    println!("  {:>12}  {:>5} {:>5}  {:>5} {:>5} {:>4}", "Window", "Hard", "Δ", "Soft", "Δ", "Saves");
    println!("  {}", "─".repeat(52));

    for &(window, name) in &windows {
        let hard = decode_fast_windowed(&samples, sample_rate, window);
        let (soft, soft_saves) = decode_fast_windowed_soft(&samples, sample_rate, window);
        let hard_delta = hard.frames.len() as i32 - fast.frames.len() as i32;
        let soft_delta = soft.frames.len() as i32 - quality.frames.len() as i32;
        println!("  {:>12}  {:>5} {:>+5}  {:>5} {:>+5} {:>4}",
            name, hard.frames.len(), hard_delta, soft.frames.len(), soft_delta, soft_saves);
    }
    println!();
}
