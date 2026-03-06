use packet_radio_core::aprs;
use packet_radio_core::ax25::Frame;
use packet_radio_core::kiss::KissDecoder;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::broadcast;

use super::convert::{
    bytes_to_string, extract_position, extract_speed_course, extract_symbol, packet_type_name,
    to_web_packet,
};
use super::db::{self, PacketInsert, StationUpsert};
use super::error::ServerError;
use crate::models::{PacketRow, WebAprsData};

/// Check whether a lat/lon pair is plausible for APRS.
/// Rejects (0,0) artifacts and out-of-range coordinates.
fn is_valid_position(lat: f64, lon: f64) -> bool {
    // Reject near (0,0) — common APRS parser artifact / placeholder
    if lat.abs() < 0.1 && lon.abs() < 0.1 {
        return false;
    }
    // Reject out-of-range
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return false;
    }
    true
}

/// Process a complete AX.25 frame: parse, store in DB, broadcast.
/// Returns Ok(true) if a packet was successfully processed.
///
/// `source_type` is "tnc" or "aprs-is" to indicate how the frame was received.
///
/// If `reference_db` is provided, positionless stations (especially weather-only
/// packets from CWOP) will have their positions enriched from the reference database.
pub async fn process_raw_frame(
    raw_ax25: &[u8],
    pool: &SqlitePool,
    tx: &broadcast::Sender<String>,
    reference_db: Option<&reference::ReferenceDb>,
    source_type: &str,
) -> Result<bool, ServerError> {
    let frame = match Frame::parse(raw_ax25) {
        Some(f) => f,
        None => return Ok(false),
    };

    // Only process UI frames (APRS uses UI)
    if !frame.is_ui() {
        return Ok(false);
    }

    let source = bytes_to_string(frame.src.callsign_str());
    let source_ssid = frame.src.ssid;
    let dest = bytes_to_string(frame.dest.callsign_str());

    // Build path string
    let path = if frame.num_digipeaters > 0 {
        let parts: Vec<String> = (0..frame.num_digipeaters as usize)
            .map(|i| {
                let digi = &frame.digipeaters[i];
                let call = bytes_to_string(digi.callsign_str());
                if digi.ssid > 0 {
                    if digi.h_bit {
                        format!("{}-{}*", call, digi.ssid)
                    } else {
                        format!("{}-{}", call, digi.ssid)
                    }
                } else if digi.h_bit {
                    format!("{}*", call)
                } else {
                    call
                }
            })
            .collect();
        Some(parts.join(","))
    } else {
        None
    };

    let raw_info = bytes_to_string(frame.info);

    // Try to parse as APRS
    let dest_callsign = frame.dest.callsign_str();
    let aprs_data = aprs::parse_packet(frame.info, dest_callsign).map(|pkt| to_web_packet(&pkt));

    let packet_type = aprs_data.as_ref().map(|d| packet_type_name(d));
    let summary = aprs_data.as_ref().map(format_summary);

    // Insert packet
    let packet_id = db::insert_packet(
        pool,
        &PacketInsert {
            source: &source,
            source_ssid,
            dest: &dest,
            path: path.as_deref(),
            packet_type,
            raw_info: &raw_info,
            summary: summary.as_deref(),
            source_type,
        },
    )
    .await?;

    // If we have APRS data, upsert station
    if let Some(ref data) = aprs_data {
        let mut position = extract_position(data)
            .filter(|&(lat, lon)| is_valid_position(lat, lon));
        let (speed, course) = extract_speed_course(data);

        // Enrich positionless stations from reference data (e.g., CWOP weather stations)
        if position.is_none() {
            if let Some(ref_db) = reference_db {
                if let Ok(Some(ref_pos)) = ref_db.lookup_position(&source).await {
                    position = Some((ref_pos.lat, ref_pos.lon));
                    tracing::debug!(
                        "Enriched {} with reference position: {:.4}, {:.4}",
                        source,
                        ref_pos.lat,
                        ref_pos.lon
                    );
                }
            }
        }
        let (sym_table, sym_code) = extract_symbol(data)
            .map(|(t, c)| (Some(t.to_string()), Some(c.to_string())))
            .unwrap_or((None, None));

        let comment = match data {
            WebAprsData::Position { comment, .. }
            | WebAprsData::Object { comment, .. }
            | WebAprsData::Item { comment, .. }
            | WebAprsData::Weather { comment, .. } => comment.as_deref(),
            _ => None,
        };

        let weather_json = match data {
            WebAprsData::Weather { weather, .. } => serde_json::to_string(weather).ok(),
            WebAprsData::Position {
                weather: Some(w), ..
            } => serde_json::to_string(w).ok(),
            _ => None,
        };

        db::upsert_station(
            pool,
            &StationUpsert {
                callsign: &source,
                ssid: source_ssid,
                station_type: packet_type.unwrap_or("Unknown"),
                lat: position.map(|(lat, _)| lat),
                lon: position.map(|(_, lon)| lon),
                speed,
                course,
                altitude: None, // altitude not in simplified core types
                comment,
                symbol_table: sym_table.as_deref(),
                symbol_code: sym_code.as_deref(),
                weather_json: weather_json.as_deref(),
                source_type,
                last_path: path.as_deref(),
            },
        )
        .await?;

        // Insert weather history for time-series charts
        let weather_data = match data {
            WebAprsData::Weather { weather, .. } => Some(weather),
            WebAprsData::Position { weather: Some(w), .. } => Some(w),
            _ => None,
        };
        if let Some(wx) = weather_data {
            db::insert_weather_history(pool, &source, source_ssid, wx).await?;
        }

        // Insert position history if we have a position
        if let Some((lat, lon)) = position {
            db::insert_position_history(pool, &source, source_ssid, lat, lon, None, speed, course)
                .await?;
        }

        // Broadcast station update
        if let Ok(Some(station_row)) =
            db::get_station_by_callsign(pool, &source, source_ssid).await
        {
            if let Ok(json) =
                serde_json::to_string(&crate::models::WsEvent::StationUpdate(station_row))
            {
                let _ = tx.send(json);
            }
        }

        // Handle messages
        if let WebAprsData::Message {
            addressee,
            text,
            message_no,
            ..
        } = data
        {
            db::insert_message(pool, &source, addressee.trim(), text, message_no.as_deref())
                .await?;
        }
    }

    // Extract RF links from the digipeater path
    if let Some(ref path_str) = path {
        extract_and_store_rf_links(pool, &source, source_ssid, path_str).await;
    }

    // Broadcast the packet event
    let packet_row = PacketRow {
        id: packet_id,
        source: source.clone(),
        source_ssid,
        dest,
        path,
        packet_type: packet_type.map(|s| s.to_string()),
        raw_info,
        summary,
        received_at: chrono::Utc::now().to_rfc3339(),
        source_type: source_type.to_string(),
    };

    let event_json = serde_json::to_string(&crate::models::WsEvent::Packet(packet_row))?;
    let _ = tx.send(event_json); // Ignore if no receivers

    Ok(true)
}

