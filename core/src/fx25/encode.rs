//! FX.25 frame encoder: tag selection, padding, and RS encoding.
//!
//! Wraps an AX.25 frame (including CRC) in an FX.25 RS block with a
//! correlation tag. The output is a sequence of raw bits (no bit-stuffing)
//! suitable for NRZI modulation.
//!
//! # Output Format
//!
//! The encoder produces bits in this order:
//! 1. Correlation tag: 64 bits, MSB first
//! 2. RS codeword: n bytes, LSB first per byte
//!
//! The caller is responsible for prepending preamble flags and appending
//! postamble flags around this bit sequence.

use super::rs;
use super::{select_tag, FX25_TAGS};

/// Maximum encoded bit count: 64-bit tag + 255 bytes * 8 bits = 2104 bits.
const MAX_ENCODED_BITS: usize = 64 + 255 * 8;

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

/// Encode an AX.25 frame as an FX.25 block.
///
/// - `frame_with_crc`: the complete AX.25 frame including the 2-byte CRC/FCS.
/// - `min_check_bytes`: minimum number of RS check bytes (16, 32, or 64).
///   The smallest tag with at least this many check bytes that fits the frame
///   is selected automatically.
///
/// Returns `None` if the frame is too large for any FX.25 code, or if
/// `min_check_bytes` is 0.
pub fn fx25_encode(frame_with_crc: &[u8], min_check_bytes: u16) -> Option<Fx25Block> {
    if min_check_bytes == 0 {
        return None;
    }

    let tag_idx = select_tag(frame_with_crc.len(), min_check_bytes)?;
    fx25_encode_with_tag(frame_with_crc, tag_idx)
}

/// Encode an AX.25 frame using a specific tag index.
///
/// - `frame_with_crc`: the complete AX.25 frame including CRC.
/// - `tag_idx`: index into `FX25_TAGS`.
///
/// Returns `None` if the frame doesn't fit in the tag's data capacity.
pub fn fx25_encode_with_tag(frame_with_crc: &[u8], tag_idx: usize) -> Option<Fx25Block> {
    if tag_idx >= FX25_TAGS.len() {
        return None;
    }
    let tag = &FX25_TAGS[tag_idx];
    let k = tag.rs_k as usize;
    let nsym = tag.check_bytes as usize;

    if frame_with_crc.len() > k || nsym == 0 {
        return None;
    }

    // Pad data to k bytes with zeros
    let mut data = [0u8; 255];
    data[..frame_with_crc.len()].copy_from_slice(frame_with_crc);
    // Remaining bytes are already zero (padding)

    // RS encode: compute parity
    let mut parity = [0u8; 64];
    rs::rs_encode(&data[..k], nsym, &mut parity).ok()?;

    // Build the bit sequence
    let mut block = Fx25Block {
        bits: [0u8; MAX_ENCODED_BITS],
        bit_count: 0,
        tag_index: tag_idx as u8,
    };

    // 1. Correlation tag: 64 bits, MSB first
    for b in (0..64).rev() {
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
    #[allow(unused_imports)]
    use alloc::vec::Vec;
    use super::*;
    use crate::fx25::decode::Fx25Decoder;

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
        let frame = make_test_frame(); // 18 bytes
        let block = fx25_encode(&frame, 16).unwrap();
        let tag = &FX25_TAGS[block.tag_index as usize];
        // Should pick the smallest tag with k >= 18 and check_bytes >= 16
        assert!(tag.rs_k >= 18);
        assert!(tag.check_bytes >= 16);
        // RS(48,32) should be the smallest with 16 check bytes that fits 18 bytes
        assert_eq!(tag.rs_k, 32, "expected RS(*,32) for 18-byte frame");
    }

    #[test]
    fn encode_frame_too_large() {
        let big_frame = [0u8; 240];
        assert!(fx25_encode(&big_frame, 32).is_none(),
            "240 bytes should not fit with 32 check bytes (max k=223)");
    }

    #[test]
    fn encode_decode_roundtrip() {
        let frame = make_test_frame();
        let block = fx25_encode(&frame, 16).unwrap();

        // Feed the encoded bits through the decoder
        let mut decoder = Fx25Decoder::new();
        // Preamble
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
        let decoded = decoded.unwrap();
        assert_eq!(&decoded[..frame.len()], &frame[..]);
    }

    #[test]
    fn encode_decode_roundtrip_all_check_sizes() {
        let frame = make_test_frame();
        for &check in &[16u16, 32, 64] {
            let block = fx25_encode(&frame, check);
            if block.is_none() {
                continue; // frame might not fit with this check size
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
            assert!(decoded.is_some(),
                "roundtrip failed for check_bytes={check}, tag RS({},{})",
                tag.rs_n, tag.rs_k);
            assert_eq!(&decoded.unwrap()[..frame.len()], &frame[..]);
        }
    }

    #[test]
    fn encode_with_specific_tag() {
        let frame = make_test_frame();
        // Tag 0: RS(255,239), 16 check bytes
        let block = fx25_encode_with_tag(&frame, 0).unwrap();
        assert_eq!(block.tag_index, 0);
        let expected = 64 + 255 * 8;
        assert_eq!(block.bit_count, expected);
    }

    #[test]
    fn encode_with_invalid_tag() {
        let frame = make_test_frame();
        assert!(fx25_encode_with_tag(&frame, 99).is_none());
    }
}
