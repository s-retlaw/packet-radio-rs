//! APRS-IS protocol utilities — TNC-2 parsing and AX.25 conversion.
//!
//! This module provides:
//! - TNC-2 text line parsing (`parse_tnc2_line`)
//! - TNC-2 to binary AX.25 frame conversion (`tnc2_to_ax25`)
//! - APRS-IS client configuration types
//!
//! These are shared between the desktop TNC, APRS viewer, and any other
//! application that needs to interface with the APRS Internet System.

/// Configuration for an APRS-IS client connection.
#[derive(Debug, Clone)]
pub struct AprsIsClientConfig {
    pub host: String,
    pub port: u16,
    pub callsign: String,
    pub passcode: String,
    pub filter: String,
}

/// Parsed TNC-2 format line.
///
/// TNC-2 format: `SOURCE>DEST,PATH1,PATH2:INFO`
/// This is the standard text format used by APRS-IS and many TNC applications.
#[derive(Debug, Clone)]
pub struct Tnc2Packet {
    pub source: String,
    pub dest: String,
    pub path: Vec<String>,
    pub info: Vec<u8>,
}

/// Parse a TNC-2 format line into its components.
///
/// Format: `SOURCE>DEST,PATH1,PATH2:INFO`
///
/// Returns `None` for empty lines, comment lines (starting with `#`),
/// or malformed input.
pub fn parse_tnc2_line(line: &str) -> Option<Tnc2Packet> {
    let line = line.trim();

    // Skip comments and empty lines
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Find source>dest separator
    let gt_pos = line.find('>')?;
    let source = &line[..gt_pos];
    if source.is_empty() {
        return None;
    }

    let rest = &line[gt_pos + 1..];

    // Find the info separator ':'
    let colon_pos = rest.find(':')?;
    let dest_path = &rest[..colon_pos];
    let info = &rest[colon_pos + 1..];

    // Split dest,path1,path2,...
    let mut parts = dest_path.split(',');
    let dest = parts.next()?.to_string();
    if dest.is_empty() {
        return None;
    }

    let path: Vec<String> = parts.map(|s| s.to_string()).collect();

    Some(Tnc2Packet {
        source: source.to_string(),
        dest,
        path,
        info: info.as_bytes().to_vec(),
    })
}

/// Build a synthetic AX.25 frame from a TNC-2 parsed packet.
///
/// This converts the text-format TNC-2 packet into a binary AX.25 frame
/// suitable for parsing with `packet_radio_core::ax25::Frame::parse()`.
///
/// The resulting frame has:
/// - Properly shifted address fields (callsign bytes << 1)
/// - H-bit preservation for digipeater markers (`*` suffix)
/// - SSID encoding from `-N` suffixes
/// - UI control (0x03) and no-L3 PID (0xF0)
pub fn tnc2_to_ax25(pkt: &Tnc2Packet) -> Vec<u8> {
    let mut frame = Vec::new();

    let has_path = !pkt.path.is_empty();

    // Destination
    frame.extend_from_slice(&encode_address(&pkt.dest, false));

    // Source (last if no digipeaters)
    frame.extend_from_slice(&encode_address(&pkt.source, !has_path));

    // Digipeaters
    for (i, digi) in pkt.path.iter().enumerate() {
        let is_last = i == pkt.path.len() - 1;
        frame.extend_from_slice(&encode_address(digi, is_last));
    }

    // Control + PID
    frame.push(0x03); // UI
    frame.push(0xF0); // No L3

    // Info field
    frame.extend_from_slice(&pkt.info);

    frame
}

/// Encode an AX.25 address field (7 bytes) from a callsign-SSID string.
///
/// Callsign characters are shifted left by 1, space-padded to 6 chars.
/// SSID is encoded in the 7th byte. The H-bit is set if the callsign
/// ends with `*` (digipeater has-been-repeated marker).
fn encode_address(call_ssid: &str, is_last: bool) -> [u8; 7] {
    let mut bytes = [0x40u8; 7]; // space << 1
    let (callsign, ssid) = if let Some(dash) = call_ssid.find('-') {
        let clean = &call_ssid[..dash];
        let ssid_part = &call_ssid[dash + 1..];
        // Strip trailing '*' from SSID part for parsing
        let ssid_clean = ssid_part.trim_end_matches('*');
        (clean, ssid_clean.parse::<u8>().unwrap_or(0))
    } else {
        let clean = call_ssid.trim_end_matches('*');
        (clean, 0u8)
    };

    for (i, &b) in callsign.as_bytes().iter().take(6).enumerate() {
        bytes[i] = b << 1;
    }

    let h_bit = if call_ssid.ends_with('*') { 0x80 } else { 0 };
    bytes[6] = 0x60 | ((ssid & 0x0F) << 1) | h_bit;
    if is_last {
        bytes[6] |= 0x01;
    }
    bytes
}

