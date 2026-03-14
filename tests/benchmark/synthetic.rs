//! Synthetic signal benchmark.
//!
//! Tests demodulator performance under controlled, reproducible impairment
//! conditions. Serves as a guard rail against overfitting to WA8LMF tracks.

use crate::common::*;

/// Apply a chain of impairments to clean audio, returning the impaired signal.
pub(crate) fn apply_impairments(
    clean: &[i16],
    sample_rate: u32,
    snr_db: Option<f64>,
    freq_offset_hz: Option<f64>,
    clock_drift: Option<f64>,
    de_emphasis_alpha: Option<f64>,
    clip_threshold: Option<i16>,
    timing_jitter: Option<f64>,
    impulse_density: Option<f64>,
    amplitude_scale: Option<f64>,
) -> Vec<i16> {
    let mut signal = clean.to_vec();

    // Order matters: de-emphasis and clipping before noise (they're signal-path effects),
    // timing jitter affects symbol boundaries, noise is additive channel.
    if let Some(alpha) = de_emphasis_alpha {
        signal = apply_de_emphasis(&signal, alpha);
    }
    if let Some(threshold) = clip_threshold {
        signal = apply_clipping(&signal, threshold);
    }
    if let Some(scale) = amplitude_scale {
        signal = scale_amplitude(&signal, scale);
    }
    if let Some(offset) = freq_offset_hz {
        signal = apply_frequency_offset(&signal, offset, sample_rate);
    }
    if let Some(drift) = clock_drift {
        signal = apply_clock_drift(&signal, drift);
    }
    if let Some(jitter) = timing_jitter {
        let samples_per_symbol = sample_rate as f64 / 1200.0;
        signal = add_timing_jitter(&signal, jitter, samples_per_symbol, 123);
    }
    if let Some(snr) = snr_db {
        signal = add_white_noise(&signal, snr, 42);
    }
    if let Some(density) = impulse_density {
        signal = add_impulse_noise(&signal, density, 20000, 77);
    }
    signal
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    Easy,
    Medium,
    Hard,
    VeryHard,
}

impl Tier {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tier::Easy => "Easy",
            Tier::Medium => "Medium",
            Tier::Hard => "Hard",
            Tier::VeryHard => "Very Hard",
        }
    }
}

pub(crate) struct Scenario {
    pub(crate) name: &'static str,
    pub(crate) tier: Tier,
    pub(crate) snr_db: Option<f64>,
    pub(crate) freq_offset_hz: Option<f64>,
    pub(crate) clock_drift: Option<f64>,
    pub(crate) de_emphasis_alpha: Option<f64>,
    pub(crate) clip_threshold: Option<i16>,
    pub(crate) timing_jitter: Option<f64>,
    pub(crate) impulse_density: Option<f64>,
    pub(crate) amplitude_scale: Option<f64>,
}

impl Scenario {
    /// Convenience: original-style scenario (SNR + freq + drift only).
    const fn basic(
        name: &'static str,
        tier: Tier,
        snr_db: Option<f64>,
        freq_offset_hz: Option<f64>,
        clock_drift: Option<f64>,
    ) -> Self {
        Self {
            name, tier, snr_db, freq_offset_hz, clock_drift,
            de_emphasis_alpha: None, clip_threshold: None,
            timing_jitter: None, impulse_density: None, amplitude_scale: None,
        }
    }
}

