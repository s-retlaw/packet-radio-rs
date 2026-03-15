//! Correlation demodulator benchmarks.

use std::time::Instant;

use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::modem::corr_slicer::CorrSlicerDecoder;
use packet_radio_core::modem::demod::{CorrelationDemodulator, DemodSymbol};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};

use crate::common::*;

// ─── Soft Decode Diagnostics ─────────────────────────────────────────────

pub fn run_corr(path: &str) {
    println!("═══ Correlation (Mixer) Demodulator ═══");
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

    let corr = decode_corr(&samples, sample_rate);
    let (corr_q, corr_soft) = decode_corr_quality(&samples, sample_rate);
    let corr_3p = decode_corr_3phase(&samples, sample_rate);
    let (corr_3pq, corr_3p_soft) = decode_corr_3phase_quality(&samples, sample_rate);
    let slicer = decode_corr_slicer(&samples, sample_rate);
    let slicer_3p = decode_corr_slicer_3phase(&samples, sample_rate);
    let fast = decode_fast(&samples, sample_rate);
    let (quality, qual_soft) = decode_quality(&samples, sample_rate);
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);

    println!(
        "  Corr hard:    {:>4} packets in {:.2}s ({:.0}x real-time)",
        corr.frames.len(),
        corr.elapsed.as_secs_f64(),
        duration_secs / corr.elapsed.as_secs_f64()
    );
    println!(
        "  Corr quality: {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        corr_q.frames.len(),
        corr_q.elapsed.as_secs_f64(),
        duration_secs / corr_q.elapsed.as_secs_f64(),
        corr_soft
    );
    println!(
        "  Corr×3 hard:  {:>4} packets in {:.2}s ({:.0}x real-time)",
        corr_3p.frames.len(),
        corr_3p.elapsed.as_secs_f64(),
        duration_secs / corr_3p.elapsed.as_secs_f64()
    );
    println!(
        "  Corr×3 qual:  {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        corr_3pq.frames.len(),
        corr_3pq.elapsed.as_secs_f64(),
        duration_secs / corr_3pq.elapsed.as_secs_f64(),
        corr_3p_soft
    );
    println!(
        "  Slicer 8×:    {:>4} packets in {:.2}s ({:.0}x real-time)",
        slicer.frames.len(),
        slicer.elapsed.as_secs_f64(),
        duration_secs / slicer.elapsed.as_secs_f64()
    );
    println!(
        "  Slicer×3:     {:>4} packets in {:.2}s ({:.0}x real-time)",
        slicer_3p.frames.len(),
        slicer_3p.elapsed.as_secs_f64(),
        duration_secs / slicer_3p.elapsed.as_secs_f64()
    );
    println!(
        "  Fast:         {:>4} packets (Goertzel baseline)",
        fast.frames.len()
    );
    println!(
        "  Quality:      {:>4} packets ({} soft saves, Goertzel+Hilbert baseline)",
        quality.frames.len(),
        qual_soft
    );
    println!(
        "  Multi (38×):  {:>4} packets ({} soft saves)",
        multi.frames.len(),
        multi_soft
    );
    let gain_hard = corr.frames.len() as i64 - fast.frames.len() as i64;
    let gain_3p = corr_3pq.frames.len() as i64 - quality.frames.len() as i64;
    let gain_slicer = slicer.frames.len() as i64 - corr.frames.len() as i64;
    println!("  Gain (single): {:>+4} packets vs fast", gain_hard);
    println!("  Gain (3-ph):   {:>+4} packets vs quality", gain_3p);
    println!(
        "  Gain (slicer): {:>+4} packets vs corr single",
        gain_slicer
    );
    println!();
}