/// Process raw KISS bytes — feed through decoder, process each complete frame.
pub async fn process_kiss_bytes(
    data: &[u8],
    pool: &SqlitePool,
    tx: &broadcast::Sender<String>,
    reference_db: Option<&reference::ReferenceDb>,
) -> Result<u32, ServerError> {
    let mut decoder = KissDecoder::new();
    let mut count = 0u32;

    for &byte in data {
        if let Some((_port, cmd, frame_data)) = decoder.feed_byte(byte) {
            if matches!(cmd, packet_radio_core::kiss::Command::DataFrame) {
                // Copy frame data before next feed_byte call
                let frame_copy: Vec<u8> = frame_data.to_vec();
                if process_raw_frame(&frame_copy, pool, tx, reference_db, "tnc").await? {
                    count += 1;
                }
            }
        }
    }

    Ok(count)
}

/// Run the KISS TCP ingest loop — connects to TNC and processes frames.
pub async fn run_kiss_ingest(
    host: &str,
    port: u16,
    pool: SqlitePool,
    tx: broadcast::Sender<String>,
    reference_db: Option<Arc<reference::ReferenceDb>>,
) {
    let mut backoff = std::time::Duration::from_secs(1);
    let max_backoff = std::time::Duration::from_secs(60);

    loop {
        tracing::info!("Connecting to KISS TNC at {}:{}", host, port);
        match tokio::net::TcpStream::connect((host, port)).await {
            Ok(mut stream) => {
                tracing::info!("Connected to KISS TNC");
                backoff = std::time::Duration::from_secs(1);

                let mut buf = [0u8; 4096];
                let mut decoder = KissDecoder::new();

                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) => {
                            tracing::warn!("KISS TNC connection closed");
                            break;
                        }
                        Ok(n) => {
                            for &byte in &buf[..n] {
                                if let Some((_port, cmd, frame_data)) = decoder.feed_byte(byte) {
                                    if matches!(
                                        cmd,
                                        packet_radio_core::kiss::Command::DataFrame
                                    ) {
                                        let frame_copy: Vec<u8> = frame_data.to_vec();
                                        if let Err(e) =
                                            process_raw_frame(&frame_copy, &pool, &tx, reference_db.as_deref(), "tnc").await
                                        {
                                            tracing::error!("Frame processing error: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("KISS TNC read error: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to connect to KISS TNC: {}. Retrying in {:?}",
                    e,
                    backoff
                );
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Parse a callsign string like "N0CALL-9" into ("N0CALL", 9).
///
/// Re-exported from the shared crate for local convenience.
pub use packet_radio_shared::aprs_is::parse_call_ssid;

/// Generic APRS path aliases and APRS-IS server prefixes that are not real stations.
const PATH_ALIASES: &[&str] = &[
    "WIDE", "RELAY", "TRACE", "RFONLY", "NOGATE", "GATE", "TCPIP",
    // APRS-IS server naming: T2*, FIRST through TENTH, etc.
    "FIRST", "SECON", "THIRD", "FOURT", "FIFTH", "SIXTH", "SEVEN", "EIGHT", "NINTH", "TENTH",
    "AMBCW",
];

/// Check if a callsign base is a path alias or APRS-IS server name.
fn is_path_alias(base: &str) -> bool {
    // T2* = APRS-IS tier-2 servers (T2CHIL, T2FINL, T2PANA, etc.)
    if base.starts_with("T2") {
        return true;
    }
    PATH_ALIASES.iter().any(|a| base.starts_with(a))
}

/// Extract IGate and digipeater relationships from a packet path and store them.
///
/// Path format: "WIDE1-1,W1ABC-1*,qAR,KD1KE-5"
///   - W1ABC-1 has H-bit → it's a digipeater that forwarded source's packet
///   - KD1KE-5 after qAR → it's the IGate that heard the packet
async fn extract_and_store_rf_links(
    pool: &SqlitePool,
    source: &str,
    source_ssid: u8,
    path_str: &str,
) {
    let hops: Vec<&str> = path_str.split(',').collect();
    let mut after_q = false;

    for hop in &hops {
        let hop = hop.trim();
        if hop.is_empty() {
            continue;
        }

        // Check for q-construct (qAR, qAO, qAS, qAC, etc.)
        if hop.starts_with('q') && hop.len() <= 4 {
            // qAC = client-to-server direct, station after it is the APRS-IS server not an IGate
            // Only qAR/qAO/qAS indicate real RF IGates
            if hop == "qAR" || hop == "qAO" || hop == "qAS" {
                after_q = true;
            }
            continue;
        }

        let has_hbit = hop.ends_with('*');
        let call = if has_hbit { &hop[..hop.len() - 1] } else { hop };
        let (base, _ssid) = parse_call_ssid(call);

        // Skip aliases and very short callsigns
        if is_path_alias(base) || base.len() <= 2 {
            continue;
        }

        let (hearer_call, hearer_ssid) = parse_call_ssid(call);

        // Skip self-links (same base callsign, any SSID)
        if hearer_call.eq_ignore_ascii_case(source) {
            continue;
        }

        if has_hbit {
            // Digipeater with H-bit: it forwarded source's packet
            if let Err(e) = db::upsert_rf_link(
                pool,
                hearer_call,
                hearer_ssid,
                source,
                source_ssid,
                "digi",
            )
            .await
            {
                tracing::debug!("Failed to upsert digi RF link: {}", e);
            }
        } else if after_q {
            // Station after q-construct: IGate that heard the packet
            if let Err(e) = db::upsert_rf_link(
                pool,
                hearer_call,
                hearer_ssid,
                source,
                source_ssid,
                "igate",
            )
            .await
            {
                tracing::debug!("Failed to upsert igate RF link: {}", e);
            }
        }
    }
}

/// Format a short summary line for a packet.
fn format_summary(data: &WebAprsData) -> String {
    match data {
        WebAprsData::Position {
            lat, lon, comment, ..
        } => {
            let ns = if *lat >= 0.0 { "N" } else { "S" };
            let ew = if *lon >= 0.0 { "E" } else { "W" };
            let base = format!("{:.3}{}, {:.3}{}", lat.abs(), ns, lon.abs(), ew);
            if let Some(c) = comment {
                let short: String = c.chars().take(40).collect();
                format!("{} — {}", base, short)
            } else {
                base
            }
        }
        WebAprsData::MicE {
            lat,
            lon,
            speed,
            course,
            ..
        } => {
            {
                let ns = if *lat >= 0.0 { "N" } else { "S" };
                let ew = if *lon >= 0.0 { "E" } else { "W" };
                format!(
                    "{:.3}{}, {:.3}{}  {:.0}kn/{:.0}\u{b0}",
                    lat.abs(), ns, lon.abs(), ew, speed, course
                )
            }
        }
        WebAprsData::Message {
            addressee, text, ..
        } => {
            format!("→{}: {}", addressee.trim(), text)
        }
        WebAprsData::Weather { weather, .. } => {
            let mut parts = Vec::new();
            if let Some(t) = weather.temperature {
                parts.push(format!("{}°F", t));
            }
            if let Some(ws) = weather.wind_speed {
                parts.push(format!("Wind {}mph", ws));
            }
            parts.join(", ")
        }
        WebAprsData::Object { name, .. } => format!("Obj: {}", name.trim()),
        WebAprsData::Item { name, .. } => format!("Item: {}", name.trim()),
        WebAprsData::Status { text, .. } => text.chars().take(60).collect(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    /// Helper: build a minimal AX.25 UI frame with the given info field.
    fn build_test_ax25_frame(src: &str, dest: &str, info: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();

        // Destination address (7 bytes)
        let mut dest_bytes = [0x40u8; 7]; // space << 1
        for (i, &b) in dest.as_bytes().iter().take(6).enumerate() {
            dest_bytes[i] = b << 1;
        }
        dest_bytes[6] = 0x60; // SSID byte (SSID=0, not last)
        frame.extend_from_slice(&dest_bytes);

        // Source address (7 bytes)
        let mut src_bytes = [0x40u8; 7];
        for (i, &b) in src.as_bytes().iter().take(6).enumerate() {
            src_bytes[i] = b << 1;
        }
        src_bytes[6] = 0x61; // SSID byte (SSID=0, last address)
        frame.extend_from_slice(&src_bytes);

        // Control + PID
        frame.push(0x03); // UI frame
        frame.push(0xF0); // No layer 3

        // Info field
        frame.extend_from_slice(info);

        frame
    }

    /// Helper: wrap raw AX.25 in KISS framing.
    fn kiss_encode(ax25: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0xC0); // FEND
        out.push(0x00); // Data frame, port 0
        for &b in ax25 {
            match b {
                0xC0 => {
                    out.push(0xDB);
                    out.push(0xDC);
                }
                0xDB => {
                    out.push(0xDB);
                    out.push(0xDD);
                }
                _ => out.push(b),
            }
        }
        out.push(0xC0); // FEND
        out
    }

    #[tokio::test]
    async fn test_process_raw_frame_position() {
        let pool = db::test_db().await;
        let (tx, mut rx) = broadcast::channel(16);

        let ax25 = build_test_ax25_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Test");
        let result = process_raw_frame(&ax25, &pool, &tx, None, "tnc").await.unwrap();
        assert!(result);

        // Check DB
        let stations = db::get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "N0CALL");
        assert!(stations[0].lat.is_some());
        assert!((stations[0].lat.unwrap() - 49.058333).abs() < 0.01);

        let packets = db::get_recent_packets(&pool, 10).await.unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].source, "N0CALL");
        assert_eq!(packets[0].packet_type.as_deref(), Some("Position"));

        // Check broadcast — StationUpdate first, then Packet
        let station_event = rx.try_recv().unwrap();
        let station_val: serde_json::Value = serde_json::from_str(&station_event).unwrap();
        assert_eq!(station_val["type"], "StationUpdate");
        assert_eq!(station_val["callsign"], "N0CALL");

        let pkt_event = rx.try_recv().unwrap();
        let pkt_val: serde_json::Value = serde_json::from_str(&pkt_event).unwrap();
        assert_eq!(pkt_val["source"], "N0CALL");
    }

    #[tokio::test]
    async fn test_process_kiss_bytes() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        let ax25 = build_test_ax25_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-Test");
        let kiss = kiss_encode(&ax25);

        let count = process_kiss_bytes(&kiss, &pool, &tx, None).await.unwrap();
        assert_eq!(count, 1);

        let stations = db::get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
    }

    #[tokio::test]
    async fn test_process_kiss_multi_frame() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        let mut all_kiss = Vec::new();
        all_kiss.extend_from_slice(&kiss_encode(&build_test_ax25_frame(
            "N0CALL",
            "APRS",
            b"!4903.50N/07201.75W-",
        )));
        all_kiss.extend_from_slice(&kiss_encode(&build_test_ax25_frame(
            "W1AW",
            "APRS",
            b"!4200.00N/07100.00W-",
        )));
        all_kiss.extend_from_slice(&kiss_encode(&build_test_ax25_frame(
            "WX0STA",
            "APRS",
            b"!3400.00N/11800.00W-",
        )));

        let count = process_kiss_bytes(&all_kiss, &pool, &tx, None).await.unwrap();
        assert_eq!(count, 3);

        let stations = db::get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 3);
    }

    #[tokio::test]
    async fn test_process_duplicate_station() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        let ax25_1 = build_test_ax25_frame("N0CALL", "APRS", b"!4903.50N/07201.75W-First");
        let ax25_2 = build_test_ax25_frame("N0CALL", "APRS", b"!4904.00N/07202.00W-Second");

        process_raw_frame(&ax25_1, &pool, &tx, None, "tnc").await.unwrap();
        process_raw_frame(&ax25_2, &pool, &tx, None, "tnc").await.unwrap();

        let stations = db::get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].packet_count, 2);

        let packets = db::get_recent_packets(&pool, 10).await.unwrap();
        assert_eq!(packets.len(), 2);
    }

    #[tokio::test]
    async fn test_process_message_packet() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        let ax25 = build_test_ax25_frame("N0CALL", "APRS", b":W1AW     :Hello!{001");
        process_raw_frame(&ax25, &pool, &tx, None, "tnc").await.unwrap();

        let msgs = db::get_messages(&pool, "N0CALL").await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].from_call, "N0CALL");
    }

    #[tokio::test]
    async fn test_process_frame_rejects_zero_position() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        // APRS position at exactly (0,0) — common artifact from `@000000z0000.00N/00000.00E`
        let ax25 = build_test_ax25_frame("N1NCB", "APRS", b"!0000.00N/00000.00E-zero");
        process_raw_frame(&ax25, &pool, &tx, None, "tnc").await.unwrap();

        // Station should exist but with no lat/lon
        let stations = db::get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert!(stations[0].lat.is_none(), "station lat should be None for (0,0)");
        assert!(stations[0].lon.is_none(), "station lon should be None for (0,0)");

        // Position history should have 0 rows
        let track = db::get_station_track(&pool, "N1NCB", 0, 24).await.unwrap();
        assert_eq!(track.len(), 0, "no track points for (0,0)");
    }

    #[tokio::test]
    async fn test_process_frame_rejects_near_zero_position() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        // ~0.017° N, ~0.017° E — within the 0.1° exclusion zone
        let ax25 = build_test_ax25_frame("KF6YVS", "APRS", b"!0001.00N/00001.00E>");
        process_raw_frame(&ax25, &pool, &tx, None, "tnc").await.unwrap();

        let track = db::get_station_track(&pool, "KF6YVS", 0, 24).await.unwrap();
        assert_eq!(track.len(), 0, "near-zero position should be rejected");
    }

    #[tokio::test]
    async fn test_process_frame_stores_valid_position() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        let ax25 = build_test_ax25_frame("W1AW", "APRS", b"!4903.50N/07201.75W-HQ");
        process_raw_frame(&ax25, &pool, &tx, None, "tnc").await.unwrap();

        let track = db::get_station_track(&pool, "W1AW", 0, 24).await.unwrap();
        assert_eq!(track.len(), 1, "valid position should produce a track point");
        assert!((track[0].lat - 49.058333).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_process_frame_zero_position_does_not_update_station_coords() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        // First: valid position
        let ax25_good = build_test_ax25_frame("N1NCB", "APRS", b"!4381.00N/06994.00W-Maine");
        process_raw_frame(&ax25_good, &pool, &tx, None, "tnc").await.unwrap();

        let s = db::get_station_by_callsign(&pool, "N1NCB", 0).await.unwrap().unwrap();
        assert!(s.lat.is_some());
        let good_lat = s.lat.unwrap();

        // Second: bogus (0,0) position
        let ax25_bad = build_test_ax25_frame("N1NCB", "APRS", b"!0000.00N/00000.00E-zero");
        process_raw_frame(&ax25_bad, &pool, &tx, None, "tnc").await.unwrap();

        // Station coords should be preserved (COALESCE keeps previous value when we pass None)
        let s2 = db::get_station_by_callsign(&pool, "N1NCB", 0).await.unwrap().unwrap();
        assert!((s2.lat.unwrap() - good_lat).abs() < 0.001,
            "lat should remain {} but got {:?}", good_lat, s2.lat);
    }

    #[tokio::test]
    async fn test_is_valid_position() {
        // (0,0) and near-zero
        assert!(!is_valid_position(0.0, 0.0));
        assert!(!is_valid_position(0.05, 0.05));
        assert!(!is_valid_position(-0.09, 0.09));
        // Out of range
        assert!(!is_valid_position(91.0, 0.0));
        assert!(!is_valid_position(0.0, 181.0));
        assert!(!is_valid_position(-91.0, 0.0));
        // Valid positions
        assert!(is_valid_position(49.058, -72.030));
        assert!(is_valid_position(-33.86, 151.21)); // Sydney
        assert!(is_valid_position(0.5, 0.5)); // Near equator/prime meridian but outside exclusion
    }

    #[tokio::test]
    async fn test_process_malformed_frame() {
        let pool = db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        // Too short to be valid AX.25
        let result = process_raw_frame(&[0x00, 0x01, 0x02], &pool, &tx, None, "tnc")
            .await
            .unwrap();
        assert!(!result);

        let stations = db::get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 0);
    }

    // === RF link extraction tests ===

    #[test]
    fn test_parse_call_ssid() {
        assert_eq!(parse_call_ssid("N0CALL"), ("N0CALL", 0));
        assert_eq!(parse_call_ssid("N0CALL-9"), ("N0CALL", 9));
        assert_eq!(parse_call_ssid("KD1KE-5"), ("KD1KE", 5));
    }

    #[test]
    fn test_is_path_alias() {
        assert!(is_path_alias("WIDE1"));
        assert!(is_path_alias("WIDE2"));
        assert!(is_path_alias("RELAY"));
        assert!(is_path_alias("TCPIP"));
        assert!(!is_path_alias("KD1KE"));
        assert!(!is_path_alias("N0CALL"));
    }

    #[tokio::test]
    async fn test_extract_rf_links_igate() {
        let pool = db::test_db().await;
        // Typical APRS-IS path: source heard by IGate KD1KE-5 via qAR
        extract_and_store_rf_links(&pool, "N0CALL", 0, "WIDE1-1,WIDE2-1,qAR,KD1KE-5").await;

        let row = sqlx::query("SELECT * FROM rf_links WHERE hearer = 'KD1KE' AND hearer_ssid = 5")
            .fetch_optional(&pool).await.unwrap();
        assert!(row.is_some(), "IGate link should be created");
        let row = row.unwrap();
        assert_eq!(row.get::<String, _>("heard"), "N0CALL");
        assert_eq!(row.get::<String, _>("link_type"), "igate");
    }

    #[tokio::test]
    async fn test_extract_rf_links_digi() {
        let pool = db::test_db().await;
        // Path with digipeater H-bit: W1ABC-1 forwarded N0CALL's packet
        extract_and_store_rf_links(&pool, "N0CALL", 0, "W1ABC-1*,WIDE2*,qAR,KD1KE").await;

        // W1ABC-1 should be a digi link
        let row = sqlx::query("SELECT * FROM rf_links WHERE hearer = 'W1ABC' AND link_type = 'digi'")
            .fetch_optional(&pool).await.unwrap();
        assert!(row.is_some(), "Digi link should be created for W1ABC-1");

        // WIDE2* should be skipped (alias)
        let alias_row = sqlx::query("SELECT * FROM rf_links WHERE hearer = 'WIDE2'")
            .fetch_optional(&pool).await.unwrap();
        assert!(alias_row.is_none(), "WIDE alias should be skipped");

        // KD1KE should be an igate link
        let igate = sqlx::query("SELECT * FROM rf_links WHERE hearer = 'KD1KE' AND link_type = 'igate'")
            .fetch_optional(&pool).await.unwrap();
        assert!(igate.is_some(), "IGate link should be created for KD1KE");
    }

    #[tokio::test]
    async fn test_extract_rf_links_tcpip_skipped() {
        let pool = db::test_db().await;
        // TCPIP* paths are pure APRS-IS originated — qAC means server, not IGate
        extract_and_store_rf_links(&pool, "N0CALL", 0, "TCPIP*,qAC,SERVER").await;

        // Nothing should be stored: TCPIP is alias, qAC is filtered, SERVER after qAC ignored
        let all = sqlx::query("SELECT COUNT(*) as cnt FROM rf_links")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(all.get::<i64, _>("cnt"), 0, "qAC path should produce no RF links");
    }

    #[tokio::test]
    async fn test_extract_rf_links_self_link_skipped() {
        let pool = db::test_db().await;
        // VE9RMO-10 IGate hearing VE9RMO-9 mobile — same operator, should skip
        extract_and_store_rf_links(&pool, "VE9RMO", 9, "WIDE1-1,qAR,VE9RMO-10").await;

        let all = sqlx::query("SELECT COUNT(*) as cnt FROM rf_links")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(all.get::<i64, _>("cnt"), 0, "Self-links should be skipped");
    }

    #[tokio::test]
    async fn test_extract_rf_links_t2_server_skipped() {
        let pool = db::test_db().await;
        // T2 servers should be skipped even after qAR (shouldn't happen in practice)
        extract_and_store_rf_links(&pool, "N0CALL", 0, "WIDE1-1,qAR,T2CHIL").await;

        let all = sqlx::query("SELECT COUNT(*) as cnt FROM rf_links")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(all.get::<i64, _>("cnt"), 0, "T2 server should be skipped");
    }
}
