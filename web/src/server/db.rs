use sqlx::{Row, SqlitePool};

use crate::models::{MessageRow, PacketRow, StationRow, TrackPoint, WebWeather};

/// Insert a decoded packet into the database.
pub async fn insert_packet(
    pool: &SqlitePool,
    source: &str,
    source_ssid: u8,
    dest: &str,
    path: Option<&str>,
    packet_type: Option<&str>,
    raw_info: &str,
    summary: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO packets (source, source_ssid, dest, path, packet_type, raw_info, summary)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(source)
    .bind(source_ssid as i64)
    .bind(dest)
    .bind(path)
    .bind(packet_type)
    .bind(raw_info)
    .bind(summary)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Upsert a station — insert or update on conflict.
pub async fn upsert_station(
    pool: &SqlitePool,
    callsign: &str,
    ssid: u8,
    station_type: &str,
    lat: Option<f64>,
    lon: Option<f64>,
    speed: Option<f64>,
    course: Option<f64>,
    altitude: Option<f64>,
    comment: Option<&str>,
    symbol_table: Option<&str>,
    symbol_code: Option<&str>,
    weather_json: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO stations (callsign, ssid, station_type, lat, lon, speed, course, altitude, comment, symbol_table, symbol_code, weather_json, packet_count, last_heard)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, datetime('now'))
         ON CONFLICT(callsign, ssid) DO UPDATE SET
           station_type = excluded.station_type,
           lat = COALESCE(excluded.lat, stations.lat),
           lon = COALESCE(excluded.lon, stations.lon),
           speed = COALESCE(excluded.speed, stations.speed),
           course = COALESCE(excluded.course, stations.course),
           altitude = COALESCE(excluded.altitude, stations.altitude),
           comment = COALESCE(excluded.comment, stations.comment),
           symbol_table = COALESCE(excluded.symbol_table, stations.symbol_table),
           symbol_code = COALESCE(excluded.symbol_code, stations.symbol_code),
           weather_json = COALESCE(excluded.weather_json, stations.weather_json),
           packet_count = stations.packet_count + 1,
           last_heard = datetime('now')",
    )
    .bind(callsign)
    .bind(ssid as i64)
    .bind(station_type)
    .bind(lat)
    .bind(lon)
    .bind(speed)
    .bind(course)
    .bind(altitude)
    .bind(comment)
    .bind(symbol_table)
    .bind(symbol_code)
    .bind(weather_json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert a position history point.
pub async fn insert_position_history(
    pool: &SqlitePool,
    callsign: &str,
    ssid: u8,
    lat: f64,
    lon: f64,
    altitude: Option<f64>,
    speed: Option<f64>,
    course: Option<f64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO position_history (callsign, ssid, lat, lon, altitude, speed, course)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(callsign)
    .bind(ssid as i64)
    .bind(lat)
    .bind(lon)
    .bind(altitude)
    .bind(speed)
    .bind(course)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get recent packets, newest first.
pub async fn get_recent_packets(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<PacketRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, source, source_ssid, dest, path, packet_type, raw_info, summary, received_at
         FROM packets ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| PacketRow {
            id: r.get::<i64, _>("id"),
            source: r.get::<String, _>("source"),
            source_ssid: r.get::<i64, _>("source_ssid") as u8,
            dest: r.get::<String, _>("dest"),
            path: r.get::<Option<String>, _>("path"),
            packet_type: r.get::<Option<String>, _>("packet_type"),
            raw_info: r.get::<String, _>("raw_info"),
            summary: r.get::<Option<String>, _>("summary"),
            received_at: r.get::<String, _>("received_at"),
        })
        .collect())
}

/// Get all stations.
pub async fn get_stations(pool: &SqlitePool) -> Result<Vec<StationRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT callsign, ssid, station_type, lat, lon, speed, course, altitude,
                comment, symbol_table, symbol_code, last_heard, packet_count, weather_json
         FROM stations ORDER BY last_heard DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| {
            let weather_json: Option<String> = r.get("weather_json");
            StationRow {
                callsign: r.get("callsign"),
                ssid: r.get::<i64, _>("ssid") as u8,
                station_type: r.get("station_type"),
                lat: r.get("lat"),
                lon: r.get("lon"),
                speed: r.get("speed"),
                course: r.get("course"),
                altitude: r.get("altitude"),
                comment: r.get("comment"),
                symbol_table: r.get("symbol_table"),
                symbol_code: r.get("symbol_code"),
                last_heard: r.get("last_heard"),
                packet_count: r.get("packet_count"),
                weather: weather_json
                    .and_then(|j| serde_json::from_str::<WebWeather>(&j).ok()),
            }
        })
        .collect())
}

