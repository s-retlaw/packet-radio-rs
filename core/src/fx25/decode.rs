//! FX.25 frame decoder: correlation tag detection and RS block accumulation.
//!
//! The decoder operates as a parallel state machine alongside HDLC, consuming
//! the same NRZI-decoded bit stream. It maintains a 64-bit sliding shift register
//! to detect correlation tags, then accumulates the exact number of raw bytes
//! specified by the matched tag, and finally applies RS error correction.
//!
//! # State Machine
//!
//! ```text
//! Hunting ──[tag match]──▶ Accumulating ──[block full]──▶ RS decode ──▶ Hunting
//!    ▲                                                       │
//!    └───────────────────────────────────────────────────────┘
//! ```

use super::rs;
use super::{match_tag, FX25_TAGS};

/// Maximum RS codeword size (RS over GF(256)).
const MAX_BLOCK: usize = 255;

/// Default maximum Hamming distance for tag correlation.
const DEFAULT_MAX_HAMMING: u32 = 5;

/// Minimum bits to shift in before tag detection (avoids false triggers on startup).
const MIN_BITS_BEFORE_DETECT: u8 = 64;

/// Decoder state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Searching for a correlation tag in the bit stream.
    Hunting,
    /// Accumulating raw bytes for the RS block.
    Accumulating,
}

/// FX.25 correlation tag detector and RS block decoder.
///
/// Feed NRZI-decoded bits (the same bits fed to the HDLC decoder).
/// When a complete FX.25 block is received and RS-decoded, the extracted
/// AX.25 frame is available via `feed_bit()`.
pub struct Fx25Decoder {
    /// Sliding 64-bit shift register for tag detection.
    shift_reg: u64,
    /// Bits shifted in so far (saturates at 255).
    bits_shifted: u8,
    /// Current state.
    state: State,
    /// RS block accumulation buffer.
    block_buf: [u8; MAX_BLOCK],
    /// Current byte being assembled (MSB or LSB first depends on convention).
    current_byte: u8,
    /// Bits accumulated in current_byte (0-7).
    bit_count: u8,
    /// Bytes accumulated in block_buf.
    byte_count: usize,
    /// Target block size from matched tag (rs_n).
    target_bytes: usize,
    /// Matched tag index into FX25_TAGS.
    tag_index: u8,
    /// Output buffer for decoded AX.25 frame.
    output_buf: [u8; MAX_BLOCK],
    /// Length of valid data in output_buf (0 = no frame ready).
    output_len: usize,
    /// Maximum Hamming distance for tag correlation.
    max_hamming: u32,

    // Statistics
    /// Number of correlation tags detected.
    pub stats_tags_detected: u32,
    /// Number of frames successfully RS-decoded (0 corrections needed).
    pub stats_rs_clean: u32,
    /// Number of frames RS-corrected (1+ byte errors fixed).
    pub stats_rs_corrected: u32,
    /// Number of frames where RS decode failed (too many errors).
    pub stats_rs_failed: u32,
}

impl Fx25Decoder {
    /// Create a new FX.25 decoder.
    pub fn new() -> Self {
        Self {
            shift_reg: 0,
            bits_shifted: 0,
            state: State::Hunting,
            block_buf: [0u8; MAX_BLOCK],
            current_byte: 0,
            bit_count: 0,
            byte_count: 0,
            target_bytes: 0,
            tag_index: 0,
            output_buf: [0u8; MAX_BLOCK],
            output_len: 0,
            max_hamming: DEFAULT_MAX_HAMMING,
            stats_tags_detected: 0,
            stats_rs_clean: 0,
            stats_rs_corrected: 0,
            stats_rs_failed: 0,
        }
    }

    /// Set the maximum Hamming distance for tag detection (default: 5).
    #[must_use]
    pub fn with_max_hamming(mut self, max: u32) -> Self {
        self.max_hamming = max;
        self
    }

    /// Reset decoder state (clear shift register, return to Hunting).
    pub fn reset(&mut self) {
        self.shift_reg = 0;
        self.bits_shifted = 0;
        self.state = State::Hunting;
        self.bit_count = 0;
        self.byte_count = 0;
        self.output_len = 0;
    }

