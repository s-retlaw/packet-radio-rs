# HARDWARE_DEVICE.md — Dual-Mode TNC / USB Sound Card Device

## Concept

A single device that serves two roles:

1. **Standalone TNC Mode** — Runs our Rust packet decoder on-device.
   Radio audio goes in, KISS packets come out over USB serial, Bluetooth,
   or WiFi. No PC-side software needed beyond an APRS app.

2. **USB Sound Card Mode** — Presents as a standard USB Audio device.
   The PC sees a sound card and a serial port. Dire Wolf (or our own
   desktop decoder) runs on the PC using this device for radio I/O.
   PTT control via the serial port.

3. **Both At Once** — The really clever option. The on-board TNC decodes
   packets from the radio AND simultaneously streams the same audio to
   the PC over USB Audio. Two decoders, same input, zero extra cost.
   For TX, pick one source (on-board modulator or USB audio from PC).

Same hardware, same radio connection, same ADC/DAC. Only the firmware
routing changes.

### Why This Matters

Nothing on the market does both:

| Product | TNC | Sound Card | Bluetooth | WiFi | Price |
|---------|-----|-----------|-----------|------|-------|
| Mobilinkd TNC4 | ✓ | ✗ | ✓ | ✗ | ~$130 |
| SignaLink USB | ✗ | ✓ | ✗ | ✗ | ~$120 |
| Digirig | ✗ | ✓ | ✗ | ✗ | ~$40 |
| NinoTNC | ✓ | ✗ | ✗ | ✗ | ~$55 |
| DRAWS (Pi HAT) | ✗ | ✓ (via Pi) | ✗ | ✗ | ~$90 |
| **Our device** | **✓** | **✓** | **✓** | **✓** | **~$25-30** |

---

## Platform Choice

### Recommended: ESP32-S3

| Feature | ESP32-S3 | Pico 2 W (RP2350) |
|---------|----------|-------------------|
| CPU | Dual Xtensa LX7 @ 240 MHz | Dual Cortex-M33 @ 150 MHz |
| FPU | Yes (single precision) | Yes (single precision) |
| RAM | 512 KB SRAM + opt. 2-8 MB PSRAM | 520 KB SRAM |
| USB | USB OTG (device + host) | USB 1.1 device (native) |
| USB Audio Class | Possible (TinyUSB) | Better supported (TinyUSB) |
| Bluetooth Classic (SPP) | **Yes** | No |
| BLE | Yes | Yes |
| WiFi | Yes | Yes |
| I2S | Native peripheral | Via PIO (works well) |
| Price (DevKit) | ~$8 | ~$7 |
| Rust support | esp-hal (good, improving) | embassy-rp (excellent) |

**ESP32-S3 wins overall** because Bluetooth Classic SPP is essential for
compatibility with existing apps (APRSdroid, etc.). The Pico 2 W only
has BLE, which limits mobile app compatibility.

**Pico 2 W wins on USB Audio** — the RP2350's USB stack with TinyUSB is
more battle-tested for isochronous audio transfers. If sound card mode
is the primary use case, it's the better choice.

**For a first prototype, use the ESP32-S3.** The Pico 2 W can be a
second target later — our core decoder is platform-independent no_std
Rust, so it runs on either.

### Recommended Dev Board

**ESP32-S3-DevKitC-1-N8R8** (~$8-12 on Amazon)
- 8 MB flash, 8 MB PSRAM (plenty for audio buffers + TNC)
- USB-C connector with native USB OTG
- Built-in antenna for WiFi + Bluetooth
- All GPIO broken out

---

## Audio Codec

### Option A: WM8960 Module (Easiest, ~$8-19)

A single-chip stereo codec with both ADC and DAC on one I2S bus.

**Waveshare WM8960 Audio Board:**
- Amazon (~$19): https://www.amazon.com/Audio-Supports-encoding-Recording-Interface/dp/B07H6FNFDD
- Direct from Waveshare (~$7-8): https://www.waveshare.com/wm8960-audio-board.htm
- AliExpress (~$5-8): Search "WM8960 audio module I2S"

**Specs:**
- ADC: 24-bit, SNR 94 dB — vastly overkill for packet radio
- DAC: 24-bit, SNR 98 dB
- Sample rates: 8, 11.025, 22.05, 44.1, 48 kHz
- Control: I2C (address 0x1A)
- Audio: I2S
- Onboard: MEMS mic, 3.5mm jack, speaker driver (1W/ch)
- Line-level analog inputs (LINPUT1/RINPUT1) — this is what we
  connect the radio's audio output to

