//! APRS Protocol — encoding and decoding of APRS packets.
//!
//! APRS (Automatic Packet Reporting System) is carried in the information
//! field of AX.25 UI frames. The first byte of the info field is the
//! Data Type Identifier (DTI) which determines the packet format.
//!
//! Reference: APRS Protocol Reference v1.0.1 (APRS101.PDF)

pub mod nmea;

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
    /// `}` — Third-party forwarded packet
    ThirdParty,
    /// `$` — Raw GPS/NMEA data
    RawGps,
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
            b'}' => Self::ThirdParty,
            b'$' => Self::RawGps,
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

/// Parsed APRS weather data. All fields optional since weather reports
/// may include only a subset of measurements.
#[derive(Clone, Debug, Default)]
pub struct WeatherData {
    /// Wind direction in degrees (0-360)
    pub wind_direction: Option<u16>,
    /// Sustained wind speed in mph
    pub wind_speed: Option<u16>,
    /// Wind gust speed in mph
    pub wind_gust: Option<u16>,
    /// Temperature in Fahrenheit (signed for below zero)
    pub temperature: Option<i16>,
    /// Rain in last hour, hundredths of inch
    pub rain_last_hour: Option<u16>,
    /// Rain in last 24 hours, hundredths of inch
    pub rain_24h: Option<u16>,
    /// Rain since midnight, hundredths of inch
    pub rain_since_midnight: Option<u16>,
    /// Humidity 1-100 (APRS `h00` means 100%)
    pub humidity: Option<u8>,
    /// Barometric pressure in tenths of millibar
    pub barometric_pressure: Option<u32>,
    /// Luminosity in W/m²
    pub luminosity: Option<u16>,
    /// Snowfall in inches
    pub snowfall: Option<u16>,
}

/// APRS timestamp format.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timestamp {
    /// DDHHMMz — day/hour/minute UTC
    Dhm { day: u8, hour: u8, minute: u8 },
    /// HHMMSSh — hour/minute/second UTC (current day)
    Hms { hour: u8, minute: u8, second: u8 },
    /// DDHHMMl — day/hour/minute local time
    DhmLocal { day: u8, hour: u8, minute: u8 },
}

/// APRS message subtype classification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MessageType {
    /// Standard person-to-person message
    Private,
    /// Acknowledgment (text starts with "ack")
    Ack,
    /// Rejection (text starts with "rej")
    Rej,
    /// Bulletin (addressee starts with "BLN" + digit)
    Bulletin,
    /// Announcement (addressee starts with "BLN" + letter)
    Announcement,
    /// NWS weather bulletin
    Nws,
}

/// Extra data from compressed position cs/type bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct CompressedExtra {
    /// Course (degrees) and speed (knots) from compressed encoding
    pub course_speed: Option<(u16, u16)>,
    /// Altitude in feet from compressed encoding
    pub altitude: Option<i32>,
    /// Pre-computed radio range in miles
    pub range: Option<u16>,
}

/// PHG — power, height, gain, directivity.
#[derive(Clone, Debug, PartialEq)]
pub struct Phg {
    /// Transmitter power in watts
    pub power_watts: u16,
    /// Antenna height above average terrain in feet
    pub height_feet: u16,
    /// Antenna gain in dB
    pub gain_db: u8,
    /// Directivity: 0=omni, or degrees (20/45/90/135/180/225/270/315)
    pub directivity: u16,
}

/// DFS — direction finding signal report.
#[derive(Clone, Debug, PartialEq)]
pub struct Dfs {
    /// Signal strength in S-units (0-9)
    pub strength: u8,
    /// Antenna height above average terrain in feet
    pub height_feet: u16,
    /// Antenna gain in dB
    pub gain_db: u8,
    /// Directivity: 0=omni, or degrees
    pub directivity: u16,
}

/// Parsed structured fields from a position/object comment.
#[derive(Debug)]
pub struct CommentFields<'a> {
    /// PHG — power, height, gain, directivity
    pub phg: Option<Phg>,
    /// RNG — pre-calculated range in miles
    pub range: Option<u16>,
    /// /A= — altitude in feet
    pub altitude: Option<i32>,
    /// CSE/SPD — course (degrees) and speed (knots)
    pub course_speed: Option<(u16, u16)>,
    /// DFS — direction finding signal report
    pub dfs: Option<Dfs>,
    /// Remaining unparsed comment text
    pub text: &'a [u8],
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
        timestamp: Option<Timestamp>,
        compressed_extra: Option<CompressedExtra>,
    },
    /// Message
    Message {
        addressee: &'a [u8],
        text: &'a [u8],
        message_no: Option<&'a [u8]>,
        message_type: MessageType,
    },
    /// Status report
    Status {
        text: &'a [u8],
        timestamp: Option<Timestamp>,
        maidenhead: Option<&'a [u8]>,
    },
    /// Mic-E encoded position
    MicE {
        position: Position,
        speed: u16,    // knots
        course: u16,   // degrees
        symbol_table: u8,
        symbol_code: u8,
    },
    /// Positionless weather report (DTI `_`)
    Weather {
        weather: WeatherData,
        comment: &'a [u8],
    },
    /// Object report (DTI `;`)
    Object {
        name: &'a [u8],
        live: bool,
        position: Position,
        symbol_table: u8,
        symbol_code: u8,
        comment: &'a [u8],
        timestamp: Option<Timestamp>,
    },
    /// Item report (DTI `)`)
    Item {
        name: &'a [u8],
        live: bool,
        position: Position,
        symbol_table: u8,
        symbol_code: u8,
        comment: &'a [u8],
    },
    /// Telemetry report (DTI `T`)
    Telemetry {
        sequence: u16,
        analog: [Option<u16>; 5],
        digital: u8,
    },
    /// Third-party forwarded packet (DTI `}`)
    ThirdParty {
        data: &'a [u8],
    },
    /// Raw GPS/NMEA sentence (DTI `$`)
    RawGps {
        data: &'a [u8],
        parsed: Option<nmea::NmeaData>,
    },
    /// Station capabilities (DTI `<`)
    Capabilities {
        data: &'a [u8],
    },
    /// Query (DTI `?`)
    Query {
        query_type: &'a [u8],
    },
    /// User-defined data (DTI `{`)
    UserDefined {
        data: &'a [u8],
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
            parse_status(info)
        }
        DataType::Weather => {
            parse_weather(info)
        }
        DataType::Object => {
            parse_object(info)
        }
        DataType::Item => {
            parse_item(info)
        }
        DataType::Telemetry => {
            parse_telemetry(info)
        }
        DataType::Capabilities => {
            Some(AprsPacket::Capabilities { data: &info[1..] })
        }
        DataType::UserDefined => {
            Some(AprsPacket::UserDefined { data: &info[1..] })
        }
        DataType::Query => {
            parse_query(info)
        }
        DataType::ThirdParty => {
            Some(AprsPacket::ThirdParty { data: &info[1..] })
        }
        DataType::RawGps => {
            let parsed = nmea::parse_nmea(info);
            Some(AprsPacket::RawGps { data: info, parsed })
        }
        DataType::Unknown(_) => {
            Some(AprsPacket::Unknown { dti: info[0], data: &info[1..] })
        }
    }
}

/// Parse a single ASCII digit byte to its numeric value.
fn parse_digit(b: u8) -> Option<u32> {
    if b.is_ascii_digit() {
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
        if !(33..=124).contains(&b) {
            return None;
        }
        val = val * 91 + (b - 33) as u32;
    }
    Some(val)
}