/// Get only stations that have a position.
pub async fn get_stations_with_position(
    pool: &SqlitePool,
) -> Result<Vec<StationRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT callsign, ssid, station_type, lat, lon, speed, course, altitude,
                comment, symbol_table, symbol_code, last_heard, packet_count, weather_json
         FROM stations WHERE lat IS NOT NULL AND lon IS NOT NULL
         ORDER BY last_heard DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| {
            let weather_json: Option<String> = r.get("weather_json");
            StationRow {
                callsign: r.get("callsign"),
                ssid: r.get::<i64, _>("ssid") as u8,
                station_type: r.get("station_type"),
                lat: r.get("lat"),
                lon: r.get("lon"),
                speed: r.get("speed"),
                course: r.get("course"),
                altitude: r.get("altitude"),
                comment: r.get("comment"),
                symbol_table: r.get("symbol_table"),
                symbol_code: r.get("symbol_code"),
                last_heard: r.get("last_heard"),
                packet_count: r.get("packet_count"),
                weather: weather_json
                    .and_then(|j| serde_json::from_str::<WebWeather>(&j).ok()),
            }
        })
        .collect())
}

/// Get track points for a station within the last N hours.
pub async fn get_station_track(
    pool: &SqlitePool,
    callsign: &str,
    ssid: u8,
    hours: u32,
) -> Result<Vec<TrackPoint>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT lat, lon, altitude, speed, course, recorded_at
         FROM position_history
         WHERE callsign = ? AND ssid = ? AND recorded_at > datetime('now', '-' || ? || ' hours')
         ORDER BY recorded_at ASC",
    )
    .bind(callsign)
    .bind(ssid as i64)
    .bind(hours)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| TrackPoint {
            lat: r.get("lat"),
            lon: r.get("lon"),
            altitude: r.get("altitude"),
            speed: r.get("speed"),
            course: r.get("course"),
            recorded_at: r.get("recorded_at"),
        })
        .collect())
}

/// Insert a message.
pub async fn insert_message(
    pool: &SqlitePool,
    from_call: &str,
    to_call: &str,
    message_text: &str,
    message_no: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO messages (from_call, to_call, message_text, message_no)
         VALUES (?, ?, ?, ?)",
    )
    .bind(from_call)
    .bind(to_call)
    .bind(message_text)
    .bind(message_no)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get messages for a callsign (sent or received).
pub async fn get_messages(
    pool: &SqlitePool,
    callsign: &str,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, from_call, to_call, message_text, message_no, acked, received_at
         FROM messages
         WHERE from_call = ? OR to_call = ?
         ORDER BY received_at ASC",
    )
    .bind(callsign)
    .bind(callsign)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| MessageRow {
            id: r.get("id"),
            from_call: r.get("from_call"),
            to_call: r.get("to_call"),
            message_text: r.get("message_text"),
            message_no: r.get("message_no"),
            acked: r.get::<bool, _>("acked"),
            received_at: r.get("received_at"),
        })
        .collect())
}

/// Get a single station by callsign and SSID.
pub async fn get_station_by_callsign(
    pool: &SqlitePool,
    callsign: &str,
    ssid: u8,
) -> Result<Option<StationRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT callsign, ssid, station_type, lat, lon, speed, course, altitude,
                comment, symbol_table, symbol_code, last_heard, packet_count, weather_json
         FROM stations WHERE callsign = ? AND ssid = ?",
    )
    .bind(callsign)
    .bind(ssid as i64)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| {
        let weather_json: Option<String> = r.get("weather_json");
        StationRow {
            callsign: r.get("callsign"),
            ssid: r.get::<i64, _>("ssid") as u8,
            station_type: r.get("station_type"),
            lat: r.get("lat"),
            lon: r.get("lon"),
            speed: r.get("speed"),
            course: r.get("course"),
            altitude: r.get("altitude"),
            comment: r.get("comment"),
            symbol_table: r.get("symbol_table"),
            symbol_code: r.get("symbol_code"),
            last_heard: r.get("last_heard"),
            packet_count: r.get("packet_count"),
            weather: weather_json
                .and_then(|j| serde_json::from_str::<WebWeather>(&j).ok()),
        }
    }))
}

/// Delete stations older than the given number of hours.
pub async fn cleanup_stale_stations(
    pool: &SqlitePool,
    max_age_hours: u32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM stations WHERE last_heard < datetime('now', '-' || ? || ' hours')",
    )
    .bind(max_age_hours)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Prune old position history entries.
