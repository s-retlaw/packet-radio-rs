use serde::{Deserialize, Serialize};

/// A station row as stored in SQLite and sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StationRow {
    pub callsign: String,
    pub ssid: u8,
    pub station_type: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub speed: Option<f64>,
    pub course: Option<f64>,
    pub altitude: Option<f64>,
    pub comment: Option<String>,
    pub symbol_table: Option<String>,
    pub symbol_code: Option<String>,
    pub last_heard: String,
    pub packet_count: i64,
    pub weather: Option<WebWeather>,
    /// Comma-separated set of source types: "tnc", "aprs-is", or "tnc,aprs-is".
    #[serde(default)]
    pub heard_via: String,
    /// The source type of the most recent packet.
    #[serde(default)]
    pub last_source_type: String,
    /// Whether the station has moved (2+ distinct positions in history).
    #[serde(default)]
    pub has_moved: bool,
}

/// A packet row as stored in SQLite and sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PacketRow {
    pub id: i64,
    pub source: String,
    pub source_ssid: u8,
    pub dest: String,
    pub path: Option<String>,
    pub packet_type: Option<String>,
    pub raw_info: String,
    pub summary: Option<String>,
    pub received_at: String,
    /// How this packet was received: "tnc" or "aprs-is".
    #[serde(default)]
    pub source_type: String,
}

/// A position history point for drawing tracks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackPoint {
    pub lat: f64,
    pub lon: f64,
    pub altitude: Option<f64>,
    pub speed: Option<f64>,
    pub course: Option<f64>,
    pub recorded_at: String,
}

/// A message row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageRow {
    pub id: i64,
    pub from_call: String,
    pub to_call: String,
    pub message_text: String,
    pub message_no: Option<String>,
    pub acked: bool,
    pub received_at: String,
}

/// Weather data — owned version suitable for serialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebWeather {
    pub wind_direction: Option<u16>,
    pub wind_speed: Option<u16>,
    pub wind_gust: Option<u16>,
    pub temperature: Option<i16>,
    pub rain_last_hour: Option<u16>,
    pub rain_24h: Option<u16>,
    pub rain_since_midnight: Option<u16>,
    pub humidity: Option<u8>,
    pub barometric_pressure: Option<u32>,
    pub luminosity: Option<u16>,
    pub snowfall: Option<u16>,
}

/// A weather history data point for time-series charts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeatherHistoryPoint {
    pub temperature: Option<i16>,
    pub wind_speed: Option<u16>,
    pub wind_direction: Option<u16>,
    pub wind_gust: Option<u16>,
    pub humidity: Option<u8>,
    pub barometric_pressure: Option<u32>,
    pub rain_last_hour: Option<u16>,
    pub rain_24h: Option<u16>,
    pub luminosity: Option<u16>,
    pub recorded_at: String,
}

/// APRS packet data — owned version of core's AprsPacket for web transport.
/// Matches the actual variants in packet_radio_core::aprs::AprsPacket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum WebAprsData {
    Position {
        lat: f64,
        lon: f64,
        symbol_table: String,
        symbol_code: String,
        comment: Option<String>,
        weather: Option<WebWeather>,
    },
    MicE {
        lat: f64,
        lon: f64,
        speed: f64,
        course: f64,
        symbol_table: String,
        symbol_code: String,
    },
    Message {
        addressee: String,
        text: String,
        message_no: Option<String>,
    },
    Weather {
        weather: WebWeather,
        comment: Option<String>,
    },
    Object {
        name: String,
        live: bool,
        lat: f64,
        lon: f64,
        symbol_table: String,
        symbol_code: String,
        comment: Option<String>,
    },
    Item {
        name: String,
        live: bool,
        lat: f64,
        lon: f64,
        symbol_table: String,
        symbol_code: String,
        comment: Option<String>,
    },
    Status {
        text: String,
    },
    Unknown {
        dti: u8,
    },
}

/// Filter criteria for station queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StationFilter {
    pub callsign: Option<String>,
    pub station_type: Option<String>,
    pub with_position: bool,
}

/// WebSocket event types sent to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsEvent {
    Init { packets: Vec<PacketRow> },
    Packet(PacketRow),
    StationUpdate(StationRow),
}

