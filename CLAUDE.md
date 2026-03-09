# CLAUDE.md — Context for Claude Code

## Project Overview

This is a cross-platform packet radio TNC (Terminal Node Controller) and APRS
engine written in Rust. It is a spiritual successor to Dire Wolf, designed to
run on everything from desktop computers to ESP32 microcontrollers.

## Architecture

- **`core/`** — `no_std` library with all computation: AFSK/G3RUH modem, AX.25, APRS, KISS
- **`shared/`** — Cross-platform `std` utilities: APRS-IS client, IGate, config
- **`desktop/`** — Desktop binary using sound card (cpal) for audio, TUI interface
- **`esp32/`** — ESP32 firmware using I2S for audio and WiFi for IGate
- **`reference/`** — Reference data crate (FCC callsign DB)
- **`aprs-viewer/`** — APRS packet viewer utility
- **`tests/benchmark/`** — WA8LMF TNC Test CD benchmark runner (18 subcommand modules)
- **`tools/`** — Utilities (kiss-dump)
- **`rp2040-test/`**, **`pico2w-test/`** — MCU test harnesses (RP2040, RP2350)
- **`esp32c3-test/`**, **`esp32c6-test/`**, **`esp32c3-host/`** — ESP32-C3/C6 test harnesses
- **`docs/`** — Design documentation (13 files — READ THESE for implementation details)

## Key Design Constraints

1. The `core` crate MUST be `#![no_std]` — no standard library, no heap allocation
   unless the `alloc` feature is enabled. This ensures it runs on bare metal MCUs.
2. All DSP (modem) code should work with fixed-size buffers and no dynamic allocation.
3. Platform I/O is abstracted via traits in `core/src/lib.rs` (`SampleSource`,
   `SampleSink`, `FrameHandler`).
4. Prefer zero-copy parsing — `Frame` and `AprsPacket` borrow from input buffers.

## Optimization Goals

The primary optimization target is **single-decoder performance**. The ESP32 MCU
target cannot run 38 parallel decoders, so improving what a single decoder can
extract from difficult signals is what matters most.

- **Track 2** (`02_100-mic-e-bursts-de-emphasized.wav`) is the hardest and most
  important benchmark — 100 de-emphasized Mic-E bursts that stress clock recovery,
  AGC, and filter design.
- All WA8LMF tracks matter, but Track 2 is the priority target.
- Multi-decoder results (98.3% of Dire Wolf) are useful for desktop but are NOT
  the MCU optimization target.
- Current single-decoder baselines on Track 2 (of 974 total, Dire Wolf=974):
  - Fast (Goertzel+Bresenham): **563**
  - Quality (Goertzel+Bresenham+SoftHDLC): **564**
  - DM+PLL (Delay-Multiply+Gardner PLL): **386**

## Important Documentation

Before implementing any component, read the relevant design doc:

- `docs/ARCHITECTURE.md` — Overall system design, traits, memory budget
- `docs/MODEM_DESIGN.md` — AFSK demodulator/modulator algorithms, DSP details
- `docs/HARDWARE.md` — Audio interfaces, PTT control, bill of materials
- `docs/ESP32_GUIDE.md` — ESP32-specific toolchain, I2S, WiFi setup
- `docs/GETTING_STARTED.md` — Implementation order and development workflow
- `docs/TEST_PLAN.md` — Comprehensive test strategy, WA8LMF benchmark, fuzz testing

## Build Commands

```bash
# Build core library
cargo build -p packet-radio-core

# Run all core tests
cargo test -p packet-radio-core

# Build desktop TNC
cargo build -p packet-radio-desktop --release

# Check everything compiles
cargo check --workspace

# Build only core with no features (strictest no_std check)
cargo build -p packet-radio-core --no-default-features
```

## Implementation Status

Most modules are scaffolded with documented stubs and TODOs. The recommended
implementation order (from docs/GETTING_STARTED.md):

