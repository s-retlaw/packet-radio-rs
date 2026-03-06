//! Cross-architecture Goertzel+Correlation LLR fusion.

use std::time::Instant;

use packet_radio_core::modem::demod::{CorrelationDemodulator, DemodSymbol, FastDemodulator, GoertzelWindow};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};

use crate::common::*;

// ─── Cross-Architecture Fusion ──────────────────────────────────────────

/// Decode with Goertzel+Correlation LLR fusion.
///
/// Runs both demodulators sample-by-sample with synchronized Bresenham timing.
/// When both produce a symbol on the same sample, their LLR values are combined
/// and fed to a single SoftHdlcDecoder for error recovery.
fn decode_fusion(
    samples: &[i16],
    sample_rate: u32,
    goertzel_weight: i16,
    corr_weight: i16,
) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let mut goertzel = FastDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut corr = CorrelationDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut g_sym = [DemodSymbol { bit: false, llr: 0 }; 1];
    let mut c_sym = [DemodSymbol { bit: false, llr: 0 }; 1];

    let start = Instant::now();

    for &sample in samples {
        let g_n = goertzel.process_samples(&[sample], &mut g_sym);
        let c_n = corr.process_samples(&[sample], &mut c_sym);

        let llr = if g_n > 0 && c_n > 0 {
            // Both produced a symbol — fuse LLRs with weights
            let g_llr = g_sym[0].llr as i16;
            let c_llr = c_sym[0].llr as i16;
            let total_weight = goertzel_weight + corr_weight;
            ((g_llr * goertzel_weight + c_llr * corr_weight) / total_weight).clamp(-127, 127) as i8
        } else if g_n > 0 {
            g_sym[0].llr
        } else if c_n > 0 {
            c_sym[0].llr
        } else {
            continue;
        };

        if let Some(result) = soft_hdlc.feed_soft_bit(llr) {
            let data = match &result {
                FrameResult::Valid(d) => d,
                FrameResult::Recovered { data, .. } => data,
            };
            frames.push(data.to_vec());
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

/// Decode with Goertzel+Correlation fusion using max-confidence strategy.
/// At each symbol boundary, use whichever demodulator's LLR has higher magnitude.
fn decode_fusion_maxconf(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let mut goertzel = FastDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut corr = CorrelationDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut g_sym = [DemodSymbol { bit: false, llr: 0 }; 1];
    let mut c_sym = [DemodSymbol { bit: false, llr: 0 }; 1];

    let start = Instant::now();

    for &sample in samples {
        let g_n = goertzel.process_samples(&[sample], &mut g_sym);
        let c_n = corr.process_samples(&[sample], &mut c_sym);

        let llr = if g_n > 0 && c_n > 0 {
            if g_sym[0].llr.unsigned_abs() >= c_sym[0].llr.unsigned_abs() {
                g_sym[0].llr
            } else {
                c_sym[0].llr
            }
        } else if g_n > 0 {
            g_sym[0].llr
        } else if c_n > 0 {
            c_sym[0].llr
        } else {
            continue;
        };

        if let Some(result) = soft_hdlc.feed_soft_bit(llr) {
            let data = match &result {
                FrameResult::Valid(d) => d,
                FrameResult::Recovered { data, .. } => data,
            };
            frames.push(data.to_vec());
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

/// Decode with Goertzel+Correlation fusion using LLR sum (MRC-style).
/// LLRs from independent observers are additive under log-likelihood theory.
fn decode_fusion_sum(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let mut goertzel = FastDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut corr = CorrelationDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut g_sym = [DemodSymbol { bit: false, llr: 0 }; 1];
    let mut c_sym = [DemodSymbol { bit: false, llr: 0 }; 1];

    let start = Instant::now();

    for &sample in samples {
        let g_n = goertzel.process_samples(&[sample], &mut g_sym);
        let c_n = corr.process_samples(&[sample], &mut c_sym);

        let llr = if g_n > 0 && c_n > 0 {
            // Sum LLRs (theoretically optimal for independent observations)
            let sum = g_sym[0].llr as i16 + c_sym[0].llr as i16;
            sum.clamp(-127, 127) as i8
        } else if g_n > 0 {
            g_sym[0].llr
        } else if c_n > 0 {
            c_sym[0].llr
        } else {
            continue;
        };

        if let Some(result) = soft_hdlc.feed_soft_bit(llr) {
            let data = match &result {
                FrameResult::Valid(d) => d,
                FrameResult::Recovered { data, .. } => data,
            };
            frames.push(data.to_vec());
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

/// Decode with windowed Goertzel + Correlation fusion.
fn decode_fusion_windowed(
    samples: &[i16],
    sample_rate: u32,
    window: GoertzelWindow,
    goertzel_weight: i16,
    corr_weight: i16,
) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());

    let mut goertzel = FastDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr()
        .with_window(window);
    let mut corr = CorrelationDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut g_sym = [DemodSymbol { bit: false, llr: 0 }; 1];
    let mut c_sym = [DemodSymbol { bit: false, llr: 0 }; 1];

    let start = Instant::now();

    for &sample in samples {
        let g_n = goertzel.process_samples(&[sample], &mut g_sym);
        let c_n = corr.process_samples(&[sample], &mut c_sym);

        let llr = if g_n > 0 && c_n > 0 {
            let g_llr = g_sym[0].llr as i16;
            let c_llr = c_sym[0].llr as i16;
            let total_weight = goertzel_weight + corr_weight;
            ((g_llr * goertzel_weight + c_llr * corr_weight) / total_weight).clamp(-127, 127) as i8
        } else if g_n > 0 {
            g_sym[0].llr
        } else if c_n > 0 {
            c_sym[0].llr
        } else {
            continue;
        };

        if let Some(result) = soft_hdlc.feed_soft_bit(llr) {
            let data = match &result {
                FrameResult::Valid(d) => d,
                FrameResult::Recovered { data, .. } => data,
            };
            frames.push(data.to_vec());
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

pub fn run_fusion(path: &str) {
    println!("═══ Cross-Architecture Goertzel+Correlation LLR Fusion ═══");
    println!("File: {}", path);

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

    // Baselines
    let fast = decode_fast(&samples, sample_rate);
    let (quality, qual_soft) = decode_quality(&samples, sample_rate);
    let (corr_q, corr_soft) = decode_corr_quality(&samples, sample_rate);
    println!("  Baselines:");
    println!("    Fast (hard):      {:>4}", fast.frames.len());
    println!("    Quality (soft):   {:>4} (soft={})", quality.frames.len(), qual_soft);
    println!("    Corr (soft):      {:>4} (soft={})", corr_q.frames.len(), corr_soft);
    println!();

    // Fusion strategies
    println!("  {:>22}  {:>5} {:>+5} {:>4}", "Strategy", "Pkts", "ΔQlt", "Soft");
    println!("  {}", "─".repeat(42));

    // 1. Equal weight average
    let (avg, avg_soft) = decode_fusion(&samples, sample_rate, 1, 1);
    let delta = avg.frames.len() as i32 - quality.frames.len() as i32;
    println!("  {:>22}  {:>5} {:>+5} {:>4}", "Average (1:1)", avg.frames.len(), delta, avg_soft);

    // 2. Goertzel-heavy
    let (g7c3, g7c3_soft) = decode_fusion(&samples, sample_rate, 7, 3);
    let delta = g7c3.frames.len() as i32 - quality.frames.len() as i32;
    println!("  {:>22}  {:>5} {:>+5} {:>4}", "G-heavy (7:3)", g7c3.frames.len(), delta, g7c3_soft);

    // 3. Corr-heavy
    let (g3c7, g3c7_soft) = decode_fusion(&samples, sample_rate, 3, 7);
    let delta = g3c7.frames.len() as i32 - quality.frames.len() as i32;
    println!("  {:>22}  {:>5} {:>+5} {:>4}", "C-heavy (3:7)", g3c7.frames.len(), delta, g3c7_soft);

    // 4. Max confidence
    let (maxc, maxc_soft) = decode_fusion_maxconf(&samples, sample_rate);
    let delta = maxc.frames.len() as i32 - quality.frames.len() as i32;
    println!("  {:>22}  {:>5} {:>+5} {:>4}", "Max-confidence", maxc.frames.len(), delta, maxc_soft);

    // 5. LLR sum (MRC-style)
    let (sum, sum_soft) = decode_fusion_sum(&samples, sample_rate);
    let delta = sum.frames.len() as i32 - quality.frames.len() as i32;
    println!("  {:>22}  {:>5} {:>+5} {:>4}", "LLR sum (MRC)", sum.frames.len(), delta, sum_soft);

    // 6. Weight sweep for best ratio
    println!();
    println!("  Weight sweep (G:C ratio):");
    println!("  {:>6}:{:<6}  {:>5} {:>+5} {:>4}", "G", "C", "Pkts", "ΔQlt", "Soft");
    println!("  {}", "─".repeat(36));
    let weights: &[(i16, i16)] = &[
        (9, 1), (8, 2), (7, 3), (6, 4), (5, 5),
        (4, 6), (3, 7), (2, 8), (1, 9),
    ];
    for &(gw, cw) in weights {
        let (res, soft) = decode_fusion(&samples, sample_rate, gw, cw);
        let delta = res.frames.len() as i32 - quality.frames.len() as i32;
        println!("  {:>6}:{:<6}  {:>5} {:>+5} {:>4}", gw, cw, res.frames.len(), delta, soft);
    }

    // 7. Best fusion strategy with window types
    println!();
    println!("  Windowed Goertzel + Corr fusion (equal weight):");
    println!("  {:>12}  {:>5} {:>+5} {:>4}", "Window", "Pkts", "ΔQlt", "Soft");
    println!("  {}", "─".repeat(36));
    let windows = [
        (GoertzelWindow::Rectangular, "Rectangular"),
        (GoertzelWindow::Hann, "Hann"),
        (GoertzelWindow::Hamming, "Hamming"),
        (GoertzelWindow::Blackman, "Blackman"),
        (GoertzelWindow::EdgeTaper, "EdgeTaper"),
    ];
    for &(window, name) in &windows {
        let (res, soft) = decode_fusion_windowed(&samples, sample_rate, window, 1, 1);
        let delta = res.frames.len() as i32 - quality.frames.len() as i32;
        println!("  {:>12}  {:>5} {:>+5} {:>4}", name, res.frames.len(), delta, soft);
    }
    println!();
}