/// Map pack metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapPack {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub size_bytes: u64,
    pub installed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_station_row_serde_roundtrip() {
        let station = StationRow {
            callsign: "N0CALL".into(),
            ssid: 0,
            station_type: "Position".into(),
            lat: Some(49.058333),
            lon: Some(-72.029583),
            speed: Some(45.0),
            course: Some(180.0),
            altitude: Some(100.0),
            comment: Some("Test station".into()),
            symbol_table: Some("/".into()),
            symbol_code: Some(">".into()),
            last_heard: "2026-03-01T12:00:00Z".into(),
            packet_count: 5,
            weather: None,
            heard_via: "tnc".into(),
            last_source_type: "tnc".into(),
            has_moved: false,
        };
        let json = serde_json::to_string(&station).unwrap();
        let back: StationRow = serde_json::from_str(&json).unwrap();
        assert_eq!(station, back);
    }

    #[test]
    fn test_packet_row_serde_roundtrip() {
        let packet = PacketRow {
            id: 1,
            source: "N0CALL".into(),
            source_ssid: 0,
            dest: "APRS".into(),
            path: Some("WIDE1-1".into()),
            packet_type: Some("Position".into()),
            raw_info: "!4903.50N/07201.75W-Test".into(),
            summary: Some("Position: 49.058N, 72.030W".into()),
            received_at: "2026-03-01T12:00:00Z".into(),
            source_type: "tnc".into(),
        };
        let json = serde_json::to_string(&packet).unwrap();
        let back: PacketRow = serde_json::from_str(&json).unwrap();
        assert_eq!(packet, back);
    }

    #[test]
    fn test_web_aprs_data_position_serde() {
        let data = WebAprsData::Position {
            lat: 49.058333,
            lon: -72.029583,
            symbol_table: "/".into(),
            symbol_code: ">".into(),
            comment: Some("Mobile".into()),
            weather: None,
        };
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"type\":\"Position\""));
        let back: WebAprsData = serde_json::from_str(&json).unwrap();
        assert_eq!(data, back);
    }

    #[test]
    fn test_web_aprs_data_mic_e_serde() {
        let data = WebAprsData::MicE {
            lat: 33.946,
            lon: -118.408,
            speed: 65.0,
            course: 252.0,
            symbol_table: "/".into(),
            symbol_code: ">".into(),
        };
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"type\":\"MicE\""));
        let back: WebAprsData = serde_json::from_str(&json).unwrap();
        assert_eq!(data, back);
    }

    #[test]
    fn test_web_aprs_data_message_serde() {
        let data = WebAprsData::Message {
            addressee: "N0CALL".into(),
            text: "Hello!".into(),
            message_no: Some("123".into()),
        };
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"type\":\"Message\""));
        let back: WebAprsData = serde_json::from_str(&json).unwrap();
        assert_eq!(data, back);
    }

    #[test]
    fn test_web_weather_serde() {
        let wx = WebWeather {
            wind_direction: Some(180),
            wind_speed: Some(10),
            wind_gust: Some(15),
            temperature: Some(72),
            rain_last_hour: Some(0),
            rain_24h: Some(50),
            rain_since_midnight: Some(25),
            humidity: Some(65),
            barometric_pressure: Some(10132),
            luminosity: Some(500),
            snowfall: None,
        };
        let json = serde_json::to_string(&wx).unwrap();
        let back: WebWeather = serde_json::from_str(&json).unwrap();
        assert_eq!(wx, back);
    }

    #[test]
    fn test_ws_event_init_serde() {
        let event = WsEvent::Init {
            packets: vec![PacketRow {
                id: 1,
                source: "N0CALL".into(),
                source_ssid: 0,
                dest: "APRS".into(),
                path: None,
                packet_type: Some("Position".into()),
                raw_info: "!4903.50N/07201.75W-".into(),
                summary: None,
                received_at: "2026-03-01T12:00:00Z".into(),
                source_type: "tnc".into(),
            }],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"Init\""));
        let back: WsEvent = serde_json::from_str(&json).unwrap();
        match back {
            WsEvent::Init { packets } => assert_eq!(packets.len(), 1),
            _ => panic!("Expected Init"),
        }
    }

    #[test]
    fn test_station_row_null_position() {
        let station = StationRow {
            callsign: "N0CALL".into(),
            ssid: 9,
            station_type: "Message".into(),
            lat: None,
            lon: None,
            speed: None,
            course: None,
            altitude: None,
            comment: None,
            symbol_table: None,
            symbol_code: None,
            last_heard: "2026-03-01T12:00:00Z".into(),
            packet_count: 1,
            weather: None,
            heard_via: String::new(),
            last_source_type: "unknown".into(),
            has_moved: false,
        };
        let json = serde_json::to_string(&station).unwrap();
        let back: StationRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lat, None);
        assert_eq!(back.lon, None);
    }

    #[test]
    fn test_station_with_weather() {
        let station = StationRow {
            callsign: "WX0STA".into(),
            ssid: 0,
            station_type: "Weather".into(),
            lat: Some(40.0),
            lon: Some(-105.0),
            speed: None,
            course: None,
            altitude: None,
            comment: None,
            symbol_table: Some("/".into()),
            symbol_code: Some("_".into()),
            last_heard: "2026-03-01T12:00:00Z".into(),
            packet_count: 10,
            weather: Some(WebWeather {
                wind_direction: Some(270),
                wind_speed: Some(5),
                wind_gust: None,
                temperature: Some(55),
                rain_last_hour: None,
                rain_24h: None,
                rain_since_midnight: None,
                humidity: Some(80),
                barometric_pressure: Some(10200),
                luminosity: None,
                snowfall: None,
            }),
            heard_via: "aprs-is".into(),
            last_source_type: "aprs-is".into(),
            has_moved: false,
        };
        let json = serde_json::to_string(&station).unwrap();
        let back: StationRow = serde_json::from_str(&json).unwrap();
        assert!(back.weather.is_some());
        assert_eq!(back.weather.unwrap().temperature, Some(55));
    }

    #[test]
    fn test_weather_history_point_serde() {
        let pt = WeatherHistoryPoint {
            temperature: Some(72),
            wind_speed: Some(10),
            wind_direction: Some(180),
            wind_gust: Some(15),
            humidity: Some(65),
            barometric_pressure: Some(10132),
            rain_last_hour: Some(0),
            rain_24h: Some(50),
            luminosity: Some(500),
            recorded_at: "2026-03-01 12:00:00".into(),
        };
        let json = serde_json::to_string(&pt).unwrap();
        let back: WeatherHistoryPoint = serde_json::from_str(&json).unwrap();
        assert_eq!(pt, back);
    }
}
