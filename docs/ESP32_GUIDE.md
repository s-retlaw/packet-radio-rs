# ESP32 Development Guide

## Overview

The ESP32 is an excellent target for this project because it offers WiFi (for
IGate functionality), adequate CPU (dual-core 240 MHz with FPU), and enough
RAM (520 KB) all for under $5. This guide covers setting up Rust development
for ESP32 and the specific considerations for our packet radio application.

## Toolchain Setup

### Install Prerequisites

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install espup - manages ESP32 Rust toolchains
cargo install espup

# Install the ESP32 Rust toolchain
# This installs the Xtensa LLVM fork and necessary tools
espup install

# Source the environment (add to your .bashrc/.zshrc)
source $HOME/export-esp.sh

# Install additional tools
cargo install espflash      # For flashing firmware
cargo install cargo-espflash # Cargo integration
```

### ESP-IDF vs Bare Metal

**ESP-IDF (std) — Recommended for this project:**
- Full Rust standard library available
- WiFi, TCP/IP, TLS all provided by ESP-IDF
- I2S driver available through `esp-idf-hal`
- Easier development, larger binary (~400KB+ flash)
- Use `esp-idf-template` to bootstrap

**Bare metal (no_std):**
- Uses `esp-hal` crate directly
- Smaller binaries, faster boot
- No WiFi stack (would need `esp-wifi` crate, still maturing)
- Better for pure KISS TNC without networking

```bash
# Create a new ESP-IDF (std) project
cargo generate esp-rs/esp-idf-template

# Or for bare metal (no_std)
cargo generate esp-rs/esp-template
```

## ESP-IDF Project Setup

### Cargo.toml

```toml
[package]
name = "packet-radio-esp32"
version = "0.1.0"
edition = "2021"

[dependencies]
# Core library from our workspace
packet-radio-core = { path = "../core", features = ["alloc"] }
packet-radio-shared = { path = "../shared" }

# ESP-IDF bindings
esp-idf-hal = "0.44"
esp-idf-svc = "0.49"
esp-idf-sys = "0.35"

# Logging
log = "0.4"
esp-idf-logger = "0.1"

# Async runtime (optional)
embassy-executor = { version = "0.6", features = ["nightly"] }
embassy-time = "0.3"

[build-dependencies]
embuild = "0.32"
```

### sdkconfig.defaults

```
# ESP-IDF configuration
CONFIG_ESP_DEFAULT_CPU_FREQ_240=y

# WiFi
CONFIG_ESP_WIFI_ENABLED=y

# I2S
CONFIG_SOC_I2S_SUPPORTED=y

# Increase main task stack size for Rust
CONFIG_ESP_MAIN_TASK_STACK_SIZE=8192

# Enable PSRAM if your board has it
# CONFIG_ESP32_SPIRAM_SUPPORT=y
```

## Audio Configuration

### I2S Setup (ESP-IDF)

```rust
use esp_idf_hal::i2s::{self, I2sDriver, I2sRx, I2sTx};
use esp_idf_hal::gpio::*;
use esp_idf_hal::peripherals::Peripherals;

fn setup_i2s_rx(peripherals: &mut Peripherals) -> I2sDriver<'_, I2sRx> {
    let config = i2s::config::StdConfig::philips(
        11025,  // Sample rate - 11025 Hz is sufficient for 1200 baud
        i2s::config::DataBitWidth::Bits16,
    );

    I2sDriver::new_std_rx(
        peripherals.i2s0,
        &config,
        peripherals.pins.gpio25, // BCK
        peripherals.pins.gpio26, // WS
        Some(peripherals.pins.gpio22), // DIN
        None::<AnyIOPin>,        // No MCLK needed for most codecs
    ).expect("Failed to initialize I2S RX")
}
```

### Built-in DAC/ADC (simpler but lower quality)

```rust
use esp_idf_hal::adc::{self, AdcDriver, AdcChannelDriver, Atten11dB};
use esp_idf_hal::dac::DacDriver;

// ADC for receive
let adc = AdcDriver::new(peripherals.adc1)?;
let mut adc_pin = AdcChannelDriver::<Atten11dB, _>::new(peripherals.pins.gpio34)?;

