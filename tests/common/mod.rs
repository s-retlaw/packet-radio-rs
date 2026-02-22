//! Modem Test Harness
//!
//! Test utilities for validating the demodulator approaches.
//! Generates test signals, adds impairments, and compares demodulator
//! performance under controlled conditions.
//!
//! Usage: `cargo test -p packet-radio-core --test modem_tests`

#![allow(dead_code)]

use core::f64::consts::TAU;

// ─── Audio Signal Generation ───────────────────────────────────────────────

/// Generate a pure sine wave tone at the given frequency.
pub fn generate_tone(freq_hz: f64, sample_rate: u32, num_samples: usize) -> Vec<i16> {
    (0..num_samples)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            (f64::sin(TAU * freq_hz * t) * 16000.0) as i16
        })
        .collect()
}

/// Generate continuous-phase AFSK audio from a bit stream.
/// This is a reference modulator for test signal generation.
pub fn generate_afsk(
    bits: &[bool],
    sample_rate: u32,
    mark_freq: f64,
    space_freq: f64,
    baud_rate: f64,
    amplitude: f64,
) -> Vec<i16> {
    let samples_per_symbol = sample_rate as f64 / baud_rate;
    let total_samples = (bits.len() as f64 * samples_per_symbol) as usize + 1;
    let mut output = Vec::with_capacity(total_samples);

    let mut phase: f64 = 0.0;
    let mut current_tone = true; // Start with mark (NRZI state)

    for &bit in bits {
        // NRZI: 0 = toggle, 1 = no change
        if !bit {
            current_tone = !current_tone;
        }

        let freq = if current_tone { mark_freq } else { space_freq };
        let phase_step = TAU * freq / sample_rate as f64;

        let n_samples = samples_per_symbol.round() as usize;
        for _ in 0..n_samples {
            output.push((f64::sin(phase) * amplitude) as i16);
            phase += phase_step;
            // Wrap phase to avoid precision loss over long signals
            if phase > TAU {
                phase -= TAU;
            }
        }
    }

    output
}

/// Generate an AX.25 flag byte (0x7E) as a bit stream.
/// Returns NRZI-encoded bits (before NRZI decode, this is what's on the air).
pub fn generate_flag_bits() -> [bool; 8] {
    // 0x7E = 01111110
    // NRZI-encoded: each 0 toggles, each 1 stays
    // But for test purposes, we provide the raw bits and let
    // NRZI encoding happen in generate_afsk
    [false, true, true, true, true, true, true, false]
}

/// Generate a preamble of N flag bytes.
pub fn generate_preamble(num_flags: usize) -> Vec<bool> {
    let flag = generate_flag_bits();
    let mut bits = Vec::with_capacity(num_flags * 8);
    for _ in 0..num_flags {
        bits.extend_from_slice(&flag);
    }
    bits
}

/// Generate a complete test packet with preamble, data, and postamble.
/// Returns AFSK audio samples.
pub fn generate_test_packet(
    data_bits: &[bool],
    sample_rate: u32,
    preamble_flags: usize,
) -> Vec<i16> {
    let mut all_bits = generate_preamble(preamble_flags);
    all_bits.extend_from_slice(data_bits);
    all_bits.extend_from_slice(&generate_preamble(2)); // Postamble

    generate_afsk(&all_bits, sample_rate, 1200.0, 2200.0, 1200.0, 16000.0)
}

// ─── Signal Impairments ────────────────────────────────────────────────────

