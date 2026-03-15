//! NMEA sentence parser for extracting GPS position data from APRS RawGps packets.
//!
//! Supports `$GPRMC`, `$GPGGA`, and equivalent talker IDs (`$GN`, `$GL`, `$GA`, `$GB`).
//! Pure `no_std`, zero-copy, integer-only arithmetic.

/// Parsed NMEA data extracted from $GPRMC/$GPGGA sentences.
#[derive(Clone, Debug, Default)]
pub struct NmeaData {
    /// Parsed position (reuses existing Position with i32 microdegrees)
    pub position: Option<super::Position>,
    /// Speed in knots × 10 (fixed-point, no floats)
    pub speed_tenths_kts: Option<u32>,
    /// Course in degrees × 10
    pub course_tenths_deg: Option<u32>,
    /// Altitude in decimeters (meters × 10)
    pub altitude_dm: Option<i32>,
    /// Number of satellites in use
    pub satellites: Option<u8>,
    /// GGA fix quality: 0=invalid, 1=GPS, 2=DGPS
    pub fix_quality: Option<u8>,
    /// HDOP × 10
    pub hdop_tenths: Option<u16>,
    /// UTC time (hour, minute, second)
    pub time: Option<(u8, u8, u8)>,
    /// Date (day, month, year_2digit)
    pub date: Option<(u8, u8, u8)>,
    /// RMC fix valid: 'A' = true, 'V' = false
    pub fix_valid: bool,
}

/// Parse an NMEA sentence (with or without leading `$`). Returns None if invalid.
pub fn parse_nmea(sentence: &[u8]) -> Option<NmeaData> {
    if sentence.is_empty() {
        return None;
    }

    // Skip leading '$' if present
    let body = if sentence[0] == b'$' {
        &sentence[1..]
    } else {
        sentence
    };

    // Validate checksum if '*' is present in the original sentence
    if !validate_checksum(sentence) {
        return None;
    }

    // Strip checksum suffix (everything after '*') for field parsing
    let body_no_cksum = if let Some(star) = body.iter().position(|&b| b == b'*') {
        &body[..star]
    } else {
        body
    };

    // Split into fields
    let mut fields = [&[][..]; 16];
    let n = split_fields(body_no_cksum, &mut fields);
    if n == 0 {
        return None;
    }

    // Check sentence type (skip talker ID prefix: GP, GN, GL, GA, GB)
    let sentence_id = fields[0];
    if sentence_id.len() < 5 {
        return None;
    }
    let type_suffix = &sentence_id[2..]; // e.g. "RMC" from "GPRMC"

    if type_suffix == b"RMC" {
        parse_rmc(&fields, n)
    } else if type_suffix == b"GGA" {
        parse_gga(&fields, n)
    } else {
        None
    }
}

/// Validate NMEA checksum: XOR bytes between '$' and '*', compare to 2-char hex after '*'.
/// Returns true if no checksum present (no '*') or checksum matches.
fn validate_checksum(sentence: &[u8]) -> bool {
    // Find '$' start
    let start = if sentence.first() == Some(&b'$') {
        1
    } else {
        0
    };

    // Find '*'
    let star_pos = match sentence.iter().position(|&b| b == b'*') {
        Some(p) => p,
        None => return true, // No checksum to validate
    };

    // Need exactly 2 hex chars after '*'
    if sentence.len() < star_pos + 3 {
        return false;
    }

    // XOR all bytes between '$' and '*'
    let mut xor: u8 = 0;
    for &b in &sentence[start..star_pos] {
        xor ^= b;
    }

    // Parse expected checksum
    let expected = match (
        hex_digit(sentence[star_pos + 1]),
        hex_digit(sentence[star_pos + 2]),
    ) {
        (Some(h), Some(l)) => h << 4 | l,
        _ => return false,
    };

    xor == expected
}

/// Parse a single hex digit.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Split comma-separated fields into a fixed-size stack array.
/// Returns the number of fields found.
fn split_fields<'a>(body: &'a [u8], buf: &mut [&'a [u8]; 16]) -> usize {
    let mut count = 0;
    let mut start = 0;

    for i in 0..body.len() {
        if body[i] == b',' {
            if count < 16 {
                buf[count] = &body[start..i];
                count += 1;
            }
            start = i + 1;
        }
    }

    // Last field (after final comma or entire string if no comma)
    if count < 16 {
        buf[count] = &body[start..];
        count += 1;
    }

    count
}