/// Decode using corr slicer with phase scoring enabled.
fn decode_corr_slicer_phase(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let mut decoder = CorrSlicerDecoder::new(config)
        .with_adaptive_gain()
        .with_phase_scoring();
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let output = decoder.process_samples(chunk);
        for i in 0..output.len() {
            frames.push(output.frame(i).to_vec());
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using corr slicer with adaptive retune enabled.
fn decode_corr_slicer_retune(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let mut decoder = CorrSlicerDecoder::new(config)
        .with_adaptive_gain()
        .with_adaptive_retune();
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let output = decoder.process_samples(chunk);
        for i in 0..output.len() {
            frames.push(output.frame(i).to_vec());
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using corr slicer with both phase scoring and adaptive retune.
fn decode_corr_slicer_both(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let mut decoder = CorrSlicerDecoder::new(config)
        .with_adaptive_gain()
        .with_phase_scoring()
        .with_adaptive_retune();
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let start = Instant::now();

    for chunk in samples.chunks(1024) {
        let output = decoder.process_samples(chunk);
        for i in 0..output.len() {
            frames.push(output.frame(i).to_vec());
        }
    }

    DecodeResult {
        frames,
        elapsed: start.elapsed(),
    }
}

pub fn run_corr_slicer(path: &str) {
    println!("═══ Correlation Multi-Slicer Demodulator ═══");
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

    let slicer = decode_corr_slicer(&samples, sample_rate);
    let slicer_3p = decode_corr_slicer_3phase(&samples, sample_rate);
    let slicer_phase = decode_corr_slicer_phase(&samples, sample_rate);
    let slicer_retune = decode_corr_slicer_retune(&samples, sample_rate);
    let slicer_both = decode_corr_slicer_both(&samples, sample_rate);
    let fast = decode_fast(&samples, sample_rate);
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);

    println!(
        "  Slicer 8× (base):    {:>4} packets in {:.2}s ({:.0}x RT)",
        slicer.frames.len(),
        slicer.elapsed.as_secs_f64(),
        duration_secs / slicer.elapsed.as_secs_f64()
    );
    println!(
        "  Slicer +phase:       {:>4} packets in {:.2}s ({:.0}x RT)  {:>+4}",
        slicer_phase.frames.len(),
        slicer_phase.elapsed.as_secs_f64(),
        duration_secs / slicer_phase.elapsed.as_secs_f64(),
        slicer_phase.frames.len() as i64 - slicer.frames.len() as i64
    );
    println!(
        "  Slicer +retune:      {:>4} packets in {:.2}s ({:.0}x RT)  {:>+4}",
        slicer_retune.frames.len(),
        slicer_retune.elapsed.as_secs_f64(),
        duration_secs / slicer_retune.elapsed.as_secs_f64(),
        slicer_retune.frames.len() as i64 - slicer.frames.len() as i64
    );
    println!(
        "  Slicer +both:        {:>4} packets in {:.2}s ({:.0}x RT)  {:>+4}",
        slicer_both.frames.len(),
        slicer_both.elapsed.as_secs_f64(),
        duration_secs / slicer_both.elapsed.as_secs_f64(),
        slicer_both.frames.len() as i64 - slicer.frames.len() as i64
    );
    println!(
        "  Slicer 8×+3ph:       {:>4} packets in {:.2}s ({:.0}x RT)",
        slicer_3p.frames.len(),
        slicer_3p.elapsed.as_secs_f64(),
        duration_secs / slicer_3p.elapsed.as_secs_f64()
    );
    println!("  Fast (Goertzel):     {:>4} packets", fast.frames.len());
    println!(
        "  Multi (38×):         {:>4} packets ({} soft saves)",
        multi.frames.len(),
        multi_soft
    );
    println!();
}

// ─── Correlation LPF Sweep ──────────────────────────────────────────────

pub fn run_corr_lpf_sweep(path: &str) {
    use packet_radio_core::modem::filter::lowpass_coeffs;

    println!("═══ Correlation LPF Cutoff Sweep ═══");
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

    // Baseline: default 500 Hz cutoff (tone_separation / 2)
    let corr_baseline = decode_corr(&samples, sample_rate);
    let (corr_q_baseline, soft_baseline) = decode_corr_quality(&samples, sample_rate);
    println!(
        "Baseline (500 Hz): {} hard, {} quality ({} soft saves)",
        corr_baseline.frames.len(),
        corr_q_baseline.frames.len(),
        soft_baseline
    );
    println!();

    let cutoffs = [
        400.0, 450.0, 500.0, 550.0, 600.0, 650.0, 700.0, 750.0, 800.0, 850.0, 900.0, 950.0, 1000.0,
    ];

    println!(
        "{:<10} {:>8} {:>8} {:>8}",
        "Cutoff", "Hard", "Quality", "Soft"
    );
    println!("{}", "─".repeat(40));

    for &cutoff in &cutoffs {
        let lpf = lowpass_coeffs(sample_rate, cutoff, 0.707);

        // Hard decode
        let config = config_for_rate(sample_rate, get_baud());
        let mut demod = CorrelationDemodulator::new(config)
            .with_adaptive_gain()
            .with_corr_lpf(lpf);
        let mut hdlc = HdlcDecoder::new();
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut symbols = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; 1024];
        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if let Some(frame) = hdlc.feed_bit(sym.bit) {
                    frames.push(frame.to_vec());
                }
            }
        }
        let hard_count = frames.len();

        // Quality decode
        let lpf2 = lowpass_coeffs(sample_rate, cutoff, 0.707);
        let mut demod2 = CorrelationDemodulator::new(config)
            .with_adaptive_gain()
            .with_energy_llr()
            .with_corr_lpf(lpf2);
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut frames2: Vec<Vec<u8>> = Vec::new();
        let mut symbols2 = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; 1024];
        for chunk in samples.chunks(1024) {
            let n = demod2.process_samples(chunk, &mut symbols2);
            for sym in &symbols2[..n] {
                if let Some(result) = soft_hdlc.feed_soft_bit(sym.llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => d,
                        FrameResult::Recovered { data, .. } => data,
                    };
                    frames2.push(data.to_vec());
                }
            }
        }
        let quality_count = frames2.len();
        let soft_saves = soft_hdlc.stats_total_soft_recovered();

        println!(
            "{:<10} {:>8} {:>8} {:>8}",
            format!("{:.0} Hz", cutoff),
            hard_count,
            quality_count,
            soft_saves
        );
    }

    println!();
}

