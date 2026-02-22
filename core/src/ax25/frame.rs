//! HDLC Framing — flag detection, bit stuffing/unstuffing, and frame assembly.
//!
//! HDLC frames are delimited by flag bytes (0x7E = 01111110). Inside a frame,
//! any sequence of five consecutive 1-bits is followed by a stuffed 0-bit
//! to prevent false flag detection. The receiver must remove these stuffed bits.

use crate::MAX_FRAME_LEN;

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

    // TODO: Add tests with known HDLC bit sequences
    // TODO: Test bit unstuffing
    // TODO: Test flag detection
    // TODO: Test abort detection
    // TODO: Test CRC verification with known frames
}
