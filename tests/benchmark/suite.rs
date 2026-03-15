//\! Suite and single WAV file benchmarks.

use crate::common::*;

pub fn run_single_wav(path: &str, mcu_only: bool) {
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

    let display = track_display_name(path);
    let result = decode_all_unified(&display, &samples, sample_rate, None, mcu_only);
    let results = vec![result];
    print_unified_grid("═══ Single WAV Decode ═══", &results, mcu_only);
    print_timing_summary(&results, mcu_only);
}

/// Run the suite on a set of WAV files, returning UnifiedResults.
pub fn run_suite_on_files(
    wav_files: &[String],
    dw_entries: &[DireWolfEntry],
    mcu_only: bool,
) -> Vec<UnifiedResult> {
    let mut results = Vec::new();

    for wav_path in wav_files {
        let (sample_rate, samples) = match read_wav_file(wav_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  Error reading {}: {}", wav_path, e);
                continue;
            }
        };

        let display = track_display_name(wav_path);
        eprint!("  Decoding {}... ", display);

        // Match DW: try full filename, then try without rate suffix
        let fname = wav_filename(wav_path);
        let dw_count = dw_entries
            .iter()
            .find(|e| e.track_file == fname)
            .map(|e| e.decoded_packets);

        let result = decode_all_unified(&display, &samples, sample_rate, dw_count, mcu_only);

        // Progress output
        let mut progress = String::new();
        if let Some((c, _)) = result.fast {
            progress.push_str(&format!("fast={}", c));
        }
        if let Some((c, _)) = result.smart3 {
            if !progress.is_empty() {
                progress.push_str(", ");
            }
            progress.push_str(&format!("smart3={}", c));
        }
        if let Some((c, _)) = result.multi {
            if !progress.is_empty() {
                progress.push_str(", ");
            }
            progress.push_str(&format!("multi={}", c));
        }
        if let Some((c, _)) = result.combined {
            if !progress.is_empty() {
                progress.push_str(", ");
            }
            progress.push_str(&format!("combined={}", c));
        }
        eprintln!("{}", progress);

        results.push(result);
    }

    results
}

/// Print best result per decoder per track across all rates.
pub fn print_best_across_rates(all_rate_results: &[(u32, Vec<UnifiedResult>)], mcu_only: bool) {
    let cols = num_cols(mcu_only);

    // Collect all unique display names (base names, without rate suffixes)
    let mut track_names: Vec<String> = Vec::new();
    for (_, results) in all_rate_results {
        for r in results {
            // Strip rate suffix from display name for matching
            let base = r.display_name.clone();
            let base_name = {
                let stem = &base;
                if let Some((b, _)) = extract_rate_suffix(stem) {
                    b.to_string()
                } else {
                    stem.clone()
                }
            };
            if !track_names.contains(&base_name) {
                track_names.push(base_name);
            }
        }
    }

    println!("═══ Best Results Across All Rates ═══");
    println!();

    // Header with DW
    let have_dw = all_rate_results
        .iter()
        .any(|(_, rs)| rs.iter().any(|r| r.dw_count.is_some()));
    let dw_hdr = if have_dw {
        format!("{:>5}", "DW")
    } else {
        String::new()
    };
    let mut hdr = format!("{:<30} {}", "Track", dw_hdr);
    for (i, col_name) in COL_NAMES.iter().enumerate().take(cols) {
        if i == MCU_COLS && !mcu_only {
            hdr.push_str(" \u{2502}");
        }
        hdr.push_str(&format!(" {:>5}", col_name));
    }
    println!("{}", hdr);

    // Best counts row + rate annotation row
    for track_name in &track_names {
        let mut best_count = vec![0usize; cols];
        let mut best_rate = vec![0u32; cols]; // 0 = native
        let mut dw: Option<u32> = None;

        for (rate, results) in all_rate_results {
            for r in results {
                let base = {
                    if let Some((b, _)) = extract_rate_suffix(&r.display_name) {
                        b.to_string()
                    } else {
                        r.display_name.clone()
                    }
                };
                if &base != track_name {
                    continue;
                }

                if dw.is_none() {
                    dw = r.dw_count;
                }

                for (i, (bc, br)) in best_count.iter_mut().zip(best_rate.iter_mut()).enumerate() {
                    if let Some(c) = r.count(i) {
                        if c > *bc {
                            *bc = c;
                            *br = *rate;
                        }
                    }
                }
            }
        }

        // Count row
        let dw_str = if have_dw {
            format!(" {:>5}", dw.map_or("?".to_string(), |d| d.to_string()))
        } else {
            String::new()
        };
        let truncated = if track_name.len() > 30 {
            &track_name[..30]
        } else {
            track_name.as_str()
        };
        let mut row = format!("{:<30}{}", truncated, dw_str);
        for (i, bc) in best_count.iter().enumerate() {
            if i == MCU_COLS && !mcu_only {
                row.push_str(" \u{2502}");
            }
            if *bc > 0 {
                row.push_str(&format!(" {:>5}", bc));
            } else {
                row.push_str("     -");
            }
        }
        println!("{}", row);

        // Rate annotation row
        let mut rate_row = format!("{:<30}{}", "", if have_dw { "      " } else { "" });
        for (i, (bc, br)) in best_count.iter().zip(best_rate.iter()).enumerate() {
            if i == MCU_COLS && !mcu_only {
                rate_row.push_str("  \u{2502}");
            }
            if *bc > 0 {
                let abbr = if *br == 0 { "nat" } else { rate_abbrev(*br) };
                rate_row.push_str(&format!(" {:>5}", abbr));
            } else {
                rate_row.push_str("      ");
            }
        }
        println!("{}", rate_row);
    }
    println!();
}