/// Parse NMEA latitude: `ddmm.mmmm` + `N`/`S` → microdegrees.
fn parse_nmea_lat(field: &[u8], ns: &[u8]) -> Option<i32> {
    // Minimum: "ddmm.m" = 6 chars, but we handle "ddmm.mm" to "ddmm.mmmm"
    if field.len() < 6 || !field.contains(&b'.') {
        return None;
    }

    let dot_pos = field.iter().position(|&b| b == b'.')?;
    if dot_pos < 4 {
        return None; // Need at least ddmm before dot
    }

    // Parse degrees (first dot_pos-2 digits)
    let deg = parse_uint(&field[..dot_pos - 2])? as i32;

    // Parse minutes integer part (2 digits before dot)
    let min_int = parse_uint(&field[dot_pos - 2..dot_pos])? as i32;

    // Parse fractional minutes — normalize to ten-thousandths
    let frac_str = &field[dot_pos + 1..];
    let min_frac = parse_frac_ten_thousandths(frac_str)? as i32;

    // Total minutes in ten-thousandths
    let total_min_ten_thousandths = min_int * 10_000 + min_frac;

    // Convert to microdegrees: deg * 1_000_000 + (min_tenthousandths * 100) / 6
    let microdeg = deg * 1_000_000 + (total_min_ten_thousandths * 100 + 30) / 60;

    if ns == b"S" || ns == b"s" {
        Some(-microdeg)
    } else {
        Some(microdeg)
    }
}

/// Parse NMEA longitude: `dddmm.mmmm` + `E`/`W` → microdegrees.
fn parse_nmea_lon(field: &[u8], ew: &[u8]) -> Option<i32> {
    if field.len() < 7 || !field.contains(&b'.') {
        return None;
    }

    let dot_pos = field.iter().position(|&b| b == b'.')?;
    if dot_pos < 4 {
        return None;
    }

    let deg = parse_uint(&field[..dot_pos - 2])? as i32;
    let min_int = parse_uint(&field[dot_pos - 2..dot_pos])? as i32;
    let frac_str = &field[dot_pos + 1..];
    let min_frac = parse_frac_ten_thousandths(frac_str)? as i32;

    let total_min_ten_thousandths = min_int * 10_000 + min_frac;
    let microdeg = deg * 1_000_000 + (total_min_ten_thousandths * 100 + 30) / 60;

    if ew == b"W" || ew == b"w" {
        Some(-microdeg)
    } else {
        Some(microdeg)
    }
}

/// Parse fractional part, normalizing to ten-thousandths.
/// "5" → 5000, "50" → 5000, "500" → 5000, "5000" → 5000, "038" → 380
fn parse_frac_ten_thousandths(frac: &[u8]) -> Option<u32> {
    if frac.is_empty() {
        return Some(0);
    }
    let mut val = 0u32;
    let mut digits = 0;
    for &b in frac.iter().take(4) {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val * 10 + (b - b'0') as u32;
        digits += 1;
    }
    // Pad to 4 digits
    for _ in digits..4 {
        val *= 10;
    }
    Some(val)
}