/// Parse a 7-byte APRS timestamp.
///
/// Formats: `DDHHMMz` (UTC), `HHMMSSh` (HMS UTC), `DDHHMMl` (local).
fn parse_timestamp(data: &[u8]) -> Option<Timestamp> {
    if data.len() < 7 {
        return None;
    }
    let d0 = parse_digit(data[0])? as u8;
    let d1 = parse_digit(data[1])? as u8;
    let d2 = parse_digit(data[2])? as u8;
    let d3 = parse_digit(data[3])? as u8;
    let d4 = parse_digit(data[4])? as u8;
    let d5 = parse_digit(data[5])? as u8;
    match data[6] {
        b'z' => Some(Timestamp::Dhm {
            day: d0 * 10 + d1,
            hour: d2 * 10 + d3,
            minute: d4 * 10 + d5,
        }),
        b'h' => Some(Timestamp::Hms {
            hour: d0 * 10 + d1,
            minute: d2 * 10 + d3,
            second: d4 * 10 + d5,
        }),
        b'/' => Some(Timestamp::DhmLocal {
            day: d0 * 10 + d1,
            hour: d2 * 10 + d3,
            minute: d4 * 10 + d5,
        }),
        _ => None,
    }
}

/// Parsed plain position fields: (position, symbol_table, symbol_code, total_bytes_consumed).
/// `data` starts at the first latitude byte (not DTI/timestamp).
fn parse_plain_position_fields(data: &[u8]) -> Option<(Position, u8, u8, usize)> {
    // Need at least 19 bytes: 8 lat + 1 sym_table + 9 lon + 1 sym_code
    if data.len() < 19 {
        return None;
    }

    // Copy lat/lon bytes so we can replace spaces with '0'
    let mut lat_bytes = [0u8; 8];
    lat_bytes.copy_from_slice(&data[0..8]);
    let mut lon_bytes = [0u8; 9];
    lon_bytes.copy_from_slice(&data[9..18]);

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
    let lat_min_hundredths = lat_min_int * 100 + lat_min_frac;
    let mut lat = (lat_deg * 1_000_000 + (lat_min_hundredths * 10_000 + 30) / 60) as i32;
    if lat_ns == b'S' {
        lat = -lat;
    }

    let symbol_table = data[8];

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

    let symbol_code = data[18];
    Some((Position { lat, lon, ambiguity }, symbol_table, symbol_code, 19))
}

/// Parsed compressed position fields: (position, symbol_table, symbol_code, total_bytes_consumed, compressed_extra).
/// `data` starts at the symbol_table byte.
fn parse_compressed_position_fields(data: &[u8]) -> Option<(Position, u8, u8, usize, Option<CompressedExtra>)> {
    // Minimum: 1 sym_table + 4 lat + 4 lon + 1 sym_code = 10
    if data.len() < 10 {
        return None;
    }
    let symbol_table = data[0];

    let lat_val = base91_decode(&data[1..5])?;
    let lon_val = base91_decode(&data[5..9])?;

    // lat = 90.0 - value / 380926.0, in microdegrees using i64
    let lat = (90_000_000i64 - (lat_val as i64) * 1_000_000 / 380926) as i32;
    // lon = -180.0 + value / 190463.0, in microdegrees using i64
    let lon = (-180_000_000i64 + (lon_val as i64) * 1_000_000 / 190463) as i32;

    let symbol_code = data[9];

    // Parse cs/type bytes (3) if present
    let (consumed, extra) = if data.len() >= 13 {
        let cs = data[10];
        let se = data[11];
        let t = data[12];
        let ctype = t & 0x18; // bits 3-4 = compression type origin
        let extra = if cs == b' ' && se == b' ' {
            // No cs/se data
            None
        } else if cs >= 33 && se >= 33 {
            let cs_val = (cs - 33) as u32;
            let se_val = (se - 33) as u32;
            let nmeq = ctype >> 3; // NMEA source + compression type indicator
            match nmeq {
                0b00 => {
                    // Compressed course/speed
                    let course = cs_val * 4; // degrees
                    // speed = 1.08^(se_val) - 1 knots — use integer approx
                    let speed = compressed_speed(se_val);
                    Some(CompressedExtra {
                        course_speed: Some((course as u16, speed)),
                        altitude: None,
                        range: None,
                    })
                }
                0b01 => {
                    // Pre-computed radio range
                    let range = compressed_range(se_val);
                    Some(CompressedExtra {
                        course_speed: None,
                        altitude: None,
                        range: Some(range),
                    })
                }
                0b10 => {
                    // Altitude
                    let alt_code = cs_val * 91 + se_val;
                    let alt = compressed_altitude(alt_code);
                    Some(CompressedExtra {
                        course_speed: None,
                        altitude: Some(alt),
                        range: None,
                    })
                }
                _ => None,
            }
        } else {
            None
        };
        (13, extra)
    } else {
        (10, None)
    };
    Some((Position { lat, lon, ambiguity: 0 }, symbol_table, symbol_code, consumed, extra))
}

/// Compute 1.08^n - 1 (speed in knots) using integer math.
fn compressed_speed(n: u32) -> u16 {
    // 1.08^n for small n: use lookup or iterative multiply
    // 1.08 ≈ 27/25 in fixed-point
    let mut val = 1000u64; // ×1000 for precision
    for _ in 0..n {
        val = val * 108 / 100;
    }
    ((val - 1000) / 1000) as u16
}

/// Compute 2 × 1.08^n (range in miles) using integer math.
fn compressed_range(n: u32) -> u16 {
    let mut val = 2000u64; // 2.0 × 1000
    for _ in 0..n {
        val = val * 108 / 100;
    }
    (val / 1000) as u16
}

/// Compute 1.002^n (altitude in feet) using integer math.
fn compressed_altitude(n: u32) -> i32 {
    let n = n.min(4600);
    // 1.002^n — use iterative multiply with high-precision fixed-point
    let mut val = 1_000_000u64; // ×1e6
    for _ in 0..n {
        val = val * 1002 / 1000;
    }
    (val / 1_000_000) as i32
}

/// Parse position fields (plain or compressed) starting at `data`.
/// Returns (position, symbol_table, symbol_code, bytes_consumed, compressed_extra).
fn parse_position_auto(data: &[u8]) -> Option<(Position, u8, u8, usize, Option<CompressedExtra>)> {
    if data.is_empty() {
        return None;
    }
    if data[0] == b'/' || data[0] == b'\\' {
        parse_compressed_position_fields(data)
    } else {
        let (pos, st, sc, consumed) = parse_plain_position_fields(data)?;
        Some((pos, st, sc, consumed, None))
    }
}

/// Parse plain (uncompressed) APRS position starting at `offset` in `info`.
///
/// Position data at offset: `DDMM.MMN/DDDMM.MME$comment`
/// (8 bytes lat + 1 symbol_table + 9 bytes lon + 1 symbol_code + comment)
fn parse_plain_position<'a>(info: &'a [u8], offset: usize, timestamp: Option<Timestamp>) -> Option<AprsPacket<'a>> {
    if info.len() < offset + 19 {
        return None;
    }
    let (position, symbol_table, symbol_code, consumed) =
        parse_plain_position_fields(&info[offset..])?;
    let comment = if info.len() > offset + consumed {
        &info[offset + consumed..]
    } else {
        &[]
    };
    Some(AprsPacket::Position { position, symbol_table, symbol_code, comment, timestamp, compressed_extra: None })
}

/// Parse compressed APRS position starting at `offset` in `info`.
///
/// At offset: symbol_table (1) + lat base91 (4) + lon base91 (4) + symbol_code (1)
/// + optional cs/type (3)
fn parse_compressed_position<'a>(info: &'a [u8], offset: usize, timestamp: Option<Timestamp>) -> Option<AprsPacket<'a>> {
    if info.len() < offset + 10 {
        return None;
    }
    let (position, symbol_table, symbol_code, consumed, compressed_extra) =
        parse_compressed_position_fields(&info[offset..])?;
    let comment = if info.len() > offset + consumed {
        &info[offset + consumed..]
    } else {
        &[]
    };
    Some(AprsPacket::Position { position, symbol_table, symbol_code, comment, timestamp, compressed_extra })
}