pub fn run_suite(dir: &str, rate: Option<u32>, all_rates: bool, mcu_only: bool) {
    println!("═══ Benchmark Suite ═══");
    println!();

    // Collect all WAV files
    let mut all_wav_files: Vec<String> = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "wav").unwrap_or(false) {
                    all_wav_files.push(path.to_string_lossy().to_string());
                }
            }
        }
        Err(e) => {
            eprintln!("Error reading directory {}: {}", dir, e);
            return;
        }
    }
    all_wav_files.sort();

    if all_wav_files.is_empty() {
        println!("No WAV files found in {}", dir);
        println!("Download test files from http://wa8lmf.net/TNCtest/");
        return;
    }

    // Load DW baselines
    let dw_entries = load_dw_data(dir);

    // Classify files: separate base files from rate-suffixed variants
    let mut base_files: Vec<String> = Vec::new();
    let mut rate_files: std::collections::HashMap<u32, Vec<String>> =
        std::collections::HashMap::new();

    for wav_path in &all_wav_files {
        let stem = std::path::Path::new(wav_path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if let Some((_base, r)) = extract_rate_suffix(&stem) {
            rate_files.entry(r).or_default().push(wav_path.clone());
        } else {
            base_files.push(wav_path.clone());
        }
    }

    if all_rates {
        // Discover all available rates
        let mut available_rates: Vec<u32> = rate_files.keys().copied().collect();
        available_rates.sort();

        // Run base files first (native rate)
        if !base_files.is_empty() {
            let results = run_suite_on_files(&base_files, &dw_entries, mcu_only);
            print_unified_grid("═══ Native Rate ═══", &results, mcu_only);
            print_timing_summary(&results, mcu_only);
        }

        // Run each discovered rate
        let mut all_rate_results: Vec<(u32, Vec<UnifiedResult>)> = Vec::new();

        if !base_files.is_empty() {
            // Collect native rate results for best-across-rates
            let native_results = run_suite_on_files(&base_files, &dw_entries, mcu_only);
            all_rate_results.push((0, native_results)); // 0 = native
        }

        for &r in &available_rates {
            if let Some(files) = rate_files.get(&r) {
                let mut sorted = files.clone();
                sorted.sort();
                let results = run_suite_on_files(&sorted, &dw_entries, mcu_only);
                let title = format!("═══ {} Hz ═══", r);
                print_unified_grid(&title, &results, mcu_only);
                print_timing_summary(&results, mcu_only);
                all_rate_results.push((r, results));
            }
        }

        // Print best-across-rates summary
        if all_rate_results.len() > 1 {
            print_best_across_rates(&all_rate_results, mcu_only);
        }
    } else if let Some(target_rate) = rate {
        // Run only files matching the target rate
        if let Some(files) = rate_files.get(&target_rate) {
            let mut sorted = files.clone();
            sorted.sort();
            let results = run_suite_on_files(&sorted, &dw_entries, mcu_only);
            let title = format!("═══ {} Hz ═══", target_rate);
            print_unified_grid(&title, &results, mcu_only);
            print_timing_summary(&results, mcu_only);
        } else {
            println!("No WAV files found at {} Hz in {}", target_rate, dir);
            let available: Vec<u32> = rate_files.keys().copied().collect();
            if !available.is_empty() {
                let mut sorted = available;
                sorted.sort();
                println!("Available rates: {:?}", sorted);
            }
        }
    } else {
        // Default: only base files (no rate suffix) — same as legacy behavior
        let wav_files = if base_files.is_empty() {
            // If no base files, use all files (for directories with only rate-suffixed files)
            all_wav_files.clone()
        } else {
            base_files
        };
        let results = run_suite_on_files(&wav_files, &dw_entries, mcu_only);
        print_unified_grid("═══ Benchmark Results ═══", &results, mcu_only);
        print_timing_summary(&results, mcu_only);
    }
}
