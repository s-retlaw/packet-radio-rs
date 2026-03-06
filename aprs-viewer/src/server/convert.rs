use crate::models::{WebAprsData, WebWeather};
use packet_radio_core::aprs;

/// Convert fixed-point microdegrees (i32) to f64 degrees.
pub fn fixed_to_f64(val: i32) -> f64 {
    val as f64 / 1_000_000.0
}

/// Convert a byte slice to a lossy UTF-8 String.
pub fn bytes_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Convert core WeatherData to web-compatible WebWeather.
pub fn to_web_weather(w: &aprs::WeatherData) -> WebWeather {
    WebWeather {
        wind_direction: w.wind_direction,
        wind_speed: w.wind_speed,
        wind_gust: w.wind_gust,
        temperature: w.temperature,
        rain_last_hour: w.rain_last_hour,
        rain_24h: w.rain_24h,
        rain_since_midnight: w.rain_since_midnight,
        humidity: w.humidity,
        barometric_pressure: w.barometric_pressure,
        luminosity: w.luminosity,
        snowfall: w.snowfall,
    }
}

/// Convert a borrowed core AprsPacket to an owned WebAprsData.
pub fn to_web_packet(pkt: &aprs::AprsPacket<'_>) -> WebAprsData {
    match pkt {
        aprs::AprsPacket::Position {
            position,
            symbol_table,
            symbol_code,
            comment,
            ..
        } => {
            let weather = aprs::parse_weather_from_comment(comment).map(|w| to_web_weather(&w));
            let comment_str = if comment.is_empty() {
                None
            } else {
                Some(bytes_to_string(comment))
            };

            WebAprsData::Position {
                lat: fixed_to_f64(position.lat),
                lon: fixed_to_f64(position.lon),
                symbol_table: String::from(*symbol_table as char),
                symbol_code: String::from(*symbol_code as char),
                comment: comment_str,
                weather,
            }
        }
        aprs::AprsPacket::MicE {
            position,
            speed,
            course,
            symbol_table,
            symbol_code,
        } => WebAprsData::MicE {
            lat: fixed_to_f64(position.lat),
            lon: fixed_to_f64(position.lon),
            speed: *speed as f64,
            course: *course as f64,
            symbol_table: String::from(*symbol_table as char),
            symbol_code: String::from(*symbol_code as char),
        },
        aprs::AprsPacket::Message {
            addressee,
            text,
            message_no,
            ..
        } => WebAprsData::Message {
            addressee: bytes_to_string(addressee),
            text: bytes_to_string(text),
            message_no: message_no.map(bytes_to_string),
        },
        aprs::AprsPacket::Weather { weather, comment, .. } => WebAprsData::Weather {
            weather: to_web_weather(weather),
            comment: if comment.is_empty() {
                None
            } else {
                Some(bytes_to_string(comment))
            },
        },
        aprs::AprsPacket::Object {
            name,
            live,
            position,
            symbol_table,
            symbol_code,
            comment,
            ..
        } => WebAprsData::Object {
            name: bytes_to_string(name),
            live: *live,
            lat: fixed_to_f64(position.lat),
            lon: fixed_to_f64(position.lon),
            symbol_table: String::from(*symbol_table as char),
            symbol_code: String::from(*symbol_code as char),
            comment: if comment.is_empty() {
                None
            } else {
                Some(bytes_to_string(comment))
            },
        },
        aprs::AprsPacket::Item {
            name,
            live,
            position,
            symbol_table,
            symbol_code,
            comment,
            ..
        } => WebAprsData::Item {
            name: bytes_to_string(name),
            live: *live,
            lat: fixed_to_f64(position.lat),
            lon: fixed_to_f64(position.lon),
            symbol_table: String::from(*symbol_table as char),
            symbol_code: String::from(*symbol_code as char),
            comment: if comment.is_empty() {
                None
            } else {
                Some(bytes_to_string(comment))
            },
        },
        aprs::AprsPacket::Status { text, .. } => WebAprsData::Status {
            text: bytes_to_string(text),
        },
        aprs::AprsPacket::Unknown { dti, .. } => WebAprsData::Unknown { dti: *dti },
        // Telemetry, ThirdParty, RawGps, etc. — map to Unknown
        other => WebAprsData::Unknown {
            dti: match other {
                aprs::AprsPacket::Unknown { dti, .. } => *dti,
                _ => b'?',
            },
        },
    }
}

