
 RP2040 Live ADC Decode — APRS from Handheld Radio

 Context

 The RP2040 test harness currently receives audio over USB from a host tool.
 With only 12.1% CPU utilization at 125 MHz, the Pico has massive headroom to
 sample audio directly from a radio via its built-in 12-bit ADC and decode APRS
 packets in real-time — no host computer needed during operation.

 Hardware Circuit

 Radio Audio Out                         RP2040 Pico
 (3.5mm jack tip) ────┤├────────┬─────── GPIO26 (ADC0, Pin 31)
                   C1=100nF     │
                             R1=47k
                                │
                         3.3V ──┤
                             R2=47k
                                │
                               GND

 Radio GND ──────────────────────────── GND (Pin 33 or 38)

 Components (5 total):
 - C1 = 100 nF — AC coupling, blocks DC from radio (HP corner ~34 Hz with 47k)
 - R1 = R2 = 47 kohm — bias ADC input to ~1.65V mid-rail (low impedance minimizes ADC loading)
 - Radio audio is ~300-600 mV p-p → ~150-300 ADC counts p-p, plenty for AFSK demod

 If radio output is too hot (>1V p-p), add a 10k series resistor before C1.

 ADC Configuration

 - ADC clock: 48 MHz
 - Target sample rate: 11025 Hz
 - Clock divider: int=4352, frac=190 → actual 11024.9 Hz (0.001% error)
 - 12-bit unsigned (0–4095) → convert to i16: (raw as i16) - 2048
 - Use ADC FIFO mode (8-deep hardware FIFO, polled — no DMA or interrupts needed)


