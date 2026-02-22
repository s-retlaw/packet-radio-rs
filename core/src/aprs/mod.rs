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

/// Parse a single ASCII digit byte to its numeric value.
fn parse_digit(b: u8) -> Option<u32> {
    if b >= b'0' && b <= b'9' {
        Some((b - b'0') as u32)
    } else {
        None
    }
}

/// Decode a 4-byte base-91 encoded value.
fn base91_decode(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }
    let mut val = 0u32;
    for &b in &bytes[0..4] {
        if b < 33 || b > 124 {
            return None;
        }
        val = val * 91 + (b - 33) as u32;
    }
    Some(val)
}

/// Parse plain (uncompressed) APRS position starting at `offset` in `info`.
///
/// Position data at offset: `DDMM.MMN/DDDMM.MME$comment`
/// (8 bytes lat + 1 symbol_table + 9 bytes lon + 1 symbol_code + comment)
fn parse_plain_position<'a>(info: &'a [u8], offset: usize) -> Option<AprsPacket<'a>> {
    // Need at least 19 bytes: 8 lat + 1 sym_table + 9 lon + 1 sym_code
    if info.len() < offset + 19 {
        return None;
    }
    let pos = &info[offset..];

    // Copy lat/lon bytes so we can replace spaces with '0'
    let mut lat_bytes = [0u8; 8];
    lat_bytes.copy_from_slice(&pos[0..8]);
    let mut lon_bytes = [0u8; 9];
    lon_bytes.copy_from_slice(&pos[9..18]);

    // Count ambiguity: spaces in latitude digit positions (right to left)
    // Lat format "DDMM.MMN" — digit indices: 0,1,2,3,5,6
    let mut ambiguity = 0u8;
    for &idx in &[0usize, 1, 2, 3, 5, 6] {
        if lat_bytes[idx] == b' ' {
            ambiguity += 1;
            lat_bytes[idx] = b'0';
        }
    }
    // Replace spaces in longitude digits too: "DDDMM.MME" — indices 0,1,2,3,4,6,7
    for &idx in &[0usize, 1, 2, 3, 4, 6, 7] {
        if lon_bytes[idx] == b' ' {
            lon_bytes[idx] = b'0';
        }
    }

    // Parse latitude: DDMM.MMN
    let lat_deg = parse_digit(lat_bytes[0])? * 10 + parse_digit(lat_bytes[1])?;
    let lat_min_int = parse_digit(lat_bytes[2])? * 10 + parse_digit(lat_bytes[3])?;
    // lat_bytes[4] == b'.'
    let lat_min_frac = parse_digit(lat_bytes[5])? * 10 + parse_digit(lat_bytes[6])?;
    let lat_ns = lat_bytes[7];

    // Convert to microdegrees: DD * 1_000_000 + MM.MM * 1_000_000 / 60
    // MM.MM as hundredths = min_int * 100 + min_frac
    // microdeg from minutes = (hundredths * 10_000 + 30) / 60  (rounded)
    let lat_min_hundredths = lat_min_int * 100 + lat_min_frac;
    let mut lat = (lat_deg * 1_000_000 + (lat_min_hundredths * 10_000 + 30) / 60) as i32;
    if lat_ns == b'S' {
        lat = -lat;
    }

    let symbol_table = pos[8];

    // Parse longitude: DDDMM.MME
    let lon_deg = parse_digit(lon_bytes[0])? * 100
        + parse_digit(lon_bytes[1])? * 10
        + parse_digit(lon_bytes[2])?;
    let lon_min_int = parse_digit(lon_bytes[3])? * 10 + parse_digit(lon_bytes[4])?;
    // lon_bytes[5] == b'.'
    let lon_min_frac = parse_digit(lon_bytes[6])? * 10 + parse_digit(lon_bytes[7])?;
    let lon_ew = lon_bytes[8];

    let lon_min_hundredths = lon_min_int * 100 + lon_min_frac;
    let mut lon = (lon_deg * 1_000_000 + (lon_min_hundredths * 10_000 + 30) / 60) as i32;
    if lon_ew == b'W' {
        lon = -lon;
    }

    let symbol_code = pos[18];
    let comment = if info.len() > offset + 19 {
        &info[offset + 19..]
    } else {
        &[]
    };

    Some(AprsPacket::Position {
        position: Position { lat, lon, ambiguity },
        symbol_table,
        symbol_code,
        comment,
    })
}