/// Get the packet type name for a WebAprsData variant.
pub fn packet_type_name(data: &WebAprsData) -> &'static str {
    match data {
        WebAprsData::Position { weather: Some(_), .. } => "Weather",
        WebAprsData::Position { .. } => "Position",
        WebAprsData::MicE { .. } => "MicE",
        WebAprsData::Message { .. } => "Message",
        WebAprsData::Weather { .. } => "Weather",
        WebAprsData::Object { .. } => "Object",
        WebAprsData::Item { .. } => "Item",
        WebAprsData::Status { .. } => "Status",
        WebAprsData::Unknown { .. } => "Unknown",
    }
}

/// Extract lat/lon from a WebAprsData if it has position.
pub fn extract_position(data: &WebAprsData) -> Option<(f64, f64)> {
    match data {
        WebAprsData::Position { lat, lon, .. }
        | WebAprsData::MicE { lat, lon, .. }
        | WebAprsData::Object { lat, lon, .. }
        | WebAprsData::Item { lat, lon, .. } => Some((*lat, *lon)),
        _ => None,
    }
}

/// Extract speed/course from a WebAprsData if available.
pub fn extract_speed_course(data: &WebAprsData) -> (Option<f64>, Option<f64>) {
    match data {
        WebAprsData::MicE { speed, course, .. } => (Some(*speed), Some(*course)),
        _ => (None, None),
    }
}

