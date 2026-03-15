//! Frame export to files.

use crate::common::*;
use crate::dm::decode_dm_pll;

// ─── Frame Export ──────────────────────────────────────────────────────

pub fn run_export(wav_path: &str, output_dir: &str) {
    println!("═══ Frame Export ═══");
    println!("File: {}", wav_path);

    let (sample_rate, samples) = match read_wav_file(wav_path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", wav_path, e);
            return;
        }
    };

    // Create output directory
    if let Err(e) = std::fs::create_dir_all(output_dir) {
        eprintln!("Error creating {}: {}", output_dir, e);
        return;
    }

    #[allow(clippy::type_complexity)]
    let paths: &[(&str, Box<dyn Fn(&[i16], u32) -> DecodeResult>)] = &[
        ("fast", Box::new(decode_fast)),
        ("dm", Box::new(decode_dm)),
        ("dm_pll", Box::new(decode_dm_pll)),
        ("multi", Box::new(|s, sr| decode_multi(s, sr).0)),
    ];

    for &(name, ref decode_fn) in paths {
        let result = decode_fn(&samples, sample_rate);
        let out_path = format!("{}/{}.txt", output_dir, name);
        let mut content = String::new();
        for frame in &result.frames {
            content.push_str(&frame_to_hex(frame));
            // Try to parse AX.25 callsigns
            if frame.len() >= 14 {
                let dst = parse_callsign_tnc2(&frame[0..7], false);
                let src = parse_callsign_tnc2(&frame[7..14], false);
                content.push_str(&format!(" {}>{}", src, dst));
            }
            content.push('\n');
        }
        match std::fs::write(&out_path, &content) {
            Ok(_) => println!("  {} → {} ({} frames)", name, out_path, result.frames.len()),
            Err(e) => eprintln!("  Error writing {}: {}", out_path, e),
        }
    }

    // Frame comparison: show overlap between paths
    println!();
    let fast = decode_fast(&samples, sample_rate);
    let dm = decode_dm(&samples, sample_rate);
    let dm_pll = decode_dm_pll(&samples, sample_rate);
    let (multi, _) = decode_multi(&samples, sample_rate);

    let sets: Vec<(&str, std::collections::HashSet<Vec<u8>>)> = vec![
        ("fast", fast.frames.into_iter().collect()),
        ("dm", dm.frames.into_iter().collect()),
        ("dm_pll", dm_pll.frames.into_iter().collect()),
        ("multi", multi.frames.into_iter().collect()),
    ];

    println!("  Frame overlap matrix:");
    print!("  {:>8}", "");
    for &(name, _) in &sets {
        print!(" {:>8}", name);
    }
    println!();

    for &(name_a, ref set_a) in &sets {
        print!("  {:>8}", name_a);
        for (_, set_b) in &sets {
            let overlap = set_a.intersection(set_b).count();
            print!(" {:>8}", overlap);
        }
        println!();
    }
}