// DAC for transmit (8-bit, GPIO25 or GPIO26)
let mut dac = DacDriver::new(peripherals.dac1, peripherals.pins.gpio25)?;
```

## WiFi IGate Configuration

```rust
use esp_idf_svc::wifi::{EspWifi, ClientConfiguration, Configuration};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

fn connect_wifi(
    modem: esp_idf_hal::modem::Modem,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> Result<EspWifi<'static>, Box<dyn std::error::Error>> {
    let mut wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs))?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: "YOUR_SSID".try_into().unwrap(),
        password: "YOUR_PASSWORD".try_into().unwrap(),
        ..Default::default()
    }))?;

    wifi.start()?;
    wifi.connect()?;

    Ok(wifi)
}
```

## Memory Layout

ESP32 has 520 KB of SRAM. Typical usage for our application:

```
Total SRAM:                     520 KB
├── ESP-IDF system overhead:    ~80 KB
├── WiFi stack:                 ~60 KB
├── TCP/IP stack:               ~30 KB
├── I2S DMA buffers:            ~8 KB
├── Our application:            ~30 KB
│   ├── Demodulator(s):         ~4 KB
│   ├── Modulator:              ~1 KB
│   ├── Audio buffers:          ~8 KB
│   ├── Frame buffers:          ~4 KB
│   ├── APRS-IS TX queue:       ~4 KB
│   ├── Config:                 ~1 KB
│   └── Stack:                  ~8 KB
├── Free heap:                  ~310 KB
```

Plenty of room. With PSRAM (available on some ESP32 modules), you get an
additional 4-8 MB for logging, web UI, etc.

## Dual Core Strategy

The ESP32 has two cores (PRO_CPU and APP_CPU). Recommended task distribution:

**Core 0 (PRO_CPU):** WiFi stack, TCP/IP, APRS-IS client
- ESP-IDF WiFi runs here by default
- Keep network I/O on this core

**Core 1 (APP_CPU):** Audio processing, modem, protocol
- I2S DMA interrupts
- AFSK demodulation
- AX.25/APRS parsing
- KISS interface

This separation keeps the real-time audio processing isolated from the
non-deterministic WiFi stack.

## GPIO Pin Assignments (Suggested)

```
ESP32 Pin    Function           Notes
─────────    ────────           ─────
GPIO25       I2S BCK            Bit clock
GPIO26       I2S WS             Word select (L/R clock)
GPIO22       I2S DATA IN        Receive audio
GPIO21       I2S DATA OUT       Transmit audio
GPIO19       PTT Output         Push-to-talk control
GPIO18       Carrier Detect     Optional, from radio
GPIO5        Status LED         Activity indicator
GPIO4        TX LED             Transmit indicator
GPIO2        Built-in LED       Heartbeat
```

## Flashing and Debugging

```bash
# Build
cd esp32
cargo build --release

# Flash
espflash flash target/xtensa-esp32-espidf/release/packet-radio-esp32

# Monitor serial output
espflash monitor

# Combined flash + monitor
espflash flash --monitor target/xtensa-esp32-espidf/release/packet-radio-esp32
```

### Debugging with probe-rs

```bash
cargo install probe-rs-tools

# If you have a JTAG debugger connected:
probe-rs run --chip esp32
```

## Common Pitfalls

1. **Stack overflow**: Rust on ESP-IDF defaults to 8KB stack. Deep call chains
   (especially with APRS parsing) may need more. Increase in sdkconfig.

2. **WiFi + I2S DMA conflicts**: Both use DMA. Ensure they're on different
   DMA channels. ESP-IDF handles this automatically in most cases.

3. **ADC noise**: The ESP32's built-in ADC is notoriously noisy. If using it
   for audio input, oversample (read at 4x rate) and average.

4. **Power supply**: WiFi transmit spikes can cause brownouts on cheap USB
   power supplies. Use a supply rated for 500mA+.

5. **GPIO matrix**: Not all GPIOs support all functions. Check the ESP32
   technical reference manual for I2S-capable pins.

6. **Watchdog timer**: Long-running DSP loops can trigger the watchdog.
   Either feed it periodically or increase the timeout in sdkconfig.
