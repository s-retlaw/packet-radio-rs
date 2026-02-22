# CLAUDE.md — Context for Claude Code

## Project Overview

This is a cross-platform packet radio TNC (Terminal Node Controller) and APRS
engine written in Rust. It is a spiritual successor to Dire Wolf, designed to
run on everything from desktop computers to ESP32 microcontrollers.

## Architecture

- **`core/`** — `no_std` library with all computation: AFSK modem, AX.25, APRS, KISS
- **`shared/`** — Cross-platform `std` utilities: APRS-IS client, IGate, config
- **`desktop/`** — Desktop binary using sound card (cpal) for audio
- **`esp32/`** — ESP32 firmware using I2S for audio and WiFi for IGate
- **`docs/`** — Design documentation (READ THESE for implementation details)

## Key Design Constraints

1. The `core` crate MUST be `#![no_std]` — no standard library, no heap allocation
   unless the `alloc` feature is enabled. This ensures it runs on bare metal MCUs.
2. All DSP (modem) code should work with fixed-size buffers and no dynamic allocation.
3. Platform I/O is abstracted via traits in `core/src/lib.rs` (`SampleSource`,
   `SampleSink`, `FrameHandler`).
4. Prefer zero-copy parsing — `Frame` and `AprsPacket` borrow from input buffers.

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
cargo build -p desktop --release

# Check everything compiles
cargo check --workspace

# Build only core with no features (strictest no_std check)
cargo build -p packet-radio-core --no-default-features
```

## Implementation Status

Most modules are scaffolded with documented stubs and TODOs. The recommended
implementation order (from docs/GETTING_STARTED.md):

1. ✅ AX.25 address parser (scaffolded, needs tests)
2. ✅ HDLC framing (scaffolded, needs implementation)
3. ✅ KISS protocol (scaffolded with working encode/decode)
4. 🔲 AFSK demodulator — DUAL PATH ARCHITECTURE:
      - **Fast path**: Delay-multiply discriminator → LPF → PLL → hard bits
        (for RP2040, Cortex-M0, resource-constrained targets)
      - **Quality path**: Hilbert transform → instantaneous frequency →
        adaptive tracker → PLL → soft bits → bit-flip recovery
        (for desktop, ESP32 — significantly better decode performance)
      - See docs/MODEM_DESIGN.md for complete design
5. 🔲 AFSK modulator (NCO phase accumulator, sin table)
6. 🔲 APRS parser (stub — position and Mic-E parsing needed)
7. 🔲 Desktop audio I/O
8. 🔲 APRS-IS client
9. 🔲 ESP32 firmware

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
# Run all core unit tests
cargo test -p packet-radio-core

# Run comparative demodulator tests (once implemented)
cargo test --test demod_comparative -- --nocapture

# Run TNC Test CD benchmark
cargo run --release -p benchmark -- --wav tests/wav/track1.wav

# Compare fast vs quality demodulator paths
cargo run --release -p benchmark -- --compare-approaches tests/wav/track1.wav

# Synthetic signal benchmark (no WAV files needed)
cargo run --release -p benchmark -- --synthetic
```

Test infrastructure lives in `tests/`:
- `tests/common/mod.rs` — Signal generation, impairments, WAV I/O, analysis
- `tests/demod_comparative.rs` — A/B comparison of demodulator paths
- `tests/benchmark/main.rs` — TNC Test CD and synthetic benchmarks
- `tests/wav/` — WAV files (not in git, see README.md for download links)

- Dire Wolf source: https://github.com/wb2osz/direwolf (study demod_afsk.c)
- AX.25 spec: https://www.ax25.net/AX25.2.2-Jul%2098-2.pdf
- APRS spec: http://www.aprs.org/doc/APRS101.PDF
- KISS protocol: https://en.wikipedia.org/wiki/KISS_(TNC)
- esp-rs: https://github.com/esp-rs
