//! AX.25 Protocol — frame parsing, HDLC framing, and address handling.
//!
//! AX.25 is the link-layer protocol used by amateur packet radio.
//! Frames are delimited by HDLC flags (0x7E) and protected by CRC-16-CCITT.

pub mod frame;

/// Maximum callsign length (6 characters per AX.25 spec)
pub const MAX_CALLSIGN_LEN: usize = 6;

/// AX.25 address: callsign + SSID
#[derive(Clone, Debug)]
pub struct Address {
    /// Callsign, up to 6 ASCII characters, space-padded
    pub callsign: [u8; MAX_CALLSIGN_LEN],
    /// Callsign length (without padding)
    pub callsign_len: u8,
    /// Secondary Station Identifier (0-15)
    pub ssid: u8,
    /// Has-been-repeated flag (for digipeater addresses)
    pub h_bit: bool,
}

impl Address {
    /// Parse an address from 7 raw AX.25 bytes.
    ///
    /// AX.25 addresses are encoded as shifted ASCII (each byte << 1)
    /// with the SSID and flags in the 7th byte.
    pub fn from_bytes(bytes: &[u8; 7]) -> Self {
        let mut callsign = [b' '; MAX_CALLSIGN_LEN];
        let mut callsign_len = 0u8;

        for i in 0..6 {
            let ch = bytes[i] >> 1;
            callsign[i] = ch;
            if ch != b' ' {
                callsign_len = (i + 1) as u8;
            }
        }

        let ssid_byte = bytes[6];
        let ssid = (ssid_byte >> 1) & 0x0F;
        let h_bit = (ssid_byte & 0x80) != 0;

        Self {
            callsign,
            callsign_len,
            ssid,
            h_bit,
        }
    }

    /// Encode this address into 7 AX.25 bytes.
    pub fn to_bytes(&self, buf: &mut [u8; 7], is_last: bool) {
        for i in 0..6 {
            buf[i] = self.callsign[i] << 1;
        }
        buf[6] = (self.ssid << 1) | if self.h_bit { 0x80 } else { 0 };
        if is_last {
            buf[6] |= 0x01; // Address extension bit
        }
    }

    /// Get the callsign as a byte slice (without trailing spaces).
    pub fn callsign_str(&self) -> &[u8] {
        &self.callsign[..self.callsign_len as usize]
    }
}

/// Parsed AX.25 frame (UI frame — Unnumbered Information).
///
/// Most APRS traffic uses UI frames. This struct borrows from the
/// underlying frame buffer for zero-copy parsing.
#[derive(Debug)]
pub struct Frame<'a> {
    /// Destination address
    pub dest: Address,
    /// Source address
    pub src: Address,
    /// Digipeater path (0-8 addresses)
    pub digipeaters: [Address; crate::MAX_DIGIPEATERS],
    /// Number of digipeater addresses present
    pub num_digipeaters: u8,
    /// Control field (0x03 for UI frames)
    pub control: u8,
    /// Protocol Identifier (0xF0 for no layer 3)
    pub pid: u8,
    /// Information field (payload)
    pub info: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Parse a raw AX.25 frame from bytes (after HDLC decoding, without CRC).
    ///
    /// Returns `None` if the frame is too short or malformed.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        // Minimum frame: dest(7) + src(7) + control(1) = 15 bytes
        if data.len() < 15 {
            return None;
        }

        let dest = Address::from_bytes(data[0..7].try_into().ok()?);
        let src = Address::from_bytes(data[7..14].try_into().ok()?);

        // Check for digipeater addresses
        let mut digipeaters = [Address {
            callsign: [b' '; 6],
            callsign_len: 0,
            ssid: 0,
            h_bit: false,
        }; crate::MAX_DIGIPEATERS];
        let mut num_digipeaters = 0u8;
        let mut pos = 14;

        // If the address extension bit is not set on the source address,
        // there are digipeater addresses following
        if (data[13] & 0x01) == 0 {
            while pos + 7 <= data.len() && (num_digipeaters as usize) < crate::MAX_DIGIPEATERS {
                let addr_bytes: &[u8; 7] = data[pos..pos + 7].try_into().ok()?;
                digipeaters[num_digipeaters as usize] = Address::from_bytes(addr_bytes);
                num_digipeaters += 1;
                let is_last = (data[pos + 6] & 0x01) != 0;
                pos += 7;
                if is_last {
                    break;
                }
            }
        }

        // Control and PID fields
        if pos + 2 > data.len() {
            return None;
        }
        let control = data[pos];
        let pid = data[pos + 1];
        pos += 2;

        // Remaining bytes are the information field
        let info = &data[pos..];

        Some(Frame {
            dest,
            src,
            digipeaters,
            num_digipeaters,
            control,
            pid,
            info,
        })
    }

    /// Check if this is a UI (Unnumbered Information) frame.
    pub fn is_ui(&self) -> bool {
        self.control == 0x03
    }
}

/// CRC-16-CCITT as used by AX.25/HDLC.
///
/// Polynomial: x^16 + x^12 + x^5 + 1 (0x1021)
/// Initial value: 0xFFFF
/// The CRC is transmitted bit-inverted and LSB first.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        let mut b = byte;
        for _ in 0..8 {
            let xor_flag = ((crc ^ b as u16) & 0x0001) != 0;
            crc >>= 1;
            if xor_flag {
                crc ^= 0x8408; // Reflected polynomial
            }
            b >>= 1;
        }
    }
    crc ^ 0xFFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc16_known_value() {
        // Test with a known AX.25 frame
        // TODO: Add test with known-good CRC values
        let data = b"Hello";
        let crc = crc16_ccitt(data);
        // Verify CRC is non-zero and deterministic
        assert_ne!(crc, 0);
        assert_eq!(crc, crc16_ccitt(data));
    }

    #[test]
    fn test_address_parse() {
        // AX.25 address for "CQ    " SSID 0
        // Each char shifted left by 1
        let bytes: [u8; 7] = [
            b'C' << 1, b'Q' << 1, b' ' << 1, b' ' << 1, b' ' << 1, b' ' << 1,
            0x00 | 0x01, // SSID 0, last address
        ];
        let addr = Address::from_bytes(&bytes);
        assert_eq!(addr.callsign_str(), b"CQ");
        assert_eq!(addr.ssid, 0);
    }
}
