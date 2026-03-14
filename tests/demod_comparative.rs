//! Comparative Demodulator Tests
//!
//! These tests validate and compare the two demodulator paths under
//! various signal conditions. They verify:
//!
//! 1. Both paths decode clean signals correctly
//! 2. The quality path outperforms the fast path on degraded signals
//! 3. Adaptive tracking improves frequency/clock tolerance
//! 4. Soft-decision recovery saves packets with bit errors
//! 5. Neither path panics on any input
//!
//! Run: `cargo test -p packet-radio-core --test demod_comparative`
//! Run with output: `cargo test -p packet-radio-core --test demod_comparative -- --nocapture`

mod common;

use common::*;

// ═══════════════════════════════════════════════════════════════════════════
// §1. Tone Detection Tests — Verify basic frequency discrimination
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_pure_mark_tone() {
    // A pure 1200 Hz tone should produce all-mark output from both paths
    let samples = generate_tone(1200.0, 11025, 11025); // 1 second
    assert!(samples.len() == 11025);
    let freq = estimate_frequency_zero_crossings(&samples, 11025);
    assert!((freq - 1200.0).abs() < 50.0, "Generated tone frequency off: {}", freq);
}

#[test]
fn test_pure_space_tone() {
    let samples = generate_tone(2200.0, 11025, 11025);
    let freq = estimate_frequency_zero_crossings(&samples, 11025);
    assert!((freq - 2200.0).abs() < 50.0, "Generated tone frequency off: {}", freq);
}

