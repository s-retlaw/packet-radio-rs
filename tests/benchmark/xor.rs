//! Binary XOR correlator benchmarks.

use packet_radio_core::modem::binary_xor::BinaryXorDemodulator;

use crate::common::*;

// ─── Binary XOR Correlator ──────────────────────────────────────────────

/// Decode using Binary XOR correlator + hard HDLC.
fn decode_xor(samples: &[i16], sample_rate: u32) -> DecodeResult {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = BinaryXorDemodulator::new(config);
    run_hard_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

/// Decode using Binary XOR correlator + energy LLR + soft HDLC.
fn decode_xor_quality(samples: &[i16], sample_rate: u32) -> (DecodeResult, u32) {
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = BinaryXorDemodulator::new(config).with_energy_llr();
    run_soft_decode(samples, |chunk, syms| demod.process_samples(chunk, syms))
}

pub fn run_xor(path: &str) {
    println!("═══ Binary XOR Correlator ═══");
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

    let xor = decode_xor(&samples, sample_rate);
    let (xor_q, xor_soft) = decode_xor_quality(&samples, sample_rate);
    let fast = decode_fast(&samples, sample_rate);
    let dm = decode_dm(&samples, sample_rate);

    println!(
        "  XOR hard:     {:>4} packets in {:.2}s ({:.0}x real-time)",
        xor.frames.len(),
        xor.elapsed.as_secs_f64(),
        duration_secs / xor.elapsed.as_secs_f64()
    );
    println!(
        "  XOR quality:  {:>4} packets in {:.2}s ({:.0}x real-time, {} soft saves)",
        xor_q.frames.len(),
        xor_q.elapsed.as_secs_f64(),
        duration_secs / xor_q.elapsed.as_secs_f64(),
        xor_soft
    );
    println!(
        "  Fast:         {:>4} packets (Goertzel baseline)",
        fast.frames.len()
    );
    println!(
        "  DM:           {:>4} packets (delay-multiply baseline)",
        dm.frames.len()
    );
    let gain_hard = xor.frames.len() as i64 - fast.frames.len() as i64;
    let gain_dm = xor.frames.len() as i64 - dm.frames.len() as i64;
    println!("  Gain vs fast: {:>+4} packets", gain_hard);
    println!("  Gain vs DM:   {:>+4} packets", gain_dm);

    // Exclusive frame analysis: compare XOR vs MCU-feasible decoders
    let (smart3, _) = decode_smart3(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);

    use std::collections::HashSet;
    let xor_set: HashSet<Vec<u8>> = xor.frames.iter().cloned().collect();
    let xor_q_set: HashSet<Vec<u8>> = xor_q.frames.iter().cloned().collect();
    let smart3_set: HashSet<Vec<u8>> = smart3.frames.iter().cloned().collect();
    let multi_set: HashSet<Vec<u8>> = multi.frames.iter().cloned().collect();
    let fast_set: HashSet<Vec<u8>> = fast.frames.iter().cloned().collect();
    let dm_set: HashSet<Vec<u8>> = dm.frames.iter().cloned().collect();

    let xor_not_in_smart3 = xor_set.difference(&smart3_set).count();
    let xor_q_not_in_smart3 = xor_q_set.difference(&smart3_set).count();
    let _xor_not_in_fast = xor_set.difference(&fast_set).count();
    let xor_not_in_multi = xor_set.difference(&multi_set).count();
    // XOR frames not in any MCU-feasible single decoder
    let mcu_union: HashSet<Vec<u8>> = fast_set.union(&dm_set).cloned().collect();
    let xor_not_in_mcu_singles = xor_set.difference(&mcu_union).count();
    // Smart3 + XOR combined
    let smart3_xor: HashSet<Vec<u8>> = smart3_set.union(&xor_set).cloned().collect();

    println!();
    println!("  Exclusive frame analysis:");
    println!("    XOR unique frames:        {:>4}", xor_set.len());
    println!("    Smart3 unique frames:     {:>4}", smart3_set.len());
    println!("    Multi unique frames:      {:>4}", multi_set.len());
    println!(
        "    XOR not in Smart3:        {:>4}  ← MCU-relevant exclusives",
        xor_not_in_smart3
    );
    println!("    XOR qual not in Smart3:   {:>4}", xor_q_not_in_smart3);
    println!(
        "    XOR not in Fast+DM:       {:>4}",
        xor_not_in_mcu_singles
    );
    println!("    XOR not in Multi:         {:>4}", xor_not_in_multi);
    println!(
        "    Smart3+XOR combined:      {:>4}  (Smart3 alone: {})",
        smart3_xor.len(),
        smart3_set.len()
    );
    println!();
}
