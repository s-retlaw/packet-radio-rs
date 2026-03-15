//! FX.25 Forward Error Correction for AX.25.
//!
//! FX.25 wraps AX.25 frames in Reed-Solomon FEC, allowing receivers to correct
//! byte errors without retransmission. The protocol uses 64-bit correlation tags
//! to identify the RS code parameters, followed by the RS-encoded block
//! (AX.25 frame + padding + parity) transmitted as raw bytes (no bit-stuffing).
//!
//! # Frame Structure
//!
//! ```text
//! [Preamble flags] [64-bit correlation tag] [RS codeword: data+pad+parity] [Postamble flags]
//!                   └── identifies RS code ──┘└── error-corrected block ────┘
//! ```
//!
//! # Supported RS Codes
//!
//! Each correlation tag selects an RS(n,k) code over GF(256):
//! - RS(255,239): 16 check bytes, corrects up to 8 byte errors
//! - RS(144,128): 16 check bytes, corrects up to 8 byte errors
//! - RS(255,223): 32 check bytes, corrects up to 16 byte errors
//! - RS(160,128): 32 check bytes, corrects up to 16 byte errors
//! - RS(96,64):   32 check bytes, corrects up to 16 byte errors
//! - RS(64,32):   32 check bytes, corrects up to 16 byte errors

pub mod gf256;
pub mod rs;
pub mod decode;
pub mod encode;

/// A correlation tag entry from the FX.25 specification.
#[derive(Clone, Copy, Debug)]
pub struct Fx25Tag {
    /// 64-bit correlation tag value (post-NRZI decoded).
    pub tag: u64,
    /// RS codeword length (n) — total bytes transmitted in the RS block.
    pub rs_n: u16,
    /// RS data portion length (k) — max payload bytes (AX.25 frame + CRC + padding).
    pub rs_k: u16,
    /// Number of check (parity) bytes: rs_n - rs_k.
    pub check_bytes: u16,
}

impl Fx25Tag {
    /// Maximum correctable byte errors: check_bytes / 2.
    pub const fn max_errors(&self) -> u16 {
        self.check_bytes / 2
    }
}

/// All 16 FX.25 correlation tags from the specification.
///
/// Tag values from Dire Wolf `fx25_init.c` (verified against Stensat spec).
/// These are the bit patterns seen in the NRZI-decoded bit stream.
pub static FX25_TAGS: [Fx25Tag; 16] = [
    // Tag 0x00: reserved (no FEC)
    Fx25Tag { tag: 0x566E_D271_7946_107E, rs_n: 0, rs_k: 0, check_bytes: 0 },
    // Tag 0x01: RS(255, 239), 16 check bytes
    Fx25Tag { tag: 0xB74D_B7DF_8A53_2F3E, rs_n: 255, rs_k: 239, check_bytes: 16 },
    // Tag 0x02: RS(144, 128), 16 check bytes (shortened)
    Fx25Tag { tag: 0x26FF_60A6_00CC_8FDE, rs_n: 144, rs_k: 128, check_bytes: 16 },
    // Tag 0x03: RS(80, 64), 16 check bytes (shortened)
    Fx25Tag { tag: 0xC7DC_0508_F3D9_B09E, rs_n: 80, rs_k: 64, check_bytes: 16 },
    // Tag 0x04: RS(48, 32), 16 check bytes (shortened)
    Fx25Tag { tag: 0x8F05_6EB4_3696_60EE, rs_n: 48, rs_k: 32, check_bytes: 16 },
    // Tag 0x05: RS(255, 223), 32 check bytes
    Fx25Tag { tag: 0x6E26_0B1A_C583_5FAE, rs_n: 255, rs_k: 223, check_bytes: 32 },
    // Tag 0x06: RS(160, 128), 32 check bytes (shortened)
    Fx25Tag { tag: 0xFF94_DC63_4F1C_FF4E, rs_n: 160, rs_k: 128, check_bytes: 32 },
    // Tag 0x07: RS(96, 64), 32 check bytes (shortened)
    Fx25Tag { tag: 0x1EB7_B9CD_BC09_C00E, rs_n: 96, rs_k: 64, check_bytes: 32 },
    // Tag 0x08: RS(64, 32), 32 check bytes (shortened)
    Fx25Tag { tag: 0xDBF8_69BD_2DBB_1776, rs_n: 64, rs_k: 32, check_bytes: 32 },
    // Tag 0x09: RS(255, 191), 64 check bytes
    Fx25Tag { tag: 0x3ADB_0C13_DEAE_2836, rs_n: 255, rs_k: 191, check_bytes: 64 },
    // Tag 0x0A: RS(192, 128), 64 check bytes (shortened)
    Fx25Tag { tag: 0xAB69_DB6A_5431_88D6, rs_n: 192, rs_k: 128, check_bytes: 64 },
    // Tag 0x0B: RS(128, 64), 64 check bytes (shortened)
    Fx25Tag { tag: 0x4A4A_BEC4_A724_B796, rs_n: 128, rs_k: 64, check_bytes: 64 },
    // Tags 0x0C-0x0F: undefined/reserved in spec
    Fx25Tag { tag: 0x0293_D578_626B_67E6, rs_n: 0, rs_k: 0, check_bytes: 0 },
    Fx25Tag { tag: 0xE3B0_B0D6_917E_58A6, rs_n: 0, rs_k: 0, check_bytes: 0 },
    Fx25Tag { tag: 0x7202_67AF_1BE1_F846, rs_n: 0, rs_k: 0, check_bytes: 0 },
    Fx25Tag { tag: 0x9321_0201_E8F4_C706, rs_n: 0, rs_k: 0, check_bytes: 0 },
];