**Advantages:**
- Single chip, single I2S bus for both ADC and DAC
- I2C programmable gain — software-controlled input level
- Built-in ALC (automatic level control) — useful for varying
  signal levels from different radios
- 3.5mm jack already on-board for quick prototyping
- Extensive ESP32 + Arduino/ESP-IDF examples available

**Disadvantages:**
- $19 on Amazon is steep for what it is
- The MEMS mic on the board isn't useful for us (we need line input)
- Board has more stuff on it than we need

### Option B: Separate DAC + ADC (Cheapest, ~$5-6)

**PCM5102A I2S DAC** (~$2-3 on Amazon/AliExpress)
- 32-bit, 384 kHz capable, SNR 112 dB
- I2S input, line-level analog output
- Tiny breakout boards widely available
- Drives headphones or line-level directly

**INMP441 I2S MEMS Microphone** (~$2-3)
- 24-bit digital output, I2S interface
- SNR 61 dB, sensitivity -26 dBFS
- Problem: This is a microphone, not a line-level input.
  Radio discriminator output is line-level (~100 mV to 1V pp).
  You'd need a voltage divider or op-amp to match levels, and
  signal quality may suffer.

**Better ADC alternative: External I2S ADC**
- **PCM1808** or **CS5343** — proper line-level I2S ADCs
- Harder to find as breakout boards
- Would need a custom PCB

**Verdict:** Option B is cheaper but the ADC situation is awkward.
The WM8960 is better because it has real analog line inputs with
programmable gain, which is exactly what you need for radio audio.

### Option C: ESP32-S3 Built-in ADC + External I2S DAC (~$3)

- Use the ESP32-S3's built-in 12-bit SAR ADC for RX
- Use a PCM5102A I2S DAC for TX
- Cheapest possible option

**Problems:**
- ESP32 ADC is noisy — maybe 9-10 effective bits (ENOB)
- No DMA-to-ADC at audio rates without workarounds
- Non-linear at the extremes of its range
- Probably good enough for 1200 baud AFSK, not for 9600 baud

**Verdict:** Quick and dirty for prototyping 1200 baud. Not
recommended for a real product or for 9600 baud.

### Recommendation

**Start with Option A (WM8960 module)** for prototyping. It works out
of the box with I2S, has proper line inputs, and there are ESP32
examples everywhere. If the device becomes a product, design a custom
PCB with the WM8960 chip (~$3 on LCSC) and skip the breakout board.

---

## Hardware Connections

### ESP32-S3 ↔ WM8960 (I2S + I2C)

```
ESP32-S3                WM8960 Module
─────────               ─────────────
GPIO 4  ──────────────── BCLK        (I2S bit clock)
GPIO 5  ──────────────── LRCLK/WS    (I2S word select / L-R clock)
GPIO 6  ──────────────── DOUT/ADCDAT (I2S data from ADC — radio RX)
GPIO 7  ──────────────── DIN/DACDAT  (I2S data to DAC — radio TX)
GPIO 8  ──────────────── SDA         (I2C data — codec control)
GPIO 9  ──────────────── SCL         (I2C clock — codec control)
3.3V    ──────────────── 3.3V
GND     ──────────────── GND
5V (USB)──────────────── VIN         (speaker amp power, if used)
```

### ESP32-S3 ↔ Radio

```
WM8960 LINPUT1 ←────── Radio speaker / data-out jack
                        (via voltage divider if needed,
                         radio output is typically 100mV-1Vpp,
                         WM8960 line input max is ~1Vpp)

WM8960 LOUT1   ──────→ Radio mic / data-in jack
                        (via attenuator — radio mic input
                         expects ~10-50mV for proper deviation)

ESP32 GPIO 10  ──────→ 2N7000 gate ──→ Radio PTT line
                        (open-drain, pulls PTT to ground)
```

### PTT Circuit

```
ESP32 GPIO 10 ───[1kΩ]───┬─── 2N7000 Gate
                          │
                    [10kΩ to GND]  (pull-down, PTT off at boot)
                          │
Radio PTT ────────────── 2N7000 Drain
                          │
                      2N7000 Source ─── GND
```

When GPIO 10 goes high, the 2N7000 pulls the radio's PTT line to
ground, keying the transmitter.

### Full Wiring Diagram (ASCII)

