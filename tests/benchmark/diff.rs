//! Frame-level diff against Dire Wolf reference.

use crate::common::*;

// ─── Frame Diff: Dire Wolf Comparison ─────────────────────────────────────

pub fn run_diff(wav_path: &str, reference: Option<&str>) {
    println!("═══ Frame-Level Diff vs Dire Wolf ═══");
    println!("File: {}", wav_path);

    let (sample_rate, samples) = match read_wav_file(wav_path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", wav_path, e);
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

    // Load DW reference
    let (pkt_path, log_path) = if let Some(ref_path) = reference {
        (ref_path.to_string(), String::new())
    } else {
        match discover_dw_reference(wav_path) {
            Some((p, l)) => (p, l),
            None => {
                eprintln!("Cannot find Dire Wolf reference for {}", wav_path);
                eprintln!("Use --reference <file> to specify explicitly");
                return;
            }
        }
    };

    println!("Reference: {}", pkt_path);

    let dw_packets = match load_dw_packets(&pkt_path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error loading reference: {}", e);
            return;
        }
    };

    // Load clean log for enrichment (optional — may not exist)
    let dw_log = if !log_path.is_empty() {
        parse_dw_clean_log(&log_path).unwrap_or_default()
    } else {
        Vec::new()
    };

    // Build DW lookup: TNC2 string → DwFrameInfo (first occurrence)
    let dw_info: std::collections::HashMap<String, DwFrameInfo> = dw_log
        .iter()
        .map(|(pkt, info)| (pkt.clone(), info.clone()))
        .collect();

    let dw_set: std::collections::HashSet<String> = dw_packets.iter().cloned().collect();

    println!(
        "DW total: {} frames ({} unique)",
        dw_packets.len(),
        dw_set.len()
    );
    println!();

    // Run all decoder modes including best single-decoder configs from attribution
    struct ModeResult {
        name: &'static str,
        tnc2_frames: Vec<String>,
    }

    let fast = decode_fast(&samples, sample_rate);
    let (quality, qual_soft) = decode_quality(&samples, sample_rate);
    let fast_adapt = decode_fast_adaptive(&samples, sample_rate);
    let qual_adapt = decode_quality_adaptive(&samples, sample_rate);
    let best_single = decode_best_single(&samples, sample_rate);
    let (smart3, smart3_soft) = decode_smart3(&samples, sample_rate);
    let (multi, multi_soft) = decode_multi(&samples, sample_rate);
    let dm = decode_dm(&samples, sample_rate);
    // Best single decoders from attribution coverage curve
    let best1 = decode_custom_goertzel(&samples, sample_rate, -50, 2, -1); // G:freq-50/t2
    let best2 = decode_custom_goertzel(&samples, sample_rate, 0, 0, 0); // G:narrow/t0
    let best3 = decode_custom_goertzel(&samples, sample_rate, 0, 1, 0); // G:narrow/t1

    let modes = vec![
        ModeResult {
            name: "fast",
            tnc2_frames: frames_to_tnc2(&fast.frames),
        },
        ModeResult {
            name: "quality",
            tnc2_frames: frames_to_tnc2(&quality.frames),
        },
        ModeResult {
            name: "fast+adapt",
            tnc2_frames: frames_to_tnc2(&fast_adapt.frames),
        },
        ModeResult {
            name: "qual+adapt",
            tnc2_frames: frames_to_tnc2(&qual_adapt.frames),
        },
        ModeResult {
            name: "best-single",
            tnc2_frames: frames_to_tnc2(&best_single.frames),
        },
        ModeResult {
            name: "dm",
            tnc2_frames: frames_to_tnc2(&dm.frames),
        },
        ModeResult {
            name: "freq-50/t2",
            tnc2_frames: frames_to_tnc2(&best1.frames),
        },
        ModeResult {
            name: "narrow/t0",
            tnc2_frames: frames_to_tnc2(&best2.frames),
        },
        ModeResult {
            name: "narrow/t1",
            tnc2_frames: frames_to_tnc2(&best3.frames),
        },
        ModeResult {
            name: "smart3",
            tnc2_frames: frames_to_tnc2(&smart3.frames),
        },
        ModeResult {
            name: "multi",
            tnc2_frames: frames_to_tnc2(&multi.frames),
        },
    ];

    // Summary table
    println!("=== Decoder Mode Comparison vs Dire Wolf ===");
    println!(
        "{:<14} {:>8} {:>8} {:>8} {:>8} {:>6}",
        "Mode", "Decoded", "Overlap", "DW-only", "Us-only", "Soft"
    );
    println!("{}", "─".repeat(62));

    // Map mode names to their soft recovery counts
    let soft_map: std::collections::HashMap<&str, u32> = [
        ("quality", qual_soft),
        ("smart3", smart3_soft),
        ("multi", multi_soft),
    ]
    .iter()
    .copied()
    .collect();

    for mode in &modes {
        let us_set: std::collections::HashSet<&str> =
            mode.tnc2_frames.iter().map(|s| s.as_str()).collect();
        let overlap = dw_set
            .iter()
            .filter(|p| us_set.contains(p.as_str()))
            .count();
        let dw_only = dw_set.len() - overlap;
        let us_only = us_set.len() - overlap;
        let soft_str = match soft_map.get(mode.name) {
            Some(&n) if n > 0 => format!("{}", n),
            _ => "-".to_string(),
        };
        println!(
            "{:<14} {:>8} {:>8} {:>8} {:>8} {:>6}",
            mode.name,
            mode.tnc2_frames.len(),
            overlap,
            dw_only,
            us_only,
            soft_str
        );
    }
    println!();

    // Detailed analysis for multi-decoder (our best — last in modes list)
    let multi_tnc2 = &modes[modes.len() - 1].tnc2_frames;
    let multi_set: std::collections::HashSet<&str> =
        multi_tnc2.iter().map(|s| s.as_str()).collect();
    let overlap: Vec<&String> = dw_set
        .iter()
        .filter(|p| multi_set.contains(p.as_str()))
        .collect();
    let dw_only: Vec<&String> = dw_set
        .iter()
        .filter(|p| !multi_set.contains(p.as_str()))
        .collect();
    let us_only: Vec<String> = multi_set
        .iter()
        .filter(|&&p| !dw_set.contains(p))
        .map(|&s| s.to_string())
        .collect();

    println!("=== Detailed: Multi-Decoder vs Dire Wolf ===");
    println!(
        "DW decoded:   {:>5}    Us decoded: {:>5}",
        dw_set.len(),
        multi_set.len()
    );
    println!(
        "Overlap:      {:>5}    DW-only:    {:>5}    Us-only: {:>5}",
        overlap.len(),
        dw_only.len(),
        us_only.len()
    );
    println!();

    // DW-only frames with enrichment
    if !dw_only.is_empty() {
        println!("--- DW-only frames (we miss, multi-decoder) ---");
        println!(
            "  {:>3}  {:<10} {:>5} {:>5}  Packet",
            "#", "Time", "Audio", "Mk/Sp"
        );
        let mut sorted_dw_only: Vec<(&String, Option<&DwFrameInfo>)> =
            dw_only.iter().map(|p| (*p, dw_info.get(*p))).collect();
        sorted_dw_only.sort_by_key(|(_, info)| info.map(|i| i.seq).unwrap_or(9999));

        for (i, (pkt, info)) in sorted_dw_only.iter().enumerate() {
            let (time, audio, ms) = match info {
                Some(inf) => (
                    inf.timestamp.as_str(),
                    format!("{}", inf.audio_level),
                    inf.mark_space.clone(),
                ),
                None => ("?", "?".to_string(), "?".to_string()),
            };
            let display = truncate_str(pkt, 80);
            println!(
                "  {:>3}  {:<10} {:>5} {:>5}  {}",
                i + 1,
                time,
                audio,
                ms,
                display
            );
        }
        println!();

        // Audio level distribution
        let mut level_bins = [0u32; 5]; // 0-19, 20-39, 40-59, 60-79, 80+
        let mut ratio_bins = [0u32; 3]; // <=2, 3-5, 6+
        let mut enriched = 0u32;

        for (_, info) in &sorted_dw_only {
            if let Some(inf) = info {
                enriched += 1;
                let bin = match inf.audio_level {
                    0..=19 => 0,
                    20..=39 => 1,
                    40..=59 => 2,
                    60..=79 => 3,
                    _ => 4,
                };
                level_bins[bin] += 1;

                let ratio = if inf.space > 0 {
                    inf.mark / inf.space
                } else {
                    0
                };
                let r_bin = match ratio {
                    0..=2 => 0,
                    3..=5 => 1,
                    _ => 2,
                };
                ratio_bins[r_bin] += 1;
            }
        }

        if enriched > 0 {
            println!("--- DW-only by audio level distribution ---");
            let level_labels = [
                "Level  0-19",
                "Level 20-39",
                "Level 40-59",
                "Level 60-79",
                "Level 80+  ",
            ];
            for (i, &label) in level_labels.iter().enumerate() {
                let count = level_bins[i];
                if count > 0 || i >= 1 {
                    let pct = count as f64 / enriched as f64 * 100.0;
                    println!("  {}: {:>3} frames ({:.0}%)", label, count, pct);
                }
            }
            println!();

            println!("--- DW-only by mark/space ratio ---");
            let ratio_labels = [
                "Ratio <=2 (flat)     ",
                "Ratio  3-5 (moderate)",
                "Ratio  6+  (severe) ",
            ];
            for (i, &label) in ratio_labels.iter().enumerate() {
                let count = ratio_bins[i];
                let pct = count as f64 / enriched as f64 * 100.0;
                println!("  {}: {:>3} frames ({:.0}%)", label, count, pct);
            }
            println!();
        }
    }

    // Us-only frames
    if !us_only.is_empty() {
        println!("--- Us-only frames (we find, DW misses) ---");
        for (i, pkt) in us_only.iter().enumerate().take(20) {
            let display = truncate_str(pkt, 80);
            println!("  {:>3}  {}", i + 1, display);
        }
        if us_only.len() > 20 {
            println!("  ... and {} more", us_only.len() - 20);
        }
        println!();
    }

    // Per-mode DW-only analysis: which modes find which DW-only frames?
    if !dw_only.is_empty() && modes.len() > 1 {
        println!("--- DW-only recovery by mode ---");
        println!(
            "  Of {} DW-only frames (vs multi), how many does each mode find?",
            dw_only.len()
        );
        for mode in &modes {
            let mode_set: std::collections::HashSet<&str> =
                mode.tnc2_frames.iter().map(|s| s.as_str()).collect();
            let recovered_frames: Vec<&&String> = dw_only
                .iter()
                .filter(|p| mode_set.contains(p.as_str()))
                .collect();
            println!(
                "    {:<14}: {} of {}",
                mode.name,
                recovered_frames.len(),
                dw_only.len()
            );
            if !recovered_frames.is_empty() && recovered_frames.len() <= 10 {
                for f in &recovered_frames {
                    println!("      >> {}", f);
                    // Search multi's output for near-matches (same source callsign)
                    if mode.name != "multi" {
                        let src = f.split('>').next().unwrap_or("");
                        let multi_mode = modes.last().unwrap(); // multi is last
                        let near: Vec<&String> = multi_mode
                            .tnc2_frames
                            .iter()
                            .filter(|m| m.starts_with(src) && m.as_str() != f.as_str())
                            .collect();
                        if !near.is_empty() {
                            println!("         multi has {} variant(s) from {}:", near.len(), src);
                            for n in &near {
                                println!("           {}", n);
                            }
                        } else {
                            println!("         multi has NO frames from {}", src);
                        }
                    }
                }
            }
        }
        println!();
    }
}

/// Convert a batch of raw AX.25 frames to TNC2 strings.
pub fn frames_to_tnc2(frames: &[Vec<u8>]) -> Vec<String> {
    frames.iter().filter_map(|f| frame_to_tnc2(f)).collect()
}
