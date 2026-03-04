use sqlx::{Row, SqlitePool};

use crate::models::{MessageRow, PacketRow, StationRow, TrackPoint, WebWeather, WeatherHistoryPoint};

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
    source_type: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO packets (source, source_ssid, dest, path, packet_type, raw_info, summary, source_type)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(source)
    .bind(source_ssid as i64)
    .bind(dest)
    .bind(path)
    .bind(packet_type)
    .bind(raw_info)
    .bind(summary)
    .bind(source_type)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Upsert a station — insert or update on conflict.
/// `source_type` is "tnc" or "aprs-is". On insert, sets heard_via and last_source_type.
/// On update, appends to heard_via if not already present and updates last_source_type.
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
    source_type: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO stations (callsign, ssid, station_type, lat, lon, speed, course, altitude, comment, symbol_table, symbol_code, weather_json, packet_count, last_heard, heard_via, last_source_type)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, datetime('now'), ?, ?)
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
           last_heard = datetime('now'),
           heard_via = CASE
             WHEN stations.heard_via = '' THEN excluded.heard_via
             WHEN instr(stations.heard_via, excluded.heard_via) > 0 THEN stations.heard_via
             ELSE stations.heard_via || ',' || excluded.heard_via
           END,
           last_source_type = excluded.last_source_type",
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
    .bind(source_type)
    .bind(source_type)
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

/// Helper: map a sqlx Row to a PacketRow.
fn row_to_packet(r: &sqlx::sqlite::SqliteRow) -> PacketRow {
    PacketRow {
        id: r.get::<i64, _>("id"),
        source: r.get::<String, _>("source"),
        source_ssid: r.get::<i64, _>("source_ssid") as u8,
        dest: r.get::<String, _>("dest"),
        path: r.get::<Option<String>, _>("path"),
        packet_type: r.get::<Option<String>, _>("packet_type"),
        raw_info: r.get::<String, _>("raw_info"),
        summary: r.get::<Option<String>, _>("summary"),
        received_at: r.get::<String, _>("received_at"),
        source_type: r.get::<String, _>("source_type"),
    }
}

/// Helper: map a sqlx Row to a StationRow.
fn row_to_station(r: &sqlx::sqlite::SqliteRow) -> StationRow {
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
        weather: weather_json.and_then(|j| serde_json::from_str::<WebWeather>(&j).ok()),
        heard_via: r.get("heard_via"),
        last_source_type: r.get("last_source_type"),
        has_moved: r.get::<bool, _>("has_moved"),
    }
}

/// Get recent packets, newest first.
pub async fn get_recent_packets(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<PacketRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, source, source_ssid, dest, path, packet_type, raw_info, summary, received_at, source_type
         FROM packets ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(row_to_packet).collect())
}

/// Get all stations.
pub async fn get_stations(pool: &SqlitePool) -> Result<Vec<StationRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT callsign, ssid, station_type, lat, lon, speed, course, altitude,
                comment, symbol_table, symbol_code, last_heard, packet_count, weather_json,
                heard_via, last_source_type,
                (SELECT (MAX(lat) - MIN(lat)) > 0.003 OR (MAX(lon) - MIN(lon)) > 0.003
                    FROM position_history
                    WHERE callsign = s.callsign AND ssid = s.ssid
                ) AS has_moved
         FROM stations s ORDER BY last_heard DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(row_to_station).collect())
}

/// Get only stations that have a position.
pub async fn get_stations_with_position(
    pool: &SqlitePool,
) -> Result<Vec<StationRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT callsign, ssid, station_type, lat, lon, speed, course, altitude,
                comment, symbol_table, symbol_code, last_heard, packet_count, weather_json,
                heard_via, last_source_type,
                (SELECT (MAX(lat) - MIN(lat)) > 0.003 OR (MAX(lon) - MIN(lon)) > 0.003
                    FROM position_history
                    WHERE callsign = s.callsign AND ssid = s.ssid
                ) AS has_moved
         FROM stations s WHERE lat IS NOT NULL AND lon IS NOT NULL
         ORDER BY last_heard DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(row_to_station).collect())
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
         WHERE callsign = ? AND ssid = ?
           AND recorded_at > datetime('now', '-' || ? || ' hours')
           AND NOT (abs(lat) < 0.1 AND abs(lon) < 0.1)
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
                comment, symbol_table, symbol_code, last_heard, packet_count, weather_json,
                heard_via, last_source_type,
                (SELECT (MAX(lat) - MIN(lat)) > 0.003 OR (MAX(lon) - MIN(lon)) > 0.003
                    FROM position_history
                    WHERE callsign = s.callsign AND ssid = s.ssid
                ) AS has_moved
         FROM stations s WHERE callsign = ? AND ssid = ?",
    )
    .bind(callsign)
    .bind(ssid as i64)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| row_to_station(&r)))
}

