use packet_radio_core::ax25::Frame;
use packet_radio_core::aprs;

use crate::tui;

/// Parse raw AX.25 bytes into a `DecodedFrameInfo` for the TUI.
pub fn make_frame_info(count: u64, data: &[u8]) -> tui::state::DecodedFrameInfo {
    let timestamp = chrono_lite_timestamp();

    if let Some(frame) = Frame::parse(data) {
        let src_ssid = format_address(&frame.src);
        let dest_ssid = format_address(&frame.dest);
        let via = format_via_path(&frame);

        let info_str = core::str::from_utf8(frame.info).unwrap_or("<binary>").to_string();

        let parsed = aprs::parse_packet(frame.info, frame.dest.callsign_str());

        let aprs_summary = parsed.as_ref().map(|pkt| match pkt {
            aprs::AprsPacket::Position { position, .. } => {
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                format!("Position: {lat:.4}, {lon:.4}")
            }
            aprs::AprsPacket::MicE { position, speed, course, .. } => {
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                format!("Mic-E: {lat:.4}, {lon:.4} {speed}kts {course}°")
            }
            aprs::AprsPacket::Message { addressee, text, .. } => {
                let to = core::str::from_utf8(addressee).unwrap_or("?");
                let msg = core::str::from_utf8(text).unwrap_or("?");
                format!("Msg to {to}: {msg}")
            }
            aprs::AprsPacket::Weather { weather, .. } => {
                let temp = weather.temperature.map(|t| format!("{t}F")).unwrap_or_default();
                let wind = weather.wind_speed.map(|s| format!("{s}mph")).unwrap_or_default();
                format!("Weather: {temp} {wind}")
            }
            aprs::AprsPacket::Object { name, live, position, .. } => {
                let n = core::str::from_utf8(name).unwrap_or("?");
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                let status = if *live { "live" } else { "killed" };
                format!("Object {n} ({status}): {lat:.4}, {lon:.4}")
            }
            aprs::AprsPacket::Item { name, live, position, .. } => {
                let n = core::str::from_utf8(name).unwrap_or("?");
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                let status = if *live { "live" } else { "killed" };
                format!("Item {n} ({status}): {lat:.4}, {lon:.4}")
            }
            aprs::AprsPacket::Status { text, .. } => {
                let s = core::str::from_utf8(text).unwrap_or("?");
                format!("Status: {s}")
            }
            aprs::AprsPacket::Telemetry { sequence, .. } => {
                format!("Telemetry #{sequence}")
            }
            aprs::AprsPacket::ThirdParty { .. } => "Third-party".to_string(),
            aprs::AprsPacket::RawGps { parsed, .. } => {
                if let Some(ref nmea) = parsed {
                    if let Some(ref pos) = nmea.position {
                        let lat = pos.lat as f64 / 1_000_000.0;
                        let lon = pos.lon as f64 / 1_000_000.0;
                        format!("GPS: {lat:.4}, {lon:.4}")
                    } else {
                        "Raw GPS (no fix)".to_string()
                    }
                } else {
                    "Raw GPS".to_string()
                }
            }
            aprs::AprsPacket::Capabilities { .. } => "Capabilities".to_string(),
            aprs::AprsPacket::Query { query_type, .. } => {
                let q = core::str::from_utf8(query_type).unwrap_or("?");
                format!("Query: {q}")
            }
            aprs::AprsPacket::UserDefined { .. } => "User-defined".to_string(),
            aprs::AprsPacket::Unknown { .. } => "APRS".to_string(),
        });

        let aprs_data = parsed.map(|pkt| aprs_packet_to_data(&pkt));

        tui::state::DecodedFrameInfo {
            frame_number: count,
            timestamp,
            source: src_ssid,
            dest: dest_ssid,
            via,
            info: info_str,
            aprs_summary,
            aprs_data,
            raw_len: data.len(),
        }
    } else {
        tui::state::DecodedFrameInfo {
            frame_number: count,
            timestamp,
            source: "<raw>".to_string(),
            dest: String::new(),
            via: String::new(),
            info: hex_preview(data, 32),
            aprs_summary: None,
            aprs_data: None,
            raw_len: data.len(),
        }
    }
}

/// Format an APRS timestamp for display.
fn format_timestamp(ts: &aprs::Timestamp) -> String {
    match ts {
        aprs::Timestamp::Dhm { day, hour, minute } => format!("{day:02}{hour:02}{minute:02}z"),
        aprs::Timestamp::Hms { hour, minute, second } => format!("{hour:02}{minute:02}{second:02}h"),
        aprs::Timestamp::DhmLocal { day, hour, minute } => format!("{day:02}{hour:02}{minute:02}/"),
    }
}