pub(crate) fn all_scenarios() -> Vec<Scenario> {
    vec![
        // ── Easy (5 scenarios, 500 max) ──
        Scenario::basic("Clean signal", Tier::Easy, None, None, None),
        Scenario::basic("20 dB SNR", Tier::Easy, Some(20.0), None, None),
        Scenario::basic("+25 Hz offset", Tier::Easy, None, Some(25.0), None),
        Scenario::basic("0.5% clock drift", Tier::Easy, None, None, Some(1.005)),
        Scenario {
            name: "Clip 90% (14745)",
            tier: Tier::Easy,
            clip_threshold: Some(14745),
            ..Scenario::basic("", Tier::Easy, None, None, None)
        },

        // ── Medium (5 scenarios, 500 max) ──
        Scenario::basic("10 dB SNR", Tier::Medium, Some(10.0), None, None),
        Scenario {
            name: "De-emph mild (0.3)",
            tier: Tier::Medium,
            de_emphasis_alpha: Some(0.3),
            ..Scenario::basic("", Tier::Medium, None, None, None)
        },
        Scenario::basic("+50 Hz + 1% drift", Tier::Medium, None, Some(50.0), Some(1.01)),
        Scenario {
            name: "Jitter +/-0.5 samp",
            tier: Tier::Medium,
            timing_jitter: Some(0.5),
            ..Scenario::basic("", Tier::Medium, None, None, None)
        },
        Scenario {
            name: "Weak 0.1x + 15dB SNR",
            tier: Tier::Medium,
            amplitude_scale: Some(0.1),
            snr_db: Some(15.0),
            ..Scenario::basic("", Tier::Medium, None, None, None)
        },

        // ── Hard (7 scenarios, 700 max) ──
        Scenario::basic("6 dB SNR", Tier::Hard, Some(6.0), None, None),
        Scenario {
            name: "De-emph heavy (0.6)",
            tier: Tier::Hard,
            de_emphasis_alpha: Some(0.6),
            ..Scenario::basic("", Tier::Hard, None, None, None)
        },
        Scenario::basic("+100 Hz + 2% drift", Tier::Hard, None, Some(100.0), Some(1.02)),
        Scenario {
            name: "Clip 50% (8192) + 10dB",
            tier: Tier::Hard,
            clip_threshold: Some(8192),
            snr_db: Some(10.0),
            ..Scenario::basic("", Tier::Hard, None, None, None)
        },
        Scenario {
            name: "Impulse 0.5% + 10dB",
            tier: Tier::Hard,
            impulse_density: Some(0.005),
            snr_db: Some(10.0),
            ..Scenario::basic("", Tier::Hard, None, None, None)
        },
        Scenario {
            name: "Combined: 10dB+deemph+1%drift",
            tier: Tier::Hard,
            snr_db: Some(10.0),
            de_emphasis_alpha: Some(0.4),
            clock_drift: Some(1.01),
            ..Scenario::basic("", Tier::Hard, None, None, None)
        },
        Scenario::basic("0 dB SNR (noise floor)", Tier::Hard, Some(0.0), None, None),

        // ── Very Hard (4 scenarios, 400 max) ──
        Scenario::basic("3 dB SNR", Tier::VeryHard, Some(3.0), None, None),
        Scenario {
            name: "Combined: 6dB+clip+jitter",
            tier: Tier::VeryHard,
            snr_db: Some(6.0),
            clip_threshold: Some(10000),
            timing_jitter: Some(0.5),
            ..Scenario::basic("", Tier::VeryHard, None, None, None)
        },
        Scenario {
            name: "Worst: 3dB+deemph+clip+jit+imp",
            tier: Tier::VeryHard,
            snr_db: Some(3.0),
            de_emphasis_alpha: Some(0.5),
            clip_threshold: Some(10000),
            timing_jitter: Some(0.5),
            impulse_density: Some(0.001),
            ..Scenario::basic("", Tier::VeryHard, None, None, None)
        },
        Scenario {
            name: "De-emph heavy (0.6) + 6dB",
            tier: Tier::VeryHard,
            de_emphasis_alpha: Some(0.6),
            snr_db: Some(6.0),
            ..Scenario::basic("", Tier::VeryHard, None, None, None)
        },
    ]
}

/// Number of synthetic test packets per scenario.
pub(crate) const NUM_SYNTHETIC_PACKETS: usize = 100;

