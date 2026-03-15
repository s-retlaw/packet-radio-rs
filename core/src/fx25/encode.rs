//! FX.25 frame encoder: CRC, HDLC bit-stuffing, RS encoding (Dire Wolf compatible).
//!
//! Wraps an AX.25 frame in an FX.25 RS block with a correlation tag.
//! The RS data block uses DW's HDLC-wrapped format:
//!
//! ```text
//! [0x7E flag | HDLC bit-stuffed frame+CRC | 0x7E flag | 0x7E flag padding...]
//! ```
//!
//! After the closing flag, remaining bytes are filled with repeating 0x7E flag
//! patterns (at the bit level, continuing from the bit position after the closing
//! flag). This matches Dire Wolf's `stuff_it()` implementation.
//!
//! # Output Format
//!
//! The encoder produces bits in this order:
//! 1. Correlation tag: 64 bits, LSB first
//! 2. RS codeword: n bytes, LSB first per byte
//!
//! The caller is responsible for prepending preamble flags and appending
//! postamble flags around this bit sequence.

use super::rs;
use super::{select_tag, FX25_TAGS};
use crate::ax25::crc16_ccitt;

/// Maximum encoded bit count: 64-bit tag + 255 bytes * 8 bits = 2104 bits.
const MAX_ENCODED_BITS: usize = 64 + 255 * 8;

/// Buffer for HDLC-wrapped data. 300 bytes covers worst-case bit-stuffing
/// for frames up to ~240 bytes (max RS data capacity is 239).
const HDLC_BUF_SIZE: usize = 300;

/// Encoded FX.25 block as a bit sequence.
pub struct Fx25Block {
    /// Bit buffer. Each element is 0 or 1.
    pub bits: [u8; MAX_ENCODED_BITS],
    /// Number of valid bits.
    pub bit_count: usize,
    /// Tag index used for encoding.
    pub tag_index: u8,
}

impl Fx25Block {
    /// Iterate over the encoded bits as booleans.
    pub fn iter_bits(&self) -> impl Iterator<Item = bool> + '_ {
        self.bits[..self.bit_count].iter().map(|&b| b != 0)
    }
}

/// Encode an AX.25 frame as an FX.25 block (Dire Wolf compatible).
///
/// - `frame`: the raw AX.25 frame WITHOUT CRC. The encoder computes CRC,
///   HDLC bit-stuffs, and wraps with flag bytes internally.
/// - `min_check_bytes`: minimum number of RS check bytes (16, 32, or 64).
///   The smallest tag with at least this many check bytes that fits the
///   HDLC-wrapped frame is selected automatically.
///
/// Returns `None` if the frame is too large for any FX.25 code, or if
/// `min_check_bytes` is 0.
pub fn fx25_encode(frame: &[u8], min_check_bytes: u16) -> Option<Fx25Block> {
    if min_check_bytes == 0 || frame.is_empty() {
        return None;
    }

    // CRC + HDLC bit-stuff + flag wrapping
    let (hdlc_buf, hdlc_byte_len, hdlc_bit_len) = hdlc_wrap(frame)?;

    // Select smallest fitting tag based on HDLC-wrapped byte size
    let tag_idx = select_tag(hdlc_byte_len, min_check_bytes)?;

    encode_rs_block(&hdlc_buf[..hdlc_byte_len], hdlc_bit_len, tag_idx)
}

/// Encode an AX.25 frame using a specific tag index (Dire Wolf compatible).
///
/// - `frame`: the raw AX.25 frame WITHOUT CRC.
/// - `tag_idx`: index into `FX25_TAGS`.
///
/// Returns `None` if the HDLC-wrapped frame doesn't fit in the tag's data capacity.
pub fn fx25_encode_with_tag(frame: &[u8], tag_idx: usize) -> Option<Fx25Block> {
    if frame.is_empty() {
        return None;
    }

    let (hdlc_buf, hdlc_byte_len, hdlc_bit_len) = hdlc_wrap(frame)?;
    encode_rs_block(&hdlc_buf[..hdlc_byte_len], hdlc_bit_len, tag_idx)
}

