//! Per-decoder attribution analysis.

use crate::common::*;
use packet_radio_core::modem::multi::MultiDecoder;

// ─── Attribution Mode ─────────────────────────────────────────────────────

pub fn run_attribution(wav_path: &str) {
    use packet_radio_core::modem::multi::AttributionReport;

    println!("═══ Per-Decoder Attribution Analysis ═══");
    println!("File: {}", wav_path);

    let (sample_rate, samples) = match read_wav_file(wav_path) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error reading {}: {}", wav_path, e); return; }
    };

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    println!("Duration: {:.1}s, {} samples at {} Hz", duration_secs, samples.len(), sample_rate);
    println!();

    let config = config_for_rate(sample_rate, get_baud());

    let mut multi = MultiDecoder::new(config);
    let configs = multi.decoder_configs();

    println!("Active decoders: {} ({} Goertzel + {} DM)",
        configs.len(),
        configs.iter().filter(|c| c.algorithm == "goertzel").count(),
        configs.iter().filter(|c| c.algorithm == "dm").count());
    println!();

    let mut report = AttributionReport::new(configs.clone());
    let mut total_frames = 0usize;

    let start = std::time::Instant::now();
    for chunk in samples.chunks(1024) {
        let attributed = multi.process_samples_attributed(chunk);
        total_frames += attributed.output.len();
        report.merge(&attributed);
    }
    let elapsed = start.elapsed();
    report.finalize();

    println!("Decoded {} unique frames in {:.2}s", total_frames, elapsed.as_secs_f64());
    println!();

    // Per-decoder table
    println!("=== Per-Decoder Statistics ===");
    println!("  {:>3}  {:<28} {:>6} {:>6} {:>9}", "#", "Decoder", "Total", "First", "Exclusive");
    println!("  {}", "─".repeat(60));

    for (i, cfg) in configs.iter().enumerate() {
        let stat = &report.stats[i];
        let exc_str = if stat.exclusive > 0 {
            format!("{}", stat.exclusive)
        } else {
            "-".to_string()
        };
        println!("  {:>3}  {:<28} {:>6} {:>6} {:>9}",
            i, cfg.label, stat.total, stat.first, exc_str);
    }
    println!();

    // By-tag aggregation
    println!("=== Stats by Dimension ===");
    let tag_stats = report.stats_by_tag();
    println!("  {:<16} {:>8} {:>9} {:>9}", "Tag", "Frames", "Exclusive", "RawHits");
    println!("  {}", "─".repeat(48));
    for (tag, stat) in &tag_stats {
        println!("  {:<16} {:>8} {:>9} {:>9}", tag, stat.first, stat.exclusive, stat.total);
    }
    println!();

    // Coverage curve
    println!("=== Coverage Curve (Greedy Set Cover) ===");
    let curve = report.coverage_curve();
    let total_unique = report.total_unique();
    println!("  {:>3}  {:<28} {:>6} {:>7}", "#", "Decoder", "Cumul.", "% Total");
    println!("  {}", "─".repeat(50));

    for (step, &(dec_idx, cumulative)) in curve.iter().enumerate() {
        let label = if dec_idx < configs.len() {
            configs[dec_idx].label.as_str()
        } else {
            "?"
        };
        let pct = if total_unique > 0 {
            cumulative as f64 / total_unique as f64 * 100.0
        } else {
            0.0
        };
        println!("  {:>3}  {:<28} {:>6} {:>6.1}%", step + 1, label, cumulative, pct);
        // Stop printing after 100% or 15 entries
        if cumulative >= total_unique || step >= 14 {
            if step < curve.len() - 1 {
                println!("  ... ({} more decoders needed for remaining frames)", curve.len() - step - 1);
            }
            break;
        }
    }
    println!();

    // ESP32 recommendation
    if curve.len() >= 3 {
        println!("=== ESP32 Recommendation (top 3 decoders) ===");
        for &(dec_idx, cumulative) in curve.iter().take(3) {
            let label = if dec_idx < configs.len() {
                configs[dec_idx].label.as_str()
            } else {
                "?"
            };
            let pct = if total_unique > 0 {
                cumulative as f64 / total_unique as f64 * 100.0
            } else {
                0.0
            };
            println!("  {} → {} frames ({:.1}%)", label, cumulative, pct);
        }
        println!();
    }
}
