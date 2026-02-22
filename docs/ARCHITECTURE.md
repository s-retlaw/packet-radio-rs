# Architecture Design Document

## Overview

This project is a cross-platform packet radio TNC and APRS engine. The architecture
is designed around a central principle: **the core DSP and protocol logic must have
zero platform dependencies**.

This is achieved through a layered workspace with clearly defined boundaries:

1. **Core** (`no_std`) — All computation: modem, protocols, parsing
2. **Shared** (`std`) — Cross-platform utilities that need networking/filesystem
3. **Platform binaries** — Thin wrappers that wire core to hardware

## Design Principles

### 1. `no_std` First

The core library must compile with `#![no_std]`. This ensures it can run on bare
metal microcontrollers as well as desktop systems. Practically this means:

- No heap allocation in the core modem or AX.25 framing (use fixed-size buffers)
- No filesystem, networking, or threading
- No `println!` or logging that requires std (use `defmt` or feature-gated logging)
- Optional `alloc` feature for the APRS parser where variable-length data helps

### 2. Trait-Based Hardware Abstraction

The core defines traits for I/O. Platform layers implement them.

```rust
/// Audio sample source — implemented per platform
pub trait SampleSource {
    /// Fill buffer with audio samples. Returns number of samples written.
    fn read_samples(&mut self, buf: &mut [i16]) -> usize;
}

/// Audio sample sink — implemented per platform
pub trait SampleSink {
    /// Write audio samples for transmission. Returns number consumed.
    fn write_samples(&mut self, samples: &[i16]) -> usize;
}

/// Decoded frame output
pub trait FrameSink {
    /// Called when a valid AX.25 frame has been decoded
    fn on_frame(&mut self, frame: &ax25::Frame) -> bool;
}

/// Frame input for transmission
pub trait FrameSource {
    /// Get next frame to transmit, if any
    fn next_frame(&mut self, buf: &mut [u8]) -> Option<usize>;
}
```

### 3. Zero-Copy Where Possible

AX.25 frames are parsed in-place from byte slices. The `Frame` type borrows from
the underlying buffer rather than copying:

```rust
pub struct Frame<'a> {
    pub dest: Address<'a>,
    pub src: Address<'a>,
    pub digipeaters: DigiPath<'a>,
    pub control: u8,
    pub pid: u8,
    pub info: &'a [u8],
}
```

This avoids allocation and works naturally with fixed-size DMA buffers on embedded.

### 4. Feature Flags for Scalability

```toml
[features]
default = []
alloc = []          # Enable Vec/String in APRS parser
std = ["alloc"]     # Full standard library
multi-decoder = []  # Multiple parallel demodulators
9600-baud = []      # G3RUH/GMSK modem support
fx25 = []           # FX.25 forward error correction
```

A minimal embedded build uses no features. Desktop enables everything.

## Module Design

### Modem (`core::modem`)

The modem module handles AFSK modulation and demodulation.

**Demodulator pipeline (dual-path):**

```
EMBEDDED FAST PATH (RP2040, Cortex-M0):
Audio → BPF → Delay-Multiply → LPF → PLL → NRZI → Hard Bits → HDLC

QUALITY PATH (Desktop, ESP32):
Audio → BPF → Hilbert → InstFreq → Adaptive Tracker → PLL → Soft Bits → Soft HDLC
```

Both paths produce frames that feed into the same AX.25 and APRS parsers.
The quality path achieves better decode performance through three innovations
over the traditional correlator approach (see `docs/MODEM_DESIGN.md`):

1. **Adaptive tracking**: Uses preamble to estimate each transmitter's actual
   mark/space frequencies and baud rate, replacing Dire Wolf's brute-force
   multi-decoder with a single adaptive decoder.
2. **Soft-decision decoding**: Preserves bit confidence (LLR) through the
   pipeline, enabling bit-flip error recovery on CRC failures.
3. **Instantaneous frequency detection**: Via Hilbert transform, provides
   continuous frequency estimates rather than binary mark/space decisions.

```rust
// Fast path (~144 bytes RAM, ~30 cycles/sample)
pub struct FastDemodulator {
    bpf: BiquadFilter,
    detector: DelayMultiplyDetector, // 1 multiply per sample
    pll: ClockRecoveryPll,
}

// Quality path (~1 KB RAM, ~100 cycles/sample)
pub struct QualityDemodulator {
    bpf: BiquadFilter,
    hilbert: HilbertTransform,       // 31-tap FIR
    inst_freq: InstFreqDetector,     // Phase difference → frequency
    tracker: AdaptiveTracker,        // Preamble training
    pll: ClockRecoveryPll,
}
```

