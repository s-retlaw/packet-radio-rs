//! HDLC Framing — flag detection, bit stuffing/unstuffing, and frame assembly.
//!
//! HDLC frames are delimited by flag bytes (0x7E = 01111110). Inside a frame,
//! any sequence of five consecutive 1-bits is followed by a stuffed 0-bit
//! to prevent false flag detection. The receiver must remove these stuffed bits.

use crate::MAX_FRAME_LEN;
use super::Address;

/// Maximum number of bits in an encoded HDLC frame
const MAX_ENCODED_BITS: usize = 4096;

/// An HDLC-encoded frame as a sequence of bits.
pub struct EncodedFrame {
    /// Each element is 0 or 1
    pub bits: [u8; MAX_ENCODED_BITS],
    /// Number of valid bits in the buffer
    pub bit_count: usize,
}

impl EncodedFrame {
    /// Create an empty encoded frame.
    const fn new() -> Self {
        Self {
            bits: [0u8; MAX_ENCODED_BITS],
            bit_count: 0,
        }
    }

    /// Push a single bit. Returns false if buffer is full.
    fn push_bit(&mut self, bit: u8) -> bool {
        if self.bit_count >= MAX_ENCODED_BITS {
            return false;
        }
        self.bits[self.bit_count] = bit;
        self.bit_count += 1;
        true
    }
}

/// Encode raw frame bytes into an HDLC bit stream.
///
/// Takes raw frame data (address + control + PID + info, WITHOUT CRC).
/// Computes CRC, appends it, bit-stuffs the content, and wraps with flag bytes.
/// Bits are output LSB first within each byte.
pub fn hdlc_encode(data: &[u8]) -> EncodedFrame {
    let mut frame = EncodedFrame::new();

    // Compute CRC over the raw data
    let crc = super::crc16_ccitt(data);
    let crc_lo = crc as u8;
    let crc_hi = (crc >> 8) as u8;

    // Emit preamble flags (4x 0x7E) — NOT bit-stuffed
    for _ in 0..4 {
        emit_flag(&mut frame);
    }

    // Bit-stuff and emit data bytes, then CRC bytes (low first, then high)
    let mut ones_count: u8 = 0;
    for &byte in data.iter().chain(&[crc_lo, crc_hi]) {
        // LSB first
        for bit_pos in 0..8 {
            let bit = (byte >> bit_pos) & 1;
            frame.push_bit(bit);
            if bit == 1 {
                ones_count += 1;
                if ones_count == 5 {
                    // Insert stuffed zero
                    frame.push_bit(0);
                    ones_count = 0;
                }
            } else {
                ones_count = 0;
            }
        }
    }

    // Emit postamble flag — NOT bit-stuffed
    emit_flag(&mut frame);

    frame
}

/// Emit a single flag byte (0x7E = 01111110) as raw bits, LSB first.
fn emit_flag(frame: &mut EncodedFrame) {
    // 0x7E = 0b01111110, LSB first: 0,1,1,1,1,1,1,0
    let flag: u8 = 0x7E;
    for bit_pos in 0..8 {
        frame.push_bit((flag >> bit_pos) & 1);
    }
}

/// Build a test AX.25 UI frame from source, destination, and info payload.
///
/// Returns the raw frame bytes (address + control + PID + info) in a fixed-size buffer,
/// along with the number of valid bytes.
pub fn build_test_frame(
    src: &str,
    dest: &str,
    info: &[u8],
) -> ([u8; MAX_FRAME_LEN], usize) {
    let mut buf = [0u8; MAX_FRAME_LEN];
    let mut pos = 0;

    // Build destination address
    let dest_addr = address_from_str(dest);
    let mut dest_bytes = [0u8; 7];
    dest_addr.to_bytes(&mut dest_bytes, false);
    buf[pos..pos + 7].copy_from_slice(&dest_bytes);
    pos += 7;

    // Build source address (last address)
    let src_addr = address_from_str(src);
    let mut src_bytes = [0u8; 7];
    src_addr.to_bytes(&mut src_bytes, true);
    buf[pos..pos + 7].copy_from_slice(&src_bytes);
    pos += 7;

    // Control field: UI frame
    buf[pos] = 0x03;
    pos += 1;

    // PID: no layer 3
    buf[pos] = 0xF0;
    pos += 1;

    // Info payload
    let info_len = info.len().min(MAX_FRAME_LEN - pos);
    buf[pos..pos + info_len].copy_from_slice(&info[..info_len]);
    pos += info_len;

    (buf, pos)
}

/// Helper to create an Address from a callsign string.
fn address_from_str(call: &str) -> Address {
    let mut callsign = [b' '; 6];
    let bytes = call.as_bytes();
    let len = bytes.len().min(6);
    for i in 0..len {
        callsign[i] = bytes[i];
    }
    Address {
        callsign,
        callsign_len: len as u8,
        ssid: 0,
        h_bit: false,
    }
}

/// HDLC decoder state
#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// Searching for a flag sequence (01111110)
    Hunting,
    /// Currently receiving frame data
    Receiving,
}

/// HDLC frame decoder.
///
/// Fed one bit at a time from the AFSK demodulator. Produces complete
/// AX.25 frames (without flags or CRC — CRC is verified internally).
pub struct HdlcDecoder {
    state: State,
    /// Shift register for flag/abort detection
    shift_reg: u8,
    /// Count of consecutive 1-bits (for bit unstuffing)
    ones_count: u8,
    /// Frame assembly buffer
    frame_buf: [u8; MAX_FRAME_LEN],
    /// Current byte being assembled
    current_byte: u8,
    /// Bits received in current byte (0-7)
    bit_index: u8,
    /// Total bytes received in current frame
    frame_len: usize,
}

