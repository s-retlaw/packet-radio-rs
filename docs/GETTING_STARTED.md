# Getting Started

## Development Roadmap

This guide outlines the recommended order of implementation. Each step builds
on the previous one and is independently testable.

## Step 1: AX.25 Frame Parser

**Start here.** This is the easiest module to implement and test, and everything
else depends on it.

### What to build:
- `Address` struct: parse 7-byte AX.25 addresses (shifted ASCII callsign + SSID)
- `Frame` struct: parse a complete AX.25 frame from raw bytes
- CRC-16-CCITT calculation and verification
- Frame serialization (for TX path)

### Test with:
- Hand-crafted byte arrays representing known valid frames
- Known-bad frames (wrong CRC, truncated, etc.)
- Dire Wolf's test vectors if available

### Success criteria:
- Parse any valid AX.25 UI frame and extract all fields
- Reject invalid frames (bad CRC, too short, too long)
- Round-trip: serialize a frame, parse it back, verify identical

## Step 2: HDLC Framing

### What to build:
- HDLC decoder: flag detection, bit unstuffing, frame boundary detection
- HDLC encoder: bit stuffing, flag insertion, CRC insertion
- State machine that converts a bit stream into complete frames

### Test with:
- Known bit sequences that contain valid HDLC frames
- Edge cases: back-to-back flags, maximum stuffing, abort sequences

### Success criteria:
- Feed a bit stream containing a known AX.25 frame → get the frame out
- Encode a frame → decode it → verify round-trip

## Step 3: KISS Protocol

### What to build:
- KISS frame encoder/decoder (FEND/FESC escaping)
- Command parsing (data frame, TX delay, persistence, etc.)

### Test with:
- Wrap known AX.25 frames in KISS framing, unwrap, verify

### Why now:
- KISS is simple and gives you a way to interface with existing tools
- You can test your AX.25 parser by feeding it KISS frames from Dire Wolf

## Step 4: AFSK Demodulator

**This is the hard part.** We use a dual-path architecture (see MODEM_DESIGN.md).
Start with the fast path (simpler), then add the quality path.

### Step 4a: Fast Path (Delay-Multiply Discriminator)
- Bandpass filter (biquad, ~1000-2600 Hz)
- Delay-and-multiply detector (1 multiply per sample!)
- Lowpass filter (smooth detector output)
- Clock recovery PLL
- NRZI decoder
- Wire it all together: audio samples → disc output → PLL → bits → HDLC → frames

### Step 4b: Quality Path (add after fast path works)
- Hilbert transform (31-tap FIR → analytic signal)
- Instantaneous frequency detector (phase difference → Hz)
- Adaptive tracker (trains on preamble flags)
- Soft PLL (outputs LLR confidence values)
- Soft HDLC decoder (bit-flip recovery on CRC failures)

### Development approach:
1. Start with the test harness (`tests/common/mod.rs`) to generate test signals
2. Build the fast path first — get it decoding clean loopback signals
3. Run through the standard test scenarios (noise, freq offset, clock drift)
4. Add the quality path and run comparative tests (`tests/demod_comparative.rs`)
5. Benchmark against the WA8LMF TNC Test CD

### Key references:
- `docs/MODEM_DESIGN.md` — Full algorithm descriptions and code
- Dire Wolf's `demod_afsk.c` — For comparison (correlator approach)
- Start with a SINGLE fast-path decoder, verify it works, then add quality path

### Test with:
- Synthetic loopback: modulate → demodulate → verify (tests/common/mod.rs)
- Standard scenarios: noise, frequency offset, clock drift, combinations
- WA8LMF TNC Test CD Track 1 (download from http://wa8lmf.net/TNCtest/)
- Generated test tones at exact mark/space frequencies

### Debugging tips:
- Log intermediate values: filter output, discriminator, PLL phase, inst. freq
- Write them to a CSV, plot in Python: `plt.plot(disc_output); plt.show()`
- Use `tests/common/write_wav()` to save intermediate audio for Audacity
- The adaptive tracker's frequency estimates are great diagnostics

## Step 5: AFSK Modulator

### What to build:
- Phase-continuous AFSK tone generator
- NRZI encoder
- Complete TX path: AX.25 frame → HDLC → NRZI → AFSK → audio samples
- Preamble/postamble generation

### Test with:
- Generate audio, feed it back through your demodulator (loopback test)
- Play generated audio through a speaker, receive on a radio
- Compare generated audio against Dire Wolf's output for the same packet

## Step 6: Desktop Audio I/O

### What to build:
- Sound card input/output using `cpal`
- Audio device selection
- Sample rate configuration
- Buffer management

### Test with:
- Receive live APRS on 144.390 MHz (NA) or 144.800 MHz (EU)
- Transmit a test packet, verify with another receiver

## Step 7: APRS Parser

### What to build:
- Data Type Identifier (DTI) dispatch
- Position parsing (plain, compressed, Mic-E)
- Message parsing
- Weather, telemetry, status, objects/items

### Reference:
- APRS101.PDF is the definitive spec
- Mic-E is the trickiest part — implement it last

## Step 8: Networking

### What to build:
- KISS TCP server (for connecting APRS clients like Xastir, YAAC)
- APRS-IS client (connect to rotate.aprs2.net:14580)
- IGate logic (forward RF packets to APRS-IS and vice versa)

## Step 9: ESP32 Port

### What to build:
- I2S audio driver
- Wire core library to I2S input/output
- WiFi configuration
- APRS-IS over WiFi
- Web UI for configuration (optional, nice to have)

By this point, the core library is well-tested on desktop, so the ESP32
work is primarily I/O wiring — the hard DSP and protocol work is done.

## Step 10: Other Embedded Targets

### STM32:
- I2S audio via `stm32-hal` or `embassy-stm32`
- UART KISS interface
- Minimal standalone TNC

### RP2040:
- PIO-based I2S or ADC input
- UART KISS
- Explore PIO for AFSK generation

## Development Tools

### Essential
- `cargo test` — run all core tests on your development machine
- WAV file reader — for processing test audio offline
- A handheld radio tuned to APRS frequency — for real-world testing

### Recommended
- `cargo-fuzz` — fuzz your parsers
- Python + matplotlib — for plotting DSP debug data
- Audacity — for examining audio waveforms
- Dire Wolf — as a reference implementation to compare against
- SDR receiver (RTL-SDR, ~$25) — for capturing APRS audio without a radio

### Nice to have
- Logic analyzer — for debugging I2S timing on embedded
- Oscilloscope — for verifying audio output levels
- Second radio + TNC — for end-to-end testing

## Claude Code Usage

This repository is structured to work well with Claude Code. Recommended
workflow:

```bash
# Clone the repo
git clone <repo-url>
cd packet-radio-rs

# Start Claude Code
claude

# Ask Claude to implement a specific component:
# "Implement the AX.25 address parser in core/src/ax25/address.rs"
# "Write the HDLC decoder state machine in core/src/ax25/frame.rs"
# "Implement the delay-multiply fast-path demodulator in core/src/modem/demod.rs"
# "Add the Hilbert transform quality-path demodulator"
# "Add unit tests for the APRS position parser"
```

The documentation in `docs/` provides Claude with all the context it needs
to implement each component correctly. The architecture doc defines the
interfaces, the modem doc describes the DSP algorithms, and this file
provides the implementation order.