```
┌─────────────┐          ┌──────────────┐         ┌──────────┐
│             │   I2S    │              │  Audio   │          │
│  ESP32-S3   ├─────────→│   WM8960     ├────────→│  Radio   │
│             │←─────────┤   Codec      │←────────┤  (VHF/   │
│             │   I2C    │   Module     │         │   UHF)   │
│             ├─────────→│              │         │          │
│             │          └──────────────┘         │          │
│             │                                    │          │
│             │──── GPIO 10 ──→ [2N7000] ────────→│ PTT      │
│             │                                    │          │
│    USB-C    │◄═══════════════════════════════════│          │
│  (to PC)    │  USB Audio + USB CDC Serial        └──────────┘
│             │
│   WiFi/BT   │◄─── antenna (built-in)
└─────────────┘
```

---

## Firmware Architecture

### Core Principle

Audio samples flow through a DMA ring buffer. Everything downstream
reads from that same buffer. The "mode" just determines who consumes
the samples.

```
Radio → WM8960 ADC → I2S DMA Ring Buffer
                           │
                           ├─→ TNC Decoder (on-device) → KISS frames
                           │                                │
                           │                      ┌─────────┤
                           │                      ▼         ▼
                           │                USB CDC    BLE/SPP
                           │                Serial     (KISS)
                           │
                           └─→ USB Audio Isochronous Endpoint → PC
                               (sound card mode)
```

### Mode Selection

Three firmware modes, selectable at runtime via USB command or at boot
via GPIO pin:

| Mode | On-device TNC | USB Audio | USB Serial | BT/WiFi |
|------|:---:|:---:|:---:|:---:|
| **TNC Only** | ✓ decode + encode | ✗ | KISS | KISS, APRS-IS |
| **Sound Card Only** | ✗ | ✓ RX + TX | PTT control | ✗ |
| **Dual (default)** | ✓ decode only | ✓ RX only | KISS + PTT | KISS, APRS-IS |

In Dual mode, TX is a policy decision: either the on-board modulator
generates TX audio from KISS input, or USB Audio from the PC is routed
to the DAC. A USB serial command selects which.

### Task/Core Allocation (ESP32-S3 Dual Core)

```
Core 0: Communications                Core 1: DSP
─────────────────────                  ──────────
USB stack (TinyUSB)                    I2S DMA management
  - Audio Class (isochronous)          Audio sample processing
  - CDC Serial (KISS/config)           Demodulator pipeline
Bluetooth (SPP + BLE)                  Modulator (TX)
WiFi (APRS-IS TCP client)             Clock recovery PLL
Configuration management              HDLC/AX.25 framing
```

This split keeps the timing-critical DSP on one core and all the
interrupt-heavy communication stacks on the other.

### USB Composite Device

The device presents as a USB Composite device with multiple interfaces
on a single USB cable:

```
USB Device Descriptor
├── Configuration Descriptor
│   ├── Interface 0: Audio Control (UAC2)
│   │   └── Clock Source, Input Terminal, Output Terminal
│   ├── Interface 1: Audio Streaming IN (mic — radio RX to PC)
│   │   └── Endpoint: Isochronous IN
│   │       Format: 16-bit PCM, mono, 22050 Hz
│   ├── Interface 2: Audio Streaming OUT (speaker — PC TX to radio)
│   │   └── Endpoint: Isochronous OUT
│   │       Format: 16-bit PCM, mono, 22050 Hz
│   ├── Interface 3: CDC ACM (virtual COM port — KISS + PTT)
│   │   ├── Endpoint: Bulk IN
│   │   └── Endpoint: Bulk OUT
│   └── (optional) Interface 4: CDC ACM #2 (debug/logging)
└── String Descriptors
    ├── Manufacturer: "Packet Radio Project"
    ├── Product: "Packet Radio TNC / Audio Interface"
    └── Serial: <unique device ID>
```

The PC sees:
- A sound card named "Packet Radio TNC" with mono input + output
- A COM port (e.g., /dev/ttyACM0 or COM3)
- No drivers needed — standard USB Audio and CDC classes

TinyUSB supports this composite configuration on both ESP32-S3 and
RP2040/RP2350 with working examples.

### USB Audio Parameters

For packet radio, we need minimal bandwidth:

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Sample rate | 22050 Hz | Standard rate, sufficient for all modes up to 9600 baud |
| Bit depth | 16-bit signed | Matches WAV files, plenty of dynamic range |
| Channels | 1 (mono) | Packet radio is mono |
| USB bandwidth | ~44 KB/s each direction | Trivial for USB 1.1 (12 Mbps) |