/// Look up a correlation tag by matching against all known tags.
///
/// Returns `(index, hamming_distance)` if a match is found within the threshold.
pub fn match_tag(candidate: u64, max_hamming: u32) -> Option<(usize, u32)> {
    let mut best_idx = 0;
    let mut best_dist = u32::MAX;
    for (i, t) in FX25_TAGS.iter().enumerate() {
        let dist = (candidate ^ t.tag).count_ones();
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }
    if best_dist <= max_hamming {
        Some((best_idx, best_dist))
    } else {
        None
    }
}

/// Select the smallest FX.25 tag that can hold a frame of `data_len` bytes.
///
/// `data_len` includes the AX.25 frame + 2-byte CRC.
/// Prefers codes with more parity (better correction) when multiple fit.
/// Returns `None` if the frame is too large for any FX.25 code.
pub fn select_tag(data_len: usize, min_check_bytes: u16) -> Option<usize> {
    let mut best_idx: Option<usize> = None;
    let mut best_n: u16 = u16::MAX;

    for (i, t) in FX25_TAGS.iter().enumerate() {
        if t.check_bytes < min_check_bytes {
            continue;
        }
        if t.check_bytes == 0 {
            continue; // skip passthrough tag
        }
        if data_len <= t.rs_k as usize && t.rs_n < best_n {
            best_n = t.rs_n;
            best_idx = Some(i);
        }
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_table_consistency() {
        for (i, t) in FX25_TAGS.iter().enumerate() {
            assert_eq!(t.rs_n - t.rs_k, t.check_bytes, "tag {i} check_bytes mismatch");
            assert!(t.rs_n >= t.rs_k, "tag {i}: n < k");
            assert!(t.rs_n <= 255, "tag {i}: n > 255");
        }
    }

    #[test]
    fn tag_values_unique() {
        for i in 0..FX25_TAGS.len() {
            for j in (i + 1)..FX25_TAGS.len() {
                assert_ne!(FX25_TAGS[i].tag, FX25_TAGS[j].tag, "duplicate tag at {i} and {j}");
            }
        }
    }

    #[test]
    fn match_tag_exact() {
        for (i, t) in FX25_TAGS.iter().enumerate() {
            let result = match_tag(t.tag, 5);
            assert_eq!(result, Some((i, 0)), "exact match failed for tag {i}");
        }
    }

    #[test]
    fn match_tag_with_errors() {
        // Flip 3 bits in tag 0
        let corrupted = FX25_TAGS[0].tag ^ 0b111;
        let result = match_tag(corrupted, 5);
        assert_eq!(result, Some((0, 3)));
    }

    #[test]
    fn match_tag_too_many_errors() {
        let corrupted = FX25_TAGS[0].tag ^ 0xFFFF_FFFF; // 32 bit errors
        let result = match_tag(corrupted, 5);
        assert!(result.is_none());
    }

    #[test]
    fn select_tag_small_frame() {
        // 20 bytes should fit in smallest codes
        let idx = select_tag(20, 16);
        assert!(idx.is_some());
        let tag = &FX25_TAGS[idx.unwrap()];
        assert!(tag.rs_k >= 20);
        assert!(tag.check_bytes >= 16);
    }

    #[test]
    fn select_tag_large_frame() {
        // 200 bytes needs RS(255,223) or RS(255,239)
        let idx = select_tag(200, 16);
        assert!(idx.is_some());
        let tag = &FX25_TAGS[idx.unwrap()];
        assert!(tag.rs_k >= 200);
    }

    #[test]
    fn select_tag_too_large() {
        // 256 bytes won't fit in any code
        let idx = select_tag(256, 16);
        assert!(idx.is_none());
    }
}
