# Packet Radio RS

> A cross-platform, `no_std`-compatible packet radio TNC (Terminal Node Controller) and APRS engine written in Rust. A modern, memory-safe spiritual successor to Dire Wolf.

## Project Goals

- **Cross-platform core**: `no_std` library that runs on everything from desktop Linux to ESP32 to RP2040
- **Multi-baud AFSK/FSK modem**: 1200 baud Bell 202, 300 baud HF AFSK, 9600 baud G3RUH
- **Full APRS support**: Parse and encode all common APRS packet types
- **AX.25 protocol**: Complete HDLC framing and AX.25 header parsing
- **Multiple deployment targets**: Desktop TNC, ESP32 IGate, embedded KISS TNC, library crate
- **Memory safe**: No buffer overflows in your packet parser

## Architecture

```
┌──────────────────────────────────────────────────┐
│           Platform Layers (thin)                  │
│                                                   │
│  Desktop          ESP32            STM32/RP2040   │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    │
│  │ cpal     │    │ I2S/ADC  │    │ I2S/ADC  │    │
│  │ tokio    │    │ WiFi     │    │ HAL      │    │
│  │ TCP/IP   │    │ ESP-IDF  │    │ embassy  │    │
│  └────┬─────┘    └────┬─────┘    └────┬─────┘    │
├───────┴──────────────┴──────────────┴────────────┤
│              Shared Layer (optional std)           │
│  ┌──────────────────────────────────────────┐     │
│  │ APRS-IS client, IGate logic, config      │     │
│  └──────────────────────┬───────────────┘     │
├─────────────────────────┴────────────────────────┤
│              Core Library (no_std)                 │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐    │
│  │ Modem  │ │ AX.25  │ │ APRS   │ │ KISS   │    │
│  │ AFSK   │ │ HDLC   │ │ Parse  │ │ Proto  │    │
│  │ Mod/   │ │ Frame  │ │ Encode │ │        │    │
│  │ Demod  │ │ CRC    │ │ Mic-E  │ │        │    │
│  └────────┘ └────────┘ └────────┘ └────────┘    │
└──────────────────────────────────────────────────┘
```

## Workspace Structure