/// Extract symbol info from a WebAprsData.
pub fn extract_symbol(data: &WebAprsData) -> Option<(&str, &str)> {
    match data {
        WebAprsData::Position {
            symbol_table,
            symbol_code,
            ..
        }
        | WebAprsData::MicE {
            symbol_table,
            symbol_code,
            ..
        }
        | WebAprsData::Object {
            symbol_table,
            symbol_code,
            ..
        }
        | WebAprsData::Item {
            symbol_table,
            symbol_code,
            ..
        } => Some((symbol_table.as_str(), symbol_code.as_str())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use packet_radio_core::aprs::Position;

    #[test]
    fn test_fixed_to_f64() {
        assert!((fixed_to_f64(49_058_333) - 49.058333).abs() < 0.000001);
        assert!((fixed_to_f64(-72_029_583) - (-72.029583)).abs() < 0.000001);
        assert!((fixed_to_f64(0) - 0.0).abs() < 0.000001);
    }

    #[test]
    fn test_bytes_to_string() {
        assert_eq!(bytes_to_string(b"Hello"), "Hello");
        assert_eq!(bytes_to_string(b""), "");
        // Non-UTF8 should use replacement character
        assert_eq!(bytes_to_string(&[0xFF, 0xFE]), "\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn test_convert_position() {
        let pkt = aprs::AprsPacket::Position {
            position: Position {
                lat: 49_058_333,
                lon: -72_029_583,
                ambiguity: 0,
            },
            symbol_table: b'/',
            symbol_code: b'>',
            comment: b"Test station",
            timestamp: None,
            compressed_extra: None,
        };
        let web = to_web_packet(&pkt);
        match web {
            WebAprsData::Position {
                lat,
                lon,
                comment,
                symbol_table,
                ..
            } => {
                assert!((lat - 49.058333).abs() < 0.001);
                assert!((lon - (-72.029583)).abs() < 0.001);
                assert_eq!(comment.as_deref(), Some("Test station"));
                assert_eq!(symbol_table, "/");
            }
            _ => panic!("Expected Position"),
        }
    }

    #[test]
    fn test_convert_position_empty_comment() {
        let pkt = aprs::AprsPacket::Position {
            position: Position {
                lat: 49_000_000,
                lon: -72_000_000,
                ambiguity: 0,
            },
            symbol_table: b'/',
            symbol_code: b'>',
            comment: b"",
            timestamp: None,
            compressed_extra: None,
        };
        let web = to_web_packet(&pkt);
        match web {
            WebAprsData::Position { comment, .. } => {
                assert_eq!(comment, None);
            }
            _ => panic!("Expected Position"),
        }
    }

    #[test]
    fn test_convert_mic_e() {
        let pkt = aprs::AprsPacket::MicE {
            position: Position {
                lat: 33_946_000,
                lon: -118_408_000,
                ambiguity: 0,
            },
            speed: 65,
            course: 252,
            symbol_table: b'/',
            symbol_code: b'>',
        };
        let web = to_web_packet(&pkt);
        match web {
            WebAprsData::MicE {
                lat,
                lon,
                speed,
                course,
                ..
            } => {
                assert!((lat - 33.946).abs() < 0.001);
                assert!((lon - (-118.408)).abs() < 0.001);
                assert!((speed - 65.0).abs() < 0.01);
                assert!((course - 252.0).abs() < 0.01);
            }
            _ => panic!("Expected MicE"),
        }
    }

    #[test]
    fn test_convert_message() {
        let pkt = aprs::AprsPacket::Message {
            addressee: b"N0CALL   ",
            text: b"Hello!",
            message_no: Some(b"123"),
            message_type: aprs::MessageType::Private,
        };
        let web = to_web_packet(&pkt);
        match web {
            WebAprsData::Message {
                addressee,
                text,
                message_no,
            } => {
                assert_eq!(addressee, "N0CALL   ");
                assert_eq!(text, "Hello!");
                assert_eq!(message_no.as_deref(), Some("123"));
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_convert_weather() {
        let wd = aprs::WeatherData {
            wind_direction: Some(180),
            wind_speed: Some(10),
            wind_gust: None,
            temperature: Some(72),
            rain_last_hour: None,
            rain_24h: None,
            rain_since_midnight: None,
            humidity: Some(65),
            barometric_pressure: Some(10132),
            luminosity: None,
            snowfall: None,
        };
        let pkt = aprs::AprsPacket::Weather {
            weather: wd,
            comment: b"WX Station",
        };
        let web = to_web_packet(&pkt);
        match web {
            WebAprsData::Weather { weather, comment } => {
                assert_eq!(weather.wind_direction, Some(180));
                assert_eq!(weather.temperature, Some(72));
                assert_eq!(comment.as_deref(), Some("WX Station"));
            }
            _ => panic!("Expected Weather"),
        }
    }

    #[test]
    fn test_convert_object() {
        let pkt = aprs::AprsPacket::Object {
            name: b"FIRE     ",
            live: true,
            position: Position {
                lat: 34_000_000,
                lon: -117_000_000,
                ambiguity: 0,
            },
            symbol_table: b'/',
            symbol_code: b'f',
            comment: b"Active fire",
            timestamp: None,
        };
        let web = to_web_packet(&pkt);
        match web {
            WebAprsData::Object {
                name, live, lat, ..
            } => {
                assert_eq!(name, "FIRE     ");
                assert!(live);
                assert!((lat - 34.0).abs() < 0.001);
            }
            _ => panic!("Expected Object"),
        }
    }

    #[test]
    fn test_convert_status() {
        let pkt = aprs::AprsPacket::Status {
            text: b"On the air",
            timestamp: None,
            maidenhead: None,
        };
        let web = to_web_packet(&pkt);
        match web {
            WebAprsData::Status { text } => {
                assert_eq!(text, "On the air");
            }
            _ => panic!("Expected Status"),
        }
    }

    #[test]
    fn test_convert_position_with_weather() {
        let pkt = aprs::AprsPacket::Position {
            position: Position {
                lat: 49_058_333,
                lon: -72_029_583,
                ambiguity: 0,
            },
            symbol_table: b'/',
            symbol_code: b'_',
            comment: b"220/004g005t077r000p000P000h50b09900",
            timestamp: None,
            compressed_extra: None,
        };
        let web = to_web_packet(&pkt);
        match &web {
            WebAprsData::Position { weather, .. } => {
                let wx = weather.as_ref().expect("weather should be extracted");
                assert_eq!(wx.wind_direction, Some(220));
                assert_eq!(wx.temperature, Some(77));
            }
            _ => panic!("Expected Position"),
        }
        // Should classify as Weather, not Position
        assert_eq!(packet_type_name(&web), "Weather");
    }

    #[test]
    fn test_packet_type_name() {
        let pos = WebAprsData::Position {
            lat: 0.0,
            lon: 0.0,
            symbol_table: "/".into(),
            symbol_code: ">".into(),
            comment: None,
            weather: None,
        };
        assert_eq!(packet_type_name(&pos), "Position");
    }

    #[test]
    fn test_extract_position() {
        let data = WebAprsData::MicE {
            lat: 33.0,
            lon: -118.0,
            speed: 0.0,
            course: 0.0,
            symbol_table: "/".into(),
            symbol_code: ">".into(),
        };
        assert_eq!(extract_position(&data), Some((33.0, -118.0)));

        let msg = WebAprsData::Message {
            addressee: "N0CALL".into(),
            text: "Hi".into(),
            message_no: None,
        };
        assert_eq!(extract_position(&msg), None);
    }
}