1. ✅ AX.25 address parser
2. ✅ HDLC framing (hard + soft/bit-flip recovery)
3. ✅ KISS protocol (working encode/decode)
4. ✅ AFSK demodulator — TWO ARCHITECTURES, FOUR MODES:
      - **Goertzel**: BPF → Goertzel tone detection → Bresenham timing → hard bits
      - **Delay-Multiply**: BPF → delay-multiply discriminator → LPF → PLL/Bresenham
      - **Quality path**: Goertzel + LLR → SoftHdlcDecoder (1-2 bit recovery)
      - **Multi-decoder**: 38× parallel (32 Goertzel + 6 DM) on std, 23× on no_std
      - PLL clock recovery uses Gardner TED (alpha+beta correction)
5. ✅ AFSK modulator (NCO phase accumulator, sin table)
6. ✅ APRS parser (position, Mic-E, message parsing)
7. ✅ Desktop TNC (cpal audio, WAV decode, KISS TCP, TUI, config file)
      - Modes: fast, quality, multi, smart3, dm, corr, corr-slicer, combined, xor
8. ✅ APRS-IS client (TNC-2 parser, TCP connection in `shared/src/aprs_is.rs`)
9. 🔲 ESP32 firmware (test harnesses for ESP32/C3/C6/RP2040/Pico2W complete)

## Coding Conventions

- Use `/// doc comments` on all public items
- Use `// TODO:` for unimplemented sections
- Keep the core crate free of `std` imports (use `core::` equivalents)
- Prefer `i16` for audio samples (standard PCM format)
- Use fixed-point Q15 arithmetic for DSP on embedded (see `modem/filter.rs`)
- Test with `#[cfg(test)]` modules in each file

## Key References

## Testing

```bash
# Run all core unit tests (152+ tests)
cargo test -p packet-radio-core --features multi-decoder

# Run comparative demodulator tests
cargo test --test demod_comparative -- --nocapture

# 1200 baud WA8LMF benchmark
cargo run --release -p benchmark -- suite tests/wav/
cargo run --release -p benchmark -- wav tests/wav/01_track1.wav
cargo run --release -p benchmark -- compare-approaches tests/wav/01_track1.wav

# 1200 baud specialized modes
cargo run --release -p benchmark -- smart3 tests/wav/01_track1.wav
cargo run --release -p benchmark -- corr tests/wav/01_track1.wav
cargo run --release -p benchmark -- corr-slicer tests/wav/01_track1.wav
cargo run --release -p benchmark -- dm tests/wav/01_track1.wav
cargo run --release -p benchmark -- xor tests/wav/01_track1.wav
cargo run --release -p benchmark -- twist-mini tests/wav/01_track1.wav
cargo run --release -p benchmark -- diff tests/wav/02_100-mic-e-bursts-de-emphasized.wav
cargo run --release -p benchmark -- attribution tests/wav/02_100-mic-e-bursts-de-emphasized.wav

# 300 baud HF AFSK
cargo run --release -p benchmark -- pll-300 tests/wav/some_300_file.wav

# 9600 baud G3RUH
cargo run --release -p benchmark -- 9600 tests/wav/some_9600_file.wav
cargo run --release -p benchmark -- 9600-suite tests/wav/
cargo run --release -p benchmark -- 9600-compare tests/wav/some_9600_file.wav
cargo run --release -p benchmark -- 9600-multi tests/wav/some_9600_file.wav

# Synthetic signal benchmark (no WAV files needed)
cargo run --release -p benchmark -- synthetic
```

Test infrastructure lives in `tests/`:
- `tests/common/mod.rs` — Signal generation, impairments, WAV I/O, analysis
- `tests/demod_comparative.rs` — A/B comparison of demodulator paths
- `tests/benchmark/main.rs` — WA8LMF benchmark runner (18 subcommand modules)
- `tests/wav/` — WAV files (not in git, see README.md for download links)

- Dire Wolf source: https://github.com/wb2osz/direwolf (study demod_afsk.c)
- AX.25 spec: https://www.ax25.net/AX25.2.2-Jul%2098-2.pdf
- APRS spec: http://www.aprs.org/doc/APRS101.PDF
- KISS protocol: https://en.wikipedia.org/wiki/KISS_(TNC)
- esp-rs: https://github.com/esp-rs
