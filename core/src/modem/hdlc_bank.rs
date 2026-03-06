//! HDLC decoder abstractions — compile-time conditional wrapper types.
//!
//! `AnyHdlc` wraps either `SoftHdlcDecoder` (when `alloc` is available) or
//! `HdlcDecoder` (bare metal). `HdlcBank<N>` manages an array of N decoders,
//! heap-allocated on `alloc` to avoid stack overflow.
//!
//! All `cfg(feature = "alloc")` logic for HDLC decoder selection lives here
//! and ONLY here — callers use the uniform API without conditional compilation.

use core::ops::Range;

#[cfg(feature = "alloc")]
use super::soft_hdlc::{FrameResult, SoftHdlcDecoder};
#[cfg(not(feature = "alloc"))]
use crate::ax25::frame::HdlcDecoder;

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

// ─── AnyHdlc — single decoder wrapper ────────────────────────────────────

/// Compile-time conditional HDLC decoder.
///
/// On `alloc` builds: wraps `SoftHdlcDecoder` (bit-flip error recovery).
/// On bare-metal: wraps `HdlcDecoder` (hard-decision only).
#[cfg(feature = "alloc")]
pub struct AnyHdlc(SoftHdlcDecoder);

#[cfg(not(feature = "alloc"))]
pub struct AnyHdlc(HdlcDecoder);

impl Default for AnyHdlc {
    fn default() -> Self {
        Self::new()
    }
}

impl AnyHdlc {
    /// Create a new HDLC decoder.
    pub fn new() -> Self {
        #[cfg(feature = "alloc")]
        { Self(SoftHdlcDecoder::new()) }
        #[cfg(not(feature = "alloc"))]
        { Self(HdlcDecoder::new()) }
    }

    /// Reset the decoder state.
    pub fn reset(&mut self) {
        self.0.reset();
    }

    /// Feed a decoded bit with its soft confidence value.
    ///
    /// On `alloc`: uses `feed_soft_bit(llr)` for error recovery.
    /// On bare-metal: uses `feed_bit(bit)`, ignoring the LLR.
    ///
    /// Returns the frame bytes if a complete valid frame was decoded.
    pub fn feed(&mut self, bit: bool, llr: i8) -> Option<&[u8]> {
        #[cfg(feature = "alloc")]
        {
            let _ = bit; // soft decoder derives bit from LLR sign
            match self.0.feed_soft_bit(llr) {
                Some(FrameResult::Valid(d)) => Some(d),
                Some(FrameResult::Recovered { data, .. }) => Some(data),
                None => None,
            }
        }
        #[cfg(not(feature = "alloc"))]
        {
            let _ = llr;
            self.0.feed_bit(bit)
        }
    }

    /// Total frames recovered by soft bit-flip correction (alloc only).
    #[cfg(feature = "alloc")]
    pub fn stats_soft_recovered(&self) -> u32 {
        self.0.stats_total_soft_recovered()
    }

    /// Total false positives rejected by AX.25 validation (alloc only).
    #[cfg(feature = "alloc")]
    pub fn stats_false_positives(&self) -> u32 {
        self.0.stats_false_positives
    }
}

// ─── HdlcBank<N> — array of N decoders ───────────────────────────────────

/// Array of N HDLC decoders. Heap-allocated on `alloc` to avoid stack overflow.
pub struct HdlcBank<const N: usize> {
    #[cfg(feature = "alloc")]
    inner: Vec<AnyHdlc>,
    #[cfg(not(feature = "alloc"))]
    inner: [AnyHdlc; N],
}

impl<const N: usize> Default for HdlcBank<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> HdlcBank<N> {
    /// Create a new bank of N decoders.
    pub fn new() -> Self {
        #[cfg(feature = "alloc")]
        {
            let inner: Vec<AnyHdlc> = (0..N).map(|_| AnyHdlc::new()).collect();
            Self { inner }
        }
        #[cfg(not(feature = "alloc"))]
        {
            Self {
                inner: core::array::from_fn(|_| AnyHdlc::new()),
            }
        }
    }

    /// Feed a bit+LLR to the decoder at `idx`. Returns frame bytes on success.
    pub fn feed(&mut self, idx: usize, bit: bool, llr: i8) -> Option<&[u8]> {
        self.inner[idx].feed(bit, llr)
    }

    /// Reset a single decoder.
    pub fn reset(&mut self, idx: usize) {
        self.inner[idx].reset();
    }

    /// Reset all decoders in a range.
    pub fn reset_range(&mut self, range: Range<usize>) {
        for i in range {
            self.inner[i].reset();
        }
    }

    /// Total soft-recovered frames across the first `count` decoders (alloc only).
    #[cfg(feature = "alloc")]
    pub fn total_soft_recovered(&self, count: usize) -> u32 {
        let mut total = 0u32;
        for i in 0..count {
            total += self.inner[i].stats_soft_recovered();
        }
        total
    }

    /// Total false positives across the first `count` decoders (alloc only).
    #[cfg(feature = "alloc")]
    pub fn total_false_positives(&self, count: usize) -> u32 {
        let mut total = 0u32;
        for i in 0..count {
            total += self.inner[i].stats_false_positives();
        }
        total
    }
}