22050 Hz at 16-bit mono = 44,100 bytes/sec. USB Full Speed (12 Mbps)
isochronous bandwidth handles this easily with room to spare.

For 9600 baud G3RUH, we need response to ~5 kHz, so 22050 Hz sampling
is the minimum. 44100 Hz would give more headroom but doubles the USB
bandwidth and DMA buffer sizes.

### Bluetooth Profiles

**Bluetooth Classic SPP (Serial Port Profile):**
- Used for KISS TNC data — same as Mobilinkd
- Compatible with APRSdroid, APRS.fi, PinPoint APRS, etc.
- ~115200 baud virtual serial — plenty for KISS at 1200 baud
- Range: ~10-30 meters typical

**BLE (Low Energy):**
- Alternative for KISS data with modern devices
- Lower power, better for battery operation
- Less universal app support than SPP
- Custom GATT service with TX/RX characteristics

**Not using A2DP or HFP for audio** — Bluetooth audio profiles use
lossy compression and have too much latency for packet radio.
Sound card mode is USB only.

### WiFi Capabilities

- **APRS-IS gateway** — connect to APRS-IS servers (rotate.aprs2.net)
  and relay received packets to the internet
- **TCP KISS server** — accept KISS connections over WiFi (like Dire Wolf)
- **Web configuration UI** — serve a small web page for device setup
  (callsign, SSID, WiFi credentials, audio levels, mode selection)
- **OTA firmware update** — update firmware over WiFi

---

## Audio Path Details

### RX Path (Radio → Device)

```
Radio speaker/data jack
    │
    ▼
Voltage divider (if needed, match to ~1Vpp max)
    │
    ▼
WM8960 LINPUT1 (line input)
    │
    ▼
WM8960 PGA (programmable gain, set via I2C)
    │
    ▼
WM8960 ADC (24-bit, but we use 16-bit I2S format)
    │
    ▼
I2S DMA → Ring buffer in ESP32-S3 SRAM
    │
    ├──→ Demodulator (on-device TNC)
    └──→ USB Audio IN endpoint (sound card mode)
```

The WM8960's programmable gain amplifier (PGA) is a key advantage —
we can adjust input sensitivity in software to match different radios
without changing hardware. Range is -17.25 dB to +30 dB in 0.75 dB
steps.

### TX Path (Device → Radio)

```
KISS frame from USB/BLE/WiFi
    │
    ▼
AX.25 framer → HDLC encoder → NRZI → Bit stream
    │
    ▼
AFSK Modulator (1200 baud) or GFSK Modulator (9600 baud)
    │
    ▼
I2S DMA ← Sample buffer
    │
    ▼
WM8960 DAC
    │
    ▼
WM8960 LOUT1 (line output)
    │
    ▼
Attenuator (radio mic input expects ~10-50 mV)
    │
    ▼
Radio mic/data jack

--- OR (in sound card mode) ---

PC application (Dire Wolf, fldigi, etc.)
    │
    ▼
USB Audio OUT endpoint
    │
    ▼
I2S DMA ← USB sample buffer
    │
    ▼
WM8960 DAC → Radio
```

### Level Matching

**RX (Radio → Codec):**
Most radio speaker outputs are 0.5-2 Vpp at full volume. The WM8960
line input handles up to ~1 Vpp with 0 dB PGA gain. A simple 2:1
resistive divider (10kΩ + 10kΩ) handles hot signals. The PGA can
compensate for the rest.

For radios with a "data" or "packet" 6-pin mini-DIN connector, the
discriminator output is typically 200-600 mVpp — well within the
WM8960 line input range without any divider.

**TX (Codec → Radio):**
Radio microphone inputs expect ~5-50 mV for proper deviation. The
WM8960 line output is ~1 Vpp full scale. A resistive attenuator
(e.g., 100kΩ + 4.7kΩ → ~22:1 ratio → ~45 mVpp) matches the level.
Fine-tune the WM8960 DAC volume register via I2C for precise
deviation control.

For radios with a 6-pin data connector, the modulator input
sensitivity varies. The WM8960 DAC output with software volume
control can accommodate the range.

---

## Prototype Build Steps

### Phase 1: Audio I/O Verification

**Goal:** Confirm I2S communication between ESP32-S3 and WM8960.