    /// Feed a single NRZI-decoded bit to the decoder.
    ///
    /// Returns a slice of the decoded AX.25 frame (without CRC) if a complete
    /// FX.25 block was received and RS-decoded successfully. The returned slice
    /// is valid until the next call to `feed_bit()`.
    pub fn feed_bit(&mut self, bit: bool) -> Option<&[u8]> {
        // Clear any previous output
        self.output_len = 0;

        match self.state {
            State::Hunting => {
                // Shift bit into the 64-bit register
                self.shift_reg = (self.shift_reg << 1) | (bit as u64);
                self.bits_shifted = self.bits_shifted.saturating_add(1);

                if self.bits_shifted >= MIN_BITS_BEFORE_DETECT {
                    if let Some((idx, _dist)) = match_tag(self.shift_reg, self.max_hamming) {
                        let tag = &FX25_TAGS[idx];
                        if tag.check_bytes > 0 {
                            // Valid FEC tag — switch to accumulation
                            self.state = State::Accumulating;
                            self.tag_index = idx as u8;
                            self.target_bytes = tag.rs_n as usize;
                            self.byte_count = 0;
                            self.bit_count = 0;
                            self.current_byte = 0;
                            self.stats_tags_detected += 1;
                        }
                    }
                }
                None
            }
            State::Accumulating => {
                // Accumulate bits into bytes (LSB first, matching AX.25 convention)
                if bit {
                    self.current_byte |= 1 << self.bit_count;
                }
                self.bit_count += 1;

                if self.bit_count == 8 {
                    if self.byte_count < MAX_BLOCK {
                        self.block_buf[self.byte_count] = self.current_byte;
                        self.byte_count += 1;
                    }
                    self.current_byte = 0;
                    self.bit_count = 0;

                    // Check if we have the full block
                    if self.byte_count >= self.target_bytes {
                        self.decode_block();
                        self.state = State::Hunting;
                        self.shift_reg = 0;
                        self.bits_shifted = 0;
                        if self.output_len > 0 {
                            return Some(&self.output_buf[..self.output_len]);
                        }
                        return None;
                    }
                }
                None
            }
        }
    }

    /// Attempt RS decode on the accumulated block and extract the AX.25 frame.
    /// Sets `output_len` on success.
    fn decode_block(&mut self) {
        let tag = &FX25_TAGS[self.tag_index as usize];
        let n = tag.rs_n as usize;
        let k = tag.rs_k as usize;
        let nsym = tag.check_bytes as usize;

        // RS decode in place
        match rs::rs_decode(&mut self.block_buf, n, nsym) {
            Ok(0) => {
                self.stats_rs_clean += 1;
            }
            Ok(_corrections) => {
                self.stats_rs_corrected += 1;
            }
            Err(_) => {
                self.stats_rs_failed += 1;
                return;
            }
        }

        // The data portion is block_buf[0..k].
        // It contains the AX.25 frame padded with trailing zeros to fill k bytes.
        // Find the actual frame length by scanning backwards for the last non-zero byte.
        // AX.25 frames end with a 2-byte CRC which is extremely unlikely to be 0x0000.
        let frame_len = Self::find_frame_length(&self.block_buf[..k]);
        if frame_len < 17 {
            return;
        }

        // Copy to output buffer (frame includes CRC — caller can verify/strip)
        self.output_buf[..frame_len].copy_from_slice(&self.block_buf[..frame_len]);
        self.output_len = frame_len;
    }

