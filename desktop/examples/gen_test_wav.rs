//! Generate a test WAV file containing a modulated APRS packet.
//!
//! Usage: cargo run -p packet-radio-desktop --example gen_test_wav -- /tmp/test_aprs.wav

use packet_radio_core::ax25::frame::{build_test_frame, hdlc_encode};
use packet_radio_core::modem::afsk::AfskModulator;
use packet_radio_core::modem::ModConfig;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/test_aprs.wav".to_string());

    // Build a test APRS position report
    let (frame_data, frame_len) =
        build_test_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Test packet from gen_test_wav");
    let raw = &frame_data[..frame_len];
    let encoded = hdlc_encode(raw);

    // Modulate to audio
    let mut modulator = AfskModulator::new(ModConfig::default_1200());
    let mut audio = vec![0i16; 0];
    let mut buf = [0i16; 1024];

    // Preamble flags
    for _ in 0..50 {
        let n = modulator.modulate_flag(&mut buf);
        audio.extend_from_slice(&buf[..n]);
    }

    // Frame data
    for i in 0..encoded.bit_count {
        let bit = encoded.bits[i] != 0;
        let n = modulator.modulate_bit(bit, &mut buf);
        audio.extend_from_slice(&buf[..n]);
    }

    // Trailing silence
    audio.extend_from_slice(&[0i16; 100]);

    // Write WAV
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 11025,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create WAV");
    for &s in &audio {
        writer.write_sample(s).expect("write sample");
    }
    writer.finalize().expect("finalize WAV");

    println!("Wrote {} samples ({:.2}s) to {path}", audio.len(),
        audio.len() as f64 / 11025.0);
}