/// Get packets from a specific station.
pub async fn get_station_packets(
    pool: &SqlitePool,
    callsign: &str,
    ssid: u8,
    limit: i64,
) -> Result<Vec<PacketRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, source, source_ssid, dest, path, packet_type, raw_info, summary, received_at, source_type
         FROM packets WHERE source = ? AND source_ssid = ?
         ORDER BY id DESC LIMIT ?",
    )
    .bind(callsign)
    .bind(ssid as i64)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(row_to_packet).collect())
}

/// Insert a weather history data point.
pub async fn insert_weather_history(
    pool: &SqlitePool,
    callsign: &str,
    ssid: u8,
    wx: &WebWeather,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO weather_history (callsign, ssid, temperature, wind_speed, wind_direction, wind_gust, humidity, barometric_pressure, rain_last_hour, rain_24h, luminosity)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(callsign)
    .bind(ssid as i64)
    .bind(wx.temperature.map(|v| v as i64))
    .bind(wx.wind_speed.map(|v| v as i64))
    .bind(wx.wind_direction.map(|v| v as i64))
    .bind(wx.wind_gust.map(|v| v as i64))
    .bind(wx.humidity.map(|v| v as i64))
    .bind(wx.barometric_pressure.map(|v| v as i64))
    .bind(wx.rain_last_hour.map(|v| v as i64))
    .bind(wx.rain_24h.map(|v| v as i64))
    .bind(wx.luminosity.map(|v| v as i64))
    .execute(pool)
    .await?;
    Ok(())
}

/// Get weather history for a station within the last N hours.
pub async fn get_weather_history(
    pool: &SqlitePool,
    callsign: &str,
    ssid: u8,
    hours: u32,
) -> Result<Vec<WeatherHistoryPoint>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT temperature, wind_speed, wind_direction, wind_gust, humidity,
                barometric_pressure, rain_last_hour, rain_24h, luminosity, recorded_at
         FROM weather_history
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
        .map(|r| WeatherHistoryPoint {
            temperature: r.get::<Option<i64>, _>("temperature").map(|v| v as i16),
            wind_speed: r.get::<Option<i64>, _>("wind_speed").map(|v| v as u16),
            wind_direction: r.get::<Option<i64>, _>("wind_direction").map(|v| v as u16),
            wind_gust: r.get::<Option<i64>, _>("wind_gust").map(|v| v as u16),
            humidity: r.get::<Option<i64>, _>("humidity").map(|v| v as u8),
            barometric_pressure: r.get::<Option<i64>, _>("barometric_pressure").map(|v| v as u32),
            rain_last_hour: r.get::<Option<i64>, _>("rain_last_hour").map(|v| v as u16),
            rain_24h: r.get::<Option<i64>, _>("rain_24h").map(|v| v as u16),
            luminosity: r.get::<Option<i64>, _>("luminosity").map(|v| v as u16),
            recorded_at: r.get("recorded_at"),
        })
        .collect())
}