/// Format compressed extra data for display.
fn format_compressed_extra(extra: &aprs::CompressedExtra) -> String {
    let mut parts = Vec::new();
    if let Some((cse, spd)) = extra.course_speed {
        parts.push(format!("{cse}°/{spd}kts"));
    }
    if let Some(alt) = extra.altitude {
        parts.push(format!("{alt}ft"));
    }
    if let Some(rng) = extra.range {
        parts.push(format!("{rng}mi"));
    }
    parts.join(" ")
}

/// Format a MessageType for display.
fn format_message_type(mt: &aprs::MessageType) -> &'static str {
    match mt {
        aprs::MessageType::Private => "Private",
        aprs::MessageType::Ack => "Ack",
        aprs::MessageType::Rej => "Rej",
        aprs::MessageType::Bulletin => "Bulletin",
        aprs::MessageType::Announcement => "Announcement",
        aprs::MessageType::Nws => "NWS",
    }
}

/// Convert a parsed core AprsPacket to an owned AprsData for the TUI.
pub fn aprs_packet_to_data(pkt: &aprs::AprsPacket) -> tui::state::AprsData {
    use tui::state::{AprsData, WeatherInfo};

    match pkt {
        aprs::AprsPacket::Position { position, symbol_table, symbol_code, comment, timestamp, compressed_extra } => {
            let lat = position.lat as f64 / 1_000_000.0;
            let lon = position.lon as f64 / 1_000_000.0;
            let comment_str = core::str::from_utf8(comment).unwrap_or("").to_string();
            let weather = aprs::parse_weather_from_comment(comment)
                .map(|w| WeatherInfo::from_core(&w));
            let comment_fields = aprs::parse_comment_fields(comment);
            AprsData::Position {
                lat,
                lon,
                symbol: (*symbol_table, *symbol_code),
                comment: comment_str,
                weather,
                timestamp: timestamp.as_ref().map(format_timestamp),
                altitude: comment_fields.altitude,
                compressed_extra: compressed_extra.as_ref().map(format_compressed_extra),
            }
        }
        aprs::AprsPacket::MicE { position, speed, course, symbol_table, symbol_code } => {
            AprsData::MicE {
                lat: position.lat as f64 / 1_000_000.0,
                lon: position.lon as f64 / 1_000_000.0,
                speed: *speed,
                course: *course,
                symbol: (*symbol_table, *symbol_code),
            }
        }
        aprs::AprsPacket::Message { addressee, text, message_no, message_type } => {
            AprsData::Message {
                addressee: core::str::from_utf8(addressee).unwrap_or("?").to_string(),
                text: core::str::from_utf8(text).unwrap_or("?").to_string(),
                message_no: message_no.map(|m| core::str::from_utf8(m).unwrap_or("").to_string()),
                message_type: format_message_type(message_type).to_string(),
            }
        }
        aprs::AprsPacket::Weather { weather, comment } => {
            AprsData::Weather {
                weather: WeatherInfo::from_core(weather),
                comment: core::str::from_utf8(comment).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Object { name, live, position, symbol_table, symbol_code, comment, timestamp } => {
            AprsData::Object {
                name: core::str::from_utf8(name).unwrap_or("?").to_string(),
                live: *live,
                lat: position.lat as f64 / 1_000_000.0,
                lon: position.lon as f64 / 1_000_000.0,
                symbol: (*symbol_table, *symbol_code),
                comment: core::str::from_utf8(comment).unwrap_or("").to_string(),
                timestamp: timestamp.as_ref().map(format_timestamp),
            }
        }
        aprs::AprsPacket::Item { name, live, position, symbol_table, symbol_code, comment } => {
            AprsData::Item {
                name: core::str::from_utf8(name).unwrap_or("?").to_string(),
                live: *live,
                lat: position.lat as f64 / 1_000_000.0,
                lon: position.lon as f64 / 1_000_000.0,
                symbol: (*symbol_table, *symbol_code),
                comment: core::str::from_utf8(comment).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Status { text, timestamp, maidenhead } => {
            AprsData::Status {
                text: core::str::from_utf8(text).unwrap_or("").to_string(),
                timestamp: timestamp.as_ref().map(format_timestamp),
                maidenhead: maidenhead.map(|m| core::str::from_utf8(m).unwrap_or("").to_string()),
            }
        }
        aprs::AprsPacket::Telemetry { sequence, analog, digital } => {
            AprsData::Telemetry {
                sequence: *sequence,
                analog: *analog,
                digital: *digital,
            }
        }
        aprs::AprsPacket::ThirdParty { data } => {
            AprsData::ThirdParty {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::RawGps { data, parsed } => {
            let (position, speed, course, altitude, satellites, fix_valid) =
                if let Some(ref nmea) = parsed {
                    let pos = nmea.position.as_ref().map(|p| {
                        (p.lat as f64 / 1_000_000.0, p.lon as f64 / 1_000_000.0)
                    });
                    let spd = nmea.speed_tenths_kts.map(|v| v as f64 / 10.0);
                    let crs = nmea.course_tenths_deg.map(|v| v as f64 / 10.0);
                    let alt = nmea.altitude_dm.map(|v| v as f64 / 10.0);
                    (pos, spd, crs, alt, nmea.satellites, nmea.fix_valid)
                } else {
                    (None, None, None, None, None, false)
                };
            AprsData::RawGps {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
                position,
                speed,
                course,
                altitude,
                satellites,
                fix_valid,
            }
        }
        aprs::AprsPacket::Capabilities { data } => {
            AprsData::Capabilities {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Query { query_type } => {
            AprsData::Query {
                query_type: core::str::from_utf8(query_type).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::UserDefined { data } => {
            AprsData::UserDefined {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Unknown { dti, .. } => {
            AprsData::Unknown { dti: *dti }
        }
    }
}

/// Format an AX.25 address as "CALL" or "CALL-SSID".
pub fn format_address(addr: &packet_radio_core::ax25::Address) -> String {
    let call = core::str::from_utf8(addr.callsign_str()).unwrap_or("?");
    if addr.ssid > 0 {
        format!("{call}-{}", addr.ssid)
    } else {
        call.to_string()
    }
}

/// Build a comma-separated digipeater path string from a parsed frame.
/// Each digipeater is formatted as "CALL[-SSID][*]".
pub fn format_via_path(frame: &Frame) -> String {
    let mut via = String::new();
    for i in 0..frame.num_digipeaters as usize {
        if !via.is_empty() { via.push(','); }
        let digi = &frame.digipeaters[i];
        if let Ok(call) = core::str::from_utf8(digi.callsign_str()) {
            via.push_str(call);
        }
        if digi.ssid > 0 {
            via.push('-');
            via.push_str(&digi.ssid.to_string());
        }
        if digi.h_bit {
            via.push('*');
        }
    }
    via
}

/// Format and print a decoded frame to the console.
pub fn print_frame(count: u64, data: &[u8]) {
    let now = chrono_lite_timestamp();

    if let Some(frame) = Frame::parse(data) {
        let src_ssid = format_address(&frame.src);
        let dest_ssid = format_address(&frame.dest);
        let via = format_via_path(&frame);
        let via_prefix = if via.is_empty() { String::new() } else { format!(",{via}") };

        let info = core::str::from_utf8(frame.info).unwrap_or("<binary>");

        println!("[{now}] #{count} {src_ssid}>{dest_ssid}{via_prefix}: {info}");

        // Try APRS parse for extra detail at debug level
        if let Some(pkt) = aprs::parse_packet(frame.info, frame.dest.callsign_str()) {
            match pkt {
                aprs::AprsPacket::Position { position, .. } => {
                    let lat = position.lat as f64 / 1_000_000.0;
                    let lon = position.lon as f64 / 1_000_000.0;
                    tracing::debug!("  APRS position: {lat:.4}, {lon:.4}");
                }
                aprs::AprsPacket::MicE { position, speed, course, .. } => {
                    let lat = position.lat as f64 / 1_000_000.0;
                    let lon = position.lon as f64 / 1_000_000.0;
                    tracing::debug!(
                        "  Mic-E: {lat:.4}, {lon:.4} speed={speed}kts course={course}°"
                    );
                }
                aprs::AprsPacket::Message { addressee, text, .. } => {
                    let to = core::str::from_utf8(addressee).unwrap_or("?");
                    let msg = core::str::from_utf8(text).unwrap_or("?");
                    tracing::debug!("  Message to {to}: {msg}");
                }
                _ => {}
            }
        }
    } else {
        // Couldn't parse AX.25 — show raw hex
        println!("[{now}] #{count} <raw {len} bytes: {hex}>",
            len = data.len(),
            hex = hex_preview(data, 32),
        );
    }
}

/// Simple timestamp without pulling in chrono.
pub fn chrono_lite_timestamp() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs();
            let hours = (secs / 3600) % 24;
            let mins = (secs / 60) % 60;
            let s = secs % 60;
            format!("{hours:02}:{mins:02}:{s:02}")
        }
        Err(_) => "??:??:??".to_string(),
    }
}

/// Hex preview of bytes (truncated to max_bytes).
pub fn hex_preview(data: &[u8], max_bytes: usize) -> String {
    let show = data.len().min(max_bytes);
    let mut s = String::with_capacity(show * 3);
    for (i, &b) in data[..show].iter().enumerate() {
        if i > 0 { s.push(' '); }
        s.push_str(&format!("{b:02X}"));
    }
    if data.len() > max_bytes {
        s.push_str("...");
    }
    s
}