// ─── Correlation + PLL ─────────────────────────────────────────────────

/// Decode using correlation demod + Gardner PLL timing recovery.
fn decode_corr_pll(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_pll();
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using correlation demod + Gardner PLL + energy LLR + soft HDLC.
fn decode_corr_pll_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr()
        .with_pll();
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using correlation demod + PLL with custom alpha and error_shift.
fn decode_corr_pll_custom(
    samples: &[i16],
    sample_rate: u32,
    alpha: i16,
    error_shift: u8,
) -> DecodeResult {
    use packet_radio_core::modem::pll::ClockRecoveryPll;

    let config = config_for_rate(sample_rate, get_baud());
    let pll =
        ClockRecoveryPll::new_gardner(sample_rate, 1200, alpha, 0).with_error_shift(error_shift);
    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_custom_pll(pll);
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using 2-phase correlation demod (two timing phases, dedup).
fn decode_corr_2phase(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let offsets = [0, sample_rate / 2];
    let mut phase_frames: Vec<Vec<(u64, usize, Vec<u8>)>> = Vec::new();

    let start = Instant::now();

    for &offset in &offsets {
        let mut demod = CorrelationDemodulator::new(config).with_adaptive_gain();
        demod.set_bit_phase(offset);
        let mut hdlc = HdlcDecoder::new();
        let mut symbols = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; 1024];
        let mut frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
        let mut sample_pos: usize = 0;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if let Some(frame) = hdlc.feed_bit(sym.bit) {
                    let hash = fnv1a_hash(frame);
                    frames.push((hash, sample_pos, frame.to_vec()));
                }
            }
            sample_pos += chunk.len();
        }
        phase_frames.push(frames);
    }

    let dedup_window = sample_rate as usize * 2;
    let all_frames = dedup_merge(&phase_frames, dedup_window);

    DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }
}

/// Decode using 2-phase correlation demod + PLL per phase + dedup.
fn decode_corr_2phase_pll(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());

    let offsets = [0, sample_rate / 2];
    let mut phase_frames: Vec<Vec<(u64, usize, Vec<u8>)>> = Vec::new();

    let start = Instant::now();

    for &offset in &offsets {
        let mut demod = CorrelationDemodulator::new(config)
            .with_adaptive_gain()
            .with_pll();
        demod.set_bit_phase(offset);
        let mut hdlc = HdlcDecoder::new();
        let mut symbols = [DemodSymbol {
            bit: false,
            llr: 0,
            sample_idx: 0,
            raw_bit: false,
        }; 1024];
        let mut frames: Vec<(u64, usize, Vec<u8>)> = Vec::new();
        let mut sample_pos: usize = 0;

        for chunk in samples.chunks(1024) {
            let n = demod.process_samples(chunk, &mut symbols);
            for sym in &symbols[..n] {
                if let Some(frame) = hdlc.feed_bit(sym.bit) {
                    let hash = fnv1a_hash(frame);
                    frames.push((hash, sample_pos, frame.to_vec()));
                }
            }
            sample_pos += chunk.len();
        }
        phase_frames.push(frames);
    }

    let dedup_window = sample_rate as usize * 2;
    let all_frames = dedup_merge(&phase_frames, dedup_window);

    DecodeResult {
        frames: all_frames,
        elapsed: start.elapsed(),
    }
}

