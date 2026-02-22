# Hardware Reference

## Audio Interface Options by Platform

### Desktop / Raspberry Pi

**USB Sound Card (recommended for desktop)**
- Any USB sound card supported by ALSA (Linux) or CoreAudio (macOS)
- SignaLink USB — purpose-built for ham radio, includes isolation and level control
- Digirig Mobile — compact, designed for digital modes
- Cheap CM108-based USB dongles ($2-5) work fine for receive

**I2S HAT (Raspberry Pi)**
- Audio Injector Stereo HAT
- HiFiBerry DAC+ADC
- Any I2S codec board connected to the Pi's GPIO header

**Connection to radio:**
- Audio out from radio → sound card line in (receive)
- Sound card line out → radio mic/data in (transmit)
- PTT control via serial port RTS/DTR, GPIO, or VOX

---

### ESP32

**Option 1: External I2S Codec (recommended)**
- **PCM5102** DAC module ($2-3) — for transmit audio
- **INMP441** I2S MEMS microphone ($2) — for receive audio
- **WM8731** codec ($8-10) — full duplex ADC+DAC on one chip
- **MAX98357** I2S amp — if driving a speaker for monitoring

Wiring (PCM5102 example):
```
ESP32          PCM5102
GPIO25 ──────► BCK (Bit Clock)
GPIO26 ──────► LRCK (Word Select)
GPIO22 ──────► DIN (Data)
3.3V ────────► VIN
GND ─────────► GND
               SCK → GND (use internal clock)
```

**Option 2: Built-in ADC/DAC**
- DAC: GPIO25 (DAC1) or GPIO26 (DAC2) — 8-bit, adequate for 1200 baud TX
- ADC: Any ADC-capable GPIO — 12-bit, noisy but usable for RX with filtering
- Pro: Zero additional hardware cost
- Con: Lower quality, may need external op-amp filtering

**Option 3: ESP32-S3 USB Audio**
- ESP32-S3 has USB-OTG host capability
- Could potentially drive a USB sound card
- More complex, less tested

**Radio connection:**
- Audio interface → radio data port (typically 3.5mm or 6-pin mini-DIN)
- PTT via GPIO → transistor/optocoupler → radio PTT line
- Level shifting may be needed (ESP32 is 3.3V, some radios expect different levels)

**Recommended ESP32 boards:**
- ESP32-DevKitC — cheap, widely available, all GPIOs exposed
- ESP32-S3-DevKitC — USB host, more RAM, better for future expansion
- ESP32-C3-DevKitM — cheapest, RISC-V, but no FPU

---

### STM32

**I2S Codec (recommended)**
- WM8731 or CS4344 connected via I2S
- STM32F4 has dedicated I2S peripherals with DMA
- This is the cleanest embedded audio setup

**ADC/DAC:**
- STM32F4 has 12-bit ADC (up to 2.4 Msps) and 12-bit DAC
- DMA-driven for continuous streaming
- Quality is good enough for 1200 baud

**Recommended boards:**
- WeAct STM32F411 "Black Pill" ($3) — great starting point
- STM32F446 Nucleo — more peripherals, Arduino-compatible headers
- STM32H743 Nucleo — overkill but great for development

---

### RP2040

**PIO-based I2S:**
- RP2040 doesn't have hardware I2S, but PIO can implement it
- Connect any I2S codec (PCM5102, WM8731, etc.)
- `pio-i2s` crate or implement from PIO assembly

**ADC:**
- 12-bit ADC, 500 ksps, 4 channels
- Adequate for receive with DMA
- No built-in DAC — need external (PWM + filter, or I2S codec)

**Recommended boards:**
- Raspberry Pi Pico ($4)
- Adafruit Feather RP2040 ($12)
- Pimoroni Tiny 2040 ($10)

---

## PTT (Push-To-Talk) Control

PTT must be asserted before transmitting and released after. Options:

| Method | Platform | Notes |
|--------|----------|-------|
| GPIO → transistor → PTT | All embedded | Most common, use NPN or N-FET, optocoupler for isolation |
| Serial RTS/DTR | Desktop | Traditional, works with most radios |
| CAT control | Desktop | Radio-specific serial commands |
| VOX | Any | Radio's built-in voice activation, simplest but adds delay |
| CM108 GPIO | Desktop | USB sound cards with CM108 chip have GPIO pins |

**GPIO PTT circuit (embedded):**
```
MCU GPIO ──── 1kΩ ────┬──── Base (NPN like 2N2222)
                       │
                      10kΩ
                       │
                      GND

Radio PTT ─────────── Collector
Radio GND ─────────── Emitter
```

Or with an optocoupler (PC817) for full electrical isolation between the MCU
and radio.

## Power Considerations

**ESP32:**
- Typical consumption: 80-240 mA (WiFi active: ~150 mA)
- Can be USB powered or 3.7V LiPo with onboard regulator
- Sleep modes available but not useful during active TNC operation

**STM32F4:**
- Typical consumption: 30-100 mA
- Low power modes useful for battery-operated KISS TNC
- Could wake on carrier detect

**RP2040:**
- Typical consumption: 25-50 mA
- Very efficient for a continuously running TNC

## Recommended Bill of Materials

### Minimal ESP32 IGate (~$15)

| Component | Est. Cost |
|-----------|----------|
| ESP32-DevKitC | $5 |
| INMP441 I2S microphone (RX) | $2 |
| PCM5102 DAC (TX) | $3 |
| 2N2222 + resistors (PTT) | $0.50 |
| 3.5mm cables/connectors | $2 |
| Breadboard + wires | $3 |

### Minimal KISS TNC with STM32 (~$8)

| Component | Est. Cost |
|-----------|----------|
| STM32F411 Black Pill | $3 |
| INMP441 or analog mic circuit | $2 |
| Audio output (PWM + RC filter or DAC) | $1 |
| PTT transistor circuit | $0.50 |
| Connectors | $1.50 |

### Ultra-cheap RP2040 TNC (~$6)

| Component | Est. Cost |
|-----------|----------|
| Raspberry Pi Pico | $4 |
| Analog audio input circuit | $1 |
| PWM output + filter (TX) | $0.50 |
| PTT transistor | $0.50 |