/// Parse a position report without timestamp.
/// Format: `!DDMM.MMN/DDDMM.MMW$...`  (or compressed)
fn parse_position_no_timestamp<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    if info.len() < 2 {
        return None;
    }
    // Check if compressed: byte after DTI is symbol table '/' or '\'
    if info[1] == b'/' || info[1] == b'\\' {
        parse_compressed_position(info, 1, None)
    } else {
        parse_plain_position(info, 1, None)
    }
}

/// Parse a position report with timestamp.
/// Format: `/DDHHMMzDDMM.MMN/DDDMM.MMW$...`
fn parse_position_with_timestamp<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // DTI (1) + timestamp (7) = 8 bytes before position data
    if info.len() < 9 {
        return None;
    }
    let timestamp = parse_timestamp(&info[1..8]);
    let pos_start = 8;
    if info[pos_start] == b'/' || info[pos_start] == b'\\' {
        parse_compressed_position(info, pos_start, timestamp)
    } else {
        parse_plain_position(info, pos_start, timestamp)
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
    // E/W from destination byte index 5: flag set means West (APRS101 Table 10-2)
    let is_west = flags[5];

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
    if is_west {
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

// ── Weather Parsing ──────────────────────────────────────────────────

/// Parse a numeric field of `len` digits from `data[offset..]`.
/// Returns `None` if any character is not a digit or is a dot/space (missing data).
fn parse_wx_int(data: &[u8], offset: usize, len: usize) -> Option<u16> {
    if data.len() < offset + len {
        return None;
    }
    let slice = &data[offset..offset + len];
    // All dots or spaces means "no data"
    if slice.iter().all(|&b| b == b'.' || b == b' ') {
        return None;
    }
    let mut val = 0u16;
    for &b in slice {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val * 10 + (b - b'0') as u16;
    }
    Some(val)
}

/// Parse a signed numeric field (e.g. temperature `tTTT` which can be `t-10`).
fn parse_wx_signed(data: &[u8], offset: usize, len: usize) -> Option<i16> {
    if data.len() < offset + len {
        return None;
    }
    let slice = &data[offset..offset + len];
    if slice.iter().all(|&b| b == b'.' || b == b' ') {
        return None;
    }
    // Check for leading minus
    if !slice.is_empty() && slice[0] == b'-' {
        let digits = &slice[1..];
        let mut val = 0i16;
        for &b in digits {
            if !b.is_ascii_digit() {
                return None;
            }
            val = val * 10 + (b - b'0') as i16;
        }
        Some(-val)
    } else {
        let mut val = 0i16;
        for &b in slice {
            if !b.is_ascii_digit() {
                return None;
            }
            val = val * 10 + (b - b'0') as i16;
        }
        Some(val)
    }
}

/// Parse weather data fields from a byte slice.
///
/// Weather data uses single-letter keys followed by fixed-width values:
/// `cSSS` wind direction, `sSSSS` wind speed, `gSSS` gust, `tTTT` temp,
/// `rRRR` rain/hr, `pPPP` rain/24h, `PRRR` rain since midnight,
/// `hHH` humidity, `bBBBBB` pressure, `L`/`l` luminosity, `s` snowfall.
///
/// Returns (WeatherData, index of first byte not consumed as weather).
pub fn parse_weather_fields(data: &[u8]) -> (WeatherData, usize) {
    let mut wx = WeatherData::default();
    let mut i = 0;
    let len = data.len();

    while i < len {
        match data[i] {
            b'c' => {
                wx.wind_direction = parse_wx_int(data, i + 1, 3);
                i += 4;
            }
            b's' if i + 3 < len => {
                if wx.wind_speed.is_none() {
                    wx.wind_speed = parse_wx_int(data, i + 1, 3);
                } else {
                    // Second 's' is snowfall
                    wx.snowfall = parse_wx_int(data, i + 1, 3);
                }
                i += 4;
            }
            b'g' => {
                wx.wind_gust = parse_wx_int(data, i + 1, 3);
                i += 4;
            }
            b't' => {
                wx.temperature = parse_wx_signed(data, i + 1, 3);
                i += 4;
            }
            b'r' => {
                wx.rain_last_hour = parse_wx_int(data, i + 1, 3);
                i += 4;
            }
            b'p' => {
                wx.rain_24h = parse_wx_int(data, i + 1, 3);
                i += 4;
            }
            b'P' => {
                wx.rain_since_midnight = parse_wx_int(data, i + 1, 3);
                i += 4;
            }
            b'h' => {
                let raw = parse_wx_int(data, i + 1, 2);
                wx.humidity = raw.map(|v| if v == 0 { 100u8 } else { v as u8 });
                i += 3;
            }
            b'b' => {
                // 5-digit pressure in tenths of millibar
                if i + 6 <= len {
                    let slice = &data[i + 1..i + 6];
                    if !slice.iter().all(|&b| b == b'.' || b == b' ') {
                        let mut val = 0u32;
                        let mut ok = true;
                        for &b in slice {
                            if !b.is_ascii_digit() {
                                ok = false;
                                break;
                            }
                            val = val * 10 + (b - b'0') as u32;
                        }
                        if ok {
                            wx.barometric_pressure = Some(val);
                        }
                    }
                }
                i += 6;
            }
            b'L' => {
                // Luminosity 0-999 W/m²
                wx.luminosity = parse_wx_int(data, i + 1, 3);
                i += 4;
            }
            b'l' => {
                // Luminosity 1000-1999 W/m² (add 1000)
                wx.luminosity = parse_wx_int(data, i + 1, 3).map(|v| v + 1000);
                i += 4;
            }
            // Snowfall: 's' followed by 3 digits — but 's' is already wind speed.
            // In APRS weather, wind speed uses 's' immediately after 'c'. If we
            // get here for a second 's', treat as snowfall. The first 's' is handled
            // above. For now we handle snowfall via 's' only if wind_speed is already set.
            _ => break, // unknown field = end of weather data
        }
    }
    (wx, i)
}

/// Parse a positionless weather report (DTI `_`).
///
/// Format: `_MMDDHHMMcSSS...` — 8-byte MMDDHHMM timestamp, then weather fields.
fn parse_weather<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // DTI '_' (1) + timestamp (8) + at least 'c' + 3 digits (4) = 13 minimum
    if info.len() < 13 {
        return None;
    }
    let weather_start = 9; // skip DTI + 8-byte timestamp
    let (weather, consumed) = parse_weather_fields(&info[weather_start..]);
    let comment_start = weather_start + consumed;
    let comment = if comment_start < info.len() {
        &info[comment_start..]
    } else {
        &[]
    };
    Some(AprsPacket::Weather { weather, comment })
}

/// Parse weather fields embedded in a position report comment.
///
/// Handles two formats:
/// - Format 1 (positionless): `cDDDsSSSgXXXtXXX...` — starts with `c`
/// - Format 2 (position+weather): `DDD/SSSgXXXtXXX...` — 3-digit wind dir, `/`, 3-digit speed
///
/// Returns `None` if no weather data is found.
pub fn parse_weather_from_comment(comment: &[u8]) -> Option<WeatherData> {
    if comment.is_empty() {
        return None;
    }

    // Format 1: cDDDsSSSgXXXtXXX... (positionless weather comment)
    if comment[0] == b'c' {
        let (wx, _) = parse_weather_fields(comment);
        if wx.wind_direction.is_some() || wx.temperature.is_some()
            || wx.barometric_pressure.is_some()
        {
            return Some(wx);
        }
    }

    // Format 2: DDD/SSSgXXXtXXX... (position+weather comment)
    // Wind direction is 3 chars (digits or '.'), then '/', then 3 chars wind speed
    if comment.len() >= 7 && comment[3] == b'/' {
        let dir_ok = comment[..3].iter().all(|&b| b.is_ascii_digit() || b == b'.');
        let spd_ok = comment[4..7].iter().all(|&b| b.is_ascii_digit() || b == b'.');
        if dir_ok && spd_ok {
            let mut wx = WeatherData {
                wind_direction: parse_wx_int(comment, 0, 3),
                wind_speed: parse_wx_int(comment, 4, 3),
                ..WeatherData::default()
            };
            // Parse remaining standard weather fields starting at offset 7
            if comment.len() > 7 {
                let (more_wx, _) = parse_weather_fields(&comment[7..]);
                wx.wind_gust = more_wx.wind_gust;
                wx.temperature = more_wx.temperature;
                wx.rain_last_hour = more_wx.rain_last_hour;
                wx.rain_24h = more_wx.rain_24h;
                wx.rain_since_midnight = more_wx.rain_since_midnight;
                wx.humidity = more_wx.humidity;
                wx.barometric_pressure = more_wx.barometric_pressure;
                wx.luminosity = more_wx.luminosity;
                wx.snowfall = more_wx.snowfall;
            }
            if wx.wind_direction.is_some() || wx.temperature.is_some()
                || wx.barometric_pressure.is_some()
            {
                return Some(wx);
            }
        }
    }

    None
}

// ── Object Parsing ──────────────────────────────────────────────────

/// Parse an APRS object report (DTI `;`).
///
/// Format: `;NAME_____*DDHHMMz<position><comment>`
/// - 9-char name (space-padded)
/// - `*` = live, `_` = killed
/// - 7-byte timestamp
/// - position (plain or compressed)
fn parse_object<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // DTI ';' (1) + 9-char name + live/killed (1) + timestamp (7) = 18
    // + minimum position (19 plain or 10 compressed)
    if info.len() < 28 {
        return None;
    }

    let name_raw = &info[1..10];
    // Trim trailing spaces from name
    let name_end = name_raw.iter().rposition(|&b| b != b' ').map(|i| i + 1).unwrap_or(0);
    let name = &name_raw[..name_end];

    let live = info[10] == b'*';
    let timestamp = parse_timestamp(&info[11..18]);
    let pos_start = 18;

    let (position, symbol_table, symbol_code, consumed, _compressed) =
        parse_position_auto(&info[pos_start..])?;

    let comment_start = pos_start + consumed;
    let comment = if comment_start < info.len() {
        &info[comment_start..]
    } else {
        &[]
    };

    Some(AprsPacket::Object {
        name,
        live,
        position,
        symbol_table,
        symbol_code,
        comment,
        timestamp,
    })
}