/// Compute CRC-16-CCITT, HDLC bit-stuff frame+CRC, and wrap with 0x7E flags.
///
/// Returns `(buffer, byte_count, bit_count)`.
fn hdlc_wrap(frame: &[u8]) -> Option<([u8; HDLC_BUF_SIZE], usize, usize)> {
    if frame.len() + 2 > 255 {
        return None; // frame + CRC too large for any RS code
    }

    // Compute CRC and append
    let crc = crc16_ccitt(frame);
    let mut frame_crc = [0u8; 258]; // max frame + 2 CRC bytes
    frame_crc[..frame.len()].copy_from_slice(frame);
    frame_crc[frame.len()] = crc as u8;
    frame_crc[frame.len() + 1] = (crc >> 8) as u8;
    let total = frame.len() + 2;

    // HDLC bit-stuff into bytes with flag wrapping
    let mut hdlc_buf = [0u8; HDLC_BUF_SIZE];
    let (hdlc_byte_len, hdlc_bit_len) = hdlc_stuff_to_bytes(&frame_crc[..total], &mut hdlc_buf);

    Some((hdlc_buf, hdlc_byte_len, hdlc_bit_len))
}

/// HDLC bit-stuff data bytes and pack into bytes with flag wrapping.
///
/// Takes raw bytes (frame + CRC already appended).
/// Outputs `[0x7E | bit-stuffed data | 0x7E]` packed LSB-first into bytes.
/// Returns `(byte_count, bit_count)` — byte count rounds up partial last byte.
fn hdlc_stuff_to_bytes(data_with_crc: &[u8], out: &mut [u8]) -> (usize, usize) {
    // Zero the output buffer
    for b in out.iter_mut() {
        *b = 0;
    }

    let mut bit_pos: usize = 0;
    let out_len = out.len();

    // Inline: push a single bit (LSB-first packing)
    macro_rules! push_bit {
        ($out:expr, $bit_pos:expr, $bit:expr) => {
            let byte_idx = $bit_pos / 8;
            let bit_idx = $bit_pos % 8;
            if byte_idx < out_len {
                if $bit {
                    $out[byte_idx] |= 1 << bit_idx;
                }
                $bit_pos += 1;
            }
        };
    }

    // Opening flag: 0x7E (not bit-stuffed)
    for i in 0..8u8 {
        push_bit!(out, bit_pos, (0x7Eu8 >> i) & 1 != 0);
    }

    // Bit-stuff and emit data+CRC bytes (LSB first per byte)
    let mut ones_count: u8 = 0;
    for &byte in data_with_crc {
        for i in 0..8u8 {
            let bit = (byte >> i) & 1 != 0;
            push_bit!(out, bit_pos, bit);
            if bit {
                ones_count += 1;
                if ones_count == 5 {
                    push_bit!(out, bit_pos, false); // stuffed zero
                    ones_count = 0;
                }
            } else {
                ones_count = 0;
            }
        }
    }

    // Closing flag: 0x7E (not bit-stuffed)
    for i in 0..8u8 {
        push_bit!(out, bit_pos, (0x7Eu8 >> i) & 1 != 0);
    }

    // Return (bytes rounded up, exact bit count)
    ((bit_pos + 7) / 8, bit_pos)
}