```
packet-radio-rs/
├── Cargo.toml                  # Workspace root
├── core/                       # no_std core library (packet-radio-core)
│   └── src/
│       ├── lib.rs              # Traits: SampleSource, SampleSink, FrameHandler
│       ├── tnc.rs              # TNC engine (encode/decode pipeline)
│       ├── modem/              # DSP: modulation/demodulation (19 files)
│       │   ├── mod.rs, afsk.rs         # Bell 202 AFSK modem, NCO modulator
│       │   ├── demod.rs                # Goertzel, DM, Correlation demodulators
│       │   ├── filter.rs, hilbert.rs   # BPF, LPF, Hilbert transform
│       │   ├── pll.rs, adaptive.rs     # Gardner PLL, adaptive retune
│       │   ├── multi.rs                # MultiDecoder (38×), MiniDecoder (3×)
│       │   ├── corr_slicer.rs          # Multi-slicer correlation decoder
│       │   ├── binary_xor.rs           # Binary XOR correlator
│       │   ├── soft_hdlc.rs            # SoftHdlcDecoder (LLR bit-flip recovery)
│       │   ├── hdlc_bank.rs            # HdlcBank<N> (heap-allocated multi-HDLC)
│       │   ├── demod_9600.rs, mod_9600.rs, multi_9600.rs  # 9600 baud G3RUH
│       │   ├── scrambler.rs            # G3RUH scrambler/descrambler
│       │   ├── delay_multiply.rs       # Delay-multiply discriminator
│       │   ├── fixed_vec.rs            # Fixed-capacity vector (no_std)
│       │   └── frame_output.rs         # Frame output abstraction
│       ├── ax25/               # AX.25 protocol
│       │   ├── mod.rs          # Callsign/SSID parsing, address handling
│       │   └── frame.rs        # HDLC framing, bit stuffing, CRC
│       ├── aprs/               # APRS encoding/decoding
│       │   ├── mod.rs          # Position, Mic-E, message, compressed parsing
│       │   └── nmea.rs         # NMEA sentence handling
│       └── kiss/               # KISS TNC protocol
│           └── mod.rs
├── shared/                     # Cross-platform std utilities
│   └── src/
│       ├── lib.rs
│       ├── aprs_is.rs          # APRS-IS client (TNC-2 parser, TCP connection)
│       ├── igate.rs            # IGate logic
│       └── config.rs           # Configuration
├── desktop/                    # Desktop binary (packet-radio-desktop)
│   └── src/
│       ├── main.rs, cli.rs     # Entry point, CLI argument parsing
│       ├── audio.rs            # Sound card I/O via cpal
│       ├── decoder.rs          # Decoder mode selection
│       ├── kiss_server.rs      # KISS TCP server (tokio)
│       ├── tx.rs               # TX audio generation
│       ├── headless.rs         # Headless (non-TUI) mode
│       ├── config.rs           # TOML config (packet-radio.toml)
│       ├── frame_fmt.rs        # Frame formatting
│       ├── processing.rs       # Audio processing pipeline
│       └── tui/                # Terminal UI (ratatui)
│           ├── mod.rs, state.rs, event.rs
│           ├── ui/             # Tab views: packets, aprs, settings
│           └── widgets/        # Dialog, file picker, text input
├── reference/                  # Reference data crate (FCC callsign DB)
│   └── src/
│       ├── lib.rs, db.rs, geo.rs, source.rs
├── aprs-viewer/                # APRS packet viewer utility
│   └── src/
│       ├── main.rs, lib.rs, models.rs
├── tests/                      # Integration tests & benchmarks
│   ├── benchmark/              # WA8LMF TNC Test CD benchmark (18 modules)
│   │   ├── main.rs             # CLI: wav, suite, diff, attribution, 9600-*...
│   │   └── *.rs                # Subcommands per mode/baud rate
│   ├── common/                 # Signal generation, impairments, WAV I/O
│   ├── demod_comparative.rs    # A/B demodulator comparison
│   └── wav/                    # Test WAV files (not in git)
├── esp32/                      # ESP32 firmware (I2S audio, WiFi IGate)
├── esp32c3-host/               # ESP32-C3 host-side tools
├── esp32c3-test/               # ESP32-C3 test harness
├── esp32c6-test/               # ESP32-C6 test harness
├── rp2040-test/                # RP2040 test harness (USB-CDC, ADC decode)
├── pico2w-test/                # Pico 2 W (RP2350) test harness (Embassy)
├── tools/                      # Utilities (kiss-dump)
├── docs/                       # Design documentation (13 files)
│   ├── ARCHITECTURE.md         # System design, traits, memory budget
│   ├── MODEM_DESIGN.md         # AFSK demodulator/modulator algorithms
│   ├── MULTI_BAUD.md           # 300/1200/9600 baud support
│   ├── SAMPLE_RATE_ANALYSIS.md # Sample rate trade-offs
│   ├── NOVEL_STRATEGIES.md     # Advanced optimization ideas
│   ├── OPTIMIZATION_LOG.md     # Performance tuning history
│   ├── HARDWARE.md, HARDWARE_DEVICE.md  # Audio interfaces, BOM
│   ├── ESP32_GUIDE.md          # ESP32 toolchain, I2S, WiFi
│   ├── MCU_DEMOD_REVIEW.md, MCU_TEST_TOOLS.md  # MCU-specific docs
│   ├── GETTING_STARTED.md      # Implementation order, dev workflow
│   └── TEST_PLAN.md            # Test strategy, WA8LMF benchmark
└── fcc-data/                   # FCC license database files
```

## Target Platforms

| Platform | Chip Examples | Audio Interface | Connectivity | Use Case |
|----------|--------------|-----------------|-------------|----------|
| Desktop Linux/Mac/Win | x86_64, aarch64 | Sound card (cpal) | TCP/IP native | Full TNC, Dire Wolf replacement |
| Raspberry Pi | BCM2711 | USB sound card, I2S HAT | TCP/IP native | Headless IGate/digipeater |
| ESP32 / ESP32-S3 | Xtensa 240MHz, FPU | I2S codec, built-in DAC/ADC | WiFi built-in | Standalone WiFi IGate |
| ESP32-C3 / C6 | RISC-V 160MHz | I2S, ADC | WiFi built-in | Cheap WiFi TNC (no FPU) |
| STM32F4 | Cortex-M4 168MHz, FPU | I2S + codec | UART KISS | Standalone KISS TNC |
| RP2040 | Dual Cortex-M0+ 133MHz | PIO I2S, ADC | UART KISS, USB-CDC | Ultra-cheap $1 TNC |
| nRF52840 | Cortex-M4 64MHz, FPU | I2S, PDM | BLE | Low-power BLE TNC |