// ── Item Parsing ────────────────────────────────────────────────────

/// Parse an APRS item report (DTI `)`).
///
/// Format: `)NAME!<position>` or `)NAME_<position>`
/// - Name is 3-9 characters, terminated by `!` (live) or `_` (killed)
fn parse_item<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // DTI ')' (1) + at least 3-char name + separator (1) + position (19 min)
    if info.len() < 24 {
        return None;
    }

    // Scan bytes 1..10 for '!' or '_' separator (name is 3-9 chars)
    let max_scan = core::cmp::min(10, info.len());
    let sep_idx = info[4..max_scan].iter().position(|&b| b == b'!' || b == b'_').map(|i| i + 4);
    let sep_idx = sep_idx?;

    let name = &info[1..sep_idx];
    let live = info[sep_idx] == b'!';
    let pos_start = sep_idx + 1;

    let (position, symbol_table, symbol_code, consumed, _compressed) =
        parse_position_auto(&info[pos_start..])?;

    let comment_start = pos_start + consumed;
    let comment = if comment_start < info.len() {
        &info[comment_start..]
    } else {
        &[]
    };

    Some(AprsPacket::Item {
        name,
        live,
        position,
        symbol_table,
        symbol_code,
        comment,
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

    let message_type = classify_message(addressee, text);

    Some(AprsPacket::Message {
        addressee,
        text,
        message_no,
        message_type,
    })
}

/// Classify an APRS message into its subtype.
fn classify_message(addressee: &[u8], text: &[u8]) -> MessageType {
    // Check for ack/rej
    if text.starts_with(b"ack") {
        return MessageType::Ack;
    }
    if text.starts_with(b"rej") {
        return MessageType::Rej;
    }
    // Check for NWS
    if addressee.starts_with(b"NWS") {
        return MessageType::Nws;
    }
    // Check for BLN — bulletin vs announcement
    if addressee.starts_with(b"BLN") {
        if addressee.len() > 3 {
            let ch = addressee[3];
            if ch.is_ascii_uppercase() {
                return MessageType::Announcement;
            }
        }
        return MessageType::Bulletin;
    }
    MessageType::Private
}

// ── Telemetry Parsing ──────────────────────────────────────────────

/// Parse a telemetry report (DTI `T`).
///
/// Format: `T#seq,a1,a2,a3,a4,a5,bbbbbbbb`
fn parse_telemetry<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // Minimum: T#nnn,
    if info.len() < 3 {
        return None;
    }
    // Skip 'T' and optional '#'
    let start = if info.len() > 1 && info[1] == b'#' { 2 } else { 1 };
    let data = &info[start..];

    // Split by commas
    let mut fields = [&b""[..]; 7]; // seq + 5 analog + 1 digital
    let mut field_count = 0;
    let mut field_start = 0;

    for (i, &b) in data.iter().enumerate() {
        if b == b',' {
            if field_count < 7 {
                fields[field_count] = &data[field_start..i];
                field_count += 1;
            }
            field_start = i + 1;
        }
    }
    // Last field
    if field_count < 7 && field_start <= data.len() {
        fields[field_count] = &data[field_start..];
        field_count += 1;
    }

    if field_count == 0 {
        return None;
    }

    // Parse sequence number
    let sequence = parse_ascii_u16(fields[0]).unwrap_or(0);

    // Parse 5 analog values
    let mut analog = [None; 5];
    for j in 0..5 {
        if j + 1 < field_count {
            analog[j] = parse_ascii_u16(fields[j + 1]);
        }
    }

    // Parse 8 digital bits from field 6
    let digital = if field_count >= 7 {
        parse_digital_bits(fields[6])
    } else {
        0
    };

    Some(AprsPacket::Telemetry {
        sequence,
        analog,
        digital,
    })
}

/// Parse ASCII decimal to u16.
fn parse_ascii_u16(data: &[u8]) -> Option<u16> {
    if data.is_empty() || data.iter().all(|&b| b == b' ' || b == b'.') {
        return None;
    }
    let mut val = 0u16;
    for &b in data {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u16)?;
    }
    Some(val)
}

/// Parse 8 digital bits from ASCII '0'/'1' characters.
fn parse_digital_bits(data: &[u8]) -> u8 {
    let mut bits = 0u8;
    for (i, &b) in data.iter().take(8).enumerate() {
        if b == b'1' {
            bits |= 1 << (7 - i);
        }
    }
    bits
}

// ── Query Parsing ──────────────────────────────────────────────────

/// Parse an APRS query (DTI `?`).
///
/// Format: `?APRS?`, `?WX?`, `?IGATE?` etc.
fn parse_query<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    // DTI '?' + at least 1 char
    if info.len() < 2 {
        return Some(AprsPacket::Query { query_type: &info[1..] });
    }
    // Find the closing '?' if present
    let query_data = &info[1..];
    let end = query_data.iter().position(|&b| b == b'?').unwrap_or(query_data.len());
    Some(AprsPacket::Query { query_type: &query_data[..end] })
}