/// Generate clean synthetic audio (100 APRS packets).
/// Uses 11025 Hz sample rate (from ModConfig). Deterministic: same seed always
/// produces the same audio. The `_sample_rate` parameter is reserved for future use.
pub(crate) fn generate_synthetic_audio(_sample_rate: u32) -> Vec<i16> {
    use packet_radio_core::ax25::frame::{build_test_frame, hdlc_encode};
    use packet_radio_core::modem::afsk::AfskModulator;
    use packet_radio_core::modem::ModConfig;

    let num_packets = NUM_SYNTHETIC_PACKETS;
    let mut rng: u64 = 42;
    let next_rng = |state: &mut u64| -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    };

    let callsigns = [
        "N0CALL", "WA1ABC", "VE3XYZ", "K4DEF", "W5GHI",
        "KA6JKL", "N7MNO", "W8PQR", "K9STU", "WB0VWX",
    ];

    let mut clean_audio: Vec<i16> = Vec::new();
    let mod_config = if get_baud() == 300 {
        ModConfig::default_300()
    } else {
        ModConfig::default_1200()
    };
    let mut modulator = AfskModulator::new(mod_config);

    for i in 0..num_packets {
        let src = callsigns[i % callsigns.len()];
        let payload = format!(
            "!{:04}.{:02}N/{:05}.{:02}W-Packet {}",
            3000 + (next_rng(&mut rng) % 6000),
            next_rng(&mut rng) % 100,
            7000 + (next_rng(&mut rng) % 12000),
            next_rng(&mut rng) % 100,
            i
        );

        let (frame_data, frame_len) = build_test_frame(src, "APRS", payload.as_bytes());
        let encoded = hdlc_encode(&frame_data[..frame_len]);

        clean_audio.extend_from_slice(&vec![0i16; 1000]);
        for _ in 0..25 {
            let mut buf = [0i16; 128];
            let n = modulator.modulate_flag(&mut buf);
            clean_audio.extend_from_slice(&buf[..n]);
        }
        for bit_idx in 0..encoded.bit_count {
            let bit = encoded.bits[bit_idx] != 0;
            let mut buf = [0i16; 128];
            let n = modulator.modulate_bit(bit, &mut buf);
            clean_audio.extend_from_slice(&buf[..n]);
        }
        clean_audio.extend_from_slice(&[0i16; 20]);
    }

    clean_audio
}

