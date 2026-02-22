# Packet Radio RS

> A cross-platform, `no_std`-compatible packet radio TNC (Terminal Node Controller) and APRS engine written in Rust. A modern, memory-safe spiritual successor to Dire Wolf.

## Project Goals

- **Cross-platform core**: `no_std` library that runs on everything from desktop Linux to ESP32 to STM32
- **High-performance AFSK modem**: Bell 202 (1200 baud) modulator/demodulator with multi-decoder support
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
│  └──────────────────────────┬───────────────┘     │
├─────────────────────────────┴────────────────────┤
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
├── Cargo.toml              # Workspace root
├── core/                   # no_std core library
│   └── src/
│       ├── lib.rs
│       ├── modem/          # AFSK modulation/demodulation
│       │   ├── mod.rs
│       │   ├── afsk.rs     # Bell 202 AFSK modem
│       │   ├── demod.rs    # Demodulator implementations
│       │   └── filter.rs   # DSP filters (bandpass, LPF, PLL)
│       ├── ax25/           # AX.25 protocol
│       │   ├── mod.rs
│       │   ├── frame.rs    # HDLC framing, bit stuffing
│       │   └── address.rs  # Callsign/SSID parsing
│       ├── aprs/           # APRS encoding/decoding
│       │   ├── mod.rs
│       │   ├── position.rs # Position reports
│       │   ├── message.rs  # APRS messages
│       │   ├── weather.rs  # Weather reports
│       │   ├── telemetry.rs
│       │   └── mic_e.rs    # Mic-E encoding
│       └── kiss/           # KISS TNC protocol
│           └── mod.rs
├── shared/                 # Cross-platform std utilities
│   └── src/
│       ├── lib.rs
│       ├── igate.rs        # APRS-IS client
│       └── config.rs       # Configuration
├── desktop/                # Desktop binary
│   └── src/
│       ├── main.rs
│       ├── audio.rs        # Sound card I/O via cpal
│       └── network.rs      # TCP KISS, AGW interfaces
├── esp32/                  # ESP32 firmware
│   └── src/
│       ├── main.rs
│       ├── audio.rs        # I2S / ADC+DAC audio
│       └── wifi.rs         # WiFi for IGate
├── docs/                   # Design documentation
│   ├── ARCHITECTURE.md
│   ├── MODEM_DESIGN.md
│   ├── HARDWARE.md
│   ├── ESP32_GUIDE.md
│   └── GETTING_STARTED.md
└── tests/                  # Integration test assets
    └── wav/                # Test WAV files with known packets
```

## Target Platforms

| Platform | Chip Examples | Audio Interface | Connectivity | Use Case |
|----------|--------------|-----------------|-------------|----------|
| Desktop Linux/Mac/Win | x86_64, aarch64 | Sound card (cpal) | TCP/IP native | Full TNC, Dire Wolf replacement |
| Raspberry Pi | BCM2711 | USB sound card, I2S HAT | TCP/IP native | Headless IGate/digipeater |
| ESP32 / ESP32-S3 | Xtensa 240MHz, FPU | I2S codec, built-in DAC/ADC | WiFi built-in | Standalone WiFi IGate |
| ESP32-C3 / C6 | RISC-V 160MHz | I2S, ADC | WiFi built-in | Cheap WiFi TNC (no FPU) |
| STM32F4 | Cortex-M4 168MHz, FPU | I2S + codec | UART KISS | Standalone KISS TNC |
| RP2040 | Dual Cortex-M0+ 133MHz | PIO I2S, ADC | UART KISS | Ultra-cheap $1 TNC |
| nRF52840 | Cortex-M4 64MHz, FPU | I2S, PDM | BLE | Low-power BLE TNC |

## Quick Start

### Desktop Development

```bash
# Clone the repo
git clone https://github.com/YOUR_USER/packet-radio-rs.git
cd packet-radio-rs

# Build the core library
cargo build -p core

# Run tests
cargo test -p core

# Build the desktop TNC
cargo build -p desktop --release

# Run the desktop TNC
cargo run -p desktop -- --help
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
- [ ] Bell 202 AFSK demodulator (single decoder)
- [ ] Bell 202 AFSK modulator
- [ ] HDLC framing (flag detect, bit unstuffing, CRC-16)
- [ ] AX.25 frame parsing (callsigns, SSID, digipeater path)
- [ ] KISS protocol encode/decode
- [ ] WAV file test harness

### Phase 2: APRS
- [ ] Position report parsing (plain and compressed)
- [ ] Mic-E decoding
- [ ] Message parsing and encoding
- [ ] Weather report parsing
- [ ] Object/Item parsing
- [ ] Telemetry

### Phase 3: Desktop TNC
- [ ] Sound card audio I/O via cpal
- [ ] KISS TCP server
- [ ] AGW port emulator
- [ ] APRS-IS client (IGate)
- [ ] Multi-decoder (parallel demodulators with varied parameters)
- [ ] Configuration file support

### Phase 4: Embedded Targets
- [ ] ESP32 firmware with I2S audio
- [ ] ESP32 WiFi IGate
- [ ] STM32F4 KISS TNC
- [ ] RP2040 KISS TNC
- [ ] Fixed-point DSP option for chips without FPU

### Phase 5: Advanced Features
- [ ] 9600 baud G3RUH/GMSK modem
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