/// Parse compressed APRS position starting at `offset` in `info`.
///
/// At offset: symbol_table (1) + lat base91 (4) + lon base91 (4) + symbol_code (1)
/// + optional cs/type (3)
fn parse_compressed_position<'a>(info: &'a [u8], offset: usize) -> Option<AprsPacket<'a>> {
    // Minimum: 1 sym_table + 4 lat + 4 lon + 1 sym_code = 10
    if info.len() < offset + 10 {
        return None;
    }
    let pos = &info[offset..];
    let symbol_table = pos[0];

    let lat_val = base91_decode(&pos[1..5])?;
    let lon_val = base91_decode(&pos[5..9])?;

    // lat = 90.0 - value / 380926.0, in microdegrees using i64
    let lat = (90_000_000i64 - (lat_val as i64) * 1_000_000 / 380926) as i32;
    // lon = -180.0 + value / 190463.0, in microdegrees using i64
    let lon = (-180_000_000i64 + (lon_val as i64) * 1_000_000 / 190463) as i32;

    let symbol_code = pos[9];

    // Skip cs/type bytes (3) if present
    let data_len = if pos.len() >= 13 { 13 } else { 10 };
    let comment = if info.len() > offset + data_len {
        &info[offset + data_len..]
    } else {
        &[]
    };

    Some(AprsPacket::Position {
        position: Position { lat, lon, ambiguity: 0 },
        symbol_table,
        symbol_code,
        comment,
    })
}

/// Parse a position report without timestamp.
/// Format: `!DDMM.MMN/DDDMM.MMW$...`  (or compressed)
fn parse_position_no_timestamp<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    if info.len() < 2 {
        return None;
    }
    // Check if compressed: byte after DTI is symbol table '/' or '\'
    if info[1] == b'/' || info[1] == b'\\' {
        parse_compressed_position(info, 1)
    } else {
        parse_plain_position(info, 1)
    }
}

/// Parse a position report with timestamp.
/// Format: `/DDHHMMzDDMM.MMN/DDDMM.MMW$...`
fn parse_position_with_timestamp<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // DTI (1) + timestamp (7) = 8 bytes before position data
    if info.len() < 9 {
        return None;
    }
    let pos_start = 8;
    if info[pos_start] == b'/' || info[pos_start] == b'\\' {
        parse_compressed_position(info, pos_start)
    } else {
        parse_plain_position(info, pos_start)
    }
}

/// Decode a Mic-E destination callsign byte into a latitude digit and flag.
///
/// Returns `(digit, flag_set)` where flag_set indicates N/S, lon offset, or E/W
/// depending on byte position (indices 3, 4, 5 respectively).
fn mic_e_dest_byte(b: u8) -> Option<(u8, bool)> {
    match b {
        b'0'..=b'9' => Some((b - b'0', false)),
        b'A'..=b'J' => Some((b - b'A', true)),
        b'K'..=b'L' => Some((0, true)),
        b'P'..=b'Y' => Some((b - b'P', true)),
        b'Z' => Some((0, true)),
        _ => None,
    }
}

/// Parse Mic-E encoded position.
///
/// Mic-E encodes latitude in the destination address field and
/// longitude/speed/course in the information field. This is the
/// most complex APRS format to parse.
///
/// Reference: APRS101.PDF Chapter 10.
fn parse_mic_e<'a>(info: &'a [u8], dest: &[u8]) -> Option<AprsPacket<'a>> {
    // Need at least 9 bytes in info and 6 bytes in destination
    if info.len() < 9 || dest.len() < 6 {
        return None;
    }

    // Extract latitude digits and flags from destination callsign
    let mut lat_digits = [0u8; 6];
    let mut flags = [false; 6];
    for i in 0..6 {
        let (digit, flag) = mic_e_dest_byte(dest[i])?;
        lat_digits[i] = digit;
        flags[i] = flag;
    }

    // Latitude: DDMM.HH from the 6 destination digits
    let lat_deg = lat_digits[0] as i32 * 10 + lat_digits[1] as i32;
    let lat_min = lat_digits[2] as i32 * 10 + lat_digits[3] as i32;
    let lat_hun = lat_digits[4] as i32 * 10 + lat_digits[5] as i32;
    let mut lat = lat_deg * 1_000_000 + (lat_min * 100 + lat_hun) * 10_000 / 60;

    // N/S from destination byte index 3: flag set means North
    if !flags[3] {
        lat = -lat;
    }

    // Longitude offset (+100 degrees) from destination byte index 4
    let lon_offset = flags[4];
    // E/W from destination byte index 5: flag set means East
    let is_east = flags[5];

    // Longitude degrees from info[1] (Dire Wolf algorithm)
    let mut d = info[1] as i32 - 28;
    if lon_offset {
        d += 100;
    }
    if d >= 190 {
        d -= 190;
    }
    if d >= 180 {
        d -= 80;
    }

    // Longitude minutes from info[2]
    let mut m = info[2] as i32 - 28;
    if m >= 60 {
        m -= 60;
    }

    // Longitude hundredths of minutes from info[3]
    let h = info[3] as i32 - 28;

    let mut lon = d * 1_000_000 + (m * 100 + h) * 10_000 / 60;
    if !is_east {
        lon = -lon;
    }

    // Speed and course from info[4..7]
    let sp_tens = info[4] as i32 - 28;
    let sp_units_cse_hun = info[5] as i32 - 28;
    let cse_tens_units = info[6] as i32 - 28;

    let mut speed = sp_tens * 10 + sp_units_cse_hun / 10;
    let mut course = (sp_units_cse_hun % 10) * 100 + cse_tens_units;

    if speed >= 800 {
        speed -= 800;
    }
    if course >= 400 {
        course -= 400;
    }

    Some(AprsPacket::MicE {
        position: Position {
            lat,
            lon,
            ambiguity: 0,
        },
        speed: speed as u16,
        course: course as u16,
        symbol_table: info[8],
        symbol_code: info[7],
    })
}