pub fn run_corr_pll(path: &str) {
    println!("═══ Correlation + PLL Demodulator ═══");
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

    let corr = decode_corr(&samples, sample_rate);
    let (corr_q, corr_soft) = decode_corr_quality(&samples, sample_rate);
    let corr_pll = decode_corr_pll(&samples, sample_rate);
    let (corr_pll_q, corr_pll_soft) = decode_corr_pll_quality(&samples, sample_rate);
    let corr_2p = decode_corr_2phase(&samples, sample_rate);
    let corr_2p_pll = decode_corr_2phase_pll(&samples, sample_rate);
    let corr_3p = decode_corr_3phase(&samples, sample_rate);

    println!(
        "  Corr hard:      {:>4} packets (Bresenham baseline)",
        corr.frames.len()
    );
    println!(
        "  Corr quality:   {:>4} packets ({} soft saves)",
        corr_q.frames.len(),
        corr_soft
    );
    println!(
        "  Corr+PLL hard:  {:>4} packets (Gardner PLL)",
        corr_pll.frames.len()
    );
    println!(
        "  Corr+PLL qual:  {:>4} packets ({} soft saves)",
        corr_pll_q.frames.len(),
        corr_pll_soft
    );
    println!(
        "  Corr×2 hard:    {:>4} packets (2-phase diversity)",
        corr_2p.frames.len()
    );
    println!(
        "  Corr×2+PLL:     {:>4} packets (2-phase + PLL)",
        corr_2p_pll.frames.len()
    );
    println!(
        "  Corr×3 hard:    {:>4} packets (3-phase diversity)",
        corr_3p.frames.len()
    );
    println!();
    let pll_gain = corr_pll.frames.len() as i64 - corr.frames.len() as i64;
    let pll_q_gain = corr_pll_q.frames.len() as i64 - corr_q.frames.len() as i64;
    println!("  PLL gain (hard):    {:>+4}", pll_gain);
    println!("  PLL gain (quality): {:>+4}", pll_q_gain);
    println!(
        "  3-phase gap closed: {:.0}%",
        if corr_3p.frames.len() > corr.frames.len() {
            pll_gain as f64 / (corr_3p.frames.len() as f64 - corr.frames.len() as f64) * 100.0
        } else {
            0.0
        }
    );
    println!();
}

pub fn run_corr_pll_sweep(path: &str) {
    println!("═══ Correlation PLL Parameter Sweep ═══");
    println!("File: {}", path);

    let (sample_rate, samples) = match read_wav_file(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };

    println!(
        "Duration: {:.1}s, {} samples at {} Hz",
        samples.len() as f64 / sample_rate as f64,
        samples.len(),
        sample_rate
    );
    println!();

    // Baseline
    let corr = decode_corr(&samples, sample_rate);
    println!("Baseline (Bresenham): {} packets", corr.frames.len());
    println!();

    let alphas: &[i16] = &[200, 400, 600, 800, 936, 1200, 1600];
    let error_shifts: &[u8] = &[6, 7, 8, 9, 10];

    println!(
        "{:<8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "alpha", "es=6", "es=7", "es=8", "es=9", "es=10"
    );
    println!("{}", "─".repeat(56));

    let mut best_count = 0usize;
    let mut best_alpha = 0i16;
    let mut best_shift = 0u8;

    for &alpha in alphas {
        print!("{:<8}", alpha);
        for &es in error_shifts {
            let result = decode_corr_pll_custom(&samples, sample_rate, alpha, es);
            let count = result.frames.len();
            print!(" {:>8}", count);
            if count > best_count {
                best_count = count;
                best_alpha = alpha;
                best_shift = es;
            }
        }
        println!();
    }

    println!("{}", "─".repeat(56));
    println!(
        "Best: alpha={}, error_shift={} → {} packets ({:+} vs Bresenham)",
        best_alpha,
        best_shift,
        best_count,
        best_count as i64 - corr.frames.len() as i64
    );
    println!();
}
