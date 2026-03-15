//! FX.25 frame decoder: correlation tag detection and RS block accumulation.
//!
//! The decoder operates as a parallel state machine alongside HDLC, consuming
//! the same NRZI-decoded bit stream. It maintains a 64-bit sliding shift register
//! to detect correlation tags, then accumulates the exact number of raw bytes
//! specified by the matched tag, and finally applies RS error correction.
//!
//! After RS decode, the data block contains DW-compatible HDLC-wrapped content:
//! `[0x7E flag | bit-stuffed frame+CRC | 0x7E flag | 0x00 padding...]`
//!
//! The decoder feeds these bytes through an `HdlcDecoder` to extract the
//! CRC-verified AX.25 frame.
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
use crate::ax25::frame::HdlcDecoder;

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
    /// Bit count of the last successfully decoded RS block (64 tag + rs_n*8 data).
    /// Used by callers to backdate sample timestamps for dedup.
    last_block_bits: usize,

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

impl Default for Fx25Decoder {
    fn default() -> Self {
        Self::new()
    }
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
            last_block_bits: 0,
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

    /// Bit count of the last decoded block (64-bit tag + RS data).
    /// Used to backdate sample timestamps for dedup against HDLC.
    pub fn last_block_bits(&self) -> usize {
        self.last_block_bits
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
                // Shift bit into the 64-bit register (right-shift, new bit at MSB)
                // This matches Dire Wolf's convention: F->accum >>= 1; if(dbit) accum |= 1<<63
                self.shift_reg = (self.shift_reg >> 1) | ((bit as u64) << 63);
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
                        self.last_block_bits = 64 + self.target_bytes * 8;
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

    /// Attempt RS decode on the accumulated block, then extract the HDLC-wrapped
    /// AX.25 frame from the corrected data. Sets `output_len` on success.
    ///
    /// DW uses full RS(255, 255-nsym) with pad=0. The received n-byte block
    /// must be rearranged into a 255-byte codeword before RS decoding:
    /// `[data(k) | zeros(255-nsym-k) | parity(nsym)]`
    fn decode_block(&mut self) {
        let tag = &FX25_TAGS[self.tag_index as usize];
        let n = tag.rs_n as usize;
        let k = tag.rs_k as usize;
        let nsym = tag.check_bytes as usize;

        // Rearrange received block into DW's 255-byte layout:
        // Received block_buf[0..n] = [data(k) | parity(nsym)]
        // Full layout: [data(k) | zeros(255-nsym-k) | parity(nsym)]
        let mut full_block = [0u8; MAX_BLOCK]; // 255 bytes, zero-initialized
        full_block[..k].copy_from_slice(&self.block_buf[..k]);
        full_block[255 - nsym..255].copy_from_slice(&self.block_buf[k..n]);

        // RS decode on full 255-byte codeword (pad=0, matching DW)
        match rs::rs_decode(&mut full_block, 255, nsym) {
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

        // The corrected data is full_block[0..k] (DW-format HDLC-wrapped content).
        // Feed each byte's bits (LSB first) through an HdlcDecoder to extract
        // the CRC-verified AX.25 frame.
        let mut hdlc = HdlcDecoder::new();
        for &byte in &full_block[..k] {
            for bit_pos in 0..8 {
                let bit = (byte >> bit_pos) & 1 != 0;
                if let Some(frame) = hdlc.feed_bit(bit) {
                    if frame.len() >= 15 {
                        let len = frame.len().min(MAX_BLOCK);
                        self.output_buf[..len].copy_from_slice(&frame[..len]);
                        self.output_len = len;
                        return;
                    }
                }
            }
        }
        // No valid HDLC frame found in RS data
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use crate::ax25::crc16_ccitt;
    use crate::fx25::encode::fx25_encode;
    use crate::fx25::rs::rs_encode;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Helper: build a DW-compatible FX.25 bit stream (tag + RS block) for testing.
    ///
    /// Takes a raw AX.25 frame (WITHOUT CRC), wraps it in HDLC (CRC + bit-stuff + flags),
    /// zero-pads to k bytes, RS-encodes, and returns the bit stream.
    fn build_fx25_bitstream(ax25_frame: &[u8], tag_idx: usize) -> Vec<bool> {
        let tag = &FX25_TAGS[tag_idx];
        let k = tag.rs_k as usize;
        let n = tag.rs_n as usize;
        let nsym = tag.check_bytes as usize;

        // Compute CRC
        let crc = crc16_ccitt(ax25_frame);
        let mut frame_crc = vec![0u8; ax25_frame.len() + 2];
        frame_crc[..ax25_frame.len()].copy_from_slice(ax25_frame);
        frame_crc[ax25_frame.len()] = crc as u8;
        frame_crc[ax25_frame.len() + 1] = (crc >> 8) as u8;

        // HDLC bit-stuff into bytes with flag wrapping
        let mut hdlc_buf = [0u8; 300];
        let (hdlc_len, hdlc_bits) = hdlc_stuff_to_bytes_for_test(&frame_crc, &mut hdlc_buf);

        assert!(
            hdlc_len <= k,
            "HDLC-wrapped frame ({}) exceeds k ({})",
            hdlc_len,
            k
        );

        // Build full RS message: [hdlc_data(k) + flag padding | zeros(full_k - k)]
        // DW uses pad=0 (full RS(255, full_k)) — data at position 0, not position pad.
        let full_k = 255 - nsym;
        let mut data = [0u8; 255];
        data[..hdlc_len].copy_from_slice(&hdlc_buf[..hdlc_len]);
        flag_pad_bits_for_test(&mut data, hdlc_bits, k * 8);
        // Positions k..full_k-1 are already zero

        // RS encode with full_k bytes (pad=0, DW compatible)
        let mut parity = [0u8; 64];
        rs_encode(&data[..full_k], nsym, &mut parity).unwrap();

        // Build codeword
        let mut codeword = [0u8; 255];
        codeword[..k].copy_from_slice(&data[..k]);
        codeword[k..n].copy_from_slice(&parity[..nsym]);

        // Convert to bit stream: 64-bit tag (LSB first) + codeword bytes (LSB first)
        let mut bits = Vec::new();

        for b in 0..64 {
            bits.push((tag.tag >> b) & 1 == 1);
        }

        for &byte in &codeword[..n] {
            for b in 0..8 {
                bits.push((byte >> b) & 1 == 1);
            }
        }

        bits
    }

    /// Minimal HDLC bit-stuffing for test use (same algorithm as encoder).
    /// Returns (byte_count, bit_count).
    fn hdlc_stuff_to_bytes_for_test(data_with_crc: &[u8], out: &mut [u8]) -> (usize, usize) {
        for b in out.iter_mut() {
            *b = 0;
        }

        let mut bit_pos: usize = 0;
        let out_len = out.len();

        macro_rules! push_bit {
            ($bit:expr) => {
                let byte_idx = bit_pos / 8;
                let bit_idx = bit_pos % 8;
                if byte_idx < out_len {
                    if $bit {
                        out[byte_idx] |= 1 << bit_idx;
                    }
                    bit_pos += 1;
                }
            };
        }

        // Opening flag
        for i in 0..8u8 {
            push_bit!((0x7Eu8 >> i) & 1 != 0);
        }

        // Bit-stuff data
        let mut ones_count: u8 = 0;
        for &byte in data_with_crc {
            for i in 0..8u8 {
                let bit = (byte >> i) & 1 != 0;
                push_bit!(bit);
                if bit {
                    ones_count += 1;
                    if ones_count == 5 {
                        push_bit!(false);
                        ones_count = 0;
                    }
                } else {
                    ones_count = 0;
                }
            }
        }

        // Closing flag
        for i in 0..8u8 {
            push_bit!((0x7Eu8 >> i) & 1 != 0);
        }

        ((bit_pos + 7) / 8, bit_pos)
    }

    /// Fill remaining bits with repeating 0x7E flag pattern (matches DW's stuff_it).
    fn flag_pad_bits_for_test(buf: &mut [u8], start_bit: usize, end_bit: usize) {
        const FLAG: u8 = 0x7E;
        let mut flag_bit_idx: u8 = 0;
        let buf_len = buf.len();
        for bit_pos in start_bit..end_bit {
            let byte_idx = bit_pos / 8;
            let bit_idx = bit_pos % 8;
            if byte_idx >= buf_len {
                break;
            }
            if (FLAG >> flag_bit_idx) & 1 != 0 {
                buf[byte_idx] |= 1 << bit_idx;
            } else {
                buf[byte_idx] &= !(1 << bit_idx);
            }
            flag_bit_idx = (flag_bit_idx + 1) % 8;
        }
    }

    /// Create a minimal valid AX.25 frame (WITHOUT CRC).
    fn make_test_frame() -> Vec<u8> {
        let mut frame = vec![0u8; 18];
        for (i, &c) in b"TEST  ".iter().enumerate() {
            frame[i] = c << 1;
        }
        frame[6] = 0x60;
        for (i, &c) in b"SRC   ".iter().enumerate() {
            frame[7 + i] = c << 1;
        }
        frame[13] = 0x61;
        frame[14] = 0x03;
        frame[15] = 0xF0;
        frame[16] = b'H';
        frame[17] = b'i';
        frame
    }

    #[test]
    fn decode_clean_frame() {
        let frame = make_test_frame();
        let bits = build_fx25_bitstream(&frame, 2);

        let mut decoder = Fx25Decoder::new();
        for _ in 0..128 {
            assert!(decoder.feed_bit(false).is_none());
        }

        let mut decoded_frame = None;
        for &bit in &bits {
            if let Some(f) = decoder.feed_bit(bit) {
                decoded_frame = Some(f.to_vec());
                break;
            }
        }

        assert!(decoded_frame.is_some(), "no frame decoded");
        let decoded = decoded_frame.unwrap();
        assert_eq!(&decoded[..], &frame[..]);
        assert_eq!(decoder.stats_tags_detected, 1);
        assert_eq!(decoder.stats_rs_clean, 1);
    }

    #[test]
    fn decode_with_byte_errors() {
        let frame = make_test_frame();
        let mut bits = build_fx25_bitstream(&frame, 2);

        // Corrupt 5 bytes in the RS block area (after the 64-bit tag)
        for i in 0..5 {
            let byte_start = 64 + (i * 3 + 10) * 8;
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

        let mut decoded_frame = None;
        for &bit in &bits {
            if let Some(f) = decoder.feed_bit(bit) {
                decoded_frame = Some(f.to_vec());
                break;
            }
        }

        assert!(
            decoded_frame.is_some(),
            "RS should correct 5 byte errors with 16 check bytes"
        );
        let decoded = decoded_frame.unwrap();
        assert_eq!(&decoded[..], &frame[..]);
        assert_eq!(decoder.stats_rs_corrected, 1);
    }

    #[test]
    fn decode_too_many_errors_fails() {
        let frame = make_test_frame();
        let mut bits = build_fx25_bitstream(&frame, 2);

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
        let mut bits = build_fx25_bitstream(&frame, 2);

        // Flip 3 bits in the correlation tag (should still match with hamming <= 5)
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

        assert!(
            decoded_frame.is_some(),
            "tag should match with 3 bit errors"
        );
        assert_eq!(&decoded_frame.unwrap()[..], &frame[..]);
    }

    #[test]
    fn multiple_frames_sequential() {
        let frame1 = make_test_frame();
        let mut frame2 = make_test_frame();
        frame2[16] = b'X';

        let bits1 = build_fx25_bitstream(&frame1, 2);
        let bits2 = build_fx25_bitstream(&frame2, 2);

        let mut decoder = Fx25Decoder::new();
        let mut frames_decoded = Vec::new();

        for _ in 0..128 {
            decoder.feed_bit(false);
        }
        for &bit in &bits1 {
            if let Some(f) = decoder.feed_bit(bit) {
                frames_decoded.push(f.to_vec());
            }
        }
        for _ in 0..64 {
            decoder.feed_bit(false);
        }
        for &bit in &bits2 {
            if let Some(f) = decoder.feed_bit(bit) {
                frames_decoded.push(f.to_vec());
            }
        }

        assert_eq!(frames_decoded.len(), 2);
        assert_eq!(&frames_decoded[0][..], &frame1[..]);
        assert_eq!(&frames_decoded[1][..], &frame2[..]);
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
        for tag_idx in 0..FX25_TAGS.len() {
            let tag = &FX25_TAGS[tag_idx];
            if tag.check_bytes == 0 {
                continue;
            }
            // Check HDLC-wrapped frame fits in k
            let crc = crc16_ccitt(&frame);
            let mut frame_crc = vec![0u8; frame.len() + 2];
            frame_crc[..frame.len()].copy_from_slice(&frame);
            frame_crc[frame.len()] = crc as u8;
            frame_crc[frame.len() + 1] = (crc >> 8) as u8;
            let mut hdlc_buf = [0u8; 300];
            let (hdlc_len, _) = hdlc_stuff_to_bytes_for_test(&frame_crc, &mut hdlc_buf);
            if hdlc_len > tag.rs_k as usize {
                continue;
            }

            let bits = build_fx25_bitstream(&frame, tag_idx);
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

            assert!(
                decoded_frame.is_some(),
                "tag {tag_idx} RS({},{}) decode failed",
                tag.rs_n,
                tag.rs_k
            );
            assert_eq!(
                &decoded_frame.unwrap()[..],
                &frame[..],
                "tag {tag_idx} data mismatch"
            );
        }
    }

    #[test]
    fn encode_decode_roundtrip_via_encoder() {
        // Verify that the encoder's output can be decoded by the decoder
        let frame = make_test_frame();
        let block = fx25_encode(&frame, 16).unwrap();

        let mut decoder = Fx25Decoder::new();
        for _ in 0..128 {
            decoder.feed_bit(false);
        }

        let mut decoded = None;
        for bit in block.iter_bits() {
            if let Some(f) = decoder.feed_bit(bit) {
                decoded = Some(f.to_vec());
                break;
            }
        }

        assert!(decoded.is_some(), "encoder output should decode");
        assert_eq!(&decoded.unwrap()[..], &frame[..]);
    }
}