## Quick Start

### Desktop Development

```bash
# Clone the repo
git clone https://github.com/s-retlaw/packet-radio-rs.git
cd packet-radio-rs

# Build the core library
cargo build -p packet-radio-core

# Run tests (use --features multi-decoder for full test suite)
cargo test -p packet-radio-core
cargo test -p packet-radio-core --features multi-decoder

# Build the desktop TNC
cargo build -p packet-radio-desktop --release

# Run the desktop TNC (TUI mode by default)
cargo run -p packet-radio-desktop -- --help
```

### Benchmark

```bash
# 1200 baud WA8LMF benchmark suite
cargo run --release -p benchmark -- suite tests/wav/

# Single WAV file
cargo run --release -p benchmark -- wav tests/wav/01_track1.wav

# Compare demodulator approaches
cargo run --release -p benchmark -- compare-approaches tests/wav/01_track1.wav

# 9600 baud G3RUH
cargo run --release -p benchmark -- 9600 tests/wav/some_9600_file.wav
cargo run --release -p benchmark -- 9600-suite tests/wav/

# 300 baud HF AFSK
cargo run --release -p benchmark -- pll-300 tests/wav/some_300_file.wav

# Other modes: diff, attribution, smart3, corr, corr-slicer, dm, xor, twist-mini
cargo run --release -p benchmark -- --help
```

### ESP32 Development

```bash
# Install ESP32 Rust toolchain
cargo install espup
espup install

# Build for ESP32
cd esp32
cargo build --release

# Flash to device
espflash flash target/xtensa-esp32-espidf/release/esp32-tnc
```

See [docs/ESP32_GUIDE.md](docs/ESP32_GUIDE.md) for detailed hardware setup.

## Roadmap

### Phase 1: Core Foundation
- [x] Bell 202 AFSK demodulator (single decoder)
- [x] Bell 202 AFSK modulator
- [x] HDLC framing (flag detect, bit unstuffing, CRC-16, soft bit-flip recovery)
- [x] AX.25 frame parsing (callsigns, SSID, digipeater path)
- [x] KISS protocol encode/decode
- [x] WAV file test harness

### Phase 2: APRS
- [x] Position report parsing (plain and compressed)
- [x] Mic-E decoding
- [x] Message parsing and encoding
- [ ] Weather report parsing
- [ ] Object/Item parsing
- [ ] Telemetry

### Phase 3: Desktop TNC
- [x] Sound card audio I/O via cpal
- [x] KISS TCP server (bidirectional, TX WAV generation)
- [x] Multi-decoder (38× parallel: 32 Goertzel + 6 DM)
- [x] Configuration file support (TOML)
- [x] Terminal UI (ratatui — Packets/APRS/Settings tabs)
- [x] Multiple decoder modes (fast, quality, multi, smart3, dm, corr, corr-slicer, combined)
- [ ] AGW port emulator
- [ ] APRS-IS IGate mode

### Phase 4: Embedded Targets
- [x] ESP32 test harness (I2S audio decode)
- [x] RP2040 test harness (USB-CDC, ADC live decode)
- [x] Pico 2 W (RP2350) test harness (Embassy async)
- [ ] ESP32 full firmware (WiFi IGate)
- [ ] Fixed-point DSP option for chips without FPU

### Phase 5: Advanced Features
- [x] 9600 baud G3RUH modem (modulator + demodulator + multi-decoder)
- [x] 300 baud HF AFSK modem
- [ ] FX.25 forward error correction
- [ ] Digipeater logic
- [ ] Web UI for configuration/monitoring
- [ ] Packet logging/replay

## References

- [Dire Wolf source code](https://github.com/wb2osz/direwolf) — reference implementation in C
- [AX.25 Link Access Protocol specification](https://www.ax25.net/AX25.2.2-Jul%2098-2.pdf)
- [APRS Protocol Reference (APRS101.PDF)](http://www.aprs.org/doc/APRS101.PDF)
- [KISS TNC Protocol](https://en.wikipedia.org/wiki/KISS_(TNC))
- [Bell 202 modem theory](https://en.wikipedia.org/wiki/Bell_202_modem)
- [esp-rs project](https://github.com/esp-rs) — Rust on ESP32
- [Embassy](https://embassy.dev/) — async Rust for embedded
- [The Rust Embedded Book](https://docs.rust-embedded.org/book/)

## License

Licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