/// Parse a callsign-SSID string like "N0CALL-9" into (callsign, ssid).
pub fn parse_call_ssid(call: &str) -> (&str, u8) {
    if let Some((cs, ssid_str)) = call.rsplit_once('-') {
        if let Ok(ssid) = ssid_str.parse::<u8>() {
            return (cs, ssid);
        }
    }
    (call, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tnc2_basic() {
        let pkt = parse_tnc2_line("N0CALL>APRS,WIDE1-1:!4903.50N/07201.75W-Test").unwrap();
        assert_eq!(pkt.source, "N0CALL");
        assert_eq!(pkt.dest, "APRS");
        assert_eq!(pkt.path, vec!["WIDE1-1"]);
        assert_eq!(pkt.info, b"!4903.50N/07201.75W-Test");
    }

    #[test]
    fn test_parse_tnc2_no_path() {
        let pkt = parse_tnc2_line("N0CALL>APRS:!4903.50N/07201.75W-").unwrap();
        assert_eq!(pkt.source, "N0CALL");
        assert_eq!(pkt.dest, "APRS");
        assert!(pkt.path.is_empty());
    }

    #[test]
    fn test_parse_tnc2_multiple_digipeaters() {
        let pkt =
            parse_tnc2_line("N0CALL>APRS,DIGI1*,DIGI2,WIDE2-1:!4903.50N/07201.75W-").unwrap();
        assert_eq!(pkt.path.len(), 3);
        assert_eq!(pkt.path[0], "DIGI1*");
        assert_eq!(pkt.path[1], "DIGI2");
        assert_eq!(pkt.path[2], "WIDE2-1");
    }

    #[test]
    fn test_parse_tnc2_comment_line() {
        assert!(parse_tnc2_line("# logresp N0CALL verified").is_none());
    }

    #[test]
    fn test_parse_tnc2_empty_line() {
        assert!(parse_tnc2_line("").is_none());
        assert!(parse_tnc2_line("   ").is_none());
    }

    #[test]
    fn test_parse_tnc2_missing_gt() {
        assert!(parse_tnc2_line("N0CALLAPRS:data").is_none());
    }

    #[test]
    fn test_parse_tnc2_missing_colon() {
        assert!(parse_tnc2_line("N0CALL>APRS").is_none());
    }

    #[test]
    fn test_parse_tnc2_empty_info() {
        let pkt = parse_tnc2_line("N0CALL>APRS:").unwrap();
        assert!(pkt.info.is_empty());
    }

    #[test]
    fn test_parse_tnc2_info_with_colons() {
        let pkt = parse_tnc2_line("N0CALL>APRS::W1AW     :Hello{001").unwrap();
        assert_eq!(pkt.info, b":W1AW     :Hello{001");
    }

    #[test]
    fn test_tnc2_to_ax25_roundtrip() {
        let pkt = parse_tnc2_line("N0CALL>APRS,WIDE1-1:!4903.50N/07201.75W-Test").unwrap();
        let ax25 = tnc2_to_ax25(&pkt);

        // Should be parseable by core Frame::parse
        let frame = packet_radio_core::ax25::Frame::parse(&ax25);
        assert!(frame.is_some(), "AX.25 frame should parse");

        let frame = frame.unwrap();
        assert_eq!(frame.src.callsign_str(), b"N0CALL");
        // callsign_str() returns the raw 6-byte field; short calls are space-padded
        let dest_trimmed: Vec<u8> = frame.dest.callsign_str().iter()
            .copied().filter(|&b| b != b' ').collect();
        assert_eq!(dest_trimmed, b"APRS");
        assert_eq!(frame.info, b"!4903.50N/07201.75W-Test");
        assert!(frame.is_ui());
    }

    #[test]
    fn test_tnc2_to_ax25_with_ssid() {
        let pkt = parse_tnc2_line("N0CALL-9>APRS:test").unwrap();
        let ax25 = tnc2_to_ax25(&pkt);
        let frame = packet_radio_core::ax25::Frame::parse(&ax25).unwrap();
        assert_eq!(frame.src.ssid, 9);
    }

    #[test]
    fn test_tnc2_to_ax25_hbit() {
        let pkt = parse_tnc2_line("N0CALL>APRS,DIGI1*,WIDE2-1:test").unwrap();
        let ax25 = tnc2_to_ax25(&pkt);
        let frame = packet_radio_core::ax25::Frame::parse(&ax25).unwrap();
        assert_eq!(frame.num_digipeaters, 2);
        assert!(frame.digipeaters[0].h_bit);
        assert!(!frame.digipeaters[1].h_bit);
    }

    #[test]
    fn test_parse_call_ssid() {
        assert_eq!(parse_call_ssid("N0CALL"), ("N0CALL", 0));
        assert_eq!(parse_call_ssid("N0CALL-9"), ("N0CALL", 9));
        assert_eq!(parse_call_ssid("KD1KE-5"), ("KD1KE", 5));
    }
}
