//! Soft decode diagnostics.

use packet_radio_core::modem::demod::{DemodSymbol, QualityDemodulator};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};

use crate::common::*;

pub fn run_soft_diag(path: &str) {
    println!("═══ Soft Decode Diagnostics ═══");
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

    // Run quality single-decoder with detailed stats
    let config = config_for_rate(sample_rate, get_baud());
    let mut demod = QualityDemodulator::new(config);
    let mut soft_hdlc = SoftHdlcDecoder::new();
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
            if let Some(result) = soft_hdlc.feed_soft_bit(sym.llr) {
                let data = match &result {
                    FrameResult::Valid(d) => d,
                    FrameResult::Recovered { data, .. } => data,
                };
                frames.push(data.to_vec());
            }
        }
    }

    println!("=== Quality Single-Decoder Stats ===");
    println!("  Decoded frames:      {:>5}", frames.len());
    println!("  Hard decodes:        {:>5}", soft_hdlc.stats_hard_decode);
    println!("  CRC failures:        {:>5}", soft_hdlc.stats_crc_failures);
    println!(
        "  Soft recovered:      {:>5} total",
        soft_hdlc.stats_total_soft_recovered()
    );
    println!("    Syndrome 1-bit:    {:>5}", soft_hdlc.stats_syndrome);
    println!("    Single flip:       {:>5}", soft_hdlc.stats_single_flip);
    println!("    Pair flip:         {:>5}", soft_hdlc.stats_pair_flip);
    println!("    NRZI pair:         {:>5}", soft_hdlc.stats_nrzi_pair);
    println!("    Triple flip:       {:>5}", soft_hdlc.stats_triple_flip);
    println!("    NRZI triple:       {:>5}", soft_hdlc.stats_nrzi_triple);
    println!(
        "  False positives:     {:>5}",
        soft_hdlc.stats_false_positives
    );
    println!();

    // Run multi-decoder with soft stats
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);
    let (smart3, smart3_soft) = decode_smart3(&samples, sample_rate);

    println!("=== Multi-Decoder Soft Stats ===");
    println!(
        "  Multi decoded:       {:>5} ({} soft saves)",
        multi.frames.len(),
        multi_soft
    );
    println!(
        "  Smart3 decoded:      {:>5} ({} soft saves)",
        smart3.frames.len(),
        smart3_soft
    );
    println!();

    // Run fast+adaptive single decoder with energy LLR
    let fast_adapt = decode_fast_adaptive(&samples, sample_rate);
    println!("=== Adaptive Goertzel ===");
    println!("  Fast+adapt decoded:  {:>5}", fast_adapt.frames.len());
    println!();

    // Load DW reference for comparison if available
    let dw_ref = discover_dw_reference(path);
    if let Some((pkt_path, _)) = dw_ref {
        if let Ok(dw_packets) = load_dw_packets(&pkt_path) {
            let dw_set: std::collections::HashSet<String> = dw_packets.into_iter().collect();
            let multi_tnc2 = frames_to_tnc2(&multi.frames);
            let multi_set: std::collections::HashSet<&str> =
                multi_tnc2.iter().map(|s| s.as_str()).collect();
            let dw_only = dw_set
                .iter()
                .filter(|p| !multi_set.contains(p.as_str()))
                .count();

            println!("=== vs Dire Wolf ===");
            println!("  DW unique:           {:>5}", dw_set.len());
            println!(
                "  Multi overlap:       {:>5}",
                dw_set
                    .iter()
                    .filter(|p| multi_set.contains(p.as_str()))
                    .count()
            );
            println!("  DW-only (we miss):   {:>5}", dw_only);
            println!();
        }
    }
}
