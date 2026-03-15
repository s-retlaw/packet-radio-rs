//! # Packet Radio Core
//!
//! A `no_std` library providing packet radio functionality:
//! - AFSK modem (Bell 202, 1200 baud) — modulation and demodulation
//! - AX.25 protocol — HDLC framing, address parsing, frame construction
//! - APRS — packet encoding and decoding for all common types
//! - KISS — TNC protocol framing
//!
//! ## Feature Flags
//!
//! - `alloc` — Enables `Vec`/`String` in APRS parser (requires a heap allocator)
//! - `std` — Enables full standard library (implies `alloc`)
//! - `float` — Use `f32` for DSP instead of fixed-point `i16`
//! - `multi-decoder` — Support for multiple parallel demodulators
//!
//! ## Usage
//!
//! The core library is platform-agnostic. Platform-specific code (audio I/O,
//! networking) lives in the platform crates (`desktop`, `esp32`, etc.).
//!
//! ```rust,ignore
//! use packet_radio_core::modem::AfskDemodulator;
//! use packet_radio_core::ax25::HdlcDecoder;
//!
//! let mut demod = AfskDemodulator::new(DemodConfig::default_1200());
//! let mut hdlc = HdlcDecoder::new();
//!
//! // Feed audio samples through the pipeline
//! let mut bits = [0u8; 256];
//! let num_bits = demod.process_samples(&audio_samples, &mut bits);
//! // Feed bits into HDLC decoder to extract frames...
//! ```

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod modem;
pub mod ax25;
pub mod aprs;
pub mod kiss;
pub mod tnc;
#[cfg(feature = "fx25")]
pub mod fx25;

/// Maximum AX.25 frame length (bytes), per spec
pub const MAX_FRAME_LEN: usize = 330;

/// Maximum AX.25 information field length
pub const MAX_INFO_LEN: usize = 256;

/// Maximum number of digipeaters in an AX.25 frame
pub const MAX_DIGIPEATERS: usize = 8;

// ── Platform abstraction traits ──────────────────────────────────────────

/// Audio sample source — implemented per platform.
///
/// Desktop: wraps `cpal` sound card input
/// ESP32: wraps I2S DMA or ADC
/// STM32: wraps I2S/ADC peripheral
pub trait SampleSource {
    /// Fill `buf` with audio samples. Returns number of samples written.
    fn read_samples(&mut self, buf: &mut [i16]) -> usize;
}

/// Audio sample sink — implemented per platform.
pub trait SampleSink {
    /// Write audio samples for transmission. Returns number consumed.
    fn write_samples(&mut self, samples: &[i16]) -> usize;
}

/// Callback for decoded AX.25 frames.
pub trait FrameHandler {
    /// Called when a valid AX.25 frame has been decoded.
    fn on_frame(&mut self, frame: &ax25::Frame);
}