/// Add white Gaussian noise to a signal at the specified SNR (in dB).
pub fn add_white_noise(samples: &[i16], snr_db: f64, seed: u64) -> Vec<i16> {
    // Compute signal power
    let signal_power: f64 = samples.iter()
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>() / samples.len() as f64;

    // Compute noise power from SNR
    let noise_power = signal_power / f64::powf(10.0, snr_db / 10.0);
    let noise_stddev = f64::sqrt(noise_power);

    // Simple PRNG (xoshiro-style) for reproducible noise
    let mut rng_state = seed;
    let mut next_random = move || -> f64 {
        // Xorshift64
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        // Convert to approximate Gaussian using Box-Muller (simplified)
        let u1 = (rng_state & 0xFFFFFFFF) as f64 / 4294967296.0 + 0.0001;
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        let u2 = (rng_state & 0xFFFFFFFF) as f64 / 4294967296.0;
        f64::sqrt(-2.0 * f64::ln(u1)) * f64::cos(TAU * u2)
    };

    samples.iter()
        .map(|&s| {
            let noise = next_random() * noise_stddev;
            let noisy = s as f64 + noise;
            noisy.clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// Add a DC offset to samples.
pub fn add_dc_offset(samples: &[i16], offset: i16) -> Vec<i16> {
    samples.iter()
        .map(|&s| (s as i32 + offset as i32).clamp(-32768, 32767) as i16)
        .collect()
}

/// Scale signal amplitude (simulate weak/strong signals).
pub fn scale_amplitude(samples: &[i16], factor: f64) -> Vec<i16> {
    samples.iter()
        .map(|&s| (s as f64 * factor).clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// Apply a frequency offset to simulate transmitter crystal drift.
///
/// Uses SSB (single-sideband) shift via Hilbert transform so each tone
/// shifts to a single new frequency without image artifacts.
pub fn apply_frequency_offset(
    samples: &[i16],
    offset_hz: f64,
    sample_rate: u32,
) -> Vec<i16> {
    // Hilbert transform via windowed FIR (length 31)
    const HALF_LEN: usize = 15;
    const HILBERT_LEN: usize = 2 * HALF_LEN + 1;
    let mut hilbert_coeffs = [0.0f64; HILBERT_LEN];
    for i in 0..HILBERT_LEN {
        let n = i as isize - HALF_LEN as isize;
        if n != 0 && n % 2 != 0 {
            let hamming = 0.54 - 0.46 * f64::cos(TAU * i as f64 / (HILBERT_LEN - 1) as f64);
            hilbert_coeffs[i] = (2.0 / (std::f64::consts::PI * n as f64)) * hamming;
        }
    }

    let phase_step = TAU * offset_hz / sample_rate as f64;
    let mut delay_line = vec![0.0f64; HILBERT_LEN];
    let mut write_idx = 0;
    let mut phase = 0.0;

    samples.iter()
        .map(|&s| {
            let x = s as f64;
            delay_line[write_idx] = x;
            write_idx = (write_idx + 1) % HILBERT_LEN;

            let mut q = 0.0;
            for k in 0..HILBERT_LEN {
                let idx = (write_idx + k) % HILBERT_LEN;
                q += delay_line[idx] * hilbert_coeffs[k];
            }
            let i_delayed = delay_line[(write_idx + HALF_LEN) % HILBERT_LEN];

            let cos_p = f64::cos(phase);
            let sin_p = f64::sin(phase);
            let shifted = i_delayed * cos_p - q * sin_p;

            phase += phase_step;
            if phase > TAU { phase -= TAU; }

            shifted.clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// Simulate clock drift by resampling (stretching/compressing in time).
/// A ratio > 1.0 means the transmitter's clock is fast (more samples per symbol).
pub fn apply_clock_drift(samples: &[i16], ratio: f64) -> Vec<i16> {
    let new_len = (samples.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;

        if src_idx + 1 < samples.len() {
            // Linear interpolation
            let s = samples[src_idx] as f64 * (1.0 - frac)
                + samples[src_idx + 1] as f64 * frac;
            output.push(s.clamp(-32768.0, 32767.0) as i16);
        }
    }

    output
}

// ─── WAV File I/O (for test data) ──────────────────────────────────────────

/// Write samples to a WAV file (16-bit mono PCM).
/// Useful for debugging: listen to test signals in Audacity.
pub fn write_wav(path: &str, sample_rate: u32, samples: &[i16]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;

    let data_size = (samples.len() * 2) as u32;
    let file_size = 36 + data_size;

    // RIFF header
    f.write_all(b"RIFF")?;
    f.write_all(&file_size.to_le_bytes())?;
    f.write_all(b"WAVE")?;

    // fmt chunk
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;       // chunk size
    f.write_all(&1u16.to_le_bytes())?;         // PCM format
    f.write_all(&1u16.to_le_bytes())?;         // mono
    f.write_all(&sample_rate.to_le_bytes())?;  // sample rate
    f.write_all(&(sample_rate * 2).to_le_bytes())?; // byte rate
    f.write_all(&2u16.to_le_bytes())?;         // block align
    f.write_all(&16u16.to_le_bytes())?;        // bits per sample

    // data chunk
    f.write_all(b"data")?;
    f.write_all(&data_size.to_le_bytes())?;
    for &s in samples {
        f.write_all(&s.to_le_bytes())?;
    }

    Ok(())
}

/// Read samples from a WAV file (16-bit mono PCM).
pub fn read_wav(path: &str) -> std::io::Result<(u32, Vec<i16>)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;

    // Minimal WAV parser — assumes 16-bit mono PCM
    if buf.len() < 44 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Not a WAV file"));
    }

    let sample_rate = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let _bits_per_sample = u16::from_le_bytes([buf[34], buf[35]]);

    // Find data chunk
    let mut pos = 12;
    while pos + 8 < buf.len() {
        let chunk_id = &buf[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7],
        ]) as usize;

        if chunk_id == b"data" {
            let data_start = pos + 8;
            let data_end = (data_start + chunk_size).min(buf.len());
            let samples: Vec<i16> = buf[data_start..data_end]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            return Ok((sample_rate, samples));
        }

        pos += 8 + chunk_size;
        if chunk_size % 2 != 0 { pos += 1; } // Pad byte
    }

    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "No data chunk"))
}

// ─── Analysis Utilities ────────────────────────────────────────────────────

/// Estimate the dominant frequency in a signal using zero-crossing counting.
/// Returns frequency in Hz. Simple but adequate for verifying tones.
pub fn estimate_frequency_zero_crossings(samples: &[i16], sample_rate: u32) -> f64 {
    let mut crossings = 0u32;
    for i in 1..samples.len() {
        if (samples[i] > 0) != (samples[i - 1] > 0) {
            crossings += 1;
        }
    }
    // Each full cycle has 2 zero crossings
    let duration = samples.len() as f64 / sample_rate as f64;
    crossings as f64 / (2.0 * duration)
}

/// Compute signal-to-noise ratio in dB.
pub fn compute_snr(signal: &[i16], noisy: &[i16]) -> f64 {
    let signal_power: f64 = signal.iter()
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>() / signal.len() as f64;

    let noise_power: f64 = signal.iter().zip(noisy.iter())
        .map(|(&s, &n)| {
            let diff = n as f64 - s as f64;
            diff * diff
        })
        .sum::<f64>() / signal.len() as f64;

    if noise_power < 0.001 { return 100.0; }
    10.0 * f64::log10(signal_power / noise_power)
}

// ─── Demodulator Comparison Framework ──────────────────────────────────────

/// Result of a decode test on a single audio buffer.
#[derive(Debug, Clone)]
pub struct DecodeResult {
    pub name: &'static str,
    pub packets_decoded: usize,
    pub bits_correct: usize,
    pub bits_total: usize,
    pub soft_recoveries: usize,   // Packets recovered via bit-flipping
    pub false_positives: usize,   // Invalid frames that passed CRC
    pub processing_time_us: u64,
}

impl DecodeResult {
    pub fn bit_error_rate(&self) -> f64 {
        if self.bits_total == 0 { return 0.0; }
        1.0 - (self.bits_correct as f64 / self.bits_total as f64)
    }

    pub fn print_summary(&self) {
        println!("  {}: {} packets ({} soft recoveries), BER={:.4}%, {} false positives, {}μs",
            self.name,
            self.packets_decoded,
            self.soft_recoveries,
            self.bit_error_rate() * 100.0,
            self.false_positives,
            self.processing_time_us,
        );
    }
}

/// A test scenario with specific signal conditions.
#[derive(Debug, Clone)]
pub struct TestScenario {
    pub name: &'static str,
    pub snr_db: Option<f64>,
    pub freq_offset_hz: Option<f64>,
    pub clock_drift_ratio: Option<f64>,
    pub dc_offset: Option<i16>,
    pub amplitude_scale: Option<f64>,
}

impl TestScenario {
    /// Apply all impairments in this scenario to a clean signal.
    pub fn apply(&self, clean: &[i16], sample_rate: u32) -> Vec<i16> {
        let mut signal = clean.to_vec();

        if let Some(scale) = self.amplitude_scale {
            signal = scale_amplitude(&signal, scale);
        }
        if let Some(offset) = self.dc_offset {
            signal = add_dc_offset(&signal, offset);
        }
        if let Some(freq_off) = self.freq_offset_hz {
            signal = apply_frequency_offset(&signal, freq_off, sample_rate);
        }
        if let Some(drift) = self.clock_drift_ratio {
            signal = apply_clock_drift(&signal, drift);
        }
        if let Some(snr) = self.snr_db {
            signal = add_white_noise(&signal, snr, 42);
        }

        signal
    }
}

/// Standard test scenarios covering real-world conditions.
pub fn standard_test_scenarios() -> Vec<TestScenario> {
    vec![
        TestScenario {
            name: "Clean (ideal)",
            snr_db: None,
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "20 dB SNR (good signal)",
            snr_db: Some(20.0),
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "10 dB SNR (moderate)",
            snr_db: Some(10.0),
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "6 dB SNR (weak)",
            snr_db: Some(6.0),
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "3 dB SNR (marginal)",
            snr_db: Some(3.0),
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "+50 Hz frequency offset",
            snr_db: None,
            freq_offset_hz: Some(50.0),
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "-50 Hz frequency offset",
            snr_db: None,
            freq_offset_hz: Some(-50.0),
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "+100 Hz frequency offset",
            snr_db: None,
            freq_offset_hz: Some(100.0),
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "Clock +1% fast",
            snr_db: None,
            freq_offset_hz: None,
            clock_drift_ratio: Some(1.01),
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "Clock -1% slow",
            snr_db: None,
            freq_offset_hz: None,
            clock_drift_ratio: Some(0.99),
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "Clock +2% fast",
            snr_db: None,
            freq_offset_hz: None,
            clock_drift_ratio: Some(1.02),
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "DC offset +2000",
            snr_db: None,
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: Some(2000),
            amplitude_scale: None,
        },
        TestScenario {
            name: "Low amplitude (0.1x)",
            snr_db: None,
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: Some(0.1),
        },
        TestScenario {
            name: "Low amplitude (0.01x)",
            snr_db: None,
            freq_offset_hz: None,
            clock_drift_ratio: None,
            dc_offset: None,
            amplitude_scale: Some(0.01),
        },
        TestScenario {
            name: "Combined: 10dB + 30Hz offset + 0.5% drift",
            snr_db: Some(10.0),
            freq_offset_hz: Some(30.0),
            clock_drift_ratio: Some(1.005),
            dc_offset: None,
            amplitude_scale: None,
        },
        TestScenario {
            name: "Combined: 6dB + 50Hz offset + 1% drift + DC",
            snr_db: Some(6.0),
            freq_offset_hz: Some(50.0),
            clock_drift_ratio: Some(1.01),
            dc_offset: Some(1000),
            amplitude_scale: Some(0.5),
        },
        TestScenario {
            name: "Worst case: 3dB + 100Hz + 2% drift + DC + low amp",
            snr_db: Some(3.0),
            freq_offset_hz: Some(100.0),
            clock_drift_ratio: Some(1.02),
            dc_offset: Some(3000),
            amplitude_scale: Some(0.2),
        },
    ]
}

// ─── Benchmark Runner ──────────────────────────────────────────────────────

/// Configuration for a benchmark run.
pub struct BenchmarkConfig {
    pub sample_rate: u32,
    pub num_test_packets: usize,
    pub preamble_flags: usize,
    /// Random bits per packet (simulating AX.25 frame content)
    pub bits_per_packet: usize,
    /// Gap between packets in samples
    pub inter_packet_gap: usize,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            sample_rate: 11025,
            num_test_packets: 100,
            preamble_flags: 25,
            bits_per_packet: 200 * 8,  // ~200-byte packet
            inter_packet_gap: 1000,     // ~90ms gap
        }
    }
}

/// Generate a stream of test packets as audio, with known bit content.
/// Returns (audio_samples, vec_of_bit_sequences_per_packet).
pub fn generate_test_stream(config: &BenchmarkConfig, seed: u64) -> (Vec<i16>, Vec<Vec<bool>>) {
    let mut audio = Vec::new();
    let mut packet_bits = Vec::new();
    let mut rng = seed;

    for _ in 0..config.num_test_packets {
        // Generate random data bits for this packet
        let mut bits = Vec::with_capacity(config.bits_per_packet);
        for _ in 0..config.bits_per_packet {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            bits.push(rng & 1 == 1);
        }

        // Generate audio
        let pkt_audio = generate_test_packet(&bits, config.sample_rate, config.preamble_flags);
        audio.extend_from_slice(&vec![0i16; config.inter_packet_gap]);
        audio.extend_from_slice(&pkt_audio);

        packet_bits.push(bits);
    }

    (audio, packet_bits)
}
