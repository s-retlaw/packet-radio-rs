# MCU Test Harness — Required Tools

Tools needed to build, flash, and test the ESP32-C3, ESP32-C6, and RP2040
test harness firmware.

## Rust Targets

```bash
# ESP32-C3 (RISC-V)
rustup target add riscv32imc-unknown-none-elf

# ESP32-C6 (RISC-V)
rustup target add riscv32imac-unknown-none-elf

# RP2040 (Cortex-M0+)
rustup target add thumbv6m-none-eabi
```

Both ESP targets also require nightly `build-std` (configured in each
crate's `.cargo/config.toml` via `[unstable] build-std = ["core"]`).

## ESP32 Tools

```bash
# espup — manages ESP32 Rust toolchains (Xtensa LLVM fork, etc.)
cargo install espup
espup install

# espflash — flash and monitor ESP32 devices
cargo install espflash
```

### Flashing

```bash
# ESP32-C3 (USB-Serial-JTAG on /dev/ttyACMx)
espflash flash --port /dev/ttyACM1 \
  esp32c3-test/target/riscv32imc-unknown-none-elf/release/esp32c3-test

# ESP32-C6 (CP2102N UART on /dev/ttyUSBx)
espflash flash --port /dev/ttyUSB0 \
  esp32c6-test/target/riscv32imac-unknown-none-elf/release/esp32c6-test
```

### Board-Specific Notes

| Board | LED Type | LED GPIO | Serial Interface |
|-------|----------|----------|-----------------|
| ESP32-C3-DevKit-RUST-1 | WS2812 (addressable RGB) | GPIO2 | USB-Serial-JTAG (`/dev/ttyACMx`) |
| ESP32-C6-DevKitC-1 v1.2 | WS2812 (addressable RGB) | GPIO8 | CP2102N UART (`/dev/ttyUSBx`) |

Both boards use WS2812 addressable LEDs driven via the RMT peripheral
(`esp-hal-smartled` crate), not simple GPIO toggle.

## RP2040 Tools

```bash
# elf2uf2-rs — converts ELF to UF2 and deploys to Pico in BOOTSEL mode
cargo install elf2uf2-rs
```

### Flashing

The RP2040 (Raspberry Pi Pico) requires BOOTSEL mode for flashing:

1. Unplug the Pico
2. Hold the **BOOTSEL** button while plugging it back in
3. The Pico mounts as a USB mass storage device (`RPI-RP2`)
4. Flash with: `elf2uf2-rs -d rp2040-test/target/thumbv6m-none-eabi/release/rp2040-test`

Or use `cargo run` from `rp2040-test/` (the runner is configured in
`.cargo/config.toml`).

### Board-Specific Notes

| Board | LED Type | LED GPIO | Serial Interface |
|-------|----------|----------|-----------------|
| Raspberry Pi Pico (v1.1) | Simple green LED | GPIO25 | USB-CDC (`/dev/ttyACMx`) |

Simple GPIO toggle — no special driver needed.

## Host Test Tool

The `esp32c3-host` crate runs on the desktop and streams WAV audio to any
of the three MCU targets over serial.

```bash
# Build
cargo build --release -p esp32c3-host

# Ping (verify connectivity)
cargo run --release -p esp32c3-host -- --port /dev/ttyACM1 --ping

# Stream WAV and compare with local decode
cargo run --release -p esp32c3-host -- \
  --port /dev/ttyACM1 \
  --wav tests/wav/03_100-mic-e-bursts-flat.wav \
  --compare

# RP2040 — skip DTR/RTS reset (USB-CDC has no reset lines)
cargo run --release -p esp32c3-host -- \
  --port /dev/ttyACM0 --no-reset --cpu-freq 125 \
  --wav tests/wav/03_100-mic-e-bursts-flat.wav

# ESP32-C6 via UART
cargo run --release -p esp32c3-host -- \
  --port /dev/ttyUSB0 --cpu-freq 160 \
  --wav tests/wav/03_100-mic-e-bursts-flat.wav
```

### Host tool flags

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `/dev/ttyACM0` | Serial port path |
| `--baud` | `921600` | UART baud rate (ESP32-C6 only) |
| `--wav` | — | WAV file to stream |
| `--ping` | — | Just verify connectivity |
| `--compare` | — | Compare MCU results with local MiniDecoder |
| `--mode` | `mini` | Decoder: `fast`, `quality`, or `mini` |
| `--cpu-freq` | `160` | MCU clock in MHz (125 for RP2040) |
| `--no-reset` | — | Skip DTR/RTS (needed for RP2040 USB-CDC) |

## Linux USB Permissions

If serial ports show up as `nobody:nobody`, add a udev rule:

```bash
# /etc/udev/rules.d/99-mcu-test.rules
# ESP32 USB-Serial-JTAG
SUBSYSTEM=="tty", ATTRS{idVendor}=="303a", ATTRS{idProduct}=="1001", MODE="0666"
# Silicon Labs CP2102N (ESP32-C6 UART bridge)
SUBSYSTEM=="tty", ATTRS{idVendor}=="10c4", ATTRS{idProduct}=="ea60", MODE="0666"
# Raspberry Pi Pico USB-CDC
SUBSYSTEM=="tty", ATTRS{idVendor}=="2e8a", MODE="0666"
# Pico BOOTSEL mode (mass storage for flashing)
SUBSYSTEM=="usb", ATTRS{idVendor}=="2e8a", ATTRS{idProduct}=="0003", MODE="0666"
```

Then reload: `sudo udevadm control --reload-rules && sudo udevadm trigger`

## Device Identification

```bash
# List connected serial devices
ls /dev/ttyACM* /dev/ttyUSB*

# Identify which device is which
for dev in /dev/ttyACM* /dev/ttyUSB*; do
  echo "=== $dev ==="
  udevadm info --name=$dev | grep -E 'ID_MODEL|ID_VENDOR'
done
```

Typical mapping:
- `/dev/ttyACM0` — RP2040 Pico (USB-CDC)
- `/dev/ttyACM1` — ESP32-C3 (USB-Serial-JTAG)
- `/dev/ttyUSB0` — ESP32-C6 (CP2102N UART)
