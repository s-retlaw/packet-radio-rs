//! APRS Protocol — encoding and decoding of APRS packets.
//!
//! APRS (Automatic Packet Reporting System) is carried in the information
//! field of AX.25 UI frames. The first byte of the info field is the
//! Data Type Identifier (DTI) which determines the packet format.
//!
//! Reference: APRS Protocol Reference v1.0.1 (APRS101.PDF)

/// APRS Data Type Identifier
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DataType {
    /// `!` — Position without timestamp, no messaging
    PositionNoTimestamp,
    /// `=` — Position without timestamp, with messaging
    PositionNoTimestampMsg,
    /// `/` — Position with timestamp, no messaging
    PositionWithTimestamp,
    /// `@` — Position with timestamp, with messaging
    PositionWithTimestampMsg,
    /// `` ` `` or `'` — Mic-E encoded position
    MicE,
    /// `:` — Message (person-to-person, bulletin, announcement)
    Message,
    /// `;` — Object
    Object,
    /// `)` — Item
    Item,
    /// `_` — Weather report (positionless)
    Weather,
    /// `T` — Telemetry
    Telemetry,
    /// `>` — Status
    Status,
    /// `<` — Station capabilities
    Capabilities,
    /// `{` — User-defined
    UserDefined,
    /// `?` — Query
    Query,
    /// Unknown/unsupported DTI
    Unknown(u8),
}

impl DataType {
    /// Determine the data type from the first byte of the info field.
    pub fn from_dti(byte: u8) -> Self {
        match byte {
            b'!' => Self::PositionNoTimestamp,
            b'=' => Self::PositionNoTimestampMsg,
            b'/' => Self::PositionWithTimestamp,
            b'@' => Self::PositionWithTimestampMsg,
            b'`' | b'\'' => Self::MicE,
            b':' => Self::Message,
            b';' => Self::Object,
            b')' => Self::Item,
            b'_' => Self::Weather,
            b'T' => Self::Telemetry,
            b'>' => Self::Status,
            b'<' => Self::Capabilities,
            b'{' => Self::UserDefined,
            b'?' => Self::Query,
            other => Self::Unknown(other),
        }
    }
}

/// Parsed APRS position (latitude/longitude).
#[derive(Clone, Debug, Default)]
pub struct Position {
    /// Latitude in degrees (positive = North, negative = South)
    pub lat: i32, // Fixed-point: degrees * 1_000_000
    /// Longitude in degrees (positive = East, negative = West)
    pub lon: i32, // Fixed-point: degrees * 1_000_000
    /// Position ambiguity level (0 = exact, 1-4 = progressively less precise)
    pub ambiguity: u8,
}

/// Parsed APRS packet.
#[derive(Debug)]
pub enum AprsPacket<'a> {
    /// Position report
    Position {
        position: Position,
        symbol_table: u8,
        symbol_code: u8,
        comment: &'a [u8],
    },
    /// Message
    Message {
        addressee: &'a [u8],
        text: &'a [u8],
        message_no: Option<&'a [u8]>,
    },
    /// Status report
    Status {
        text: &'a [u8],
    },
    /// Mic-E encoded position
    MicE {
        position: Position,
        speed: u16,    // knots
        course: u16,   // degrees
        symbol_table: u8,
        symbol_code: u8,
    },
    /// Unrecognized packet type
    Unknown {
        dti: u8,
        data: &'a [u8],
    },
}

/// Parse an APRS packet from the information field of an AX.25 frame.
///
/// The `dest` parameter is needed for Mic-E decoding, where latitude
/// is encoded in the destination address.
pub fn parse_packet<'a>(info: &'a [u8], dest_callsign: &[u8]) -> Option<AprsPacket<'a>> {
    if info.is_empty() {
        return None;
    }

    let dti = DataType::from_dti(info[0]);

    match dti {
        DataType::PositionNoTimestamp | DataType::PositionNoTimestampMsg => {
            parse_position_no_timestamp(info)
        }
        DataType::PositionWithTimestamp | DataType::PositionWithTimestampMsg => {
            parse_position_with_timestamp(info)
        }
        DataType::MicE => {
            parse_mic_e(info, dest_callsign)
        }
        DataType::Message => {
            parse_message(info)
        }
        DataType::Status => {
            Some(AprsPacket::Status { text: &info[1..] })
        }
        _ => {
            Some(AprsPacket::Unknown { dti: info[0], data: &info[1..] })
        }
    }
}

/// Parse a position report without timestamp.
/// Format: `!DDMM.MMN/DDDMM.MMW$...`  (or compressed)
fn parse_position_no_timestamp<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // TODO: Implement position parsing
    // - Check if compressed (info[1] is '/' or '\')
    // - Parse latitude (DDMM.MM + N/S)
    // - Parse symbol table character
    // - Parse longitude (DDDMM.MM + E/W)
    // - Parse symbol code
    // - Remaining bytes are comment
    Some(AprsPacket::Unknown { dti: info[0], data: &info[1..] })
}

/// Parse a position report with timestamp.
fn parse_position_with_timestamp<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // TODO: Implement
    // Format: /DDHHMM[zh/]DDMM.MMN/DDDMM.MMW$...
    Some(AprsPacket::Unknown { dti: info[0], data: &info[1..] })
}

/// Parse Mic-E encoded position.
///
/// Mic-E encodes latitude in the destination address field and
/// longitude/speed/course in the information field. This is the
/// most complex APRS format to parse.
fn parse_mic_e<'a>(info: &'a [u8], _dest: &[u8]) -> Option<AprsPacket<'a>> {
    // TODO: Implement Mic-E decoding
    // See APRS101.PDF Chapter 10
    // 1. Extract latitude from destination address digits
    // 2. Extract longitude from info bytes [1..4]
    // 3. Extract speed/course from info bytes [4..7]
    // 4. Extract symbol from info bytes
    Some(AprsPacket::Unknown { dti: info[0], data: &info[1..] })
}

/// Parse an APRS message.
/// Format: `:ADDRESSEE:message text{message_no`
fn parse_message<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // TODO: Implement message parsing
    // - Addressee is 9 characters, space-padded
    // - Text follows after ':'
    // - Optional message number after '{'
    Some(AprsPacket::Unknown { dti: info[0], data: &info[1..] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dti_identification() {
        assert_eq!(DataType::from_dti(b'!'), DataType::PositionNoTimestamp);
        assert_eq!(DataType::from_dti(b':'), DataType::Message);
        assert_eq!(DataType::from_dti(b'`'), DataType::MicE);
        assert_eq!(DataType::from_dti(b'>'), DataType::Status);
    }
}