// ── Status Parsing ─────────────────────────────────────────────────

/// Parse a status report (DTI `>`).
///
/// Format: `>text` or `>DDHHMMztext` or `>IO91SX text`
fn parse_status<'a>(info: &'a [u8]) -> Option<AprsPacket<'a>> {
    let data = &info[1..]; // skip DTI '>'

    // Try timestamp first: DDHHMMz or HHMMSSh or DDHHMMl
    if data.len() >= 7 {
        if let Some(ts) = parse_timestamp(&data[..7]) {
            return Some(AprsPacket::Status {
                text: &data[7..],
                timestamp: Some(ts),
                maidenhead: None,
            });
        }
    }

    // Try Maidenhead grid locator (4 or 6 chars: IO91 or IO91SX)
    if data.len() >= 4 {
        let mh_len = if data.len() >= 6
            && data[0].is_ascii_uppercase()
            && data[1].is_ascii_uppercase()
            && data[2].is_ascii_digit()
            && data[3].is_ascii_digit()
            && data[4].is_ascii_alphabetic()
            && data[5].is_ascii_alphabetic()
        {
            Some(6)
        } else if data[0].is_ascii_uppercase()
            && data[1].is_ascii_uppercase()
            && data[2].is_ascii_digit()
            && data[3].is_ascii_digit()
        {
            Some(4)
        } else {
            None
        };

        if let Some(len) = mh_len {
            // Need space or end after the grid
            let rest_start = if data.len() > len && data[len] == b' ' { len + 1 } else { len };
            return Some(AprsPacket::Status {
                text: &data[rest_start..],
                timestamp: None,
                maidenhead: Some(&data[..len]),
            });
        }
    }

    Some(AprsPacket::Status {
        text: data,
        timestamp: None,
        maidenhead: None,
    })
}

// ── Comment Field Parsing ──────────────────────────────────────────

/// PHG directivity lookup table.
const PHG_DIR: [u16; 9] = [0, 45, 90, 135, 180, 225, 270, 315, 360];

/// Parse structured fields from a position/object comment.
///
/// Scans for PHG, RNG, DFS, /A=, and CSE/SPD patterns.
/// The remaining text is returned in `text`.
pub fn parse_comment_fields<'a>(comment: &'a [u8]) -> CommentFields<'a> {
    let mut phg = None;
    let mut range = None;
    let mut altitude = None;
    let mut course_speed = None;
    let mut dfs = None;
    let mut text_start = 0;
    // Search for /A=NNNNNN anywhere in comment
    if let Some(pos) = find_subsequence(comment, b"/A=") {
        if pos + 9 <= comment.len() {
            let alt_slice = &comment[pos + 3..pos + 9];
            if let Some(val) = parse_signed_altitude(alt_slice) {
                altitude = Some(val);
            }
        }
    }

    // Check for PHG at start of comment
    if comment.starts_with(b"PHG") && comment.len() >= 7 {
        if let Some(p) = parse_phg(&comment[3..7]) {
            phg = Some(p);
            text_start = 7;
        }
    }

    // Check for RNG at start of comment
    if comment.starts_with(b"RNG") && comment.len() >= 7 {
        if let Some(r) = parse_ascii_u16(&comment[3..7]) {
            range = Some(r);
            text_start = 7;
        }
    }

    // Check for DFS at start of comment
    if comment.starts_with(b"DFS") && comment.len() >= 7 {
        if let Some(d) = parse_dfs(&comment[3..7]) {
            dfs = Some(d);
            text_start = 7;
        }
    }

    // Check for CSE/SPD at start (after PHG/RNG/DFS if present): NNN/NNN
    let cs_start = text_start;
    if comment.len() >= cs_start + 7 {
        let cs_slice = &comment[cs_start..cs_start + 7];
        if cs_slice[3] == b'/' &&
           cs_slice[0].is_ascii_digit() && cs_slice[1].is_ascii_digit() && cs_slice[2].is_ascii_digit() &&
           cs_slice[4].is_ascii_digit() && cs_slice[5].is_ascii_digit() && cs_slice[6].is_ascii_digit()
        {
            let cse = (cs_slice[0] - b'0') as u16 * 100 + (cs_slice[1] - b'0') as u16 * 10 + (cs_slice[2] - b'0') as u16;
            let spd = (cs_slice[4] - b'0') as u16 * 100 + (cs_slice[5] - b'0') as u16 * 10 + (cs_slice[6] - b'0') as u16;
            if cse <= 360 {
                course_speed = Some((cse, spd));
                text_start = cs_start + 7;
            }
        }
    }

    // Trim leading '/' from remaining text
    if text_start < comment.len() && comment[text_start] == b'/' {
        text_start += 1;
    }

    CommentFields {
        phg,
        range,
        altitude,
        course_speed,
        dfs,
        text: &comment[text_start..],
    }
}

/// Parse PHG digits: power, height, gain, directivity.
fn parse_phg(data: &[u8]) -> Option<Phg> {
    if data.len() < 4 {
        return None;
    }
    let p = parse_digit(data[0])? as u16;
    let h = parse_digit(data[1])? as u16;
    let g = parse_digit(data[2])? as u8;
    let d = parse_digit(data[3])? as usize;

    // Power: p^2 watts (0=0, 1=1, 2=4, 3=9, 4=16, 5=25, 6=36, 7=49, 8=64, 9=81)
    let power_watts = p * p;
    // Height: 10 * 2^h feet
    let height_feet = 10 * (1u16 << h);
    // Directivity
    let directivity = if d < PHG_DIR.len() { PHG_DIR[d] } else { 0 };

    Some(Phg { power_watts, height_feet, gain_db: g, directivity })
}

/// Parse DFS digits: strength, height, gain, directivity.
fn parse_dfs(data: &[u8]) -> Option<Dfs> {
    if data.len() < 4 {
        return None;
    }
    let s = parse_digit(data[0])? as u8;
    let h = parse_digit(data[1])? as u16;
    let g = parse_digit(data[2])? as u8;
    let d = parse_digit(data[3])? as usize;

    let height_feet = 10 * (1u16 << h);
    let directivity = if d < PHG_DIR.len() { PHG_DIR[d] } else { 0 };

    Some(Dfs { strength: s, height_feet, gain_db: g, directivity })
}

