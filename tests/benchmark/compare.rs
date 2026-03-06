//! Compare fast vs quality approach (A/B test).

use crate::common::*;

pub fn run_compare_approaches(path: &str) {
    println!("═══ Approach Comparison ═══");
    println!("File: {}", path);
    println!();

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
    let (quality, soft_saves) = decode_quality(&samples, sample_rate);

    // Compare unique frames by raw bytes
    let quality_set: std::collections::HashSet<Vec<u8>> =
        quality.frames.iter().cloned().collect();
    let fast_set: std::collections::HashSet<Vec<u8>> = fast.frames.iter().cloned().collect();

    let fast_unique = fast_set.len();
    let quality_unique = quality_set.len();
    let both_unique = fast_set.intersection(&quality_set).count();
    let fast_only_unique = fast_unique - both_unique;
    let quality_only_unique = quality_unique - both_unique;

    let fast_rt = duration_secs / fast.elapsed.as_secs_f64();
    let qual_rt = duration_secs / quality.elapsed.as_secs_f64();

    println!("  ┌──────────────────────────┬────────────┬─────────────┐");
    println!("  │ Metric                   │ Fast Path  │ Quality Path│");
    println!("  ├──────────────────────────┼────────────┼─────────────┤");
    println!(
        "  │ Total frames decoded     │ {:>10} │ {:>11} │",
        fast.frames.len(),
        quality.frames.len()
    );
    println!(
        "  │ Unique frames            │ {:>10} │ {:>11} │",
        fast_unique, quality_unique
    );
    println!(
        "  │ Soft-recovery saves      │        N/A │ {:>11} │",
        soft_saves
    );
    println!(
        "  │ Processing time          │ {:>8.2}s  │ {:>9.2}s  │",
        fast.elapsed.as_secs_f64(),
        quality.elapsed.as_secs_f64()
    );
    println!(
        "  │ Speed (x real-time)      │ {:>9.0}x │ {:>10.0}x │",
        fast_rt, qual_rt
    );
    println!("  └──────────────────────────┴────────────┴─────────────┘");
    println!();
    println!(
        "  Unique frames decoded by both: {:>6}",
        both_unique
    );
    println!(
        "  Fast only (quality missed):    {:>6}",
        fast_only_unique
    );
    println!(
        "  Quality only (fast missed):    {:>6}",
        quality_only_unique
    );
    println!();

    // Show a few example fast-only and quality-only frames
    if fast_only_unique > 0 {
        println!(
            "  First {} fast-only frame(s):",
            fast_only_unique.min(3)
        );
        let mut shown = 0;
        for frame in &fast.frames {
            if !quality_set.contains(frame) && shown < 3 {
                println!("    [{} bytes] {:02X?}", frame.len(), &frame[..frame.len().min(20)]);
                shown += 1;
            }
        }
    }
    if quality_only_unique > 0 {
        println!(
            "  First {} quality-only frame(s):",
            quality_only_unique.min(3)
        );
        let mut shown = 0;
        for frame in &quality.frames {
            if !fast_set.contains(frame) && shown < 3 {
                println!("    [{} bytes] {:02X?}", frame.len(), &frame[..frame.len().min(20)]);
                shown += 1;
            }
        }
    }
}
