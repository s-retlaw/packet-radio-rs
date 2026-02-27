# BLE KISS TNC Specification

## Reference
- Spec: https://github.com/hessu/aprs-specs/blob/master/BLE-KISS-API.md
- Reference impl: Mobilinkd TNC3 (http://www.mobilinkd.com/)
- Example code: https://github.com/tomasgeci/kiss-ble-uart-tnc

## UUIDs

```
KTS_SERVICE_UUID:  00000001-ba2a-46c9-ae49-01b0961f68bb
KTS_TX_CHAR_UUID:  00000002-ba2a-46c9-ae49-01b0961f68bb  (app → TNC, Write)
KTS_RX_CHAR_UUID:  00000003-ba2a-46c9-ae49-01b0961f68bb  (TNC → app, Notify)
```

## How It Works

- TNC advertises the service UUID; phone scans for it
- KISS framing is identical to serial KISS — FEND delimiters, same escaping
- Frames may span multiple BLE transfer units (MTU typically 20-244 bytes)
- Receiver concatenates chunks until a complete KISS frame (FEND-to-FEND) is assembled
- Both directions: treat it like a serial byte stream, just chunked by BLE MTU

## Frame Size Limits

- Max AX.25 frame: 329 bytes (17-byte header + 56 digipeater bytes + 256-byte info)
- Max KISS-encoded: 660 bytes (worst case, every byte escaped)
- TNC must buffer partial frames from BLE writes before KISS-decoding

## Client Compatibility

- **APRSdroid** (Android): TNC (KISS) Protocol → Bluetooth Low Energy
- **iOS apps**: BLE is the ONLY way (Apple blocks classic BT SPP from apps)
- **Mobilinkd TNC3**: Reference device that implements this spec

## Integration with Our TncEngine

```
Radio → ADC → MiniDecoder → TncEngine
                                ├→ read_kiss() → BLE RX notify → Phone
                                ├→ read_kiss() → WiFi TCP → APRS-IS
                                └→ feed_kiss() ← BLE TX write ← Phone
```

- TncEngine already speaks KISS: `feed_kiss(byte)` for input, `read_kiss(&mut buf)` for output
- BLE layer just shuttles KISS bytes over the air instead of USB-CDC
- Need to fan out read_kiss() to multiple consumers (BLE + WiFi + USB)
  or use separate TNC instances per output path

## ESP32-S3 Implementation Notes

- ESP32-S3 runs WiFi + BLE concurrently (radio time-slicing, ~10-15% WiFi throughput cost)
- Rust BLE stack: `esp-wifi` + `bt-hci` / `trouble` crate (GATT server)
- Need to set up GATT server with the 3 UUIDs above
- BLE notify has max payload = negotiated MTU - 3 bytes (typically 20 bytes default, 244 after MTU negotiation)
- Should request MTU upgrade to minimize fragmentation of KISS frames

## Pico 2 W Notes

- CYW43439 supports BLE, Rust driver via `cyw43` + `bt-hci`
- Less mature than ESP32 BLE ecosystem
- WiFi + BLE concurrent operation supported by firmware
