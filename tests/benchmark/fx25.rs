//! FX.25 multi-decoder benchmark.
//!
//! Generates 500 FX.25-encoded APRS frames per difficulty tier (easy/medium/hard/extreme),
//! applies controlled impairments, then compares single-decoder vs multi-decoder vs DireWolf.

use std::process::Command;

use crate::common;
use crate::synthetic::{apply_impairments, Scenario, Tier};
use packet_radio_core::ax25::frame::build_test_frame;
use packet_radio_core::fx25::decode::Fx25Decoder;
use packet_radio_core::fx25::encode::fx25_encode;
use packet_radio_core::modem::afsk::AfskModulator;
use packet_radio_core::modem::demod::{DemodSymbol, FastDemodulator};
use packet_radio_core::modem::hdlc_bank::AnyHdlc;
use packet_radio_core::modem::multi::MultiDecoder;
use packet_radio_core::modem::ModConfig;

const NUM_PACKETS: usize = 500;
const SAMPLE_RATE: u32 = 11025;
const CHECK_BYTES: u16 = 16;

fn fx25_scenarios() -> Vec<Scenario> {
    vec![
        // ── Easy (5) ──
        Scenario::basic("Clean signal", Tier::Easy, None, None, None),
        Scenario::basic("20 dB SNR", Tier::Easy, Some(20.0), None, None),
        Scenario::basic("+25 Hz offset", Tier::Easy, None, Some(25.0), None),
        Scenario::basic("0.5% clock drift", Tier::Easy, None, None, Some(1.005)),
        Scenario::basic("-25 Hz offset", Tier::Easy, None, Some(-25.0), None),
        // ── Medium (5) ──
        Scenario::basic("12 dB SNR", Tier::Medium, Some(12.0), None, None),
        Scenario {
            name: "De-emph mild (0.3)",
            tier: Tier::Medium,
            de_emphasis_alpha: Some(0.3),
            ..Scenario::basic("", Tier::Medium, None, None, None)
        },
        Scenario::basic(
            "+50 Hz + 1% drift",
            Tier::Medium,
            None,
            Some(50.0),
            Some(1.01),
        ),
        Scenario {
            name: "Jitter +/-0.5 samp",
            tier: Tier::Medium,
            timing_jitter: Some(0.5),
            ..Scenario::basic("", Tier::Medium, None, None, None)
        },
        Scenario {
            name: "Weak 0.1x + 15dB",
            tier: Tier::Medium,
            amplitude_scale: Some(0.1),
            snr_db: Some(15.0),
            ..Scenario::basic("", Tier::Medium, None, None, None)
        },
        // ── Hard (5) ──
        Scenario::basic("6 dB SNR", Tier::Hard, Some(6.0), None, None),
        Scenario {
            name: "De-emph heavy (0.5)",
            tier: Tier::Hard,
            de_emphasis_alpha: Some(0.5),
            ..Scenario::basic("", Tier::Hard, None, None, None)
        },
        Scenario::basic(
            "+100 Hz + 2% drift",
            Tier::Hard,
            None,
            Some(100.0),
            Some(1.02),
        ),
        Scenario {
            name: "10dB + de-emph + 1% drift",
            tier: Tier::Hard,
            snr_db: Some(10.0),
            de_emphasis_alpha: Some(0.4),
            clock_drift: Some(1.01),
            ..Scenario::basic("", Tier::Hard, None, None, None)
        },
        Scenario {
            name: "Impulse 0.5% + 10dB",
            tier: Tier::Hard,
            impulse_density: Some(0.005),
            snr_db: Some(10.0),
            ..Scenario::basic("", Tier::Hard, None, None, None)
        },
        // ── Extreme (5) ──
        Scenario::basic("3 dB SNR", Tier::VeryHard, Some(3.0), None, None),
        Scenario {
            name: "De-emph 0.6 + 6dB",
            tier: Tier::VeryHard,
            de_emphasis_alpha: Some(0.6),
            snr_db: Some(6.0),
            ..Scenario::basic("", Tier::VeryHard, None, None, None)
        },
        Scenario {
            name: "6dB + clip + jitter",
            tier: Tier::VeryHard,
            snr_db: Some(6.0),
            clip_threshold: Some(10000),
            timing_jitter: Some(0.5),
            ..Scenario::basic("", Tier::VeryHard, None, None, None)
        },
        Scenario {
            name: "3dB + deemph + clip + jit + imp",
            tier: Tier::VeryHard,
            snr_db: Some(3.0),
            de_emphasis_alpha: Some(0.5),
            clip_threshold: Some(10000),
            timing_jitter: Some(0.5),
            impulse_density: Some(0.001),
            ..Scenario::basic("", Tier::VeryHard, None, None, None)
        },
        Scenario::basic("0 dB SNR", Tier::VeryHard, Some(0.0), None, None),
    ]
}