/// Parse an APRS message.
/// Format: `:ADDRESSEE :message text{message_no`
///
/// Addressee is exactly 9 characters, space-padded on the right.
/// Message text follows the second ':'. Optional message number after '{'.
fn parse_message<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // Need at least DTI ':' + 9-char addressee + ':' = 11 bytes
    if info.len() < 11 {
        return None;
    }
    // info[0] = ':', info[1..10] = addressee (9 chars), info[10] = ':'
    if info[10] != b':' {
        return None;
    }

    // Trim trailing spaces from addressee
    let mut addr_end = 10;
    while addr_end > 1 && info[addr_end - 1] == b' ' {
        addr_end -= 1;
    }
    let addressee = &info[1..addr_end];

    let text_start = 11;
    let remaining = &info[text_start..];

    // Look for '{' separating text from message number
    let mut split = None;
    for (i, &b) in remaining.iter().enumerate() {
        if b == b'{' {
            split = Some(i);
            break;
        }
    }

    let (text, message_no) = match split {
        Some(idx) => {
            let msg_no = &remaining[idx + 1..];
            (&remaining[..idx], if msg_no.is_empty() { None } else { Some(msg_no) })
        }
        None => (remaining, None),
    };

    Some(AprsPacket::Message {
        addressee,
        text,
        message_no,
    })
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

    #[test]
    fn test_position_no_timestamp_north_west() {
        // !4903.50N/07201.75W-
        let info = b"!4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { position, symbol_table, symbol_code, comment } => {
                assert_eq!(position.lat, 49_058_333);
                assert_eq!(position.lon, -72_029_167);
                assert_eq!(position.ambiguity, 0);
                assert_eq!(symbol_table, b'/');
                assert_eq!(symbol_code, b'-');
                assert_eq!(comment, b"");
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_position_no_timestamp_south_east() {
        // !4903.50S/07201.75E-
        let info = b"!4903.50S/07201.75E-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { position, .. } => {
                assert_eq!(position.lat, -49_058_333);
                assert_eq!(position.lon, 72_029_167);
                assert_eq!(position.ambiguity, 0);
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_position_with_ambiguity() {
        // =4903.5 N/07201.7 W-
        let info = b"=4903.5 N/07201.7 W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { position, .. } => {
                assert_eq!(position.ambiguity, 1);
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_position_with_timestamp() {
        // /092345z4903.50N/07201.75W>
        let info = b"/092345z4903.50N/07201.75W>";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { position, symbol_table, symbol_code, .. } => {
                assert_eq!(position.lat, 49_058_333);
                assert_eq!(position.lon, -72_029_167);
                assert_eq!(position.ambiguity, 0);
                assert_eq!(symbol_table, b'/');
                assert_eq!(symbol_code, b'>');
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_position_with_timestamp_msg() {
        // @092345z4903.50N/07201.75W>
        let info = b"@092345z4903.50N/07201.75W>";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { position, .. } => {
                assert_eq!(position.lat, 49_058_333);
                assert_eq!(position.lon, -72_029_167);
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_position_with_comment() {
        let info = b"!4903.50N/07201.75W-PHG2360/Hello";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { comment, .. } => {
                assert_eq!(comment, b"PHG2360/Hello");
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_position_too_short() {
        assert!(parse_packet(b"!", b"").is_none());
        assert!(parse_packet(b"!490", b"").is_none());
    }

    #[test]
    fn test_position_messaging_dti() {
        // '=' is position with messaging capability
        let info = b"=4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { position, .. } => {
                assert_eq!(position.lat, 49_058_333);
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_base91_decode() {
        // All '!' (33) → value 0
        assert_eq!(base91_decode(b"!!!!"), Some(0));
        // '!' is 0, '"' is 1: "!!!\"" → 1
        assert_eq!(base91_decode(b"!!!\""), Some(1));
        // "!!\"!" → 91
        assert_eq!(base91_decode(b"!!\"!"), Some(91));
        // Invalid: byte < 33
        assert_eq!(base91_decode(b"!! !"), None);
    }

    #[test]
    fn test_parse_digit() {
        assert_eq!(parse_digit(b'0'), Some(0));
        assert_eq!(parse_digit(b'9'), Some(9));
        assert_eq!(parse_digit(b'5'), Some(5));
        assert_eq!(parse_digit(b'a'), None);
        assert_eq!(parse_digit(b' '), None);
    }

    #[test]
    fn test_message_with_number() {
        let info = b":WA1ABC   :Hello World{123";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { addressee, text, message_no } => {
                assert_eq!(addressee, b"WA1ABC");
                assert_eq!(text, b"Hello World");
                assert_eq!(message_no, Some(&b"123"[..]));
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_without_number() {
        let info = b":WA1ABC   :Hello World";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { addressee, text, message_no } => {
                assert_eq!(addressee, b"WA1ABC");
                assert_eq!(text, b"Hello World");
                assert_eq!(message_no, None);
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_empty_text() {
        let info = b":WA1ABC   :";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { addressee, text, message_no } => {
                assert_eq!(addressee, b"WA1ABC");
                assert_eq!(text, b"");
                assert_eq!(message_no, None);
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_full_addressee() {
        // 9-char addressee with no trailing spaces
        let info = b":ABCDEFGHI:test{42";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { addressee, text, message_no } => {
                assert_eq!(addressee, b"ABCDEFGHI");
                assert_eq!(text, b"test");
                assert_eq!(message_no, Some(&b"42"[..]));
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_too_short() {
        // Less than 11 bytes
        assert!(parse_packet(b":SHORT", b"").is_none());
    }

    #[test]
    fn test_status_packet() {
        let info = b">Net Control - Loss Angeles";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Status { text } => {
                assert_eq!(text, b"Net Control - Loss Angeles");
            }
            _ => panic!("expected Status variant"),
        }
    }

    #[test]
    fn test_status_empty() {
        let info = b">";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Status { text } => {
                assert_eq!(text, b"");
            }
            _ => panic!("expected Status variant"),
        }
    }

    #[test]
    fn test_message_bulletin() {
        let info = b":BLN3     :Snow expected in Langstraat area";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { addressee, text, message_no } => {
                assert_eq!(addressee, b"BLN3");
                assert_eq!(text, b"Snow expected in Langstraat area");
                assert_eq!(message_no, None);
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_with_msgno() {
        let info = b":N0CALL   :hello{001";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { addressee, text, message_no } => {
                assert_eq!(addressee, b"N0CALL");
                assert_eq!(text, b"hello");
                assert_eq!(message_no, Some(&b"001"[..]));
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_full_addr_with_ssid() {
        let info = b":WA1ABC-15:test{123";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { addressee, text, message_no } => {
                assert_eq!(addressee, b"WA1ABC-15");
                assert_eq!(text, b"test");
                assert_eq!(message_no, Some(&b"123"[..]));
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_mic_e_dest_byte_mapping() {
        // Digits 0-9
        assert_eq!(mic_e_dest_byte(b'0'), Some((0, false)));
        assert_eq!(mic_e_dest_byte(b'9'), Some((9, false)));
        // Custom A-J
        assert_eq!(mic_e_dest_byte(b'A'), Some((0, true)));
        assert_eq!(mic_e_dest_byte(b'J'), Some((9, true)));
        // Custom K-L (space digits)
        assert_eq!(mic_e_dest_byte(b'K'), Some((0, true)));
        assert_eq!(mic_e_dest_byte(b'L'), Some((0, true)));
        // Standard P-Y
        assert_eq!(mic_e_dest_byte(b'P'), Some((0, true)));
        assert_eq!(mic_e_dest_byte(b'Y'), Some((9, true)));
        assert_eq!(mic_e_dest_byte(b'Z'), Some((0, true)));
        // Invalid
        assert_eq!(mic_e_dest_byte(b'M'), None);
        assert_eq!(mic_e_dest_byte(b'!'), None);
    }

    #[test]
    fn test_mic_e_decode() {
        // Encode: Lat 33°57.05'N, Lon 118°26.50'W, speed 45 kts, course 218°
        // Destination: SSUWP5
        //   S(digit=3,flag=1) S(3,1) U(5,1) W(7,1=North) P(0,1=lonOffset) 5(5,0=West)
        // Info bytes (after DTI '`'):
        //   lon_deg: 118 with offset → (118-100)+28=46='.'
        //   lon_min: 26+28=54='6'
        //   lon_hun: 50+28=78='N'
        //   sp_tens: 4+28=32=' '
        //   sp_u+cse_h: 5*10+2+28=80='P'
        //   cse_tu: 18+28=46='.'
        //   symbol: '>'  table: '/'
        let dest = b"SSUWP5";
        let info = b"`.6N P.>/";
        let pkt = parse_mic_e(info, dest).unwrap();
        match pkt {
            AprsPacket::MicE { position, speed, course, symbol_table, symbol_code } => {
                // lat = 33*1e6 + (57*100+5)*10000/60 = 33_000_000 + 950_833 = 33_950_833
                assert_eq!(position.lat, 33_950_833);
                // lon = 118*1e6 + (26*100+50)*10000/60 = 118_000_000 + 441_666
                // West → negative
                assert_eq!(position.lon, -118_441_666);
                assert_eq!(speed, 45);
                assert_eq!(course, 218);
                assert_eq!(symbol_code, b'>');
                assert_eq!(symbol_table, b'/');
            }
            _ => panic!("expected MicE variant"),
        }
    }

    #[test]
    fn test_mic_e_via_parse_packet() {
        // Same test but via the top-level parse_packet function
        let dest = b"SSUWP5";
        let info = b"`.6N P.>/";
        let pkt = parse_packet(info, dest).unwrap();
        match pkt {
            AprsPacket::MicE { position, speed, course, .. } => {
                assert_eq!(position.lat, 33_950_833);
                assert_eq!(position.lon, -118_441_666);
                assert_eq!(speed, 45);
                assert_eq!(course, 218);
            }
            _ => panic!("expected MicE variant"),
        }
    }

    #[test]
    fn test_mic_e_south_east() {
        // Lat 34°00.00'S, Lon 5°30.00'E
        // Dest bytes: digit3=4 digit4=0 flag=0(South), digit5=0 flag=0(noOffset), digit6=0 flag=1(East)
        // S(3,1) T(4,1) 0(0,0) 0(0,0=South) 0(0,0=noOffset) P(0,1=East)
        let dest = b"ST000P";
        // lon_deg=5, no offset → need d=5 after decode
        // d = byte-28. Need d to come from >=190 path: 5+190=195, byte=195+28=223
        // lon_min=30, byte=30+28=58=':'
        // lon_hun=0, byte=0+28=28 (control char, but valid)
        // speed=0: sp_tens=0+28=28, sp_u+cse_h=0+28=28, cse_tu=0+28=28
        let info: &[u8] = &[b'`', 223, 58, 28, 28, 28, 28, b'-', b'/'];
        let pkt = parse_mic_e(info, dest).unwrap();
        match pkt {
            AprsPacket::MicE { position, speed, course, .. } => {
                // lat = 34*1e6 + 0 = 34_000_000, South → negative
                assert_eq!(position.lat, -34_000_000);
                // lon = 5*1e6 + 30*10000/60 = 5_000_000 + 500_000 = 5_500_000, East → positive
                assert_eq!(position.lon, 5_500_000);
                assert_eq!(speed, 0);
                assert_eq!(course, 0);
            }
            _ => panic!("expected MicE variant"),
        }
    }

    #[test]
    fn test_mic_e_too_short() {
        // Info too short
        assert!(parse_mic_e(b"`12345678", b"SSUWP").is_none()); // dest too short
        assert!(parse_mic_e(b"`1234567", b"SSUWP5").is_none()); // info too short
    }

    #[test]
    fn test_mic_e_invalid_dest() {
        // Invalid character in destination
        assert!(parse_mic_e(b"`.6N P.>/", b"SS!WP5").is_none());
    }
}