/// Prune old weather history entries.
pub async fn cleanup_weather_history(
    pool: &SqlitePool,
    max_age_hours: u32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM weather_history WHERE recorded_at < datetime('now', '-' || ? || ' hours')",
    )
    .bind(max_age_hours)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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
    // Run migrations sequentially — SQLite doesn't support multiple statements in one query
    // for ALTER TABLE, so we split 002 into individual statements.
    sqlx::query(include_str!("../../migrations/001_initial.sql"))
        .execute(&pool)
        .await
        .unwrap();
    // 002: weather_history table + index
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS weather_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            callsign TEXT NOT NULL,
            ssid INTEGER NOT NULL DEFAULT 0,
            temperature INTEGER,
            wind_speed INTEGER,
            wind_direction INTEGER,
            wind_gust INTEGER,
            humidity INTEGER,
            barometric_pressure INTEGER,
            rain_last_hour INTEGER,
            rain_24h INTEGER,
            luminosity INTEGER,
            recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_weather_history_call
         ON weather_history(callsign, ssid, recorded_at)",
    )
    .execute(&pool)
    .await
    .unwrap();
    // 002: source_type on packets
    sqlx::query("ALTER TABLE packets ADD COLUMN source_type TEXT NOT NULL DEFAULT 'unknown'")
        .execute(&pool)
        .await
        .unwrap();
    // 002: heard_via and last_source_type on stations
    sqlx::query("ALTER TABLE stations ADD COLUMN heard_via TEXT NOT NULL DEFAULT ''")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE stations ADD COLUMN last_source_type TEXT NOT NULL DEFAULT 'unknown'")
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

        insert_packet(&pool, "N0CALL", 0, "APRS", Some("WIDE1-1"), Some("Position"), "!4903.50N/07201.75W-", Some("Pos"), "tnc").await.unwrap();
        insert_packet(&pool, "W1AW", 9, "APRS", None, Some("MicE"), "`data", None, "aprs-is").await.unwrap();
        insert_packet(&pool, "WX0STA", 0, "APRS", None, Some("Weather"), "_weather", None, "tnc").await.unwrap();

        let packets = get_recent_packets(&pool, 2).await.unwrap();
        assert_eq!(packets.len(), 2);
        // Newest first
        assert_eq!(packets[0].source, "WX0STA");
        assert_eq!(packets[1].source, "W1AW");
    }

    #[tokio::test]
    async fn test_upsert_station_insert() {
        let pool = test_db().await;

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.058), Some(-72.030), None, None, None, Some("Test"), Some("/"), Some(">"), None, "tnc").await.unwrap();

        let stations = get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "N0CALL");
        assert!((stations[0].lat.unwrap() - 49.058).abs() < 0.001);
        assert_eq!(stations[0].packet_count, 1);
        assert_eq!(stations[0].heard_via, "tnc");
        assert_eq!(stations[0].last_source_type, "tnc");
    }

    #[tokio::test]
    async fn test_upsert_station_update() {
        let pool = test_db().await;

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, Some("First"), Some("/"), Some(">"), None, "tnc").await.unwrap();
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.1), Some(-72.1), Some(60.0), None, None, Some("Second"), None, None, None, "tnc").await.unwrap();

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

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        upsert_station(&pool, "N0CALL", 0, "Message", None, None, None, None, None, None, None, None, None, "tnc").await.unwrap();

        let stations = get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert!(stations[0].lat.is_some());
        assert!((stations[0].lat.unwrap() - 49.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_get_stations_with_position() {
        let pool = test_db().await;

        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        upsert_station(&pool, "W1AW", 0, "Message", None, None, None, None, None, None, None, None, None, "tnc").await.unwrap();

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

        upsert_station(&pool, "OLD", 0, "Position", Some(40.0), Some(-74.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        sqlx::query("UPDATE stations SET last_heard = datetime('now', '-49 hours') WHERE callsign = 'OLD'")
            .execute(&pool).await.unwrap();

        upsert_station(&pool, "NEW", 0, "Position", Some(41.0), Some(-74.0), None, None, None, None, None, None, None, "tnc").await.unwrap();

        let deleted = cleanup_stale_stations(&pool, 48).await.unwrap();
        assert_eq!(deleted, 1);

        let stations = get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "NEW");
    }

    // === Position validation tests ===

    #[tokio::test]
    async fn test_get_station_track_excludes_zero_positions() {
        let pool = test_db().await;

        // Insert some valid and some (0,0) position history rows directly
        insert_position_history(&pool, "N1NCB", 3, 43.81, -69.94, None, None, None).await.unwrap();
        insert_position_history(&pool, "N1NCB", 3, 0.0, 0.0, None, None, None).await.unwrap();
        insert_position_history(&pool, "N1NCB", 3, 43.82, -69.95, None, None, None).await.unwrap();
        insert_position_history(&pool, "N1NCB", 3, 0.0, 0.0, None, None, None).await.unwrap();

        let track = get_station_track(&pool, "N1NCB", 3, 24).await.unwrap();
        assert_eq!(track.len(), 2, "track query should filter out (0,0) rows");
        assert!(track[0].lat > 40.0);
        assert!(track[1].lat > 40.0);
    }

    // === New source tracking tests ===

    #[tokio::test]
    async fn test_insert_packet_source_type() {
        let pool = test_db().await;
        insert_packet(&pool, "N0CALL", 0, "APRS", None, Some("Position"), "!4903.50N/07201.75W-", None, "tnc").await.unwrap();
        let packets = get_recent_packets(&pool, 10).await.unwrap();
        assert_eq!(packets[0].source_type, "tnc");
    }

    #[tokio::test]
    async fn test_upsert_station_heard_via_single() {
        let pool = test_db().await;
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        let s = get_station_by_callsign(&pool, "N0CALL", 0).await.unwrap().unwrap();
        assert_eq!(s.heard_via, "tnc");
    }

    #[tokio::test]
    async fn test_upsert_station_heard_via_both() {
        let pool = test_db().await;
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "aprs-is").await.unwrap();
        let s = get_station_by_callsign(&pool, "N0CALL", 0).await.unwrap().unwrap();
        assert_eq!(s.heard_via, "tnc,aprs-is");
    }

    #[tokio::test]
    async fn test_upsert_station_heard_via_no_duplicates() {
        let pool = test_db().await;
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        let s = get_station_by_callsign(&pool, "N0CALL", 0).await.unwrap().unwrap();
        assert_eq!(s.heard_via, "tnc");
    }

    #[tokio::test]
    async fn test_upsert_station_last_source_type() {
        let pool = test_db().await;
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "tnc").await.unwrap();
        let s = get_station_by_callsign(&pool, "N0CALL", 0).await.unwrap().unwrap();
        assert_eq!(s.last_source_type, "tnc");
        upsert_station(&pool, "N0CALL", 0, "Position", Some(49.0), Some(-72.0), None, None, None, None, None, None, None, "aprs-is").await.unwrap();
        let s = get_station_by_callsign(&pool, "N0CALL", 0).await.unwrap().unwrap();
        assert_eq!(s.last_source_type, "aprs-is");
    }

    // === Weather history tests ===

    #[tokio::test]
    async fn test_insert_weather_history() {
        let pool = test_db().await;
        let wx = WebWeather {
            temperature: Some(72),
            wind_speed: Some(10),
            wind_direction: Some(180),
            wind_gust: Some(15),
            humidity: Some(65),
            barometric_pressure: Some(10132),
            rain_last_hour: Some(0),
            rain_24h: Some(50),
            rain_since_midnight: None,
            luminosity: Some(500),
            snowfall: None,
        };
        insert_weather_history(&pool, "WX0STA", 0, &wx).await.unwrap();
        let history = get_weather_history(&pool, "WX0STA", 0, 24).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].temperature, Some(72));
        assert_eq!(history[0].wind_speed, Some(10));
        assert_eq!(history[0].barometric_pressure, Some(10132));
    }

    #[tokio::test]
    async fn test_get_weather_history_time_range() {
        let pool = test_db().await;
        let wx = WebWeather {
            temperature: Some(72), wind_speed: None, wind_direction: None, wind_gust: None,
            humidity: None, barometric_pressure: None, rain_last_hour: None, rain_24h: None,
            rain_since_midnight: None, luminosity: None, snowfall: None,
        };
        insert_weather_history(&pool, "WX0STA", 0, &wx).await.unwrap();
        // Age one entry beyond 6 hours
        sqlx::query("UPDATE weather_history SET recorded_at = datetime('now', '-7 hours') WHERE id = 1")
            .execute(&pool).await.unwrap();
        insert_weather_history(&pool, "WX0STA", 0, &wx).await.unwrap();

        let recent = get_weather_history(&pool, "WX0STA", 0, 6).await.unwrap();
        assert_eq!(recent.len(), 1);
        let all = get_weather_history(&pool, "WX0STA", 0, 24).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_cleanup_weather_history() {
        let pool = test_db().await;
        let wx = WebWeather {
            temperature: Some(72), wind_speed: None, wind_direction: None, wind_gust: None,
            humidity: None, barometric_pressure: None, rain_last_hour: None, rain_24h: None,
            rain_since_midnight: None, luminosity: None, snowfall: None,
        };
        insert_weather_history(&pool, "WX0STA", 0, &wx).await.unwrap();
        sqlx::query("UPDATE weather_history SET recorded_at = datetime('now', '-49 hours') WHERE id = 1")
            .execute(&pool).await.unwrap();
        insert_weather_history(&pool, "WX0STA", 0, &wx).await.unwrap();

        let deleted = cleanup_weather_history(&pool, 48).await.unwrap();
        assert_eq!(deleted, 1);
        let remaining = get_weather_history(&pool, "WX0STA", 0, 100).await.unwrap();
        assert_eq!(remaining.len(), 1);
    }

    // === Station packets test ===

    #[tokio::test]
    async fn test_get_station_packets() {
        let pool = test_db().await;
        insert_packet(&pool, "N0CALL", 0, "APRS", None, Some("Position"), "data1", None, "tnc").await.unwrap();
        insert_packet(&pool, "W1AW", 0, "APRS", None, Some("Position"), "data2", None, "aprs-is").await.unwrap();
        insert_packet(&pool, "N0CALL", 0, "APRS", None, Some("Position"), "data3", None, "aprs-is").await.unwrap();

        let pkts = get_station_packets(&pool, "N0CALL", 0, 50).await.unwrap();
        assert_eq!(pkts.len(), 2);
        // Newest first
        assert_eq!(pkts[0].raw_info, "data3");
        assert_eq!(pkts[0].source_type, "aprs-is");
        assert_eq!(pkts[1].raw_info, "data1");
        assert_eq!(pkts[1].source_type, "tnc");
    }
}