/// Generate clean FX.25 audio: 500 unique APRS packets, FX.25 encoded.
fn generate_fx25_audio() -> Vec<i16> {
    let callsigns = [
        "N0CALL", "WA1ABC", "VE3XYZ", "K4DEF", "W5GHI", "KA6JKL", "N7MNO", "W8PQR", "K9STU",
        "WB0VWX",
    ];

    let mut rng: u64 = 42;
    let next_rng = |state: &mut u64| -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    };

    let mod_config = ModConfig::default_1200();
    let mut modulator = AfskModulator::new(mod_config);
    let mut audio: Vec<i16> = Vec::new();

    for i in 0..NUM_PACKETS {
        let src = callsigns[i % callsigns.len()];
        let payload = format!(
            "!{:04}.{:02}N/{:05}.{:02}W-FX25 pkt {}",
            3000 + (next_rng(&mut rng) % 6000),
            next_rng(&mut rng) % 100,
            7000 + (next_rng(&mut rng) % 12000),
            next_rng(&mut rng) % 100,
            i
        );

        let (frame_data, frame_len) = build_test_frame(src, "APRS", payload.as_bytes());
        let block = match fx25_encode(&frame_data[..frame_len], CHECK_BYTES) {
            Some(b) => b,
            None => continue,
        };

        // Inter-packet silence
        audio.extend_from_slice(&vec![0i16; 500]);

        // Preamble flags (25 flags = ~167ms at 1200 baud)
        for _ in 0..25 {
            let mut buf = [0i16; 128];
            let n = modulator.modulate_flag(&mut buf);
            audio.extend_from_slice(&buf[..n]);
        }

        // FX.25 block (tag + RS codeword)
        for bit in block.iter_bits() {
            let mut buf = [0i16; 128];
            let n = modulator.modulate_bit(bit, &mut buf);
            audio.extend_from_slice(&buf[..n]);
        }

        // Short postamble
        for _ in 0..3 {
            let mut buf = [0i16; 128];
            let n = modulator.modulate_flag(&mut buf);
            audio.extend_from_slice(&buf[..n]);
        }
    }

    audio
}

/// Decode using single FastDemodulator + Fx25Decoder.
fn decode_single(samples: &[i16]) -> usize {
    let config = common::config_for_rate(SAMPLE_RATE, 1200);
    let mut demod = FastDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr();
    let mut hdlc = AnyHdlc::new();
    let mut fx25 = Fx25Decoder::new();
    let mut symbols = [DemodSymbol {
        bit: false,
        llr: 0,
        sample_idx: 0,
        raw_bit: false,
    }; 1024];

    let mut count = 0usize;
    let mut seen: Vec<(u64, u64)> = Vec::new();

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for sym in &symbols[..n] {
            let mut frame: Option<&[u8]> = None;
            if let Some((f, _cost)) = hdlc.feed(sym.bit, sym.llr) {
                frame = Some(f);
            }
            if let Some(f) = fx25.feed_bit(sym.bit) {
                frame = Some(f);
            }
            if let Some(f) = frame {
                let hash = common::fnv1a_hash(f);
                let pos = sym.sample_idx as u64;
                let is_dup = seen
                    .iter()
                    .any(|&(h, p)| h == hash && pos.abs_diff(p) < 5000);
                if !is_dup {
                    seen.push((hash, pos));
                    count += 1;
                }
            }
        }
    }
    count
}