/// Find a subsequence in a byte slice.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse a signed 6-digit altitude from `/A=` field.
fn parse_signed_altitude(data: &[u8]) -> Option<i32> {
    if data.len() < 6 {
        return None;
    }
    let (sign, digits) = if data[0] == b'-' {
        (-1i32, &data[1..6])
    } else {
        (1i32, &data[0..6])
    };
    let mut val = 0i32;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val * 10 + (b - b'0') as i32;
    }
    Some(sign * val)
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
            AprsPacket::Position { position, symbol_table, symbol_code, comment, .. } => {
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
            AprsPacket::Message { addressee, text, message_no, .. } => {
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
            AprsPacket::Message { addressee, text, message_no, .. } => {
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
            AprsPacket::Message { addressee, text, message_no, .. } => {
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
            AprsPacket::Message { addressee, text, message_no, .. } => {
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
            AprsPacket::Status { text, .. } => {
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
            AprsPacket::Status { text, .. } => {
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
            AprsPacket::Message { addressee, text, message_no, .. } => {
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
            AprsPacket::Message { addressee, text, message_no, .. } => {
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
            AprsPacket::Message { addressee, text, message_no, .. } => {
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
        // Destination: SSUWPU
        //   S(digit=3,flag=1) S(3,1) U(5,1) W(7,1=North) P(0,1=lonOffset) U(5,1=West)
        // Info bytes (after DTI '`'):
        //   lon_deg: 118 with offset → (118-100)+28=46='.'
        //   lon_min: 26+28=54='6'
        //   lon_hun: 50+28=78='N'
        //   sp_tens: 4+28=32=' '
        //   sp_u+cse_h: 5*10+2+28=80='P'
        //   cse_tu: 18+28=46='.'
        //   symbol: '>'  table: '/'
        let dest = b"SSUWPU";
        let info = b"`.6N P.>/";
        let pkt = parse_mic_e(info, dest).unwrap();
        match pkt {
            AprsPacket::MicE { position, speed, course, symbol_table, symbol_code } => {
                // lat = 33*1e6 + (57*100+5)*10000/60 = 33_000_000 + 950_833 = 33_950_833
                assert_eq!(position.lat, 33_950_833);
                // lon = 118*1e6 + (26*100+50)*10000/60 = 118_000_000 + 441_666
                // West (flag=1 at position 5) → negative
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
        let dest = b"SSUWPU";
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
        // Dest bytes: digit3=4 digit4=0 flag=0(South), digit5=0 flag=0(noOffset), digit6=0 flag=0(East)
        // S(3,1) T(4,1) 0(0,0) 0(0,0=South) 0(0,0=noOffset) 0(0,0=East)
        let dest = b"ST0000";
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
        assert!(parse_mic_e(b"`1234567", b"SSUWPU").is_none()); // info too short
    }

    #[test]
    fn test_mic_e_invalid_dest() {
        // Invalid character in destination
        assert!(parse_mic_e(b"`.6N P.>/", b"SS!WP5").is_none());
    }

    // ── Weather Tests ───────────────────────────────────────────────

    #[test]
    fn test_weather_complete() {
        // Positionless weather: _MMDDHHMMcSSS...
        let info = b"_10090000c220s004g005t077r001p002P003h50b10132";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, comment } => {
                assert_eq!(weather.wind_direction, Some(220));
                assert_eq!(weather.wind_speed, Some(4));
                assert_eq!(weather.wind_gust, Some(5));
                assert_eq!(weather.temperature, Some(77));
                assert_eq!(weather.rain_last_hour, Some(1));
                assert_eq!(weather.rain_24h, Some(2));
                assert_eq!(weather.rain_since_midnight, Some(3));
                assert_eq!(weather.humidity, Some(50));
                assert_eq!(weather.barometric_pressure, Some(10132));
                assert_eq!(comment, b"");
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_partial() {
        // Only wind + temp
        let info = b"_10090000c180s012t065";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.wind_direction, Some(180));
                assert_eq!(weather.wind_speed, Some(12));
                assert_eq!(weather.temperature, Some(65));
                assert_eq!(weather.wind_gust, None);
                assert_eq!(weather.rain_last_hour, None);
                assert_eq!(weather.humidity, None);
                assert_eq!(weather.barometric_pressure, None);
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_negative_temp() {
        let info = b"_10090000c000s000g000t-10";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.temperature, Some(-10));
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_humidity_100() {
        // h00 means 100% in APRS
        let info = b"_10090000c000s000g000t072h00";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.humidity, Some(100));
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_barometric() {
        let info = b"_10090000c000s000g000t072b10132";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.barometric_pressure, Some(10132));
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_with_software_comment() {
        let info = b"_10090000c220s004g005t077eWx by Davis VP2";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, comment } => {
                assert_eq!(weather.wind_direction, Some(220));
                assert_eq!(weather.temperature, Some(77));
                // Unknown field 'e' terminates weather, rest is comment
                assert_eq!(comment, b"eWx by Davis VP2");
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_luminosity() {
        // L for 0-999 W/m²
        let info = b"_10090000c000s000g000t072L123";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.luminosity, Some(123));
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_luminosity_high() {
        // l for 1000-1999 W/m² (add 1000)
        let info = b"_10090000c000s000g000t072l234";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.luminosity, Some(1234));
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_all_missing() {
        // Dots for all values = missing
        let info = b"_10090000c...s...g...t...r...p...P...h..b.....";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.wind_direction, None);
                assert_eq!(weather.wind_speed, None);
                assert_eq!(weather.wind_gust, None);
                assert_eq!(weather.temperature, None);
                assert_eq!(weather.rain_last_hour, None);
                assert_eq!(weather.rain_24h, None);
                assert_eq!(weather.rain_since_midnight, None);
                assert_eq!(weather.humidity, None);
                assert_eq!(weather.barometric_pressure, None);
            }
            _ => panic!("expected Weather variant"),
        }
    }

    #[test]
    fn test_weather_too_short() {
        assert!(parse_packet(b"_1009", b"").is_none());
    }

    #[test]
    fn test_weather_via_parse_packet() {
        let info = b"_10090000c270s015g025t050r000p010P005h75b10200";
        let pkt = parse_packet(info, b"").unwrap();
        assert!(matches!(pkt, AprsPacket::Weather { .. }));
    }

    #[test]
    fn test_weather_from_position_comment() {
        // Position with weather in comment
        let comment = b"c220s004g005t077r001p002P003h50b10132";
        let wx = parse_weather_from_comment(comment).unwrap();
        assert_eq!(wx.wind_direction, Some(220));
        assert_eq!(wx.wind_speed, Some(4));
        assert_eq!(wx.temperature, Some(77));
        assert_eq!(wx.humidity, Some(50));
        assert_eq!(wx.barometric_pressure, Some(10132));
    }

    #[test]
    fn test_weather_from_comment_no_weather() {
        // Not a weather comment
        assert!(parse_weather_from_comment(b"PHG2360/Hello").is_none());
        assert!(parse_weather_from_comment(b"").is_none());
    }

    #[test]
    fn test_weather_from_position_comment_underscore_format() {
        // Real APRS-IS format: DDD/SSS then standard weather fields
        let comment = b"220/004g005t077r000p000P000h50b09900";
        let wx = parse_weather_from_comment(comment).unwrap();
        assert_eq!(wx.wind_direction, Some(220));
        assert_eq!(wx.wind_speed, Some(4));
        assert_eq!(wx.wind_gust, Some(5));
        assert_eq!(wx.temperature, Some(77));
        assert_eq!(wx.humidity, Some(50));
        assert_eq!(wx.barometric_pressure, Some(9900));
    }

    #[test]
    fn test_weather_position_packet_end_to_end() {
        // Full packet: @timestamp + position + _ symbol + weather comment
        let info = b"@092345z4903.50N/07201.75W_220/004g005t077r000p000P000h50b09900";
        let pkt = parse_packet(info, b"APRS").unwrap();
        match pkt {
            AprsPacket::Position { position, symbol_code, comment, .. } => {
                assert_eq!(symbol_code, b'_');
                assert!((position.lat as f64 / 1_000_000.0 - 49.058333).abs() < 0.001);
                // Weather should be parseable from comment
                let wx = parse_weather_from_comment(comment).unwrap();
                assert_eq!(wx.wind_direction, Some(220));
                assert_eq!(wx.temperature, Some(77));
            }
            _ => panic!("Expected Position"),
        }
    }

    #[test]
    fn test_weather_comment_missing_wind_data() {
        // Wind direction "..." means not available
        let comment = b".../...g005t077";
        let wx = parse_weather_from_comment(comment).unwrap();
        assert_eq!(wx.wind_direction, None);
        assert_eq!(wx.wind_speed, None);
        assert_eq!(wx.wind_gust, Some(5));
        assert_eq!(wx.temperature, Some(77));
    }

    #[test]
    fn test_weather_fields_parser() {
        let data = b"c180s010g020t055r100";
        let (wx, consumed) = parse_weather_fields(data);
        assert_eq!(wx.wind_direction, Some(180));
        assert_eq!(wx.wind_speed, Some(10));
        assert_eq!(wx.wind_gust, Some(20));
        assert_eq!(wx.temperature, Some(55));
        assert_eq!(wx.rain_last_hour, Some(100));
        assert_eq!(consumed, 20);
    }

    // ── Object Tests ────────────────────────────────────────────────

    #[test]
    fn test_object_live_plain() {
        // ;OBJNAME__*092345z4903.50N/07201.75W-
        let info = b";OBJNAME  *092345z4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Object { name, live, position, symbol_table, symbol_code, comment, .. } => {
                assert_eq!(name, b"OBJNAME");
                assert!(live);
                assert_eq!(position.lat, 49_058_333);
                assert_eq!(position.lon, -72_029_167);
                assert_eq!(symbol_table, b'/');
                assert_eq!(symbol_code, b'-');
                assert_eq!(comment, b"");
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_object_killed() {
        let info = b";OBJNAME  _092345z4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Object { name, live, .. } => {
                assert_eq!(name, b"OBJNAME");
                assert!(!live);
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_object_with_comment() {
        let info = b";OBJNAME  *092345z4903.50N/07201.75W-My comment";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Object { comment, .. } => {
                assert_eq!(comment, b"My comment");
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_object_name_trimming() {
        // Name with trailing spaces should be trimmed
        let info = b";TEST     *092345z4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Object { name, .. } => {
                assert_eq!(name, b"TEST");
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_object_full_name() {
        // 9-char name with no padding
        let info = b";123456789*092345z4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Object { name, .. } => {
                assert_eq!(name, b"123456789");
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_object_via_parse_packet() {
        let info = b";WX STN   *092345z4903.50N/07201.75W_c220s004g005t077";
        let pkt = parse_packet(info, b"").unwrap();
        assert!(matches!(pkt, AprsPacket::Object { .. }));
    }

    #[test]
    fn test_object_too_short() {
        assert!(parse_packet(b";SHORT", b"").is_none());
    }

    // ── Item Tests ──────────────────────────────────────────────────

    #[test]
    fn test_item_live() {
        let info = b")ITEM!4903.50N/07201.75W-Test item";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Item { name, live, position, symbol_table, symbol_code, comment } => {
                assert_eq!(name, b"ITEM");
                assert!(live);
                assert_eq!(position.lat, 49_058_333);
                assert_eq!(position.lon, -72_029_167);
                assert_eq!(symbol_table, b'/');
                assert_eq!(symbol_code, b'-');
                assert_eq!(comment, b"Test item");
            }
            _ => panic!("expected Item variant"),
        }
    }

    #[test]
    fn test_item_killed() {
        let info = b")ITEM_4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Item { name, live, .. } => {
                assert_eq!(name, b"ITEM");
                assert!(!live);
            }
            _ => panic!("expected Item variant"),
        }
    }

    #[test]
    fn test_item_short_name() {
        let info = b")ABC!4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Item { name, .. } => {
                assert_eq!(name, b"ABC");
            }
            _ => panic!("expected Item variant"),
        }
    }

    #[test]
    fn test_item_long_name() {
        let info = b")LONGNAME!4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Item { name, .. } => {
                assert_eq!(name, b"LONGNAME");
            }
            _ => panic!("expected Item variant"),
        }
    }

    #[test]
    fn test_item_with_comment() {
        let info = b")ITEM!4903.50N/07201.75W-Hello world";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Item { comment, .. } => {
                assert_eq!(comment, b"Hello world");
            }
            _ => panic!("expected Item variant"),
        }
    }

    #[test]
    fn test_item_too_short() {
        assert!(parse_packet(b")AB", b"").is_none());
    }

    // ── DTI Identification (new types) ─────────────────────────────

    #[test]
    fn test_dti_new_types() {
        assert_eq!(DataType::from_dti(b'}'), DataType::ThirdParty);
        assert_eq!(DataType::from_dti(b'$'), DataType::RawGps);
        assert_eq!(DataType::from_dti(b'T'), DataType::Telemetry);
        assert_eq!(DataType::from_dti(b'<'), DataType::Capabilities);
        assert_eq!(DataType::from_dti(b'{'), DataType::UserDefined);
        assert_eq!(DataType::from_dti(b'?'), DataType::Query);
    }

    // ── Telemetry Tests ────────────────────────────────────────────

    #[test]
    fn test_telemetry_full() {
        let info = b"T#123,100,200,300,400,500,10110011";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Telemetry { sequence, analog, digital } => {
                assert_eq!(sequence, 123);
                assert_eq!(analog[0], Some(100));
                assert_eq!(analog[1], Some(200));
                assert_eq!(analog[2], Some(300));
                assert_eq!(analog[3], Some(400));
                assert_eq!(analog[4], Some(500));
                assert_eq!(digital, 0b10110011);
            }
            _ => panic!("expected Telemetry variant"),
        }
    }

    #[test]
    fn test_telemetry_partial() {
        let info = b"T#001,50,60";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Telemetry { sequence, analog, digital } => {
                assert_eq!(sequence, 1);
                assert_eq!(analog[0], Some(50));
                assert_eq!(analog[1], Some(60));
                assert_eq!(analog[2], None);
                assert_eq!(analog[3], None);
                assert_eq!(analog[4], None);
                assert_eq!(digital, 0);
            }
            _ => panic!("expected Telemetry variant"),
        }
    }

    #[test]
    fn test_telemetry_no_hash() {
        let info = b"T123,50,60,70,80,90,11111111";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Telemetry { sequence, analog, digital } => {
                assert_eq!(sequence, 123);
                assert_eq!(analog[0], Some(50));
                assert_eq!(digital, 0b11111111);
            }
            _ => panic!("expected Telemetry variant"),
        }
    }

    // ── Third-Party Tests ──────────────────────────────────────────

    #[test]
    fn test_third_party() {
        let info = b"}WA1ABC>APRS,TCPIP*:!4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::ThirdParty { data } => {
                assert_eq!(data, b"WA1ABC>APRS,TCPIP*:!4903.50N/07201.75W-");
            }
            _ => panic!("expected ThirdParty variant"),
        }
    }

    // ── Raw GPS Tests ──────────────────────────────────────────────

    #[test]
    fn test_raw_gps() {
        let info = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::RawGps { data, parsed } => {
                assert!(data.starts_with(b"$GPRMC"));
                let nmea = parsed.unwrap();
                assert!(nmea.fix_valid);
                assert!(nmea.position.is_some());
            }
            _ => panic!("expected RawGps variant"),
        }
    }

    // ── Capabilities Tests ─────────────────────────────────────────

    #[test]
    fn test_capabilities() {
        let info = b"<IGATE,MSG_CNT=123,LOC_CNT=45";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Capabilities { data } => {
                assert_eq!(data, b"IGATE,MSG_CNT=123,LOC_CNT=45");
            }
            _ => panic!("expected Capabilities variant"),
        }
    }

    // ── Query Tests ────────────────────────────────────────────────

    #[test]
    fn test_query_aprs() {
        let info = b"?APRS?";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Query { query_type } => {
                assert_eq!(query_type, b"APRS");
            }
            _ => panic!("expected Query variant"),
        }
    }

    #[test]
    fn test_query_wx() {
        let info = b"?WX?";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Query { query_type } => {
                assert_eq!(query_type, b"WX");
            }
            _ => panic!("expected Query variant"),
        }
    }

    #[test]
    fn test_query_igate() {
        let info = b"?IGATE?";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Query { query_type } => {
                assert_eq!(query_type, b"IGATE");
            }
            _ => panic!("expected Query variant"),
        }
    }

    // ── UserDefined Tests ──────────────────────────────────────────

    #[test]
    fn test_user_defined() {
        let info = b"{custom data here";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::UserDefined { data } => {
                assert_eq!(data, b"custom data here");
            }
            _ => panic!("expected UserDefined variant"),
        }
    }

    // ── Message Type Tests ─────────────────────────────────────────

    #[test]
    fn test_message_type_private() {
        let info = b":WA1ABC   :Hello{123";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { message_type, .. } => {
                assert_eq!(message_type, MessageType::Private);
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_type_ack() {
        let info = b":WA1ABC   :ack123";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { message_type, text, .. } => {
                assert_eq!(message_type, MessageType::Ack);
                assert_eq!(text, b"ack123");
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_type_rej() {
        let info = b":WA1ABC   :rej123";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { message_type, .. } => {
                assert_eq!(message_type, MessageType::Rej);
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_type_bulletin() {
        let info = b":BLN3     :Snow expected";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { message_type, .. } => {
                assert_eq!(message_type, MessageType::Bulletin);
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_type_announcement() {
        let info = b":BLNA     :Club meeting";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { message_type, .. } => {
                assert_eq!(message_type, MessageType::Announcement);
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_message_type_nws() {
        let info = b":NWS-WARN :Tornado warning";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Message { message_type, .. } => {
                assert_eq!(message_type, MessageType::Nws);
            }
            _ => panic!("expected Message variant"),
        }
    }

    // ── Timestamp Tests ────────────────────────────────────────────

    #[test]
    fn test_timestamp_dhm() {
        assert_eq!(
            parse_timestamp(b"092345z"),
            Some(Timestamp::Dhm { day: 9, hour: 23, minute: 45 })
        );
    }

    #[test]
    fn test_timestamp_hms() {
        assert_eq!(
            parse_timestamp(b"123456h"),
            Some(Timestamp::Hms { hour: 12, minute: 34, second: 56 })
        );
    }

    #[test]
    fn test_timestamp_dhm_local() {
        assert_eq!(
            parse_timestamp(b"092345/"),
            Some(Timestamp::DhmLocal { day: 9, hour: 23, minute: 45 })
        );
    }

    #[test]
    fn test_timestamp_in_position() {
        let info = b"/092345z4903.50N/07201.75W>";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { timestamp, .. } => {
                assert_eq!(timestamp, Some(Timestamp::Dhm { day: 9, hour: 23, minute: 45 }));
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_no_timestamp_in_bang_position() {
        let info = b"!4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { timestamp, .. } => {
                assert_eq!(timestamp, None);
            }
            _ => panic!("expected Position variant"),
        }
    }

    #[test]
    fn test_timestamp_in_object() {
        let info = b";OBJNAME  *092345z4903.50N/07201.75W-";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Object { timestamp, .. } => {
                assert_eq!(timestamp, Some(Timestamp::Dhm { day: 9, hour: 23, minute: 45 }));
            }
            _ => panic!("expected Object variant"),
        }
    }

    // ── Status Enhanced Tests ──────────────────────────────────────

    #[test]
    fn test_status_with_timestamp() {
        let info = b">092345zNet Control";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Status { text, timestamp, maidenhead, .. } => {
                assert_eq!(text, b"Net Control");
                assert_eq!(timestamp, Some(Timestamp::Dhm { day: 9, hour: 23, minute: 45 }));
                assert_eq!(maidenhead, None);
            }
            _ => panic!("expected Status variant"),
        }
    }

    #[test]
    fn test_status_with_maidenhead_6() {
        let info = b">IO91SX status text";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Status { text, timestamp, maidenhead, .. } => {
                assert_eq!(maidenhead, Some(&b"IO91SX"[..]));
                assert_eq!(text, b"status text");
                assert_eq!(timestamp, None);
            }
            _ => panic!("expected Status variant"),
        }
    }

    #[test]
    fn test_status_with_maidenhead_4() {
        let info = b">IO91 status";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Status { maidenhead, text, .. } => {
                assert_eq!(maidenhead, Some(&b"IO91"[..]));
                assert_eq!(text, b"status");
            }
            _ => panic!("expected Status variant"),
        }
    }

    #[test]
    fn test_status_plain() {
        let info = b">Net Control - Loss Angeles";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Status { text, timestamp, maidenhead, .. } => {
                assert_eq!(text, b"Net Control - Loss Angeles");
                assert_eq!(timestamp, None);
                assert_eq!(maidenhead, None);
            }
            _ => panic!("expected Status variant"),
        }
    }

    // ── Weather Snowfall Test ──────────────────────────────────────

    #[test]
    fn test_weather_snowfall() {
        // Wind speed + snowfall (second 's')
        let info = b"_10090000c220s004g005t077s012";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Weather { weather, .. } => {
                assert_eq!(weather.wind_speed, Some(4));
                assert_eq!(weather.snowfall, Some(12));
            }
            _ => panic!("expected Weather variant"),
        }
    }

    // ── Comment Field Tests ────────────────────────────────────────

    #[test]
    fn test_comment_phg() {
        let fields = parse_comment_fields(b"PHG2360/Hello");
        assert!(fields.phg.is_some());
        let phg = fields.phg.unwrap();
        assert_eq!(phg.power_watts, 4); // 2^2
        assert_eq!(phg.height_feet, 80); // 10 * 2^3
        assert_eq!(phg.gain_db, 6);
        assert_eq!(phg.directivity, 0); // 0 = omni
    }

    #[test]
    fn test_comment_altitude() {
        let fields = parse_comment_fields(b"PHG2360/A=001234");
        assert_eq!(fields.altitude, Some(1234));
    }

    #[test]
    fn test_comment_altitude_standalone() {
        let fields = parse_comment_fields(b"/A=001234rest of comment");
        assert_eq!(fields.altitude, Some(1234));
    }

    #[test]
    fn test_comment_rng() {
        let fields = parse_comment_fields(b"RNG0050");
        assert_eq!(fields.range, Some(50));
    }

    #[test]
    fn test_comment_dfs() {
        let fields = parse_comment_fields(b"DFS2360");
        assert!(fields.dfs.is_some());
        let dfs = fields.dfs.unwrap();
        assert_eq!(dfs.strength, 2);
        assert_eq!(dfs.height_feet, 80);
        assert_eq!(dfs.gain_db, 6);
        assert_eq!(dfs.directivity, 0);
    }

    #[test]
    fn test_comment_course_speed() {
        let fields = parse_comment_fields(b"088/036Hello");
        assert_eq!(fields.course_speed, Some((88, 36)));
        assert_eq!(fields.text, b"Hello");
    }

    #[test]
    fn test_comment_empty() {
        let fields = parse_comment_fields(b"");
        assert!(fields.phg.is_none());
        assert!(fields.range.is_none());
        assert!(fields.altitude.is_none());
        assert!(fields.course_speed.is_none());
        assert!(fields.dfs.is_none());
        assert_eq!(fields.text, b"");
    }

    #[test]
    fn test_comment_plain_text() {
        let fields = parse_comment_fields(b"Just a plain comment");
        assert!(fields.phg.is_none());
        assert_eq!(fields.text, b"Just a plain comment");
    }

    // ── Compressed Position Extra Tests ────────────────────────────

    #[test]
    fn test_compressed_position_basic() {
        // `/` + base91 lat (4) + base91 lon (4) + sym_code + cs + se + type
        // From APRS101 example: /YYYY XXXX >cs_t
        // Using known encoding for 49.5N, 72.75W
        // This is a basic test that the compressed path still works
        let info = b"!/5L!!<*e7>7P[";
        let pkt = parse_packet(info, b"").unwrap();
        match pkt {
            AprsPacket::Position { position, compressed_extra, .. } => {
                // Position should parse
                assert_ne!(position.lat, 0);
                assert_ne!(position.lon, 0);
                // Extra may or may not be present depending on cs/type bytes
                let _ = compressed_extra; // just verify it compiles
            }
            _ => panic!("expected Position variant"),
        }
    }
}