pub fn run_synthetic_benchmark() {
    println!("=== Synthetic Signal Benchmark ===");
    println!();

    let sample_rate: u32 = 11025;
    let num_packets = NUM_SYNTHETIC_PACKETS;

    println!("Generating {} test packets...", num_packets);
    let clean_audio = generate_synthetic_audio(sample_rate);

    let duration_secs = clean_audio.len() as f64 / sample_rate as f64;
    println!(
        "Generated {:.1}s of audio ({} samples)",
        duration_secs,
        clean_audio.len()
    );
    println!();

    let scenarios = all_scenarios();

    let mut total_fast = 0usize;
    let mut total_preemph = 0usize;
    let mut total_quality = 0usize;
    let mut total_multi = 0usize;

    let mut tier_fast = 0usize;
    let mut tier_preemph = 0usize;
    let mut tier_quality = 0usize;
    let mut tier_multi = 0usize;
    let mut tier_count = 0usize;

    // Per-tier multi totals for TIER line
    let mut easy_multi = 0usize;
    let mut easy_max = 0usize;
    let mut medium_multi = 0usize;
    let mut medium_max = 0usize;
    let mut hard_multi = 0usize;
    let mut hard_max = 0usize;
    let mut vhard_multi = 0usize;
    let mut vhard_max = 0usize;

    let mut current_tier: Option<Tier> = None;

    for scenario in &scenarios {
        // Print tier header when tier changes
        if current_tier != Some(scenario.tier) {
            // Print subtotals for previous tier
            if let Some(prev_tier) = current_tier {
                let tier_max = tier_count * num_packets;
                print_tier_subtotals(
                    prev_tier.name(), tier_fast, tier_preemph, tier_quality, tier_multi, tier_max,
                );
                tier_fast = 0;
                tier_preemph = 0;
                tier_quality = 0;
                tier_multi = 0;
                tier_count = 0;
            }
            current_tier = Some(scenario.tier);
            println!();
            println!("  --- {} ---", scenario.tier.name());
            println!(
                "  {:<34}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}",
                "Scenario", "Fast", "Preemph", "Quality", "Multi", "Soft Saves"
            );
            println!("  {}", "-".repeat(34 + 10 + 10 + 10 + 10 + 10 + 10));
        }

        let signal = apply_impairments(
            &clean_audio, sample_rate,
            scenario.snr_db, scenario.freq_offset_hz, scenario.clock_drift,
            scenario.de_emphasis_alpha, scenario.clip_threshold,
            scenario.timing_jitter, scenario.impulse_density,
            scenario.amplitude_scale,
        );

        let fast = decode_fast(&signal, sample_rate);
        let (preemph, _) = decode_fast_preemph(&signal, sample_rate);
        let (quality, soft_saves) = decode_quality(&signal, sample_rate);
        let (multi, _) = decode_multi(&signal, sample_rate);

        let f = fast.frames.len();
        let p = preemph.frames.len();
        let q = quality.frames.len();
        let m = multi.frames.len();

        total_fast += f;
        total_preemph += p;
        total_quality += q;
        total_multi += m;
        tier_fast += f;
        tier_preemph += p;
        tier_quality += q;
        tier_multi += m;
        tier_count += 1;

        match scenario.tier {
            Tier::Easy => { easy_multi += m; easy_max += num_packets; }
            Tier::Medium => { medium_multi += m; medium_max += num_packets; }
            Tier::Hard => { hard_multi += m; hard_max += num_packets; }
            Tier::VeryHard => { vhard_multi += m; vhard_max += num_packets; }
        }

        println!(
            "  {:<34}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}  {:>10}",
            scenario.name,
            f, num_packets,
            p, num_packets,
            q, num_packets,
            m, num_packets,
            soft_saves
        );
    }

    // Print subtotals for last tier
    if let Some(prev_tier) = current_tier {
        let tier_max = tier_count * num_packets;
        print_tier_subtotals(
            prev_tier.name(), tier_fast, tier_preemph, tier_quality, tier_multi, tier_max,
        );
    }

    // Overall totals
    let max_total = scenarios.len() * num_packets;
    println!();
    println!("  {}", "=".repeat(34 + 10 + 10 + 10 + 10 + 10 + 10));
    println!(
        "  {:<34}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}",
        "OVERALL TOTAL",
        total_fast, max_total,
        total_preemph, max_total,
        total_quality, max_total,
        total_multi, max_total,
    );
    println!(
        "  {:<34}  {:>9.1}%  {:>9.1}%  {:>9.1}%  {:>9.1}%",
        "",
        total_fast as f64 / max_total as f64 * 100.0,
        total_preemph as f64 / max_total as f64 * 100.0,
        total_quality as f64 / max_total as f64 * 100.0,
        total_multi as f64 / max_total as f64 * 100.0,
    );

    // Machine-parseable TIER line (multi decoder counts)
    println!();
    println!(
        "TIER easy={}/{} medium={}/{} hard={}/{} vhard={}/{} total={}/{}",
        easy_multi, easy_max,
        medium_multi, medium_max,
        hard_multi, hard_max,
        vhard_multi, vhard_max,
        total_multi, max_total,
    );
}

fn print_tier_subtotals(
    tier_name: &str,
    fast: usize,
    preemph: usize,
    quality: usize,
    multi: usize,
    max: usize,
) {
    println!("  {}", "-".repeat(34 + 10 + 10 + 10 + 10 + 10 + 10));
    println!(
        "  {:<34}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}",
        format!("{} subtotal", tier_name),
        fast, max,
        preemph, max,
        quality, max,
        multi, max,
    );
    if max > 0 {
        println!(
            "  {:<34}  {:>9.1}%  {:>9.1}%  {:>9.1}%  {:>9.1}%",
            "",
            fast as f64 / max as f64 * 100.0,
            preemph as f64 / max as f64 * 100.0,
            quality as f64 / max as f64 * 100.0,
            multi as f64 / max as f64 * 100.0,
        );
    }
}