pub async fn cleanup_position_history(
    pool: &SqlitePool,
    max_age_hours: u32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM position_history WHERE recorded_at < datetime('now', '-' || ? || ' hours')",
    )
    .bind(max_age_hours)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Create an in-memory SQLite pool for testing.
#[cfg(test)]
pub async fn test_db() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(include_str!("../../migrations/001_initial.sql"))
        .execute(&pool)
        .await
        .unwrap();
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_insert_and_get_packets() {
        let pool = test_db().await;

        insert_packet(&pool, "N0CALL", 0, "APRS", Some("WIDE1-1"), Some("Position"), "!4903.50N/07201.75W-", Some("Pos")).await.unwrap();
        insert_packet(&pool, "W1AW", 9, "APRS", None, Some("MicE"), "`data", None).await.unwrap();
        insert_packet(&pool, "WX0STA", 0, "APRS", None, Some("Weather"), "_weather", None).await.unwrap();

        let packets = get_recent_packets(&pool, 2).await.unwrap();
        assert_eq!(packets.len(), 2);
        // Newest first
        assert_eq!(packets[0].source, "WX0STA");
        assert_eq!(packets[1].source, "W1AW");
    }

    #[tokio::test]
    async fn test_upsert_station_insert() {
        let pool = test_db().await;

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.058), Some(-72.030), None, None, None, Some("Test"), Some("/"), Some(">"), None).await.unwrap();

        let stations = get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "N0CALL");
        assert!((stations[0].lat.unwrap() - 49.058).abs() < 0.001);
        assert_eq!(stations[0].packet_count, 1);
    }

    #[tokio::test]
    async fn test_upsert_station_update() {
        let pool = test_db().await;

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, Some("First"), Some("/"), Some(">"), None).await.unwrap();
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.1), Some(-72.1), Some(60.0), None, None, Some("Second"), None, None, None).await.unwrap();

        let stations = get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].packet_count, 2);
        assert!((stations[0].lat.unwrap() - 49.1).abs() < 0.001);
        assert_eq!(stations[0].comment.as_deref(), Some("Second"));
        // Symbol preserved via COALESCE
        assert_eq!(stations[0].symbol_table.as_deref(), Some("/"));
    }

    #[tokio::test]
    async fn test_upsert_coalesce_preserves_position() {
        let pool = test_db().await;

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None).await.unwrap();
        upsert_station(&pool, "N0CALL", 0, "Message", None, None, None, None, None, None, None, None, None).await.unwrap();

        let stations = get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert!(stations[0].lat.is_some());
        assert!((stations[0].lat.unwrap() - 49.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_get_stations_with_position() {
        let pool = test_db().await;

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None).await.unwrap();
        upsert_station(&pool, "W1AW", 0, "Message", None, None, None, None, None, None, None, None, None).await.unwrap();

        let stations = get_stations_with_position(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "N0CALL");
    }

    #[tokio::test]
    async fn test_position_history() {
        let pool = test_db().await;

        for i in 0..5 {
            insert_position_history(&pool, "N0CALL", 0, 49.0 + i as f64 * 0.01, -72.0, None, None, None).await.unwrap();
        }

        let track = get_station_track(&pool, "N0CALL", 0, 24).await.unwrap();
        assert_eq!(track.len(), 5);
        assert!(track[0].lat < track[4].lat);
    }

    #[tokio::test]
    async fn test_messages() {
        let pool = test_db().await;

        insert_message(&pool, "N0CALL", "W1AW", "Hello!", Some("001")).await.unwrap();
        insert_message(&pool, "W1AW", "N0CALL", "Hi back!", Some("002")).await.unwrap();
        insert_message(&pool, "OTHER", "OTHER2", "Not for us", None).await.unwrap();

        let msgs = get_messages(&pool, "N0CALL").await.unwrap();
        assert_eq!(msgs.len(), 2);
    }

    #[tokio::test]
    async fn test_cleanup_stale_stations() {
        let pool = test_db().await;

        upsert_station(&pool, "OLD", 0, "Position", Some(40.0), Some(-74.0), None, None, None, None, None, None, None).await.unwrap();
        sqlx::query("UPDATE stations SET last_heard = datetime('now', '-49 hours') WHERE callsign = 'OLD'")
            .execute(&pool).await.unwrap();

        upsert_station(&pool, "NEW", 0, "Position", Some(41.0), Some(-74.0), None, None, None, None, None, None, None).await.unwrap();

        let deleted = cleanup_stale_stations(&pool, 48).await.unwrap();
        assert_eq!(deleted, 1);

        let stations = get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "NEW");
    }
}