    /// Find the length of the AX.25 frame within a padded data block.
    ///
    /// Scans backward from the end to find the last non-zero byte.
    /// Returns the length including that byte (0 if all zeros).
    fn find_frame_length(data: &[u8]) -> usize {
        let mut len = data.len();
        while len > 0 && data[len - 1] == 0 {
            len -= 1;
        }
        len
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;
    use super::*;
    use crate::fx25::rs::rs_encode;

    /// Helper: build a complete FX.25 bit stream (tag + RS block) for testing.
    fn build_fx25_bitstream(ax25_frame: &[u8], tag_idx: usize) -> (Vec<bool>, usize) {
        let tag = &FX25_TAGS[tag_idx];
        let k = tag.rs_k as usize;
        let n = tag.rs_n as usize;
        let nsym = tag.check_bytes as usize;

        // Pad data to k bytes
        let mut data = [0u8; MAX_BLOCK];
        let frame_len = ax25_frame.len().min(k);
        data[..frame_len].copy_from_slice(&ax25_frame[..frame_len]);

        // RS encode
        let mut parity = [0u8; 64];
        rs_encode(&data[..k], nsym, &mut parity).unwrap();

        // Build codeword
        let mut codeword = [0u8; MAX_BLOCK];
        codeword[..k].copy_from_slice(&data[..k]);
        codeword[k..n].copy_from_slice(&parity[..nsym]);

        // Convert to bit stream: 64-bit tag (MSB first) + codeword bytes (LSB first)
        let mut bits = Vec::new();

        // Tag bits: MSB first (bit 63 first)
        for b in (0..64).rev() {
            bits.push((tag.tag >> b) & 1 == 1);
        }

        // Codeword bytes: LSB first per byte
        for &byte in &codeword[..n] {
            for b in 0..8 {
                bits.push((byte >> b) & 1 == 1);
            }
        }

        (bits, frame_len)
    }

    /// Create a minimal valid AX.25 frame (with CRC placeholder).
    fn make_test_frame() -> Vec<u8> {
        // Minimal: 7-byte dest + 7-byte src (with extension bit) + ctrl + PID + 2-byte payload
        let mut frame = vec![0u8; 18];
        // Dest: "TEST  " shifted left
        for (i, &c) in b"TEST  ".iter().enumerate() {
            frame[i] = c << 1;
        }
        frame[6] = 0x60; // SSID byte
        // Src: "SRC   " shifted left
        for (i, &c) in b"SRC   ".iter().enumerate() {
            frame[7 + i] = c << 1;
        }
        frame[13] = 0x61; // SSID byte with extension bit set
        frame[14] = 0x03; // UI control
        frame[15] = 0xF0; // No L3 PID
        frame[16] = b'H';
        frame[17] = b'i';
        frame
    }

    #[test]
    fn decode_clean_frame() {
        let frame = make_test_frame();
        // Use tag 3 (RS(80,64), 16 check bytes) — small enough for quick test
        let (bits, frame_len) = build_fx25_bitstream(&frame, 2);

        let mut decoder = Fx25Decoder::new();
        // Feed some preamble bits first
        for _ in 0..128 {
            assert!(decoder.feed_bit(false).is_none());
        }

        // Feed the FX.25 bitstream
        let mut decoded_frame = None;
        for &bit in &bits {
            if let Some(f) = decoder.feed_bit(bit) {
                decoded_frame = Some(f.to_vec());
                break;
            }
        }

        assert!(decoded_frame.is_some(), "no frame decoded");
        let decoded = decoded_frame.unwrap();
        assert_eq!(&decoded[..frame_len], &frame[..frame_len]);
        assert_eq!(decoder.stats_tags_detected, 1);
        assert_eq!(decoder.stats_rs_clean, 1);
    }

    #[test]
    fn decode_with_byte_errors() {
        let frame = make_test_frame();
        let (mut bits, frame_len) = build_fx25_bitstream(&frame, 2);

        // Corrupt 5 bytes in the RS block area (after the 64-bit tag)
        // Each byte is 8 bits, so corrupt bits at positions 64+offset*8
        for i in 0..5 {
            let byte_start = 64 + (i * 3 + 10) * 8; // spread errors around
            if byte_start + 7 < bits.len() {
                // Flip all 8 bits of that byte
                for b in 0..8 {
                    bits[byte_start + b] = !bits[byte_start + b];
                }
            }
        }

        let mut decoder = Fx25Decoder::new();
        for _ in 0..128 {
            decoder.feed_bit(false);
        }

        let mut decoded_frame = None;
        for &bit in &bits {
            if let Some(f) = decoder.feed_bit(bit) {
                decoded_frame = Some(f.to_vec());
                break;
            }
        }

        assert!(decoded_frame.is_some(), "RS should correct 5 byte errors with 16 check bytes");
        let decoded = decoded_frame.unwrap();
        assert_eq!(&decoded[..frame_len], &frame[..frame_len]);
        assert_eq!(decoder.stats_rs_corrected, 1);
    }

    #[test]
    fn decode_too_many_errors_fails() {
        let frame = make_test_frame();
        let (mut bits, _) = build_fx25_bitstream(&frame, 2);

        // Corrupt 9 bytes (exceeds max_t=8 for 16 check bytes)
        for i in 0..9 {
            let byte_start = 64 + (i * 5 + 2) * 8;
            if byte_start + 7 < bits.len() {
                for b in 0..8 {
                    bits[byte_start + b] = !bits[byte_start + b];
                }
            }
        }

        let mut decoder = Fx25Decoder::new();
        for _ in 0..128 {
            decoder.feed_bit(false);
        }

        let mut got_frame = false;
        for &bit in &bits {
            if decoder.feed_bit(bit).is_some() {
                got_frame = true;
                break;
            }
        }

        assert!(!got_frame, "should not decode with 9 byte errors");
        assert_eq!(decoder.stats_rs_failed, 1);
    }

    #[test]
    fn tag_detection_with_bit_errors() {
        let frame = make_test_frame();
        let (mut bits, frame_len) = build_fx25_bitstream(&frame, 2);

        // Flip 3 bits in the correlation tag (should still match with hamming ≤ 5)
        bits[10] = !bits[10];
        bits[30] = !bits[30];
        bits[50] = !bits[50];

        let mut decoder = Fx25Decoder::new();
        for _ in 0..128 {
            decoder.feed_bit(false);
        }

        let mut decoded_frame = None;
        for &bit in &bits {
            if let Some(f) = decoder.feed_bit(bit) {
                decoded_frame = Some(f.to_vec());
                break;
            }
        }

        assert!(decoded_frame.is_some(), "tag should match with 3 bit errors");
        assert_eq!(&decoded_frame.unwrap()[..frame_len], &frame[..frame_len]);
    }

    #[test]
    fn multiple_frames_sequential() {
        let frame1 = make_test_frame();
        let mut frame2 = make_test_frame();
        frame2[16] = b'X'; // different payload

        let (bits1, len1) = build_fx25_bitstream(&frame1, 2);
        let (bits2, len2) = build_fx25_bitstream(&frame2, 2);

        let mut decoder = Fx25Decoder::new();
        let mut frames_decoded = Vec::new();

        // Feed preamble
        for _ in 0..128 {
            decoder.feed_bit(false);
        }
        // Feed first frame
        for &bit in &bits1 {
            if let Some(f) = decoder.feed_bit(bit) {
                frames_decoded.push(f.to_vec());
            }
        }
        // Feed gap
        for _ in 0..64 {
            decoder.feed_bit(false);
        }
        // Feed second frame
        for &bit in &bits2 {
            if let Some(f) = decoder.feed_bit(bit) {
                frames_decoded.push(f.to_vec());
            }
        }

        assert_eq!(frames_decoded.len(), 2);
        assert_eq!(&frames_decoded[0][..len1], &frame1[..len1]);
        assert_eq!(&frames_decoded[1][..len2], &frame2[..len2]);
        assert_eq!(decoder.stats_tags_detected, 2);
    }

    #[test]
    fn reset_clears_state() {
        let mut decoder = Fx25Decoder::new();
        for _ in 0..200 {
            decoder.feed_bit(true);
        }
        decoder.reset();
        assert_eq!(decoder.state, State::Hunting);
        assert_eq!(decoder.bits_shifted, 0);
        assert_eq!(decoder.byte_count, 0);
    }

    #[test]
    fn all_tag_sizes() {
        let frame = make_test_frame();
        // Test with tags that have enough room for our test frame
        for tag_idx in 0..FX25_TAGS.len() {
            let tag = &FX25_TAGS[tag_idx];
            if tag.check_bytes == 0 || (tag.rs_k as usize) < frame.len() {
                continue;
            }

            let (bits, frame_len) = build_fx25_bitstream(&frame, tag_idx);
            let mut decoder = Fx25Decoder::new();
            for _ in 0..128 {
                decoder.feed_bit(false);
            }

            let mut decoded_frame = None;
            for &bit in &bits {
                if let Some(f) = decoder.feed_bit(bit) {
                    decoded_frame = Some(f.to_vec());
                    break;
                }
            }

            assert!(decoded_frame.is_some(),
                "tag {tag_idx} RS({},{}) decode failed", tag.rs_n, tag.rs_k);
            assert_eq!(&decoded_frame.unwrap()[..frame_len], &frame[..frame_len],
                "tag {tag_idx} data mismatch");
        }
    }
}