impl HdlcDecoder {
    /// Create a new HDLC decoder.
    pub const fn new() -> Self {
        Self {
            state: State::Hunting,
            shift_reg: 0,
            ones_count: 0,
            frame_buf: [0u8; MAX_FRAME_LEN],
            current_byte: 0,
            bit_index: 0,
            frame_len: 0,
        }
    }

    /// Reset the decoder to the hunting state.
    pub fn reset(&mut self) {
        self.state = State::Hunting;
        self.shift_reg = 0;
        self.ones_count = 0;
        self.current_byte = 0;
        self.bit_index = 0;
        self.frame_len = 0;
    }

    /// Feed a single bit into the decoder.
    ///
    /// Returns `Some(slice)` containing a complete, CRC-verified frame
    /// (without the CRC bytes) when a valid frame has been received.
    /// Returns `None` otherwise.
    pub fn feed_bit(&mut self, bit: bool) -> Option<&[u8]> {
        // Update shift register (tracks last 8 bits for flag detection)
        self.shift_reg = (self.shift_reg >> 1) | if bit { 0x80 } else { 0x00 };

        // Check for flag (0x7E = 01111110)
        if self.shift_reg == 0x7E {
            let result = if self.state == State::Receiving && self.frame_len >= 17 {
                // We have a complete frame — verify CRC
                // Frame includes 2 CRC bytes at the end
                let frame_data = &self.frame_buf[..self.frame_len];
                let crc = super::crc16_ccitt(frame_data);

                if crc == 0x0F47 {
                    // Valid CRC! Return frame without the 2 CRC bytes
                    Some(&self.frame_buf[..self.frame_len - 2])
                } else {
                    None // CRC mismatch
                }
            } else {
                None
            };

            // Reset for next frame (flag is also start of next frame)
            self.state = State::Receiving;
            self.ones_count = 0;
            self.current_byte = 0;
            self.bit_index = 0;
            self.frame_len = 0;

            return result;
        }

        // Check for abort (7+ consecutive 1-bits)
        if self.ones_count >= 7 {
            self.state = State::Hunting;
            self.ones_count = 0;
            return None;
        }

        match self.state {
            State::Hunting => {
                // Just tracking shift_reg, waiting for flag
                if bit {
                    self.ones_count += 1;
                } else {
                    self.ones_count = 0;
                }
                None
            }
            State::Receiving => {
                if bit {
                    self.ones_count += 1;

                    // Accumulate the bit
                    self.current_byte = (self.current_byte >> 1) | 0x80;
                    self.bit_index += 1;
                } else {
                    if self.ones_count == 5 {
                        // This is a stuffed bit — discard it
                        self.ones_count = 0;
                        return None;
                    }
                    self.ones_count = 0;

                    // Accumulate the bit
                    self.current_byte >>= 1; // 0 bit (MSB already 0 after shift)
                    self.bit_index += 1;
                }

                // Complete byte?
                if self.bit_index >= 8 {
                    if self.frame_len < MAX_FRAME_LEN {
                        self.frame_buf[self.frame_len] = self.current_byte;
                        self.frame_len += 1;
                    } else {
                        // Frame too long — abort
                        self.state = State::Hunting;
                    }
                    self.current_byte = 0;
                    self.bit_index = 0;
                }

                None
            }
        }
    }

    /// Feed multiple bits at once.
    ///
    /// Calls the provided callback for each complete valid frame.
    pub fn feed_bits<F>(&mut self, bits: &[u8], mut on_frame: F)
    where
        F: FnMut(&[u8]),
    {
        for &bit in bits {
            if let Some(frame) = self.feed_bit(bit != 0) {
                // Copy the frame data before calling the callback,
                // since the buffer will be reused
                // TODO: Use a callback-friendly approach that avoids the copy
                on_frame(frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hdlc_decoder_initial_state() {
        let decoder = HdlcDecoder::new();
        assert_eq!(decoder.state, State::Hunting);
        assert_eq!(decoder.frame_len, 0);
    }

    #[test]
    fn test_hdlc_encode_roundtrip() {
        // Build a test frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Hello, World!");
        let raw = &frame_data[..frame_len];

        // Encode it
        let encoded = hdlc_encode(raw);

        // Decode it
        let mut decoder = HdlcDecoder::new();
        let mut decoded: Option<([u8; 330], usize)> = None;
        for i in 0..encoded.bit_count {
            if let Some(frame) = decoder.feed_bit(encoded.bits[i] != 0) {
                let mut buf = [0u8; 330];
                let len = frame.len();
                buf[..len].copy_from_slice(frame);
                decoded = Some((buf, len));
            }
        }

        let (dec_buf, dec_len) = decoded.expect("Should have decoded a frame");
        assert_eq!(&dec_buf[..dec_len], raw);
    }

    #[test]
    fn test_build_test_frame_structure() {
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Test");
        // dest(7) + src(7) + control(1) + pid(1) + info(4) = 20
        assert_eq!(frame_len, 20);
        // Control = 0x03
        assert_eq!(frame_data[14], 0x03);
        // PID = 0xF0
        assert_eq!(frame_data[15], 0xF0);
        // Info
        assert_eq!(&frame_data[16..20], b"Test");
    }
}