1. Wire ESP32-S3 DevKit to WM8960 module (6 wires + power)
2. Write minimal firmware: init I2C, configure WM8960 registers,
   start I2S DMA, read ADC samples, write to DAC (loopback)
3. Connect a signal generator (or phone playing a tone) to input
4. Verify output matches input on oscilloscope or by ear
5. Verify sample rates: 22050 Hz and 44100 Hz

**Framework:** ESP-IDF with esp-idf-hal Rust bindings, or start with
Arduino + ESP32-WM8960 library for quick validation, then port to Rust.

### Phase 2: TNC Mode

**Goal:** Decode real APRS packets from radio audio.

1. Connect radio speaker/data output to WM8960 line input
2. Run our Rust demodulator on Core 1
3. Output decoded KISS frames on USB CDC serial
4. Test with a real radio on 144.390 MHz
5. Compare packet count against Dire Wolf on same audio

### Phase 3: USB Sound Card Mode

**Goal:** PC sees a USB sound card, Dire Wolf works.

1. Implement TinyUSB UAC2 device (audio class)
2. Route I2S DMA samples to USB isochronous IN endpoint
3. Route USB isochronous OUT samples to I2S DMA for DAC
4. Implement USB CDC for PTT control (RTS line or AT commands)
5. Test with Dire Wolf: `ADEVICE Packet Radio TNC`
6. Verify PTT works with Dire Wolf PTT configuration

### Phase 4: USB Composite (Audio + Serial)

**Goal:** Both audio and serial on one USB cable.

1. Configure TinyUSB for composite device (UAC2 + CDC)
2. Verify both interfaces enumerate and work simultaneously
3. Test: Dire Wolf uses audio interface, KISS app uses serial

### Phase 5: Bluetooth

**Goal:** Wireless KISS for mobile use.

1. Implement Bluetooth Classic SPP server
2. Implement KISS protocol over SPP
3. Test with APRSdroid on Android
4. Add BLE KISS service as alternative

### Phase 6: Dual Mode

**Goal:** On-device TNC + USB sound card simultaneously.

1. DMA ring buffer shared between TNC decoder and USB Audio
2. TNC decoded packets go to USB CDC serial and/or Bluetooth
3. Same audio simultaneously available as USB Audio input
4. PC can run Dire Wolf on same audio for comparison
5. Add mode-switch commands over USB serial

### Phase 7: WiFi & Polish

**Goal:** APRS-IS gateway, web config, OTA updates.

1. WiFi station mode, connect to home/field network
2. APRS-IS client — relay received packets
3. TCP KISS server (port 8001, like Dire Wolf)
4. Web-based configuration page
5. OTA firmware update mechanism

---

## Bill of Materials (Prototype)

| Part | Source | Price |
|------|--------|-------|
| ESP32-S3-DevKitC-1-N8R8 | Amazon | ~$10 |
| Waveshare WM8960 Audio Module | Amazon / AliExpress | ~$8-19 |
| 2N7000 N-channel MOSFET | Any | ~$0.20 |
| 10kΩ resistors (×3) | Any | ~$0.10 |
| 1kΩ resistor | Any | ~$0.05 |
| 4.7kΩ resistor (TX attenuator) | Any | ~$0.05 |
| 100kΩ resistor (TX attenuator) | Any | ~$0.05 |
| 3.5mm TRS jacks (×2) | Any | ~$1.00 |
| Breadboard + jumper wires | Any | ~$3.00 |
| **Total (prototype)** | | **~$22-33** |

### For a Custom PCB (Future)

| Part | Source | Price (qty 10) |
|------|--------|----------------|
| ESP32-S3-WROOM-1 module | LCSC | ~$3.50 |
| WM8960 codec IC | LCSC | ~$2.80 |
| Supporting passives | LCSC | ~$1.00 |
| USB-C connector | LCSC | ~$0.30 |
| 3.5mm jacks (×2) | LCSC | ~$0.60 |
| 2N7000 + resistors | LCSC | ~$0.20 |
| PCB (JLCPCB, 5 pcs) | JLCPCB | ~$2.00 each |
| **Total (per board)** | | **~$10-12** |

---

## Radio Connector Options

### Standard Approach: 3.5mm Jacks

Two 3.5mm jacks (one RX audio in, one TX audio out) plus a separate
PTT connection. Simple, universal, works with any radio via adapter
cables. This is what the SignaLink and Digirig use.

### Better: 6-Pin Mini-DIN