/// Decode using MultiDecoder (includes integrated FX.25).
fn decode_multi(samples: &[i16]) -> usize {
    let config = common::config_for_rate(SAMPLE_RATE, 1200);
    let mut multi = MultiDecoder::new(config);
    let mut count = 0usize;

    for chunk in samples.chunks(1024) {
        let output = multi.process_samples(chunk);
        count += output.len();
    }
    count
}

/// Run DireWolf atest on a WAV file, return decoded frame count.
/// Returns None if atest is not available.
fn decode_direwolf(wav_path: &str) -> Option<usize> {
    let output = Command::new("atest").arg(wav_path).output().ok()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    // DW outputs: "N from filename" or "N packets decoded in ..."
    for line in combined.lines() {
        let line = line.trim();
        if line.contains(" from ") {
            if let Some(n) = line.split_whitespace().next() {
                if let Ok(count) = n.parse::<usize>() {
                    return Some(count);
                }
            }
        }
    }
    None
}

/// Write samples to a WAV file.
fn write_wav(path: &str, sample_rate: u32, samples: &[i16]) {
    use std::io::Write;
    let mut f = std::fs::File::create(path).expect("create WAV");
    let data_size = (samples.len() * 2) as u32;
    let file_size = 36 + data_size;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&file_size.to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&sample_rate.to_le_bytes()).unwrap();
    f.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_size.to_le_bytes()).unwrap();
    for &s in samples {
        f.write_all(&s.to_le_bytes()).unwrap();
    }
}

/// Check if atest is available.
fn has_atest() -> bool {
    Command::new("atest").arg("--version").output().is_ok()
}

