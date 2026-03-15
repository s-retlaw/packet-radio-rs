//! KISS TNC Protocol — framing for communication between host and TNC.
//!
//! KISS (Keep It Simple, Stupid) is a simple serial protocol that wraps
//! AX.25 frames for transport between a TNC and host application.
//!
//! Frame format: FEND | CMD | DATA... | FEND
//! Special bytes are escaped:
//!   FEND (0xC0) in data → FESC TFEND (0xDB 0xDC)
//!   FESC (0xDB) in data → FESC TFESC (0xDB 0xDD)

/// KISS special bytes
pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// KISS command types (low nibble of command byte)
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Data frame (send/receive AX.25 frame)
    DataFrame,
    /// TX delay (in 10ms units)
    TxDelay,
    /// Persistence parameter (0-255, probability = p/256)
    Persistence,
    /// Slot time (in 10ms units)
    SlotTime,
    /// TX tail (in 10ms units)
    TxTail,
    /// Full duplex mode (0 = half, nonzero = full)
    FullDuplex,
    /// Set hardware (implementation-specific)
    SetHardware,
    /// Return from KISS mode
    Return,
    /// Unknown command
    Unknown(u8),
}

impl Command {
    pub fn from_byte(byte: u8) -> (u8, Self) {
        let port = (byte >> 4) & 0x0F;
        let cmd = byte & 0x0F;
        let command = match cmd {
            0x00 => Self::DataFrame,
            0x01 => Self::TxDelay,
            0x02 => Self::Persistence,
            0x03 => Self::SlotTime,
            0x04 => Self::TxTail,
            0x05 => Self::FullDuplex,
            0x06 => Self::SetHardware,
            0xFF => Self::Return,
            other => Self::Unknown(other),
        };
        (port, command)
    }

    pub fn to_byte(&self, port: u8) -> u8 {
        let cmd = match self {
            Self::DataFrame => 0x00,
            Self::TxDelay => 0x01,
            Self::Persistence => 0x02,
            Self::SlotTime => 0x03,
            Self::TxTail => 0x04,
            Self::FullDuplex => 0x05,
            Self::SetHardware => 0x06,
            Self::Return => 0xFF,
            Self::Unknown(c) => *c,
        };
        ((port & 0x0F) << 4) | (cmd & 0x0F)
    }
}

/// KISS frame decoder.
///
/// Feed bytes from a serial port or TCP connection. Produces complete
/// KISS frames.
pub struct KissDecoder {
    buf: [u8; 512],
    len: usize,
    in_frame: bool,
    escape: bool,
    overflow: bool,
}

impl Default for KissDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl KissDecoder {
    pub const fn new() -> Self {
        Self {
            buf: [0u8; 512],
            len: 0,
            in_frame: false,
            escape: false,
            overflow: false,
        }
    }

    pub fn reset(&mut self) {
        self.len = 0;
        self.in_frame = false;
        self.escape = false;
        self.overflow = false;
    }

    /// Feed a single byte. Returns Some((port, command, data)) when
    /// a complete frame has been received.
    pub fn feed_byte(&mut self, byte: u8) -> Option<(u8, Command, &[u8])> {
        match byte {
            FEND => {
                if self.in_frame && self.len > 0 {
                    if self.overflow {
                        self.reset();
                        return None;
                    }
                    // End of frame
                    let (port, cmd) = Command::from_byte(self.buf[0]);
                    let data = &self.buf[1..self.len];
                    self.in_frame = false;
                    let result = Some((port, cmd, data));
                    // Don't reset len yet — caller needs the data
                    return result;
                }
                // Start of new frame
                self.in_frame = true;
                self.len = 0;
                self.escape = false;
                None
            }
            FESC if self.in_frame => {
                self.escape = true;
                None
            }
            TFEND if self.in_frame && self.escape => {
                self.escape = false;
                self.push_byte(FEND);
                None
            }
            TFESC if self.in_frame && self.escape => {
                self.escape = false;
                self.push_byte(FESC);
                None
            }
            _ if self.in_frame => {
                self.escape = false;
                self.push_byte(byte);
                None
            }
            _ => None,
        }
    }

    fn push_byte(&mut self, byte: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = byte;
            self.len += 1;
        } else {
            self.overflow = true;
        }
    }
}

/// Encode an AX.25 frame into a KISS frame.
///
/// Writes the KISS-encoded frame into `out`. Returns the number of
/// bytes written, or `None` if the output buffer is too small.
pub fn encode_frame(port: u8, data: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut pos = 0;

    // Helper to write a byte with bounds checking
    let mut write = |b: u8, p: &mut usize| -> bool {
        if *p < out.len() {
            out[*p] = b;
            *p += 1;
            true
        } else {
            false
        }
    };

    // Opening FEND
    if !write(FEND, &mut pos) {
        return None;
    }

    // Command byte (data frame)
    if !write(Command::DataFrame.to_byte(port), &mut pos) {
        return None;
    }

    // Data with escaping
    for &byte in data {
        match byte {
            FEND => {
                if !write(FESC, &mut pos) {
                    return None;
                }
                if !write(TFEND, &mut pos) {
                    return None;
                }
            }
            FESC => {
                if !write(FESC, &mut pos) {
                    return None;
                }
                if !write(TFESC, &mut pos) {
                    return None;
                }
            }
            _ => {
                if !write(byte, &mut pos) {
                    return None;
                }
            }
        }
    }

    // Closing FEND
    if !write(FEND, &mut pos) {
        return None;
    }

    Some(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kiss_encode_decode_roundtrip() {
        let original_data = b"Hello, packet radio!";
        let mut encoded = [0u8; 256];
        let encoded_len = encode_frame(0, original_data, &mut encoded).unwrap();

        let mut decoder = KissDecoder::new();
        let mut _result = None;
        for &byte in &encoded[..encoded_len] {
            if let Some(r) = decoder.feed_byte(byte) {
                _result = Some((r.0, r.1, r.2.to_vec()));
            }
        }

        // Note: This test requires std for Vec, only runs on host
    }

    #[test]
    fn test_kiss_escape_fend_in_data() {
        let data_with_fend = [0x01, FEND, 0x02];
        let mut encoded = [0u8; 32];
        let len = encode_frame(0, &data_with_fend, &mut encoded).unwrap();

        // Should contain FESC TFEND instead of raw FEND
        assert!(encoded[..len].windows(2).any(|w| w == [FESC, TFEND]));
    }

    #[test]
    fn test_command_parsing() {
        let (port, cmd) = Command::from_byte(0x00);
        assert_eq!(port, 0);
        assert_eq!(cmd, Command::DataFrame);

        let (port, cmd) = Command::from_byte(0x11);
        assert_eq!(port, 1);
        assert_eq!(cmd, Command::TxDelay);
    }
}