The standard "data" connector found on most modern ham radios
(Kenwood, Yaesu, Icom). Carries RX audio (discriminator), TX audio
(modulator input), PTT, and ground on one connector. Ideal for
9600 baud because it bypasses the radio's audio filtering.

### Best: Both

Put a 6-pin mini-DIN on the board as the primary interface, plus
a 3.5mm jack as a secondary/fallback. Different radios, different
cables — flexibility matters in the field.

### Cable Pinouts (6-Pin Mini-DIN)

Varies by manufacturer, but the most common (Kenwood standard):

| Pin | Function | Connect To |
|-----|----------|-----------|
| 1 | Data In (TX audio to radio) | WM8960 LOUT1 via attenuator |
| 2 | GND | GND |
| 3 | PTT | 2N7000 drain |
| 4 | RX Audio (discriminator out) | WM8960 LINPUT1 |
| 5 | GND | GND |
| 6 | SQL (squelch status) | ESP32 GPIO (optional) |

---

## Software Components Needed

### ESP32-S3 Firmware (Rust)

| Component | Crate / Library | Status |
|-----------|----------------|--------|
| I2S driver | esp-idf-hal / esp-hal | Available |
| I2C driver (WM8960 control) | esp-idf-hal / esp-hal | Available |
| USB stack | TinyUSB (via C FFI) or esp-hal USB | Available |
| USB Audio Class | TinyUSB UAC2 | Examples exist |
| USB CDC Serial | TinyUSB CDC | Examples exist |
| Bluetooth SPP | esp-idf-sys (bluedroid) | Available |
| BLE | esp-idf-sys or trouble (BLE crate) | Available |
| WiFi | esp-idf-svc / esp-wifi | Available |
| Our TNC decoder | packet-radio-core (no_std) | We build this |
| Our modulator | packet-radio-core (no_std) | We build this |
| KISS protocol | packet-radio-core (no_std) | We build this |
| AX.25 framing | packet-radio-core (no_std) | We build this |
| WM8960 register driver | Write our own (~200 lines) | Simple I2C register map |
| Web config server | esp-idf-svc HTTP | Available |
| APRS-IS client | Write our own (~300 lines) | Simple TCP text protocol |

### PC Software (Existing, no changes needed)

| Software | How It Uses Our Device |
|----------|-----------------------|
| Dire Wolf | Selects our USB Audio as sound card, uses COM port for PTT |
| APRSdroid | Connects via Bluetooth SPP for KISS |
| Xastir | USB serial KISS or TCP KISS over WiFi |
| YAAC | USB serial KISS or TCP KISS |
| aprx | TCP KISS over WiFi (Linux APRS igate) |
| PinPoint APRS | Bluetooth SPP KISS |

No custom PC software required. The whole point of using standard
USB Audio Class and standard KISS protocol is that everything just
works with existing software.

---

## Stretch Goals

### AIS Receiver
Ship Automatic Identification System on 161.975 / 162.025 MHz.
9600 baud GMSK, similar enough to G3RUH that the same hardware
works. Dire Wolf already decodes AIS. Our device in sound card
mode with an appropriate radio/SDR can receive AIS.

### FX.25 / IL2P Forward Error Correction
Add FEC encoding/decoding to the on-board TNC. Transparent to
existing AX.25 systems but dramatically improves reliability.

### Multi-Channel
The WM8960 is stereo — we could use left channel for one radio
and right channel for another. Two radios, one device. Cross-band
digipeater in a single box.

### EAS/SAME Weather Alerts
Emergency Alert System decoding. Dire Wolf already does this.
Our device could decode weather alerts from NOAA Weather Radio
and forward them over WiFi/Bluetooth.

---

## References

- TinyUSB: https://github.com/hathach/tinyusb
- TinyUSB UAC2 example: https://github.com/hathach/tinyusb/tree/master/examples/device/audio_4_channel_mic
- ESP32-S3 USB: https://docs.espressif.com/projects/esp-idf/en/latest/esp32s3/api-reference/peripherals/usb_device.html
- WM8960 Datasheet: https://www.cirrus.com/products/wm8960/
- Mobilinkd TNC (reference design): https://github.com/mobilinkd
- SparkFun WM8960 Hookup Guide: https://learn.sparkfun.com/tutorials/audio-codec-breakout---wm8960-hookup-guide/all
- USB Audio Class 2.0 spec: https://www.usb.org/document-library/usb-audio-devices-rev-30-and-adopters-agreement
- Kenwood 6-pin mini-DIN pinout: Varies by model, consult radio manual
