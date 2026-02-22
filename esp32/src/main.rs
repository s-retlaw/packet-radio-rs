//! ESP32 Packet Radio Firmware
//!
//! Standalone WiFi-enabled IGate and TNC running on ESP32.
//! See docs/ESP32_GUIDE.md for hardware setup and build instructions.

fn main() {
    println!("Packet Radio TNC — ESP32");
    println!("========================");
    println!();
    println!("This crate requires the ESP32 Rust toolchain.");
    println!("See docs/ESP32_GUIDE.md for setup instructions.");
    println!();
    println!("Build with: cd esp32 && cargo build --target xtensa-esp32-espidf");
}

// TODO: ESP32 implementation plan:
//
// 1. Initialize ESP-IDF runtime
// 2. Configure I2S audio (or ADC/DAC)
// 3. Connect to WiFi
// 4. Start audio capture → demodulator → frame output pipeline
// 5. Forward decoded APRS packets to APRS-IS via WiFi
// 6. Optionally accept KISS connections over WiFi TCP
// 7. Status LED indicators
//
// See docs/ESP32_GUIDE.md for:
// - Pin assignments
// - I2S codec wiring
// - WiFi configuration
// - Dual-core task distribution