/// Parse unsigned integer from ASCII digits.
fn parse_uint(data: &[u8]) -> Option<u32> {
    if data.is_empty() {
        return None;
    }
    let mut val = 0u32;
    for &b in data {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(val)
}

/// Parse decimal with one fractional digit into tenths.
/// "12.3" → 123, "0.5" → 5, "123" → 1230, "" → None
fn parse_decimal_tenths(field: &[u8]) -> Option<u32> {
    if field.is_empty() {
        return None;
    }

    if let Some(dot) = field.iter().position(|&b| b == b'.') {
        let integer = if dot > 0 {
            parse_uint(&field[..dot])?
        } else {
            0
        };
        let frac_part = &field[dot + 1..];
        let frac = if frac_part.is_empty() {
            0
        } else {
            // Take first digit only for tenths
            let d = frac_part[0];
            if !d.is_ascii_digit() {
                return None;
            }
            (d - b'0') as u32
        };
        Some(integer * 10 + frac)
    } else {
        // No decimal point — treat as integer, multiply by 10
        let integer = parse_uint(field)?;
        Some(integer * 10)
    }
}

/// Parse signed decimal with one fractional digit into signed tenths.
/// "-12.3" → -123, "12.3" → 123, "" → None
fn parse_signed_decimal_tenths(field: &[u8]) -> Option<i32> {
    if field.is_empty() {
        return None;
    }
    if field[0] == b'-' {
        let val = parse_decimal_tenths(&field[1..])?;
        Some(-(val as i32))
    } else {
        let val = parse_decimal_tenths(field)?;
        Some(val as i32)
    }
}

/// Parse NMEA time field: "hhmmss" or "hhmmss.ss" → (hour, minute, second).
fn parse_nmea_time(field: &[u8]) -> Option<(u8, u8, u8)> {
    if field.len() < 6 {
        return None;
    }
    let h = parse_two_digits(field[0], field[1])?;
    let m = parse_two_digits(field[2], field[3])?;
    let s = parse_two_digits(field[4], field[5])?;
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    Some((h, m, s))
}

/// Parse NMEA date field: "ddmmyy" → (day, month, year_2digit).
fn parse_nmea_date(field: &[u8]) -> Option<(u8, u8, u8)> {
    if field.len() < 6 {
        return None;
    }
    let d = parse_two_digits(field[0], field[1])?;
    let m = parse_two_digits(field[2], field[3])?;
    let y = parse_two_digits(field[4], field[5])?;
    if d == 0 || d > 31 || m == 0 || m > 12 {
        return None;
    }
    Some((d, m, y))
}

/// Parse two ASCII digit bytes into a u8.
fn parse_two_digits(a: u8, b: u8) -> Option<u8> {
    if a.is_ascii_digit() && b.is_ascii_digit() {
        Some((a - b'0') * 10 + (b - b'0'))
    } else {
        None
    }
}

/// Parse RMC sentence fields into NmeaData.
/// Fields: 0=id, 1=time, 2=status, 3=lat, 4=N/S, 5=lon, 6=E/W,
///         7=speed, 8=course, 9=date, 10=mag_var, 11=mag_dir
fn parse_rmc(fields: &[&[u8]], n: usize) -> Option<NmeaData> {
    if n < 10 {
        return None;
    }

    // Position
    let lat = parse_nmea_lat(fields[3], fields[4]);
    let lon = parse_nmea_lon(fields[5], fields[6]);
    let position = if let (Some(lat_val), Some(lon_val)) = (lat, lon) {
        Some(super::Position {
            lat: lat_val,
            lon: lon_val,
            ambiguity: 0,
        })
    } else {
        None
    };

    Some(NmeaData {
        time: parse_nmea_time(fields[1]),
        fix_valid: fields[2] == b"A",
        position,
        speed_tenths_kts: parse_decimal_tenths(fields[7]),
        course_tenths_deg: parse_decimal_tenths(fields[8]),
        date: parse_nmea_date(fields[9]),
        ..NmeaData::default()
    })
}

/// Parse GGA sentence fields into NmeaData.
/// Fields: 0=id, 1=time, 2=lat, 3=N/S, 4=lon, 5=E/W, 6=fix_quality,
///         7=num_sats, 8=hdop, 9=alt, 10=alt_unit, 11=geoid, 12=geoid_unit,
///         13=dgps_age, 14=dgps_id
fn parse_gga(fields: &[&[u8]], n: usize) -> Option<NmeaData> {
    if n < 10 {
        return None;
    }

    // Position
    let lat = parse_nmea_lat(fields[2], fields[3]);
    let lon = parse_nmea_lon(fields[4], fields[5]);
    let position = if let (Some(lat_val), Some(lon_val)) = (lat, lon) {
        Some(super::Position {
            lat: lat_val,
            lon: lon_val,
            ambiguity: 0,
        })
    } else {
        None
    };

    // Fix quality and validity
    let fix_quality = parse_uint(fields[6]).map(|q| q as u8);
    let fix_valid = fix_quality.is_some_and(|q| q > 0);

    Some(NmeaData {
        time: parse_nmea_time(fields[1]),
        position,
        fix_quality,
        fix_valid,
        satellites: parse_uint(fields[7]).map(|s| s as u8),
        hdop_tenths: parse_decimal_tenths(fields[8]).map(|v| v as u16),
        altitude_dm: if n > 9 {
            parse_signed_decimal_tenths(fields[9])
        } else {
            None
        },
        ..NmeaData::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Phase 1: Checksum Validation ─────────────────────────────────

    #[test]
    fn test_checksum_valid() {
        let sentence = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        assert!(validate_checksum(sentence));
    }

    #[test]
    fn test_checksum_invalid() {
        let sentence = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*FF";
        assert!(!validate_checksum(sentence));
    }

    #[test]
    fn test_checksum_missing_star() {
        // No '*' means no checksum to validate — should pass
        let sentence = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W";
        assert!(validate_checksum(sentence));
    }

    #[test]
    fn test_checksum_no_dollar_prefix() {
        // Also valid — checksum computed from byte 0 to '*'
        let sentence = b"GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        assert!(validate_checksum(sentence));
    }

    #[test]
    fn test_checksum_truncated_after_star() {
        let sentence = b"$GPRMC,123519*6";
        assert!(!validate_checksum(sentence));
    }

    // ── Phase 2: Field Splitting ─────────────────────────────────────

    #[test]
    fn test_split_fields_rmc() {
        let body = b"GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W";
        let mut buf = [&[][..]; 16];
        let n = split_fields(body, &mut buf);
        assert_eq!(n, 12);
        assert_eq!(buf[0], b"GPRMC");
        assert_eq!(buf[1], b"123519");
        assert_eq!(buf[2], b"A");
        assert_eq!(buf[3], b"4807.038");
        assert_eq!(buf[11], b"W");
    }

    #[test]
    fn test_split_fields_empty_fields() {
        let body = b"GPRMC,,V,,,,,,,,,";
        let mut buf = [&[][..]; 16];
        let n = split_fields(body, &mut buf);
        assert_eq!(n, 12);
        assert_eq!(buf[0], b"GPRMC");
        assert!(buf[1].is_empty());
        assert_eq!(buf[2], b"V");
    }

    #[test]
    fn test_split_fields_max_16() {
        // More than 16 fields — should cap at 16
        let body = b"a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q,r";
        let mut buf = [&[][..]; 16];
        let n = split_fields(body, &mut buf);
        assert_eq!(n, 16);
        assert_eq!(buf[0], b"a");
        assert_eq!(buf[15], b"p");
    }

    // ── Phase 3: Coordinate Parsing ──────────────────────────────────

    #[test]
    fn test_lat_north() {
        // 4903.5000,N → 49° 03.5000' → 49 + 3.5/60 = 49.058333°
        let result = parse_nmea_lat(b"4903.5000", b"N").unwrap();
        assert_eq!(result, 49_058_333);
    }

    #[test]
    fn test_lat_south() {
        let result = parse_nmea_lat(b"4903.5000", b"S").unwrap();
        assert_eq!(result, -49_058_333);
    }

    #[test]
    fn test_lon_west() {
        // 07201.7500,W → 72° 01.7500' → 72 + 1.75/60 = 72.029167°
        let result = parse_nmea_lon(b"07201.7500", b"W").unwrap();
        assert_eq!(result, -72_029_167);
    }

    #[test]
    fn test_lon_east() {
        let result = parse_nmea_lon(b"07201.7500", b"E").unwrap();
        assert_eq!(result, 72_029_167);
    }

    #[test]
    fn test_lat_equator() {
        let result = parse_nmea_lat(b"0000.0000", b"N").unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_lon_prime_meridian() {
        let result = parse_nmea_lon(b"00000.0000", b"E").unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_lat_fewer_decimal_digits() {
        // 4807.038 — only 3 fractional digits → pad to 0380
        let result = parse_nmea_lat(b"4807.038", b"N").unwrap();
        // 48° 07.0380' = 48 + 7.038/60 = 48.1173°
        assert_eq!(result, 48_117_300);
    }

    #[test]
    fn test_lat_invalid_short() {
        assert!(parse_nmea_lat(b"48", b"N").is_none());
    }

    #[test]
    fn test_lon_invalid_no_dot() {
        assert!(parse_nmea_lon(b"0720175", b"W").is_none());
    }

    // ── Phase 4: Decimal Parsing ─────────────────────────────────────

    #[test]
    fn test_decimal_tenths_with_fraction() {
        assert_eq!(parse_decimal_tenths(b"12.3"), Some(123));
    }

    #[test]
    fn test_decimal_tenths_small() {
        assert_eq!(parse_decimal_tenths(b"0.5"), Some(5));
    }

    #[test]
    fn test_decimal_tenths_empty() {
        assert_eq!(parse_decimal_tenths(b""), None);
    }

    #[test]
    fn test_decimal_tenths_no_decimal() {
        assert_eq!(parse_decimal_tenths(b"123"), Some(1230));
    }

    #[test]
    fn test_decimal_tenths_dot_only() {
        // ".5" → integer=0, frac=5
        assert_eq!(parse_decimal_tenths(b".5"), Some(5));
    }

    #[test]
    fn test_signed_decimal_tenths_positive() {
        assert_eq!(parse_signed_decimal_tenths(b"12.3"), Some(123));
    }

    #[test]
    fn test_signed_decimal_tenths_negative() {
        assert_eq!(parse_signed_decimal_tenths(b"-12.3"), Some(-123));
    }

    #[test]
    fn test_signed_decimal_tenths_empty() {
        assert_eq!(parse_signed_decimal_tenths(b""), None);
    }

    // ── Phase 5: Time/Date Parsing ───────────────────────────────────

    #[test]
    fn test_time_valid() {
        assert_eq!(parse_nmea_time(b"123456"), Some((12, 34, 56)));
    }

    #[test]
    fn test_time_with_fractional() {
        // "123456.00" — fractional seconds ignored
        assert_eq!(parse_nmea_time(b"123456.00"), Some((12, 34, 56)));
    }

    #[test]
    fn test_time_invalid_short() {
        assert_eq!(parse_nmea_time(b"1234"), None);
    }

    #[test]
    fn test_time_invalid_hour() {
        assert_eq!(parse_nmea_time(b"250000"), None);
    }

    #[test]
    fn test_date_valid() {
        assert_eq!(parse_nmea_date(b"010203"), Some((1, 2, 3)));
    }

    #[test]
    fn test_date_valid_230394() {
        assert_eq!(parse_nmea_date(b"230394"), Some((23, 3, 94)));
    }

    #[test]
    fn test_date_invalid_short() {
        assert_eq!(parse_nmea_date(b"0102"), None);
    }

    #[test]
    fn test_date_invalid_month() {
        assert_eq!(parse_nmea_date(b"011300"), None);
    }

    #[test]
    fn test_date_invalid_day_zero() {
        assert_eq!(parse_nmea_date(b"000103"), None);
    }

    // ── Phase 6: RMC Sentence ────────────────────────────────────────

    #[test]
    fn test_rmc_full() {
        let sentence = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        let result = parse_nmea(sentence).unwrap();

        assert!(result.fix_valid);
        assert_eq!(result.time, Some((12, 35, 19)));
        assert_eq!(result.date, Some((23, 3, 94)));
        assert_eq!(result.speed_tenths_kts, Some(224)); // 22.4 knots
        assert_eq!(result.course_tenths_deg, Some(844)); // 84.4°

        let pos = result.position.unwrap();
        assert_eq!(pos.lat, 48_117_300); // 48° 07.038' N
        assert_eq!(pos.lon, 11_516_667); // 011° 31.000' E
    }

    #[test]
    fn test_rmc_void_fix() {
        let sentence = b"$GPRMC,123519,V,,,,,,,230394,,";
        let result = parse_nmea(sentence).unwrap();

        assert!(!result.fix_valid);
        assert!(result.position.is_none());
        assert_eq!(result.date, Some((23, 3, 94)));
    }

    #[test]
    fn test_rmc_without_checksum() {
        // No checksum — should still parse
        let sentence = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W";
        let result = parse_nmea(sentence).unwrap();
        assert!(result.fix_valid);
        assert!(result.position.is_some());
    }

    // ── Phase 7: GGA Sentence ────────────────────────────────────────

    #[test]
    fn test_gga_full() {
        let sentence = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,*4F";
        let result = parse_nmea(sentence).unwrap();

        assert!(result.fix_valid);
        assert_eq!(result.time, Some((12, 35, 19)));
        assert_eq!(result.fix_quality, Some(1));
        assert_eq!(result.satellites, Some(8));
        assert_eq!(result.hdop_tenths, Some(9)); // 0.9
        assert_eq!(result.altitude_dm, Some(5454)); // 545.4m

        let pos = result.position.unwrap();
        assert_eq!(pos.lat, 48_117_300);
        assert_eq!(pos.lon, 11_516_667);
    }

    #[test]
    fn test_gga_no_fix() {
        let sentence = b"$GPGGA,123519,,,,,0,00,,,M,,M,,*6B";
        let result = parse_nmea(sentence).unwrap();

        assert!(!result.fix_valid);
        assert_eq!(result.fix_quality, Some(0));
        assert!(result.position.is_none());
    }

    #[test]
    fn test_gga_negative_altitude() {
        // Dead Sea area — altitude can be negative
        let sentence = b"$GPGGA,120000,3130.000,N,03530.000,E,1,05,1.2,-400.5,M,20.0,M,,";
        let result = parse_nmea(sentence).unwrap();
        assert_eq!(result.altitude_dm, Some(-4005));
    }

    // ── Phase 8: Talker ID Variants ──────────────────────────────────

    #[test]
    fn test_gnrmc() {
        let sentence = b"$GNRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W";
        let result = parse_nmea(sentence).unwrap();
        assert!(result.fix_valid);
        assert!(result.position.is_some());
    }

    #[test]
    fn test_glrmc() {
        let sentence = b"$GLRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W";
        let result = parse_nmea(sentence).unwrap();
        assert!(result.fix_valid);
    }

    #[test]
    fn test_gngga() {
        let sentence = b"$GNGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,";
        let result = parse_nmea(sentence).unwrap();
        assert!(result.fix_valid);
        assert_eq!(result.satellites, Some(8));
    }

    // ── Phase 9: Integration (parse_nmea entry point) ────────────────

    #[test]
    fn test_parse_nmea_rmc() {
        let sentence = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        assert!(parse_nmea(sentence).is_some());
    }

    #[test]
    fn test_parse_nmea_gga() {
        let sentence = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,47.0,M,,*4F";
        assert!(parse_nmea(sentence).is_some());
    }

    #[test]
    fn test_parse_nmea_unknown_sentence() {
        let sentence = b"$GPVTG,054.7,T,034.4,M,005.5,N,010.2,K*48";
        assert!(parse_nmea(sentence).is_none());
    }

    #[test]
    fn test_parse_nmea_bad_checksum() {
        let sentence = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*FF";
        assert!(parse_nmea(sentence).is_none());
    }

    #[test]
    fn test_parse_nmea_empty() {
        assert!(parse_nmea(b"").is_none());
    }

    #[test]
    fn test_parse_nmea_no_dollar() {
        // Without '$' prefix — still works
        let sentence = b"GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W";
        assert!(parse_nmea(sentence).is_some());
    }

    // ── Phase 10: Edge cases ─────────────────────────────────────────

    #[test]
    fn test_frac_ten_thousandths_padding() {
        assert_eq!(parse_frac_ten_thousandths(b"5"), Some(5000));
        assert_eq!(parse_frac_ten_thousandths(b"50"), Some(5000));
        assert_eq!(parse_frac_ten_thousandths(b"500"), Some(5000));
        assert_eq!(parse_frac_ten_thousandths(b"5000"), Some(5000));
        assert_eq!(parse_frac_ten_thousandths(b"038"), Some(380));
        assert_eq!(parse_frac_ten_thousandths(b"0380"), Some(380));
        assert_eq!(parse_frac_ten_thousandths(b""), Some(0));
    }

    #[test]
    fn test_hex_digit() {
        assert_eq!(hex_digit(b'0'), Some(0));
        assert_eq!(hex_digit(b'9'), Some(9));
        assert_eq!(hex_digit(b'A'), Some(10));
        assert_eq!(hex_digit(b'F'), Some(15));
        assert_eq!(hex_digit(b'a'), Some(10));
        assert_eq!(hex_digit(b'f'), Some(15));
        assert_eq!(hex_digit(b'G'), None);
    }

    #[test]
    fn test_parse_two_digits() {
        assert_eq!(parse_two_digits(b'0', b'0'), Some(0));
        assert_eq!(parse_two_digits(b'9', b'9'), Some(99));
        assert_eq!(parse_two_digits(b'1', b'2'), Some(12));
        assert_eq!(parse_two_digits(b'A', b'0'), None);
    }
}
