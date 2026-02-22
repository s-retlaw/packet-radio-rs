//! AFSK Modem — Bell 202 modulation and demodulation
//!
//! This module provides the core DSP for 1200 baud AFSK as used in APRS and
//! packet radio. Two demodulation paths are available:
//!
//! **Fast path** (`fast-demod` feature): Delay-and-multiply discriminator with
//! hard-decision decoding. Minimal CPU/RAM — suitable for Cortex-M0, RP2040.
//!
//! **Quality path** (`quality-demod` feature, default): Hilbert transform for
//! instantaneous frequency estimation, adaptive preamble training, and soft-
//! decision HDLC decoding with bit-flip error correction.
//!
//! # Architecture
//!
//! ```text
//! Fast:    Samples → BPF → Delay-Multiply → LPF → PLL → NRZI → HDLC
//! Quality: Samples → BPF → Hilbert → InstFreq → Adaptive → SoftPLL → SoftHDLC
//! Mod:     Bits → NRZI Encode → Phase Accumulator (NCO) → Samples
//! ```
//!
//! See docs/MODEM_DESIGN.md for detailed algorithm descriptions.

pub mod afsk;
pub mod demod;
pub mod delay_multiply;
pub mod filter;
pub mod hilbert;
pub mod adaptive;
pub mod pll;
pub mod soft_hdlc;

/// Standard Bell 202 mark frequency (Hz)
pub const MARK_FREQ: u32 = 1200;

/// Standard Bell 202 space frequency (Hz)
pub const SPACE_FREQ: u32 = 2200;

/// Standard baud rate
pub const BAUD_RATE: u32 = 1200;

/// Midpoint frequency between mark and space (Hz)
pub const MID_FREQ: u32 = (MARK_FREQ + SPACE_FREQ) / 2; // 1700 Hz

/// Maximum delay line length (samples) for the delay-multiply detector
pub const MAX_DELAY: usize = 32;

/// Maximum number of bits in a single frame (for soft bit buffer)
/// AX.25 max frame = 330 bytes × 8 bits + flags + stuffing ≈ 3000 bits
pub const MAX_FRAME_BITS: usize = 4096;

/// Number of candidate bits to consider for bit-flip recovery
pub const MAX_FLIP_CANDIDATES: usize = 8;

/// Demodulator configuration
#[derive(Clone, Copy, Debug)]
pub struct DemodConfig {
    /// Audio sample rate in Hz
    pub sample_rate: u32,
    /// Mark frequency in Hz (nominally 1200)
    pub mark_freq: u32,
    /// Space frequency in Hz (nominally 2200)
    pub space_freq: u32,
    /// Baud rate (nominally 1200)
    pub baud_rate: u32,
    /// PLL bandwidth factor (higher = faster lock, more jitter)
    pub pll_alpha: i16,
    /// PLL frequency correction gain
    pub pll_beta: i16,
}

impl DemodConfig {
    /// Default configuration for 1200 baud AFSK at 11025 Hz sample rate.
    pub fn default_1200() -> Self {
        Self {
            sample_rate: 11025,
            mark_freq: MARK_FREQ,
            space_freq: SPACE_FREQ,
            baud_rate: BAUD_RATE,
            pll_alpha: 936,   // ~0.0286 in Q15 — moderate tracking
            pll_beta: 74,     // ~0.00226 in Q15
        }
    }

    /// Configuration at 22050 Hz sample rate (better quality, more CPU).
    pub fn default_1200_22k() -> Self {
        Self {
            sample_rate: 22050,
            ..Self::default_1200()
        }
    }

    /// Configuration at 44100 Hz sample rate (CD quality).
    pub fn default_1200_44k() -> Self {
        Self {
            sample_rate: 44100,
            ..Self::default_1200()
        }
    }

    /// Number of audio samples per symbol period.
    pub fn samples_per_symbol(&self) -> u32 {
        self.sample_rate / self.baud_rate
    }
}

/// Modulator configuration
#[derive(Clone, Debug)]
pub struct ModConfig {
    /// Audio sample rate in Hz
    pub sample_rate: u32,
    /// Mark frequency in Hz
    pub mark_freq: u32,
    /// Space frequency in Hz
    pub space_freq: u32,
    /// Baud rate
    pub baud_rate: u32,
    /// Output amplitude (0-32767 for i16)
    pub amplitude: i16,
}

impl ModConfig {
    /// Default modulator configuration at 11025 Hz.
    pub fn default_1200() -> Self {
        Self {
            sample_rate: 11025,
            mark_freq: MARK_FREQ,
            space_freq: SPACE_FREQ,
            baud_rate: BAUD_RATE,
            amplitude: 16000,
        }
    }

    /// Number of audio samples per symbol period.
    pub fn samples_per_symbol(&self) -> u32 {
        self.sample_rate / self.baud_rate
    }
}

/// 256-entry sine lookup table, Q15 format.
/// SIN_TABLE[i] = round(sin(2π·i/256) × 32767)
pub static SIN_TABLE_Q15: [i16; 256] = [
        0,   804,  1608,  2410,  3212,  4011,  4808,  5602,
     6393,  7179,  7962,  8739,  9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530,
    18204, 18868, 19519, 20159, 20787, 21403, 22005, 22594,
    23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790,
    27245, 27683, 28105, 28510, 28898, 29268, 29621, 29956,
    30273, 30571, 30852, 31113, 31356, 31580, 31785, 31971,
    32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757,
    32767, 32757, 32728, 32678, 32609, 32521, 32412, 32285,
    32137, 31971, 31785, 31580, 31356, 31113, 30852, 30571,
    30273, 29956, 29621, 29268, 28898, 28510, 28105, 27683,
    27245, 26790, 26319, 25832, 25329, 24811, 24279, 23731,
    23170, 22594, 22005, 21403, 20787, 20159, 19519, 18868,
    18204, 17530, 16846, 16151, 15446, 14732, 14010, 13279,
    12539, 11793, 11039, 10278,  9512,  8739,  7962,  7179,
     6393,  5602,  4808,  4011,  3212,  2410,  1608,   804,
        0,  -804, -1608, -2410, -3212, -4011, -4808, -5602,
    -6393, -7179, -7962, -8739, -9512,-10278,-11039,-11793,
   -12539,-13279,-14010,-14732,-15446,-16151,-16846,-17530,
   -18204,-18868,-19519,-20159,-20787,-21403,-22005,-22594,
   -23170,-23731,-24279,-24811,-25329,-25832,-26319,-26790,
   -27245,-27683,-28105,-28510,-28898,-29268,-29621,-29956,
   -30273,-30571,-30852,-31113,-31356,-31580,-31785,-31971,
   -32137,-32285,-32412,-32521,-32609,-32678,-32728,-32757,
   -32767,-32757,-32728,-32678,-32609,-32521,-32412,-32285,
   -32137,-31971,-31785,-31580,-31356,-31113,-30852,-30571,
   -30273,-29956,-29621,-29268,-28898,-28510,-28105,-27683,
   -27245,-26790,-26319,-25832,-25329,-24811,-24279,-23731,
   -23170,-22594,-22005,-21403,-20787,-20159,-19519,-18868,
   -18204,-17530,-16846,-16151,-15446,-14732,-14010,-13279,
   -12539,-11793,-11039,-10278, -9512, -8739, -7962, -7179,
    -6393, -5602, -4808, -4011, -3212, -2410, -1608,  -804,
];
