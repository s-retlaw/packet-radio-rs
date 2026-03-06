//! Synthetic signal benchmark.

use crate::common::*;

pub fn run_synthetic_benchmark() {
    use packet_radio_core::ax25::frame::{build_test_frame, hdlc_encode};
    use packet_radio_core::modem::afsk::AfskModulator;
    use packet_radio_core::modem::ModConfig;

    println!("═══ Synthetic Signal Benchmark ═══");
    println!();

    let sample_rate: u32 = 11025;
    let num_packets = 100;

    // Generate test packets
    println!("Generating {} test packets...", num_packets);
    let mut rng: u64 = 42;
    let next_rng = |state: &mut u64| -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    };

    // Build diverse test payloads
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
        // Generate varied payload
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

        // Inter-packet gap (silence)
        let gap = vec![0i16; 1000];
        clean_audio.extend_from_slice(&gap);

        // Preamble flags
        for _ in 0..25 {
            let mut buf = [0i16; 128];
            let n = modulator.modulate_flag(&mut buf);
            clean_audio.extend_from_slice(&buf[..n]);
        }

        // Frame data
        for bit_idx in 0..encoded.bit_count {
            let bit = encoded.bits[bit_idx] != 0;
            let mut buf = [0i16; 128];
            let n = modulator.modulate_bit(bit, &mut buf);
            clean_audio.extend_from_slice(&buf[..n]);
        }

        // Trailing silence
        clean_audio.extend_from_slice(&[0i16; 20]);
    }

    let duration_secs = clean_audio.len() as f64 / sample_rate as f64;
    println!(
        "Generated {:.1}s of audio ({} samples)",
        duration_secs,
        clean_audio.len()
    );
    println!();

    // Define scenarios
    struct Scenario {
        name: &'static str,
        snr_db: Option<f64>,
        freq_offset_hz: Option<f64>,
        clock_drift: Option<f64>,
    }

    let scenarios = [
        Scenario { name: "Clean signal", snr_db: None, freq_offset_hz: None, clock_drift: None },
        Scenario { name: "20 dB SNR", snr_db: Some(20.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "10 dB SNR", snr_db: Some(10.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "6 dB SNR", snr_db: Some(6.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "3 dB SNR", snr_db: Some(3.0), freq_offset_hz: None, clock_drift: None },
        Scenario { name: "+50 Hz offset", snr_db: None, freq_offset_hz: Some(50.0), clock_drift: None },
        Scenario { name: "+100 Hz offset", snr_db: None, freq_offset_hz: Some(100.0), clock_drift: None },
        Scenario { name: "1% clock drift", snr_db: None, freq_offset_hz: None, clock_drift: Some(1.01) },
        Scenario { name: "2% clock drift", snr_db: None, freq_offset_hz: None, clock_drift: Some(1.02) },
        Scenario { name: "10dB + 50Hz + 1%", snr_db: Some(10.0), freq_offset_hz: Some(50.0), clock_drift: Some(1.01) },
        Scenario { name: "6dB + 100Hz + 2%", snr_db: Some(6.0), freq_offset_hz: Some(100.0), clock_drift: Some(1.02) },
    ];

    println!(
        "  {:<32}  {:>10}  {:>10}  {:>10}  {:>10}",
        "Scenario", "Fast", "Quality", "Multi", "Soft Saves"
    );
    println!("  {}", "─".repeat(32 + 10 + 10 + 10 + 10 + 8));

    for scenario in &scenarios {
        // Apply impairments
        let mut signal = clean_audio.clone();

        if let Some(offset) = scenario.freq_offset_hz {
            signal = apply_frequency_offset(&signal, offset, sample_rate);
        }
        if let Some(drift) = scenario.clock_drift {
            signal = apply_clock_drift(&signal, drift);
        }
        if let Some(snr) = scenario.snr_db {
            signal = add_white_noise(&signal, snr, 42);
        }

        let fast = decode_fast(&signal, sample_rate);
        let (quality, soft_saves) = decode_quality(&signal, sample_rate);
        let (multi, _) = decode_multi(&signal, sample_rate);

        println!(
            "  {:<32}  {:>5}/{:<4}  {:>5}/{:<4}  {:>5}/{:<4}  {:>10}",
            scenario.name,
            fast.frames.len(),
            num_packets,
            quality.frames.len(),
            num_packets,
            multi.frames.len(),
            num_packets,
            soft_saves
        );
    }

    println!("  {}", "─".repeat(32 + 10 + 10 + 10 + 10 + 8));
}
