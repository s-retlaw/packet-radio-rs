//! Desktop Packet Radio TNC
//!
//! A full-featured TNC that runs on Linux, macOS, and Windows.
//! Uses the sound card for audio I/O and provides KISS TCP and
//! AGW interfaces for connecting to APRS client software.

fn main() {
    println!("Packet Radio TNC — Desktop");
    println!("==========================");
    println!();
    println!("TODO: Implement desktop TNC");
    println!("  1. Parse command-line arguments (clap)");
    println!("  2. Open audio device (cpal)");
    println!("  3. Start demodulator pipeline");
    println!("  4. Start KISS TCP server");
    println!("  5. Optionally connect to APRS-IS");
    println!();
    println!("For now, run `cargo test -p packet-radio-core` to test the core library.");
}

// TODO: Desktop implementation plan:
//
// mod audio;    — cpal sound card wrapper implementing SampleSource/SampleSink
// mod network;  — KISS TCP server, AGW server, APRS-IS client
//
// The main loop:
// 1. Audio callback (cpal) fills a ring buffer with samples
// 2. Processing thread reads samples, runs through demodulator(s)
// 3. Decoded frames are sent to:
//    a. KISS TCP clients
//    b. AGW clients
//    c. APRS-IS (if IGate enabled)
//    d. Console output (for monitoring)
// 4. Frames received from KISS/AGW clients are queued for TX
// 5. TX thread reads queue, modulates, sends to audio output