pub fn run_fx25_benchmark() {
    println!("=== FX.25 Multi-Decoder Benchmark ===");
    println!(
        "Generating {} FX.25 packets (RS check_bytes={})...",
        NUM_PACKETS, CHECK_BYTES
    );

    let clean = generate_fx25_audio();
    let duration = clean.len() as f64 / SAMPLE_RATE as f64;
    println!(
        "Generated {:.1}s of audio ({} samples)",
        duration,
        clean.len()
    );

    let dw_available = has_atest();
    if dw_available {
        println!("DireWolf atest: found");
    } else {
        println!("DireWolf atest: not found (DW column will be empty)");
    }

    // Create temp directory for WAV files
    let tmp_dir = std::env::temp_dir().join("fx25_bench");
    std::fs::create_dir_all(&tmp_dir).ok();

    let scenarios = fx25_scenarios();
    println!();

    // Header
    if dw_available {
        println!(
            "{:<33} {:>8} {:>8} {:>8} {:>8}",
            "Scenario", "Single", "DW", "Multi", "%DW"
        );
    } else {
        println!(
            "{:<33} {:>8} {:>8} {:>8}",
            "Scenario", "Single", "Multi", "Gain"
        );
    }
    println!("{}", "-".repeat(if dw_available { 73 } else { 65 }));

    let mut current_tier = None;
    let mut tier_single = 0usize;
    let mut tier_dw = 0usize;
    let mut tier_multi = 0usize;
    let mut tier_count = 0usize;
    let mut total_single = 0usize;
    let mut total_dw = 0usize;
    let mut total_multi = 0usize;

    for (si, scenario) in scenarios.iter().enumerate() {
        // Print tier header on change
        if current_tier != Some(scenario.tier) {
            if let Some(tier) = current_tier {
                print_tier_summary(
                    tier,
                    tier_single,
                    tier_dw,
                    tier_multi,
                    tier_count,
                    dw_available,
                );
                tier_single = 0;
                tier_dw = 0;
                tier_multi = 0;
                tier_count = 0;
            }
            current_tier = Some(scenario.tier);
            println!("  -- {} --", scenario.tier.name());
        }

        let impaired = apply_impairments(
            &clean,
            SAMPLE_RATE,
            scenario.snr_db,
            scenario.freq_offset_hz,
            scenario.clock_drift,
            scenario.de_emphasis_alpha,
            scenario.clip_threshold,
            scenario.timing_jitter,
            scenario.impulse_density,
            scenario.amplitude_scale,
        );

        let single = decode_single(&impaired);
        let multi = decode_multi(&impaired);

        let dw = if dw_available {
            let wav_path = tmp_dir.join(format!("fx25_s{:02}.wav", si));
            let wav_str = wav_path.to_string_lossy().to_string();
            write_wav(&wav_str, SAMPLE_RATE, &impaired);
            decode_direwolf(&wav_str).unwrap_or(0)
        } else {
            0
        };

        if dw_available {
            let pct_dw = if dw > 0 {
                format!("{:.0}%", multi as f64 / dw as f64 * 100.0)
            } else if multi > 0 {
                "+inf".to_string()
            } else {
                "---".to_string()
            };
            println!(
                "  {:<31} {:>5}/{} {:>5}/{} {:>5}/{} {:>7}",
                scenario.name, single, NUM_PACKETS, dw, NUM_PACKETS, multi, NUM_PACKETS, pct_dw
            );
        } else {
            let gain = if single > 0 {
                format!("+{:.0}%", (multi as f64 / single as f64 - 1.0) * 100.0)
            } else if multi > 0 {
                "+inf".to_string()
            } else {
                "---".to_string()
            };
            println!(
                "  {:<31} {:>5}/{} {:>5}/{} {:>8}",
                scenario.name, single, NUM_PACKETS, multi, NUM_PACKETS, gain
            );
        }

        tier_single += single;
        tier_dw += dw;
        tier_multi += multi;
        tier_count += 1;
        total_single += single;
        total_dw += dw;
        total_multi += multi;
    }

    // Final tier
    if let Some(tier) = current_tier {
        print_tier_summary(
            tier,
            tier_single,
            tier_dw,
            tier_multi,
            tier_count,
            dw_available,
        );
    }

    // Grand total
    let total_possible = scenarios.len() * NUM_PACKETS;
    println!();
    let s_pct = total_single as f64 / total_possible as f64 * 100.0;
    let m_pct = total_multi as f64 / total_possible as f64 * 100.0;
    if dw_available {
        let d_pct = total_dw as f64 / total_possible as f64 * 100.0;
        let vs_dw = if total_dw > 0 {
            format!("{:.0}%", total_multi as f64 / total_dw as f64 * 100.0)
        } else {
            "---".to_string()
        };
        println!(
            "{:<33} {:>5}/{} {:>5}/{} {:>5}/{} {:>7}",
            "TOTAL",
            total_single,
            total_possible,
            total_dw,
            total_possible,
            total_multi,
            total_possible,
            vs_dw
        );
        println!("{:<33} {:>7.1}% {:>7.1}% {:>7.1}%", "", s_pct, d_pct, m_pct);
    } else {
        println!(
            "{:<33} {:>5}/{} {:>5}/{}",
            "TOTAL", total_single, total_possible, total_multi, total_possible
        );
        println!("{:<33} {:>7.1}% {:>7.1}%", "", s_pct, m_pct);
    }

    // Cleanup
    std::fs::remove_dir_all(&tmp_dir).ok();
}

fn print_tier_summary(
    tier: Tier,
    single: usize,
    dw: usize,
    multi: usize,
    count: usize,
    show_dw: bool,
) {
    let possible = count * NUM_PACKETS;
    let s_pct = if possible > 0 {
        single as f64 / possible as f64 * 100.0
    } else {
        0.0
    };
    let m_pct = if possible > 0 {
        multi as f64 / possible as f64 * 100.0
    } else {
        0.0
    };
    if show_dw {
        let d_pct = if possible > 0 {
            dw as f64 / possible as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "  {:<31} {:>7.1}% {:>7.1}% {:>7.1}%",
            format!("{} subtotal", tier.name()),
            s_pct,
            d_pct,
            m_pct
        );
    } else {
        println!(
            "  {:<31} {:>7.1}% {:>7.1}%",
            format!("{} subtotal", tier.name()),
            s_pct,
            m_pct
        );
    }
    println!();
}