/// Fill remaining bits in the RS data block with repeating 0x7E flag patterns.
///
/// This matches Dire Wolf's `stuff_it()` which pads with flag patterns after
/// the closing HDLC flag, rather than leaving zeros.
fn flag_pad_bits(buf: &mut [u8], start_bit: usize, end_bit: usize) {
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

/// RS-encode HDLC-wrapped data into an FX.25 bit block.
///
/// `hdlc_bit_count` is the exact number of bits in the HDLC data (needed
/// to properly align the flag padding after the closing flag).
///
/// DW uses full RS(255, 255-nsym) with pad=0: the data is placed at the START
/// of a (255-nsym)-byte message block, NOT at the end after leading zeros.
/// This means we must pass the full (255-nsym)-byte block to the RS encoder.
fn encode_rs_block(hdlc_data: &[u8], hdlc_bit_count: usize, tag_idx: usize) -> Option<Fx25Block> {
    if tag_idx >= FX25_TAGS.len() {
        return None;
    }
    let tag = &FX25_TAGS[tag_idx];
    let k = tag.rs_k as usize;
    let nsym = tag.check_bytes as usize;

    if hdlc_data.len() > k || nsym == 0 {
        return None;
    }

    // Build the full RS message block (255 - nsym bytes).
    // DW convention: data at positions 0..k-1, flag-padded to k bytes,
    // then zero-filled to the full RS data capacity (255 - nsym).
    let full_k = 255 - nsym; // full RS(255, full_k) data size
    let mut data = [0u8; 255];
    data[..hdlc_data.len()].copy_from_slice(hdlc_data);
    flag_pad_bits(&mut data, hdlc_bit_count, k * 8);
    // Positions k..full_k-1 are already zero (DW zeroes beyond k_data_radio)

    // RS encode: pass full_k bytes so pad=0 (DW compatibility)
    let mut parity = [0u8; 64];
    rs::rs_encode(&data[..full_k], nsym, &mut parity).ok()?;

    // Build the bit sequence
    let mut block = Fx25Block {
        bits: [0u8; MAX_ENCODED_BITS],
        bit_count: 0,
        tag_index: tag_idx as u8,
    };

    // 1. Correlation tag: 64 bits, LSB first
    for b in 0..64 {
        block.bits[block.bit_count] = ((tag.tag >> b) & 1) as u8;
        block.bit_count += 1;
    }

    // 2. Data bytes: LSB first per byte
    for &byte in &data[..k] {
        for b in 0..8 {
            block.bits[block.bit_count] = (byte >> b) & 1;
            block.bit_count += 1;
        }
    }

    // 3. Parity bytes: LSB first per byte
    for &byte in &parity[..nsym] {
        for b in 0..8 {
            block.bits[block.bit_count] = (byte >> b) & 1;
            block.bit_count += 1;
        }
    }

    Some(block)
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use crate::fx25::decode::Fx25Decoder;
    #[allow(unused_imports)]
    use alloc::vec::Vec;

    /// Test frame WITHOUT CRC (raw AX.25 frame).
    fn make_test_frame() -> [u8; 18] {
        let mut frame = [0u8; 18];
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
    fn encode_basic() {
        let frame = make_test_frame();
        let block = fx25_encode(&frame, 16).expect("encode failed");
        // Should have 64 tag bits + n*8 data+parity bits
        let tag = &FX25_TAGS[block.tag_index as usize];
        let expected_bits = 64 + (tag.rs_n as usize) * 8;
        assert_eq!(block.bit_count, expected_bits);
    }

    #[test]
    fn encode_selects_smallest_fitting_tag() {
        let frame = make_test_frame(); // 18 bytes raw
        let block = fx25_encode(&frame, 16).unwrap();
        let tag = &FX25_TAGS[block.tag_index as usize];
        assert!(tag.check_bytes >= 16);
        assert_eq!(tag.rs_k, 32, "expected RS(*,32) for small frame");
    }

    #[test]
    fn encode_frame_too_large() {
        let big_frame = [0u8; 240];
        assert!(
            fx25_encode(&big_frame, 32).is_none(),
            "240 bytes should not fit with 32 check bytes (max k=223)"
        );
    }

    #[test]
    fn encode_decode_roundtrip() {
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

        assert!(decoded.is_some(), "roundtrip decode failed");
        assert_eq!(&decoded.unwrap()[..], &frame[..]);
    }

    #[test]
    fn encode_decode_roundtrip_all_check_sizes() {
        let frame = make_test_frame();
        for &check in &[16u16, 32, 64] {
            let block = fx25_encode(&frame, check);
            if block.is_none() {
                continue;
            }
            let block = block.unwrap();
            let tag = &FX25_TAGS[block.tag_index as usize];
            assert!(tag.check_bytes >= check);

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
            assert!(
                decoded.is_some(),
                "roundtrip failed for check_bytes={check}, tag RS({},{})",
                tag.rs_n,
                tag.rs_k
            );
            assert_eq!(&decoded.unwrap()[..], &frame[..]);
        }
    }

    #[test]
    fn encode_with_specific_tag() {
        let frame = make_test_frame();
        let block = fx25_encode_with_tag(&frame, 1).unwrap();
        assert_eq!(block.tag_index, 1);
        let expected = 64 + 255 * 8;
        assert_eq!(block.bit_count, expected);
    }

    #[test]
    fn encode_reserved_tag_fails() {
        let frame = make_test_frame();
        assert!(fx25_encode_with_tag(&frame, 0).is_none());
    }

    #[test]
    fn encode_with_invalid_tag() {
        let frame = make_test_frame();
        assert!(fx25_encode_with_tag(&frame, 99).is_none());
    }

    #[test]
    fn hdlc_stuff_to_bytes_basic() {
        let data = [0x03, 0xF0, 0x48, 0x69];
        let mut buf = [0u8; 32];
        let (len, _bits) = hdlc_stuff_to_bytes(&data, &mut buf);
        assert!(len >= 6);
        assert_eq!(buf[0], 0x7E, "first byte should be HDLC flag");
    }

    #[test]
    fn hdlc_stuff_roundtrip_via_hdlc_decoder() {
        use crate::ax25::frame::HdlcDecoder;

        let frame = make_test_frame();
        let crc = crc16_ccitt(&frame);
        let mut frame_crc = [0u8; 20];
        frame_crc[..18].copy_from_slice(&frame);
        frame_crc[18] = crc as u8;
        frame_crc[19] = (crc >> 8) as u8;

        let mut buf = [0u8; 64];
        let (len, _bits) = hdlc_stuff_to_bytes(&frame_crc, &mut buf);

        let mut hdlc = HdlcDecoder::new();
        let mut decoded = None;
        for &byte in &buf[..len] {
            for bit_pos in 0..8 {
                let bit = (byte >> bit_pos) & 1 != 0;
                if let Some(f) = hdlc.feed_bit(bit) {
                    decoded = Some(f.to_vec());
                }
            }
        }

        assert!(
            decoded.is_some(),
            "HdlcDecoder should extract frame from stuffed bytes"
        );
        assert_eq!(&decoded.unwrap()[..], &frame[..]);
    }

    #[test]
    fn flag_padding_fills_with_7e_pattern() {
        // Verify that flag padding produces repeating 0x7E patterns
        // when starting at a byte boundary
        let mut buf = [0u8; 8];
        flag_pad_bits(&mut buf, 0, 64);
        // At byte boundary, each byte should be 0x7E
        for (i, &b) in buf.iter().enumerate() {
            assert_eq!(b, 0x7E, "byte {i} should be 0x7E flag");
        }
    }

    #[test]
    fn flag_padding_non_aligned() {
        // When padding starts mid-byte, the flag pattern still repeats
        // correctly at the bit level
        let mut buf = [0u8; 4];
        // Start at bit 4 (middle of byte 0)
        flag_pad_bits(&mut buf, 4, 32);
        // Extract bits 4..32 and verify they're 0x7E pattern
        for i in 0..28 {
            let bit_pos = 4 + i;
            let byte_idx = bit_pos / 8;
            let bit_idx = bit_pos % 8;
            let actual = (buf[byte_idx] >> bit_idx) & 1;
            let expected = (0x7Eu8 >> (i % 8)) & 1;
            assert_eq!(actual, expected, "bit {i} of flag padding mismatch");
        }
    }
}