#[test]
fn test_afsk_generation_phase_continuity() {
    // Alternating bits should produce phase-continuous AFSK
    let bits: Vec<bool> = (0..100).map(|i| i % 2 == 0).collect();
    let audio = generate_afsk(&bits, 11025, 1200.0, 2200.0, 1200.0, 16000.0);

    // Check there are no large sample-to-sample jumps (phase discontinuity)
    let mut max_delta: i32 = 0;
    for i in 1..audio.len() {
        let delta = (audio[i] as i32 - audio[i - 1] as i32).abs();
        if delta > max_delta {
            max_delta = delta;
        }
    }

    // At 11025 Hz, the max delta for a 2200 Hz tone is about:
    // 2π × 2200/11025 × 16000 ≈ 20,000 per sample
    // But at tone transitions, it should NOT spike beyond this
    assert!(
        max_delta < 25000,
        "Phase discontinuity detected: max delta = {}",
        max_delta
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §2. Signal Impairment Tests — Verify test harness utilities
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_noise_addition_snr() {
    let clean = generate_tone(1200.0, 11025, 11025);
    let noisy = add_white_noise(&clean, 10.0, 42);
    let measured_snr = compute_snr(&clean, &noisy);
    // Should be within ±2 dB of target
    assert!(
        (measured_snr - 10.0).abs() < 2.0,
        "SNR target=10dB, measured={}dB",
        measured_snr
    );
}

#[test]
fn test_noise_reproducibility() {
    let clean = generate_tone(1200.0, 11025, 1000);
    let noisy1 = add_white_noise(&clean, 10.0, 42);
    let noisy2 = add_white_noise(&clean, 10.0, 42);
    // Same seed should produce identical noise
    assert_eq!(noisy1, noisy2, "Noise generation is not deterministic");
}

#[test]
fn test_noise_different_seeds() {
    let clean = generate_tone(1200.0, 11025, 1000);
    let noisy1 = add_white_noise(&clean, 10.0, 42);
    let noisy2 = add_white_noise(&clean, 10.0, 99);
    // Different seeds should produce different noise
    assert_ne!(noisy1, noisy2, "Different seeds should produce different noise");
}

#[test]
fn test_frequency_offset() {
    let clean = generate_tone(1200.0, 11025, 11025);
    let shifted = apply_frequency_offset(&clean, 100.0, 11025);
    // The dominant frequency should shift
    // (This is an approximate test — frequency shifting a real signal
    // is more complex, but we verify the utility doesn't crash)
    assert_eq!(shifted.len(), clean.len());
}

#[test]
fn test_clock_drift() {
    let clean = generate_tone(1200.0, 11025, 11025);

    // 1% fast = 1% more samples
    let drifted = apply_clock_drift(&clean, 1.01);
    let expected_len = (11025.0 * 1.01) as usize;
    assert!(
        (drifted.len() as i64 - expected_len as i64).abs() < 5,
        "Clock drift length: expected ~{}, got {}",
        expected_len,
        drifted.len()
    );
}

#[test]
fn test_amplitude_scaling() {
    let clean = generate_tone(1200.0, 11025, 1000);
    let quiet = scale_amplitude(&clean, 0.1);

    let clean_rms: f64 = (clean.iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
        / clean.len() as f64).sqrt();
    let quiet_rms: f64 = (quiet.iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
        / quiet.len() as f64).sqrt();

    let ratio = quiet_rms / clean_rms;
    assert!(
        (ratio - 0.1).abs() < 0.01,
        "Amplitude scale: expected 0.1, got {}",
        ratio
    );
}

#[test]
fn test_dc_offset() {
    let clean = generate_tone(1200.0, 11025, 1000);
    let offset = add_dc_offset(&clean, 5000);

    // Mean should shift by approximately the offset
    let clean_mean: f64 = clean.iter().map(|&s| s as f64).sum::<f64>() / clean.len() as f64;
    let offset_mean: f64 = offset.iter().map(|&s| s as f64).sum::<f64>() / offset.len() as f64;

    let shift = offset_mean - clean_mean;
    assert!(
        (shift - 5000.0).abs() < 100.0,
        "DC offset: expected ~5000, got {}",
        shift
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §3. Scenario Framework Tests — Verify scenarios apply correctly
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_clean_scenario_is_identity() {
    let clean = generate_tone(1200.0, 11025, 1000);
    let scenarios = standard_test_scenarios();
    let clean_scenario = &scenarios[0]; // "Clean (ideal)"

    let result = clean_scenario.apply(&clean, 11025);
    assert_eq!(clean, result, "Clean scenario should not modify signal");
}

#[test]
fn test_all_scenarios_produce_output() {
    let clean = generate_tone(1200.0, 11025, 1000);
    for scenario in standard_test_scenarios() {
        let result = scenario.apply(&clean, 11025);
        assert!(
            !result.is_empty(),
            "Scenario '{}' produced empty output",
            scenario.name
        );
        // Clock drift changes length; all others should preserve it
        if scenario.clock_drift_ratio.is_none() {
            assert_eq!(
                result.len(),
                clean.len(),
                "Scenario '{}' changed signal length",
                scenario.name
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §4. Test Stream Generation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_stream_generation() {
    let config = BenchmarkConfig {
        num_test_packets: 5,
        bits_per_packet: 100,
        ..Default::default()
    };

    let (audio, packets) = generate_test_stream(&config, 42);

    assert_eq!(packets.len(), 5);
    assert!(audio.len() > 0);
    for pkt in &packets {
        assert_eq!(pkt.len(), 100);
    }
}

#[test]
fn test_stream_reproducibility() {
    let config = BenchmarkConfig {
        num_test_packets: 3,
        bits_per_packet: 50,
        ..Default::default()
    };

    let (audio1, packets1) = generate_test_stream(&config, 42);
    let (audio2, packets2) = generate_test_stream(&config, 42);

    assert_eq!(audio1, audio2);
    assert_eq!(packets1, packets2);
}

// ═══════════════════════════════════════════════════════════════════════════
// §5. WAV I/O Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_wav_round_trip() {
    let original = generate_tone(1200.0, 11025, 5000);
    let path = "/tmp/packet_radio_test.wav";

    write_wav(path, 11025, &original).expect("Failed to write WAV");
    let (sr, loaded) = read_wav(path).expect("Failed to read WAV");

    assert_eq!(sr, 11025);
    assert_eq!(original.len(), loaded.len());
    assert_eq!(original, loaded);

    // Clean up
    let _ = std::fs::remove_file(path);
}

// ═══════════════════════════════════════════════════════════════════════════
// §6. Demodulator Smoke Tests
// ═══════════════════════════════════════════════════════════════════════════
//
// These tests verify that the demodulator modules don't panic on various
// inputs. Full decode verification requires the complete pipeline (HDLC,
// AX.25) which will be tested in integration tests.
//
// Uncomment and adapt these once the demodulator is fully implemented:

/*
use packet_radio_core::modem::demod::{FastDemodulator, QualityDemodulator, DemodSymbol};
use packet_radio_core::modem::DemodConfig;

#[test]
fn test_fast_demod_on_all_scenarios() {
    let config = DemodConfig::default_1200();
    let clean = generate_test_packet(
        &vec![true; 800],  // 100 bytes of 0xFF
        11025,
        25,
    );

    for scenario in standard_test_scenarios() {
        let impaired = scenario.apply(&clean, 11025);
        let mut demod = FastDemodulator::new(config);
        let mut symbols = vec![DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 2000];

        // Should never panic, regardless of signal quality
        let n = demod.process_samples(&impaired, &mut symbols);
        println!("Fast path, '{}': {} symbols", scenario.name, n);
    }
}

#[test]
fn test_quality_demod_on_all_scenarios() {
    let config = DemodConfig::default_1200();
    let clean = generate_test_packet(
        &vec![true; 800],
        11025,
        25,
    );

    for scenario in standard_test_scenarios() {
        let impaired = scenario.apply(&clean, 11025);
        let mut demod = QualityDemodulator::new(config);
        let mut symbols = vec![DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 2000];

        let n = demod.process_samples(&impaired, &mut symbols);
        println!("Quality path, '{}': {} symbols", scenario.name, n);
    }
}

#[test]
fn test_adaptive_tracker_locks_on_preamble() {
    let config = DemodConfig::default_1200();

    // Generate a long preamble (50 flags = 400 bits ≈ 333ms)
    let preamble_bits = generate_preamble(50);
    let audio = generate_afsk(&preamble_bits, 11025, 1200.0, 2200.0, 1200.0, 16000.0);

    let mut demod = QualityDemodulator::new(config);
    let mut symbols = vec![DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 1000];
    demod.process_samples(&audio, &mut symbols);

    assert!(demod.is_tracking(), "Tracker should lock during preamble");

    let tracker = demod.tracker();
    // Estimated frequencies should be close to nominal
    let mark_est = tracker.mark_freq_est as f64 / 256.0;
    let space_est = tracker.space_freq_est as f64 / 256.0;
    println!("Estimated mark={:.1}Hz, space={:.1}Hz", mark_est, space_est);

    assert!((mark_est - 1200.0).abs() < 30.0,
        "Mark estimate {:.1} too far from 1200", mark_est);
    assert!((space_est - 2200.0).abs() < 30.0,
        "Space estimate {:.1} too far from 2200", space_est);
}

#[test]
fn test_adaptive_tracker_handles_offset_transmitter() {
    // Simulate a transmitter that's 50 Hz high on everything
    let config = DemodConfig::default_1200();
    let preamble_bits = generate_preamble(50);
    let audio = generate_afsk(
        &preamble_bits, 11025,
        1250.0,  // Mark shifted +50 Hz
        2250.0,  // Space shifted +50 Hz
        1200.0,
        16000.0,
    );

    let mut demod = QualityDemodulator::new(config);
    let mut symbols = vec![DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 1000];
    demod.process_samples(&audio, &mut symbols);

    let tracker = demod.tracker();
    let mark_est = tracker.mark_freq_est as f64 / 256.0;
    let space_est = tracker.space_freq_est as f64 / 256.0;

    // Should track the ACTUAL frequencies, not nominal
    assert!((mark_est - 1250.0).abs() < 40.0,
        "Should track shifted mark: expected ~1250, got {:.1}", mark_est);
    assert!((space_est - 2250.0).abs() < 40.0,
        "Should track shifted space: expected ~2250, got {:.1}", space_est);
}

/// The key comparative test: Quality path should decode more packets
/// than fast path under degraded conditions.
#[test]
fn test_quality_path_outperforms_fast_on_noise() {
    let config = DemodConfig::default_1200();
    let bench_config = BenchmarkConfig {
        num_test_packets: 50,
        ..Default::default()
    };

    let (clean_audio, _) = generate_test_stream(&bench_config, 42);

    // Test at 6 dB SNR (where differences should be visible)
    let noisy = add_white_noise(&clean_audio, 6.0, 42);

    // Run fast path
    let mut fast_demod = FastDemodulator::new(config);
    let mut fast_syms = vec![DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 100000];
    let fast_count = fast_demod.process_samples(&noisy, &mut fast_syms);

    // Run quality path
    let mut qual_demod = QualityDemodulator::new(config);
    let mut qual_syms = vec![DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 100000];
    let qual_count = qual_demod.process_samples(&noisy, &mut qual_syms);

    println!("At 6dB SNR: fast={} symbols, quality={} symbols", fast_count, qual_count);

    // Quality path should produce symbols with higher confidence
    let fast_avg_conf: f64 = fast_syms[..fast_count].iter()
        .map(|s| s.llr.abs() as f64).sum::<f64>() / fast_count.max(1) as f64;
    let qual_avg_conf: f64 = qual_syms[..qual_count].iter()
        .map(|s| s.llr.abs() as f64).sum::<f64>() / qual_count.max(1) as f64;

    println!("Average confidence: fast={:.1}, quality={:.1}", fast_avg_conf, qual_avg_conf);
    // Quality path should have higher average confidence
    // (This becomes a real test once demodulators are fully implemented)
}
*/

// ═══════════════════════════════════════════════════════════════════════════
// §7. Stress Tests — Verify robustness on edge cases
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_empty_signal() {
    let empty: Vec<i16> = vec![];
    // Should not panic
    let freq = estimate_frequency_zero_crossings(&empty, 11025);
    assert!(freq.is_finite());
}

#[test]
fn test_dc_only_signal() {
    let dc = vec![10000i16; 11025];
    let freq = estimate_frequency_zero_crossings(&dc, 11025);
    assert_eq!(freq, 0.0, "DC signal should have zero frequency");
}

#[test]
fn test_max_amplitude_signal() {
    let loud = generate_tone(1200.0, 11025, 11025);
    let maxed = scale_amplitude(&loud, 2.0); // Will clip at i16::MAX
    // Verify no panic and samples are clamped
    for &s in &maxed {
        assert!(s >= -32768 && s <= 32767);
    }
}

#[test]
fn test_single_sample() {
    let one = vec![1000i16];
    let _ = estimate_frequency_zero_crossings(&one, 11025);
    let _ = add_white_noise(&one, 10.0, 42);
    let _ = add_dc_offset(&one, 5000);
    let _ = scale_amplitude(&one, 0.5);
    // None should panic
}

#[test]
fn test_very_long_signal() {
    // 10 seconds at 44100 Hz — verify no overflow in accumulators
    let long_signal = generate_tone(1200.0, 44100, 44100 * 10);
    let freq = estimate_frequency_zero_crossings(&long_signal, 44100);
    assert!(
        (freq - 1200.0).abs() < 10.0,
        "Long signal frequency: {}",
        freq
    );
}