**Why not multi-decoder?**
Dire Wolf runs 3-6 parallel correlator decoders with different parameters,
hoping one matches each transmitter. This works but wastes CPU. A single
adaptive decoder that tunes itself to each packet's preamble can match or
exceed multi-decoder performance at a fraction of the cost — critical for
embedded targets where CPU and memory are limited.

**Modulator:**
The modulator generates AFSK audio from a bit stream. It maintains phase
continuity between mark and space tones (continuous-phase FSK).

```rust
pub struct AfskModulator {
    config: ModConfig,
    phase: u32,           // Current oscillator phase (fixed-point)
    sample_rate: u32,
}

impl AfskModulator {
    pub fn generate_samples(
        &mut self,
        bits: &[u8],
        samples_out: &mut [i16],
    ) -> usize { ... }
}
```

### AX.25 (`core::ax25`)

**HDLC Framing:**
- Flag detection (0x7E)
- Bit unstuffing (remove zero after five consecutive ones)
- CRC-16-CCITT validation
- Frame boundary detection

The HDLC decoder is a state machine fed by the bit stream from the demodulator:

```rust
pub enum HdlcState {
    Hunting,        // Looking for flag
    Receiving,      // Accumulating frame bits
}

pub struct HdlcDecoder {
    state: HdlcState,
    shift_reg: u8,
    ones_count: u8,
    frame_buf: [u8; MAX_FRAME_LEN],
    frame_len: usize,
    crc: u16,
}
```

**Frame Parsing:**
AX.25 frames have a well-defined structure:
- Destination address (7 bytes)
- Source address (7 bytes)
- 0-8 digipeater addresses (7 bytes each)
- Control field (1 byte)
- PID field (1 byte)
- Information field (0-256 bytes)

Addresses are encoded in shifted ASCII with SSID in the last byte.

### APRS (`core::aprs`)

APRS is carried in the information field of UI (Unnumbered Information) frames.
The first byte of the info field is the Data Type Identifier (DTI) which
determines the packet format.

Key packet types:
- `!` or `=` — Position without/with timestamp
- `/` or `@` — Position with timestamp
- `` ` `` or `'` — Mic-E encoded position
- `:` — Message
- `;` — Object
- `)` — Item
- `_` — Weather report
- `T` — Telemetry
- `>` — Status
- `{` — User-defined

Mic-E encoding is the most complex — it encodes latitude in the destination
address field and longitude/speed/course in the information field. This saves
bytes but is notoriously tricky to parse correctly.

### KISS (`core::kiss`)

KISS is a simple framing protocol for communication between a TNC and host:
- Frame delimiter: 0xC0 (FEND)
- Escape: 0xDB (FESC)
- Transposed FEND: 0xDC (TFEND)
- Transposed FESC: 0xDD (TFESC)

The first byte after FEND contains the port number (high nibble) and command
type (low nibble). Command 0x00 is "data frame."

## Memory Budget (Embedded)

For a single-channel 1200 baud TNC on a microcontroller:

| Component | RAM Usage |
|-----------|----------|
| Audio input buffer (2x 512 samples) | 2 KB |
| Audio output buffer (2x 512 samples) | 2 KB |
| HDLC frame buffer | 512 B |
| AFSK demodulator state | ~1 KB |
| AFSK modulator state | ~256 B |
| AX.25 frame workspace | 512 B |
| KISS TX/RX buffers | 1 KB |
| Stack | 4 KB |
| **Total** | **~11 KB** |

This comfortably fits on any of our target microcontrollers, with plenty of
headroom for additional features.

## Concurrency Model

**Desktop:** Use `tokio` async runtime. Audio runs in a dedicated thread (cpal
callback), communicating with the async tasks via channels. Network I/O
(KISS TCP, APRS-IS) is async.

**ESP32 (IDF):** Use `std` threads or `embassy` async. Audio via I2S runs on
one core, protocol handling on the other.

**Bare metal:** Use `embassy` async runtime or a simple super-loop. Audio is
interrupt-driven via DMA. The main loop processes decoded bits and frames.

## Testing Strategy

1. **Unit tests** — Core modem, AX.25 parser, APRS parser all testable on host
2. **WAV file tests** — Feed known audio recordings through the demodulator,
   verify correct packet decode. Dire Wolf includes test audio files.
3. **Round-trip tests** — Modulate packets, demodulate them, verify match
4. **Fuzzing** — Fuzz the AX.25 and APRS parsers with `cargo-fuzz`
5. **Embedded integration** — Test on real hardware with known RF signals
