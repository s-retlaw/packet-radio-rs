# Test Plan — Packet Radio RS

## Overview

This document defines the testing strategy for every layer of the project,
from individual functions through full system integration. The philosophy
is: **test the core on the desktop, deploy with confidence to embedded.**

Because the core library is `no_std` and platform-independent, the vast
majority of testing happens on a development machine using `cargo test`,
WAV file processing, and automated benchmarks — no radios required for
most of it.

---

## Table of Contents

1. [Test Levels](#1-test-levels)
2. [Unit Tests by Module](#2-unit-tests-by-module)
3. [WA8LMF TNC Test CD Benchmark](#3-wa8lmf-tnc-test-cd-benchmark)
4. [Integration Tests](#4-integration-tests)
5. [Round-Trip / Loopback Tests](#5-round-trip--loopback-tests)
6. [Fuzz Testing](#6-fuzz-testing)
7. [Performance / Regression Tests](#7-performance--regression-tests)
8. [Hardware-in-the-Loop Testing](#8-hardware-in-the-loop-testing)
9. [Cross-Platform Validation](#9-cross-platform-validation)
10. [Test Infrastructure](#10-test-infrastructure)
11. [CI/CD Pipeline](#11-cicd-pipeline)

---

## 1. Test Levels

```
Level 5: Field Test        Real radio, real RF, real APRS network
Level 4: Hardware-in-Loop  Real audio hardware, loopback cables
Level 3: System/Integration Full pipeline: WAV → frames, frame → WAV
Level 2: Integration        Module-to-module (demod → HDLC → AX.25)
Level 1: Unit               Individual functions and structs
Level 0: Static Analysis    clippy, miri, cargo audit, no_std check
```

Every level below Level 4 runs without any hardware — just `cargo test`.

---

## 2. Unit Tests by Module

### 2.1 AX.25 Address Parser (`core::ax25`)

| Test | Input | Expected | Notes |
|------|-------|----------|-------|
| Parse simple callsign | Shifted ASCII "WB2OSZ" SSID 0 | callsign="WB2OSZ", ssid=0 | |
| Parse callsign with SSID | Shifted "N0CALL" SSID 15 | callsign="N0CALL", ssid=15 | Max SSID |
| Parse short callsign | Shifted "K1AB" + spaces | callsign="K1AB", len=4 | Space padding |
| Parse 1-char callsign | Shifted "W" + 5 spaces | callsign="W", len=1 | Edge case |
| H-bit set | Address with bit 7 set in byte 6 | h_bit=true | Digipeater has-been-repeated |
| Address extension bit | Bit 0 of byte 6 set | Detected as last address | End of address field |
| Round-trip encode/decode | Any valid address | Encode → decode = original | Serialization correctness |
| All printable callsigns | Various valid callsigns | All parse correctly | Including numerics |

### 2.2 CRC-16-CCITT (`core::ax25::crc16_ccitt`)

| Test | Input | Expected CRC | Notes |
|------|-------|-------------|-------|
| Known test vector | "123456789" (ASCII) | 0x29B1 | Standard CCITT test |
| Empty input | [] | 0xFFFF ^ 0xFFFF = 0x0000 | Edge case |
| Single byte | [0x00] | Precomputed value | |
| Known AX.25 frame | Full frame from Dire Wolf | Frame's own CRC | Extract from pcap/wav |
| Frame + CRC = magic | Frame bytes + CRC appended | 0x0F47 | Residue check |
| Byte-at-a-time vs bulk | Same data both ways | Same result | Implementation consistency |

### 2.3 HDLC Decoder (`core::ax25::frame`)

| Test | Input (bit stream) | Expected | Notes |
|------|-------------------|----------|-------|
| Single flag detection | 01111110 | State → Receiving | |
| Back-to-back flags | 0111111001111110 | No frame output | Empty frame between flags |
| Bit unstuffing | 1111101 → 111111 | Stuffed zero removed | After 5 ones |
| No unstuffing outside frame | Random bits in Hunting | No crash, no output | |
| Abort detection | 11111111 (7+ ones) | Reset to Hunting | |
| Minimum valid frame | 15 bytes + correct CRC | Frame output | Dest + src + control |
| Maximum frame length | 330 bytes between flags | Frame output | MAX_FRAME_LEN |
| Overlong frame | 331+ bytes | Rejected/truncated | Buffer overflow protection |
| Valid CRC | Known-good frame | CRC passes, frame output | |
| Bad CRC | Known frame with flipped bit | CRC fails, no output | |
| Consecutive frames | Flag-frame1-flag-frame2-flag | Both frames decoded | |
| Partial frame then flag | Incomplete data then flag | Partial discarded | |

**Key test data:** Generate HDLC bit streams using the encoder and verify the
decoder recovers the original frames. Also use bit streams extracted from
known-good WAV file decodes.

### 2.4 KISS Protocol (`core::kiss`)

| Test | Input | Expected | Notes |
|------|-------|----------|-------|
| Simple encode/decode | "Hello" data frame | Round-trip match | |
| FEND in data | Data containing 0xC0 | Escaped as FESC+TFEND | |
| FESC in data | Data containing 0xDB | Escaped as FESC+TFESC | |
| Both escapes | Data with 0xC0 and 0xDB | Both escaped correctly | |
| Empty data frame | Zero-length payload | Valid KISS frame | |
| Max length frame | 330-byte AX.25 frame | Encoded without overflow | |
| Port number parsing | Ports 0-15 | Correct port extraction | High nibble of cmd byte |
| All command types | TxDelay, Persistence, etc. | Correct command parsing | |
| Double FEND | FEND FEND CMD DATA FEND | Single frame, ignore leading FEND | |
| Garbage before frame | Random bytes then valid frame | Frame decoded | |
| Multiple frames | Two frames in stream | Both decoded separately | |
| Truncated frame | FEND CMD DATA (no closing FEND) | No output (waiting) | |

### 2.5 AFSK Modulator (`core::modem::afsk`)

| Test | Input | Expected | Notes |
|------|-------|----------|-------|
| Mark tone frequency | Stream of 1 bits | 1200 Hz output | FFT or zero-crossing |
| Space tone frequency | Stream of 0 bits | 2200 Hz output | FFT or zero-crossing |
| Phase continuity | Alternating bits | No discontinuities | Check sample-to-sample delta |
| Amplitude | Any input | Within configured amplitude | No clipping |
| NRZI encoding | Known bit pattern | Expected tone sequence | 0=toggle, 1=same |
| Samples per symbol | One bit | sample_rate/baud_rate samples | Correct timing |
| Sin table correctness | Phase sweep 0-2π | Valid sine wave | Compare to libm sin() |

### 2.6 AFSK Demodulator (`core::modem::demod`)

| Test | Input | Expected | Notes |
|------|-------|----------|-------|
| Pure mark tone | Clean 1200 Hz sine wave | All 1 bits | Ideal conditions |
| Pure space tone | Clean 2200 Hz sine wave | All 0 bits | Ideal conditions |
| Alternating tones | NRZI-encoded bit pattern | Correct bit recovery | |
| Modulator loopback | Modulate → demodulate | Bit-perfect recovery | Clean signal |
| Frequency offset +50 Hz | Mark=1250, space=2250 | Still decodes | Tolerance test |
| Frequency offset -50 Hz | Mark=1150, space=2150 | Still decodes | Tolerance test |
| Low amplitude | Signal at -20 dB | Still decodes | Sensitivity test |
| DC offset | Signal with DC bias | Still decodes | Bandpass filter removes |
| Clock drift | Slightly off baud rate | Still decodes | PLL tracks |
| Noise added (10 dB SNR) | Signal + white noise | High decode rate | Realistic conditions |
| Noise added (3 dB SNR) | Signal + heavy noise | Some decode | Marginal conditions |

### 2.7 Bandpass Filter (`core::modem::filter`)

| Test | Input | Expected | Notes |
|------|-------|----------|-------|
| Pass in-band signal | 1700 Hz tone | Near unity output | Center frequency |
| Reject below band | 200 Hz tone | Strong attenuation | Well below passband |
| Reject above band | 5000 Hz tone | Strong attenuation | Well above passband |
| Pass mark freq | 1200 Hz tone | Within -3 dB | Edge of passband |
| Pass space freq | 2200 Hz tone | Within -3 dB | Edge of passband |
| Impulse response | Single impulse | Decays to zero | Filter stability |
| Step response | DC step | Output → 0 (bandpass rejects DC) | DC rejection |
| Long-term stability | 10M samples of tone | No drift or overflow | Numerical stability |
| Fixed-point overflow | Max amplitude input | No wraparound | Q15 saturation |

### 2.8 APRS Parser (`core::aprs`)

| Test | Input | Expected | Notes |
|------|-------|----------|-------|
| DTI identification | All DTI bytes | Correct DataType enum | All supported types |
| Position (plain) | `!4903.50N/07201.75W-` | lat=49.0583, lon=-72.0291 | Standard format |
| Position (compressed) | Compressed format packet | Correct lat/lon | Base91 encoding |
| Position with ambiguity | Spaces in lat/lon digits | Correct ambiguity level | 1-4 spaces |
| Mic-E standard | Typical Mic-E packet | Correct position/speed/course | Most complex format |
| Mic-E edge cases | Extreme lat/lon values | No panic, correct parse | Boundary values |
| Message format | `:N0CALL-1 :Hello{123` | addressee, text, msg_no | Standard message |
| Message no ack | `:N0CALL   :Test` | No message number | Optional field |
| Weather report | `_` + weather data | Parsed weather fields | Temperature, wind, etc |
| Status report | `>En route` | Status text extracted | Simple format |
| Object report | `;OBJECT   *...` | Object name, position | Active object |
| Item report | `)ITEM!...` | Item name, position | Live item |
| Telemetry | `T#123,...` | Sequence, values | 5 analog, 8 digital |
| Empty info field | [] | None returned | Edge case |
| Single byte | [b'!'] | Partial parse or None | Incomplete packet |
| Invalid UTF-8 in comment | Non-ASCII bytes after position | No panic | Robustness |
| Very long comment | 256 bytes of comment | Parsed correctly | Max info field |
| Null bytes in data | Embedded 0x00 | No panic | Robustness |

**Source of test data:** Extract real-world APRS packets from:
- APRS-IS raw feed recordings
- Dire Wolf debug output
- aprs.fi packet archive
- The APRS spec (APRS101.PDF) has example packets throughout

---

## 3. WA8LMF TNC Test CD Benchmark

### Background

The WA8LMF TNC Test CD (created by Stephen Smith, WA8LMF) is the standard
benchmark for APRS decoder performance. It contains WAV files recorded from
real APRS traffic under various conditions — clean signals, weak signals,
collisions, interference, and so on.

The standard metric is: **how many valid packets does your decoder extract
from each track?** Dire Wolf typically decodes 1000+ packets from Track 1,
significantly outperforming hardware TNCs.

### Test Tracks

The test CD contains multiple tracks with different characteristics:

| Track | Content | Conditions | Purpose |
|-------|---------|-----------|---------|
| Track 1 | Mix of stations | Various signal levels | Overall performance |
| Track 2 | Weak signals | Low SNR | Sensitivity test |
| (others) | Various | Collisions, QRM | Edge case handling |

### Implementation

Create a test binary that processes WAV files and counts decoded packets:

```
tests/
├── wav/                    # WAV files from test CD (not committed to git)
│   ├── README.md          # Instructions for obtaining test files
│   ├── track1.wav
│   └── track2.wav
├── benchmark/
│   ├── wav_decoder.rs     # WAV file reader
│   ├── packet_counter.rs  # Count unique valid packets
│   └── main.rs            # CLI benchmark runner
└── expected/
    ├── track1_packets.txt # Known-good packet list (from Dire Wolf)
    └── track2_packets.txt
```

**Benchmark runner usage:**
```bash
# Run against a single track
cargo run --release -p benchmark -- --wav tests/wav/track1.wav

# Compare against known-good output
cargo run --release -p benchmark -- --wav tests/wav/track1.wav \
    --compare tests/expected/track1_packets.txt

# Run full benchmark suite
cargo run --release -p benchmark -- --suite tests/wav/
```

**Output format:**
```
Track: track1.wav
Duration: 60.0 seconds
Total packets decoded: 1027
Unique packets: 983
Duplicates (multi-decoder): 44
CRC failures: 12
Decode rate: 983 / 1000 known (98.3%)

Comparison with Dire Wolf reference:
  Packets we decoded that DW missed: 3
  Packets DW decoded that we missed: 20
  Exact match: 963 / 983
```

### Scoring Criteria

| Metric | Target | Notes |
|--------|--------|-------|
| Track 1 unique packets | ≥ 1000 (single decoder) | Dire Wolf single decoder baseline |
| Track 1 unique packets | ≥ 1050 (multi decoder) | Dire Wolf multi-decoder typical |
| False positives | 0 | No invalid frames accepted |
| No crashes | 0 panics | On any input |
| Processing speed | > 10x real-time | On desktop hardware |
| Memory usage | < 100 KB | Core decoder memory |

### Creating Reference Data

1. Run Dire Wolf on each test track, capture all decoded packets
2. For each packet, record: timestamp, raw hex, parsed callsigns
3. Store as reference files for regression testing
4. Any time we improve the decoder, compare against these baselines

```bash
# Generate reference using Dire Wolf
direwolf -r 44100 -t 0 track1.wav 2>&1 | tee track1_direwolf.log
# Parse the log to extract packet hex dumps
grep "^\[" track1_direwolf.log > track1_packets.txt
```

---

## 4. Integration Tests

These test multiple modules working together.

### 4.1 Demodulator → HDLC → AX.25 Pipeline

Feed WAV audio through the full receive pipeline and verify complete frames come out.

```rust
#[test]
fn test_full_receive_pipeline() {
    let wav_samples = load_wav("tests/wav/known_packet.wav");
    let config = DemodConfig::default_1200();
    let mut demod = AfskDemodulator::new(config);
    let mut hdlc = HdlcDecoder::new();
    let mut decoded_frames = Vec::new();

    let mut bits = [0u8; 4096];
    let num_bits = demod.process_samples(&wav_samples, &mut bits);

    for &bit in &bits[..num_bits] {
        if let Some(frame_data) = hdlc.feed_bit(bit != 0) {
            if let Some(frame) = Frame::parse(frame_data) {
                decoded_frames.push(frame);
            }
        }
    }

    assert!(!decoded_frames.is_empty(), "Should decode at least one frame");
    // Verify against known packet content
}
```

### 4.2 AX.25 → APRS Pipeline

Feed known AX.25 frame bytes through the APRS parser.

```rust
#[test]
fn test_ax25_to_aprs() {
    // Known APRS position packet as raw AX.25 bytes (after HDLC decode)
    let frame_bytes = hex!("...");  // From reference capture
    let frame = Frame::parse(&frame_bytes).expect("Valid frame");

    assert!(frame.is_ui());
    let aprs = parse_packet(frame.info, frame.dest.callsign_str());
    match aprs {
        Some(AprsPacket::Position { position, .. }) => {
            assert_eq!(position.lat / 1_000_000, 49);  // ~49°N
            assert_eq!(position.lon / 1_000_000, -72);  // ~72°W
        }
        _ => panic!("Expected position packet"),
    }
}
```

### 4.3 KISS → AX.25 → APRS Pipeline

Feed KISS-framed data (as if from a serial port or TCP connection) through
the full receive path.

### 4.4 Full TX Pipeline

Build an APRS position packet → AX.25 frame → HDLC encode → NRZI → AFSK
modulate → WAV audio. Then feed the WAV back through the RX pipeline and
verify the decoded packet matches the original.

---

## 5. Round-Trip / Loopback Tests

These are the most powerful tests — they verify TX and RX together.

### 5.1 Digital Loopback (No Audio)

```
Original Packet → AX.25 Encode → HDLC Encode → HDLC Decode → AX.25 Parse → Compare
```

Skip the modem entirely. Tests framing and protocol layers.

```rust
#[test]
fn test_ax25_round_trip() {
    let original = build_test_frame("N0CALL-1", "APRS", b"!4903.50N/07201.75W-Test");
    let encoded = hdlc_encode(&original);  // Returns bit stream

    let mut decoder = HdlcDecoder::new();
    let mut recovered = None;
    for bit in &encoded {
        if let Some(frame) = decoder.feed_bit(*bit) {
            recovered = Some(frame.to_vec());
        }
    }

    assert_eq!(recovered.unwrap(), original);
}
```

### 5.2 Audio Loopback (Full Modem)

```
Packet → Modulate → Audio Samples → Demodulate → Packet → Compare
```

Tests the complete modem at various conditions:

| Condition | How | Pass Criteria |
|-----------|-----|--------------|
| Clean | Direct sample buffer | Bit-perfect decode |
| Resampled | Resample 11025→22050→11025 | Still decodes |
| Level shifted | Multiply samples by 0.1 | Still decodes |
| DC offset | Add constant to all samples | Still decodes |
| White noise (20 dB SNR) | Add Gaussian noise | Decodes |
| White noise (10 dB SNR) | Add more noise | High success rate |
| White noise (6 dB SNR) | Add heavy noise | Some success |
| Frequency offset | Shift all freqs +30 Hz | Still decodes |
| Clock offset | Resample by 1.001x | PLL compensates |

### 5.3 Stress Loopback

Generate thousands of random valid packets, modulate them all into one
long audio stream (with proper preambles and spacing), demodulate, and
verify every packet is recovered.

```rust
#[test]
fn test_stress_1000_packets() {
    let mut rng = /* seed */;
    let mut audio = Vec::new();
    let mut expected_packets = Vec::new();

    for _ in 0..1000 {
        let packet = generate_random_aprs_packet(&mut rng);
        expected_packets.push(packet.clone());
        let samples = modulate_packet(&packet);
        audio.extend_from_slice(&silence(100));  // Inter-packet gap
        audio.extend_from_slice(&samples);
    }

    let decoded = decode_all_packets(&audio);
    assert_eq!(decoded.len(), 1000);
    for (expected, actual) in expected_packets.iter().zip(decoded.iter()) {
        assert_eq!(expected, actual);
    }
}
```

---

## 6. Fuzz Testing

Fuzz testing is critical for a project that parses untrusted RF input.
A malformed packet should never cause a panic or buffer overflow.

### Setup

```bash
cargo install cargo-fuzz

# In core/
mkdir fuzz
```

### Fuzz Targets

```rust
// fuzz/fuzz_targets/fuzz_ax25_parse.rs
#![no_main]
use libfuzzer_sys::fuzz_target;
use packet_radio_core::ax25::Frame;

fuzz_target!(|data: &[u8]| {
    // Should never panic, regardless of input
    let _ = Frame::parse(data);
});
```

```rust
// fuzz/fuzz_targets/fuzz_aprs_parse.rs
fuzz_target!(|data: &[u8]| {
    let _ = packet_radio_core::aprs::parse_packet(data, b"APRS");
});
```

```rust
// fuzz/fuzz_targets/fuzz_hdlc_bits.rs
fuzz_target!(|data: &[u8]| {
    let mut decoder = HdlcDecoder::new();
    for &byte in data {
        for bit in 0..8 {
            let _ = decoder.feed_bit((byte >> bit) & 1 != 0);
        }
    }
});
```

```rust
// fuzz/fuzz_targets/fuzz_kiss_decode.rs
fuzz_target!(|data: &[u8]| {
    let mut decoder = KissDecoder::new();
    for &byte in data {
        let _ = decoder.feed_byte(byte);
    }
});
```

```rust
// fuzz/fuzz_targets/fuzz_demodulator.rs
fuzz_target!(|data: &[u8]| {
    // Interpret fuzzer data as i16 audio samples
    let samples: Vec<i16> = data.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();

    let mut demod = AfskDemodulator::new(DemodConfig::default_1200());
    let mut bits = [0u8; 8192];
    let _ = demod.process_samples(&samples, &mut bits);
});
```

### Running Fuzz Tests

```bash
# Run each fuzz target for at least 10 minutes
cargo fuzz run fuzz_ax25_parse -- -max_total_time=600
cargo fuzz run fuzz_aprs_parse -- -max_total_time=600
cargo fuzz run fuzz_hdlc_bits -- -max_total_time=600
cargo fuzz run fuzz_kiss_decode -- -max_total_time=600
cargo fuzz run fuzz_demodulator -- -max_total_time=600
```

### Fuzz Criteria

- **Zero panics** on any input
- **No buffer overflows** (Rust prevents this by default, but verify with Miri)
- **No infinite loops** (set timeout)
- **Bounded memory usage** (no unbounded growth from crafted input)

---

## 7. Performance / Regression Tests

### 7.1 Decode Speed Benchmark

Measure how fast the demodulator processes audio, expressed as multiples
of real-time.

```rust
#[bench]
fn bench_demodulator_throughput() {
    // 60 seconds of audio at 11025 Hz
    let samples = generate_test_audio(60 * 11025);
    let mut demod = AfskDemodulator::new(DemodConfig::default_1200());
    let mut bits = [0u8; 65536];

    b.iter(|| {
        demod.reset();
        demod.process_samples(&samples, &mut bits);
    });
}
```

**Targets:**

| Platform | Target | Notes |
|----------|--------|-------|
| Desktop (x86_64) | > 100x real-time | Single decoder |
| Raspberry Pi 4 | > 20x real-time | ARM64, single decoder |
| ESP32 | > 5x real-time | Single decoder |
| RP2040 | > 2x real-time | Single decoder |

### 7.2 Packet Count Regression

After every change to the modem, run the TNC Test CD benchmark and verify
the packet count hasn't decreased.

```bash
# Store baseline
cargo run --release -p benchmark -- --wav tests/wav/track1.wav > baseline.txt

# After changes, compare
cargo run --release -p benchmark -- --wav tests/wav/track1.wav > current.txt
diff baseline.txt current.txt
```

**Regression rule:** The packet count must never decrease unless there's a
documented reason (e.g., removing a false positive).

### 7.3 Memory Usage

Track heap allocations and peak memory usage on each platform.

```rust
#[test]
fn test_core_no_alloc() {
    // Verify the core decoder works with zero heap allocations
    // by running in a custom allocator that panics on alloc
}
```

### 7.4 Latency

Measure the time from receiving the last sample of a packet to producing
the decoded frame. Important for real-time applications.

---

## 8. Hardware-in-the-Loop Testing

These tests require real audio hardware but NOT a radio.

### 8.1 Audio Loopback Cable

Connect sound card output to sound card input with a 3.5mm cable (with
appropriate attenuation — line out is hot, mic in is sensitive).

```
Sound Card Line Out ──── 10kΩ ──┬── Sound Card Line In
                                │
                               10kΩ
                                │
                               GND
```

**Test procedure:**
1. Generate APRS packets
2. Modulate to audio
3. Play through sound card output
4. Capture from sound card input
5. Demodulate captured audio
6. Verify decoded packets match originals

This tests the entire desktop audio path including `cpal`, sample rate
conversion, buffering, and real-world timing.

### 8.2 Radio Loopback

Two radios, one TX and one RX, connected to the same computer or two
computers on a LAN.

```
Computer → Sound Card → Radio TX ~~~RF~~~ Radio RX → Sound Card → Computer
```

**Test procedure:**
1. Transmit known packets
2. Receive and decode
3. Verify packet recovery
4. Measure decode rate at various power levels

### 8.3 ESP32 Hardware Test

ESP32 with I2S codec, connected via loopback or to a radio.

**Smoke tests:**
- I2S audio produces correct sample rate (verify with oscilloscope)
- WiFi connects and reaches APRS-IS server
- Decoded packets appear on APRS-IS (aprs.fi)
- KISS interface responds correctly
- PTT GPIO toggles at correct times
- Watchdog doesn't trigger during normal operation
- Runs for 24+ hours without crash or memory leak

### 8.4 Cross-Decode Test

Use two different TNC implementations to validate each other:

```
Our modulator → Dire Wolf decoder → Compare
Dire Wolf modulator → Our decoder → Compare
Our modulator → Our decoder → Compare (loopback)
```

If all three agree, confidence is very high.

---

## 9. Cross-Platform Validation

### 9.1 Compile Checks

```bash
# Desktop targets
cargo check --target x86_64-unknown-linux-gnu
cargo check --target x86_64-apple-darwin
cargo check --target x86_64-pc-windows-msvc
cargo check --target aarch64-unknown-linux-gnu   # Raspberry Pi

# Embedded targets (core only, no_std)
cargo check -p packet-radio-core --target thumbv7em-none-eabihf  # Cortex-M4 (STM32F4)
cargo check -p packet-radio-core --target thumbv6m-none-eabi      # Cortex-M0 (RP2040)
cargo check -p packet-radio-core --target riscv32imc-unknown-none-elf # RISC-V (ESP32-C3)
```

### 9.2 Numerical Consistency

The demodulator must produce identical results regardless of platform.
This is tricky because floating-point behavior varies across architectures.

**Strategy:**
- Run the same WAV file through the demodulator on x86_64, ARM64, and RISC-V
- Compare decoded packet lists
- If using fixed-point (`no float` feature), results must be bit-identical
- If using `f32`, allow minor differences but packet counts must match

### 9.3 Endianness

AX.25 CRC and HDLC bit ordering are sensitive to endianness. Test on
both little-endian (x86, ARM, RISC-V) and big-endian (if applicable)
targets.

---

## 10. Test Infrastructure

### 10.1 WAV File Utilities

A small utility crate for reading/writing WAV files in tests. This is
**test-only** (not part of the core) and can use `std`.

```rust
// tests/common/wav.rs
pub fn read_wav(path: &str) -> (u32, Vec<i16>) {
    // Returns (sample_rate, samples)
    // Supports 16-bit PCM mono WAV files
}

pub fn write_wav(path: &str, sample_rate: u32, samples: &[i16]) {
    // Write samples to a WAV file for debugging
}
```

### 10.2 Test Packet Generator

Generate known-valid APRS packets for testing.

```rust
// tests/common/packet_gen.rs
pub fn make_position_packet(
    src: &str, lat: f64, lon: f64, comment: &str
) -> Vec<u8> {
    // Build a complete AX.25 UI frame with APRS position payload
}

pub fn make_message_packet(
    src: &str, dest: &str, message: &str, msg_no: Option<u32>
) -> Vec<u8> { ... }

pub fn make_random_packet(rng: &mut impl Rng) -> Vec<u8> {
    // Random but valid APRS packet
}
```

### 10.3 Audio Test Signal Generator

Generate audio test signals programmatically.

```rust
// tests/common/audio_gen.rs
pub fn generate_tone(freq: f64, sample_rate: u32, duration_secs: f64) -> Vec<i16>;
pub fn add_white_noise(samples: &mut [i16], snr_db: f64);
pub fn add_dc_offset(samples: &mut [i16], offset: i16);
pub fn resample(samples: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16>;
pub fn shift_frequency(samples: &[i16], shift_hz: f64, sample_rate: u32) -> Vec<i16>;
```

### 10.4 Dire Wolf Comparison Harness

A script that runs both our decoder and Dire Wolf on the same audio file
and compares the results.

```bash
#!/bin/bash
# compare_with_direwolf.sh <wav_file>
WAV=$1

# Our decoder
cargo run --release -p benchmark -- --wav "$WAV" --output ours.txt

# Dire Wolf
direwolf -r 44100 -t 0 "$WAV" 2>&1 | parse_direwolf_output.py > dw.txt

# Compare
diff_packets.py ours.txt dw.txt
```

### 10.5 Test Data Management

WAV files are large and should NOT be committed to git. Use git-lfs or
a separate download mechanism.

```
tests/wav/README.md:
    Instructions for obtaining test WAV files:
    1. WA8LMF TNC Test CD: http://wa8lmf.net/TNCtest/
    2. Dire Wolf test files: included with Dire Wolf source
    3. Self-generated: run `cargo run -p test-gen -- generate-test-wav`
```

---

## 11. CI/CD Pipeline

### GitHub Actions Workflow

```yaml
name: CI

on: [push, pull_request]

jobs:
  # Basic compile checks
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: >
            thumbv7em-none-eabihf,
            thumbv6m-none-eabi,
            riscv32imc-unknown-none-elf
      - name: Check core (no_std, no features)
        run: cargo check -p packet-radio-core --no-default-features
      - name: Check core (all features)
        run: cargo check -p packet-radio-core --all-features
      - name: Check embedded targets
        run: |
          cargo check -p packet-radio-core --target thumbv7em-none-eabihf --no-default-features
          cargo check -p packet-radio-core --target thumbv6m-none-eabi --no-default-features
          cargo check -p packet-radio-core --target riscv32imc-unknown-none-elf --no-default-features
      - name: Check workspace
        run: cargo check --workspace

  # Unit and integration tests
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Run tests
        run: cargo test --workspace

  # Clippy lints
  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - run: cargo clippy --workspace -- -D warnings

  # Format check
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --check

  # Miri (memory safety checks)
  miri:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          components: miri
      - name: Run Miri on core
        run: cargo +nightly miri test -p packet-radio-core

  # TNC Test CD benchmark (only on main branch merges)
  benchmark:
    runs-on: ubuntu-latest
    if: github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Download test WAV files
        run: ./tests/download_test_files.sh
      - name: Run benchmark
        run: cargo run --release -p benchmark -- --suite tests/wav/
      - name: Check for regressions
        run: ./tests/check_regression.sh
```

### Pre-Commit Hooks

```bash
#!/bin/bash
# .git/hooks/pre-commit
cargo fmt --check || exit 1
cargo clippy --workspace -- -D warnings || exit 1
cargo test -p packet-radio-core || exit 1
```

---

## Test Priority Order

Implement tests in this order (matches the development roadmap):

| Priority | What | Why |
|----------|------|-----|
| 1 | CRC-16 unit tests with known vectors | Foundation — everything depends on CRC |
| 2 | AX.25 address parse/encode round-trip | Next simplest, well-defined |
| 3 | HDLC encode → decode round-trip | Critical path component |
| 4 | KISS encode → decode round-trip | Already partially implemented |
| 5 | APRS DTI identification tests | Quick wins |
| 6 | Modulator → WAV file output | Visual/audible verification |
| 7 | Modulator → demodulator loopback (clean) | First full-modem test |
| 8 | WAV file → demodulator → packet | First real-world test |
| 9 | TNC Test CD Track 1 benchmark | The gold standard |
| 10 | Fuzz all parsers | Security and robustness |
| 11 | Noisy loopback tests | Modem tuning |
| 12 | Multi-decoder benchmark | Performance optimization |
| 13 | Hardware loopback | Platform validation |
| 14 | 24-hour soak test | Stability |

---

## 12. Advanced Modem Technique Validation

These tests specifically validate the novel demodulation approaches.

### 12.1 Delay-Multiply Detector Characterization

Sweep tone frequency through the detector and verify the frequency response:

```rust
#[test]
fn test_delay_multiply_frequency_response() {
    let sample_rate = 11025;
    let mut responses = Vec::new();

    for freq in (400..4000).step_by(50) {
        let mut det = DelayMultiplyDetector::new(sample_rate, lpf);
        let tone = generate_tone(freq as f64, sample_rate, 500);

        let mut sum: i64 = 0;
        for &s in &tone[100..] {
            sum += det.process(s) as i64;
        }
        let avg = sum / 400;
        responses.push((freq, avg));
    }

    // Verify: mark (1200) and space (2200) produce opposite signs
    let mark_response = responses.iter().find(|&&(f, _)| f == 1200).unwrap().1;
    let space_response = responses.iter().find(|&&(f, _)| f == 2200).unwrap().1;
    assert!(mark_response * space_response < 0, "Mark and space must have opposite polarity");

    // Verify: midpoint (1700) is near zero crossing
    let mid_response = responses.iter().find(|&&(f, _)| f == 1700).unwrap().1;
    assert!(mid_response.abs() < mark_response.abs() / 4);
}
```

Test multiple delay values to find the optimal for each sample rate:

| Sample Rate | Delays to Test | Best Separation Expected |
|-------------|---------------|------------------------|
| 11025 Hz    | 3, 4, 5       | Delay=4                |
| 22050 Hz    | 6, 7, 8       | Delay=7                |
| 44100 Hz    | 13, 14, 15, 16| Delay=15               |

### 12.2 Hilbert Transform Accuracy

Verify the Hilbert filter produces a correct 90° phase shift:

```rust
#[test]
fn test_hilbert_phase_shift() {
    let mut h = hilbert_31();
    let sample_rate = 11025;
    let freq = 1700.0;  // Center frequency

    // Generate a tone and pass through Hilbert
    let tone = generate_tone(freq, sample_rate, 500);
    let mut reals = Vec::new();
    let mut imags = Vec::new();

    for &s in &tone {
        let (r, i) = h.process(s);
        reals.push(r);
        imags.push(i);
    }

    // After the filter settles (skip group_delay samples), the imaginary
    // part should lead the real part by 90°. Test by verifying they are
    // approximately in quadrature (real² + imag² ≈ constant).
    let start = 50; // Skip transient
    for i in start..reals.len() {
        let envelope_sq = (reals[i] as i64).pow(2) + (imags[i] as i64).pow(2);
        // Envelope should be roughly constant for a pure tone
        // (within ~10% of average)
    }
}
```

### 12.3 Instantaneous Frequency Accuracy

Feed known tones through Hilbert + InstFreq and verify frequency estimates:

| Input | Expected Output | Tolerance |
|-------|----------------|-----------|
| Pure 1200 Hz | 1200 Hz ± 20 Hz | After 50-sample settling |
| Pure 2200 Hz | 2200 Hz ± 20 Hz | |
| Pure 1700 Hz | 1700 Hz ± 20 Hz | Midpoint |
| Chirp 1200→2200 Hz | Ramp from 1200 to 2200 | Smooth transition |
| 1200 Hz + noise (10 dB SNR) | ~1200 Hz ± 50 Hz | Noisy but biased correctly |

### 12.4 Adaptive Tracker Convergence

Verify the tracker correctly estimates transmitter parameters:

```rust
#[test]
fn test_adaptive_tracker_offset_transmitter() {
    // Simulate a transmitter that's 30 Hz high on both tones
    let actual_mark = 1230;
    let actual_space = 2230;
    let sample_rate = 11025;
    let mut tracker = AdaptiveTracker::new(sample_rate);

    // Feed simulated preamble with offset frequencies
    let sps = sample_rate / 1200;
    for i in 0..500 {
        let symbol_idx = i / sps;
        let freq = if symbol_idx % 2 == 0 { actual_mark } else { actual_space };
        tracker.feed(freq * 256, i);
    }

    assert!(tracker.is_locked());
    let mark_est_hz = tracker.mark_freq_est / 256;
    let space_est_hz = tracker.space_freq_est / 256;

    assert!((mark_est_hz - actual_mark as i32).abs() < 15,
        "Mark estimate {} should be near {}", mark_est_hz, actual_mark);
    assert!((space_est_hz - actual_space as i32).abs() < 15,
        "Space estimate {} should be near {}", space_est_hz, actual_space);
}
```

Test across a range of frequency offsets:

| Offset | Mark (actual) | Space (actual) | Should Lock? |
|--------|--------------|---------------|--------------|
| Nominal | 1200 | 2200 | Yes |
| +30 Hz | 1230 | 2230 | Yes |
| +50 Hz | 1250 | 2250 | Yes |
| −50 Hz | 1150 | 2150 | Yes |
| +100 Hz | 1300 | 2300 | Yes (edge case) |
| Asymmetric | 1180 | 2220 | Yes (different offsets) |

### 12.5 Soft-Decision Bit-Flip Recovery

The critical test: inject known bit errors and verify recovery.

```rust
#[test]
fn test_soft_recovery_single_bit_error() {
    // 1. Build a valid AX.25 frame
    let frame = build_test_frame("N0CALL-1", "APRS", b"!4903.50N/07201.75W-");

    // 2. HDLC encode to bit stream with soft values
    let (bits, soft) = hdlc_encode_with_soft(&frame);

    // 3. Corrupt one bit (flip it) and set its soft value to low confidence
    let mut corrupted_bits = bits.clone();
    let mut corrupted_soft = soft.clone();
    let error_pos = bits.len() / 2;  // Middle of frame
    corrupted_bits[error_pos] ^= 1;
    corrupted_soft[error_pos] = 2;  // Very low confidence

    // 4. Feed through soft HDLC decoder
    let mut decoder = SoftHdlcDecoder::new();
    let result = feed_bits_to_decoder(&mut decoder, &corrupted_bits, &corrupted_soft);

    // 5. Should recover the frame
    assert!(matches!(result, Some(FrameResult::Recovered { flips: 1, .. })));
    assert_eq!(decoder.stats_soft_recovered, 1);
}

#[test]
fn test_soft_recovery_two_bit_errors() {
    // Same as above but corrupt two bits with low confidence
    // ...
    assert!(matches!(result, Some(FrameResult::Recovered { flips: 2, .. })));
}

#[test]
fn test_hard_decode_no_errors() {
    // Clean frame should decode on first try, no bit-flipping needed
    assert!(matches!(result, Some(FrameResult::Valid(_))));
    assert_eq!(decoder.stats_hard_decode, 1);
    assert_eq!(decoder.stats_soft_recovered, 0);
}

#[test]
fn test_unrecoverable_errors() {
    // Corrupt 5+ bits → should fail (CRC failure, not recovered)
    assert!(result.is_none());
    assert_eq!(decoder.stats_crc_failures, 1);
}
```

### 12.6 False Positive Rate of Bit-Flipping

Verify that bit-flipping doesn't create spurious valid frames from noise:

```rust
#[test]
fn test_bit_flip_false_positive_rate() {
    let mut rng = /* seeded */;
    let mut false_positives = 0;
    let trials = 100_000;

    for _ in 0..trials {
        // Generate random noise (NOT a valid frame)
        let noise_bits: Vec<u8> = (0..300).map(|_| rng.gen_range(0..2)).collect();
        let noise_soft: Vec<i8> = (0..300)
            .map(|_| rng.gen_range(-127..128) as i8)
            .collect();

        let mut decoder = SoftHdlcDecoder::new();
        let result = feed_bits_to_decoder(&mut decoder, &noise_bits, &noise_soft);
        if result.is_some() {
            false_positives += 1;
        }
    }

    // Expected: < 0.04% (23 trials / 65536 CRC space)
    let rate = false_positives as f64 / trials as f64;
    assert!(rate < 0.001, "False positive rate {} too high", rate);
}
```

### 12.7 A/B Comparison: Fast vs. Quality Path

Run both demodulators on the same audio and compare:

```rust
#[test]
fn test_fast_vs_quality_clean_signal() {
    let audio = modulate_test_packets(100);  // 100 clean packets

    let fast_decoded = decode_with_fast_path(&audio);
    let quality_decoded = decode_with_quality_path(&audio);

    // Both should decode all 100 clean packets
    assert_eq!(fast_decoded.len(), 100);
    assert_eq!(quality_decoded.len(), 100);
}

#[test]
fn test_quality_advantage_noisy_signal() {
    let audio = modulate_test_packets(100);
    let noisy = add_white_noise(&audio, 8.0);  // 8 dB SNR

    let fast_decoded = decode_with_fast_path(&noisy);
    let quality_decoded = decode_with_quality_path(&noisy);

    // Quality path should decode MORE packets than fast path
    println!("Fast: {} packets, Quality: {} packets",
        fast_decoded.len(), quality_decoded.len());
    assert!(quality_decoded.len() >= fast_decoded.len(),
        "Quality path should be at least as good as fast path");
}
```

### 12.8 WA8LMF Test CD Comparison Matrix

Run each test track through multiple decoder configurations and compare:

| Track | Fast Path | Quality (no adapt) | Quality (adaptive) | Quality (adaptive + soft) | Dire Wolf (1 decoder) | Dire Wolf (6 decoders) |
|-------|-----------|-------------------|-------------------|--------------------------|----------------------|----------------------|
| Track 1 | ? | ? | ? | ? | ~1000 | ~1050 |
| Track 2 | ? | ? | ? | ? | (reference) | (reference) |

This matrix is the ultimate scorecard. The goal:
- **Quality (adaptive + soft)** ≥ Dire Wolf (6 decoders)
- **Quality (adaptive)** ≥ Dire Wolf (1 decoder)
- **Fast path** within 10% of Dire Wolf (1 decoder)

### 12.9 Frequency Offset Tolerance Comparison

Modulate packets with intentional frequency offsets and compare decode rates:

| Offset | Fast Path | Quality | Dire Wolf | Notes |
|--------|-----------|---------|-----------|-------|
| 0 Hz | 100% | 100% | 100% | Baseline |
| ±25 Hz | ? | ? | ? | Typical crystal drift |
| ±50 Hz | ? | ? | ? | Moderate offset |
| ±75 Hz | ? | ? | ? | Significant offset |
| ±100 Hz | ? | ? | ? | Extreme offset |

The quality path (adaptive tracker) should maintain high decode rates
at offsets where fixed-parameter decoders degrade.

### 12.10 Baud Rate Tolerance Comparison

Modulate packets with intentional baud rate errors:

| Baud Error | Fast Path | Quality | Dire Wolf |
|------------|-----------|---------|-----------|
| 1200.0 Bd | 100% | 100% | 100% |
| 1195.0 Bd | ? | ? | ? |
| 1190.0 Bd | ? | ? | ? |
| 1205.0 Bd | ? | ? | ? |
| 1210.0 Bd | ? | ? | ? |

---

## Appendix: Obtaining Test Data

### WA8LMF TNC Test CD
- URL: http://wa8lmf.net/TNCtest/
- Contains WAV files and documentation
- Standard benchmark used by Dire Wolf and others
- Track 1 is the primary benchmark — aim for > 1000 packets

### Dire Wolf Test Files
- Included in the Dire Wolf source distribution
- `direwolf/test/` directory
- Various WAV files and expected output

### Generate Your Own
- Record from local APRS frequency (144.390 MHz NA, 144.800 MHz EU)
- Use an RTL-SDR ($25) with GQRX or SDR# to record
- Record at 44100 or 48000 Hz, 16-bit mono WAV
- Even a few minutes of recording gives dozens of test packets

### APRS-IS Packet Captures
- Connect to rotate.aprs2.net:14580 and log raw packets
- Useful for APRS parser testing (not modem testing)
- Can capture thousands of packets per minute
