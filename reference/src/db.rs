use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::geo::RangeFilter;

/// Core position record — shared across all sources.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StationPosition {
    pub callsign: String,
    pub source: String,
    pub lat: f64,
    pub lon: f64,
}

/// Sync log entry.
#[derive(Debug, Clone)]
pub struct SyncEntry {
    pub source: String,
    pub region: Option<String>,
    pub synced_at: String,
    pub station_count: Option<i64>,
    pub duration_ms: Option<i64>,
}

/// The reference database — shared across all tools.
pub struct ReferenceDb {
    pool: SqlitePool,
    path: PathBuf,
}

impl ReferenceDb {
    /// Open (or create) a reference database at the given path.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        let path = path.as_ref().to_path_buf();

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let opts = SqliteConnectOptions::from_str(path.to_str().unwrap_or("reference.db"))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        let db = Self { pool, path };
        db.migrate().await?;
        Ok(db)
    }

    /// Open the reference database at the default XDG data directory.
    /// `~/.local/share/packet-radio/reference.db`
    pub async fn open_default() -> Result<Self, sqlx::Error> {
        let path = default_db_path();
        Self::open(&path).await
    }

    /// Open an in-memory database (for testing).
    pub async fn open_memory() -> Result<Self, sqlx::Error> {
        let opts = SqliteConnectOptions::from_str(":memory:")?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;

        let db = Self {
            pool,
            path: PathBuf::from(":memory:"),
        };
        db.migrate().await?;
        Ok(db)
    }

    /// Run schema migrations.
    async fn migrate(&self) -> Result<(), sqlx::Error> {
        let sql = include_str!("migrations/001_initial.sql");
        for statement in sql.split(';') {
            let trimmed = statement.trim();
            if !trimmed.is_empty() {
                sqlx::query(trimmed).execute(&self.pool).await?;
            }
        }
        Ok(())
    }

    /// Get the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get the database file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Look up a single callsign's position (any source).
    pub async fn lookup_position(&self, callsign: &str) -> Result<Option<StationPosition>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, f64, f64)>(
            "SELECT callsign, source, lat, lon FROM positions WHERE callsign = ?1 LIMIT 1",
        )
        .bind(callsign)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(callsign, source, lat, lon)| StationPosition {
            callsign,
            source,
            lat,
            lon,
        }))
    }

    /// Batch lookup positions for multiple callsigns.
    pub async fn lookup_positions_batch(
        &self,
        callsigns: &[&str],
    ) -> Result<Vec<StationPosition>, sqlx::Error> {
        if callsigns.is_empty() {
            return Ok(Vec::new());
        }

        // SQLite doesn't support array binding, so we build placeholders
        let placeholders: Vec<String> = (1..=callsigns.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT callsign, source, lat, lon FROM positions WHERE callsign IN ({})",
            placeholders.join(", ")
        );

        let mut query = sqlx::query_as::<_, (String, String, f64, f64)>(&sql);
        for cs in callsigns {
            query = query.bind(*cs);
        }

        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|(callsign, source, lat, lon)| StationPosition {
                callsign,
                source,
                lat,
                lon,
            })
            .collect())
    }

    /// Query positions within a geographic range.
    pub async fn query_positions_within(
        &self,
        filter: &RangeFilter,
    ) -> Result<Vec<StationPosition>, sqlx::Error> {
        // Use bounding box for initial SQL filter, then refine with haversine
        let dlat = filter.radius_km / 111.0;
        let dlon = filter.radius_km / (111.0 * filter.lat.to_radians().cos().max(0.01));

        let rows = sqlx::query_as::<_, (String, String, f64, f64)>(
            "SELECT callsign, source, lat, lon FROM positions \
             WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4",
        )
        .bind(filter.lat - dlat)
        .bind(filter.lat + dlat)
        .bind(filter.lon - dlon)
        .bind(filter.lon + dlon)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(callsign, source, lat, lon)| {
                if filter.contains(lat, lon) {
                    Some(StationPosition {
                        callsign,
                        source,
                        lat,
                        lon,
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    /// Record a sync event.
    pub async fn record_sync(
        &self,
        source: &str,
        region: Option<&str>,
        station_count: i64,
        duration_ms: i64,
    ) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO sync_log (source, region, synced_at, station_count, duration_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(source)
        .bind(region)
        .bind(&now)
        .bind(station_count)
        .bind(duration_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get the most recent sync entry for a source/region.
    pub async fn last_sync(
        &self,
        source: &str,
        region: Option<&str>,
    ) -> Result<Option<SyncEntry>, sqlx::Error> {
        let row = if let Some(region) = region {
            sqlx::query_as::<_, (String, Option<String>, String, Option<i64>, Option<i64>)>(
                "SELECT source, region, synced_at, station_count, duration_ms \
                 FROM sync_log WHERE source = ?1 AND region = ?2 \
                 ORDER BY synced_at DESC LIMIT 1",
            )
            .bind(source)
            .bind(region)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, (String, Option<String>, String, Option<i64>, Option<i64>)>(
                "SELECT source, region, synced_at, station_count, duration_ms \
                 FROM sync_log WHERE source = ?1 AND region IS NULL \
                 ORDER BY synced_at DESC LIMIT 1",
            )
            .bind(source)
            .fetch_optional(&self.pool)
            .await?
        };

        Ok(row.map(
            |(source, region, synced_at, station_count, duration_ms)| SyncEntry {
                source,
                region,
                synced_at,
                station_count,
                duration_ms,
            },
        ))
    }

    /// Count positions by source.
    pub async fn count_by_source(&self) -> Result<Vec<(String, i64)>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT source, COUNT(*) FROM positions GROUP BY source ORDER BY source",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Total position count.
    pub async fn total_count(&self) -> Result<i64, sqlx::Error> {
        let (count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM positions")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }
}

/// Default database path: `~/.local/share/packet-radio/reference.db`
pub fn default_db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("packet-radio")
        .join("reference.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_open_creates_tables() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let count = db.total_count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_upsert_and_lookup() {
        let db = ReferenceDb::open_memory().await.unwrap();

        // Insert a position directly
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind("KD1KE")
        .bind("cwop")
        .bind(44.489)
        .bind(-69.35)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        let pos = db.lookup_position("KD1KE").await.unwrap().unwrap();
        assert_eq!(pos.callsign, "KD1KE");
        assert_eq!(pos.source, "cwop");
        assert!((pos.lat - 44.489).abs() < 0.001);
        assert!((pos.lon - (-69.35)).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_upsert_updates() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        // Insert
        sqlx::query(
            "INSERT OR REPLACE INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind("TEST1")
        .bind("cwop")
        .bind(40.0)
        .bind(-70.0)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        // Update with new position
        sqlx::query(
            "INSERT OR REPLACE INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind("TEST1")
        .bind("cwop")
        .bind(41.0)
        .bind(-71.0)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        let pos = db.lookup_position("TEST1").await.unwrap().unwrap();
        assert!((pos.lat - 41.0).abs() < 0.001);

        // Should still be 1 row, not 2
        assert_eq!(db.total_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_batch_lookup() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        for (call, lat, lon) in [("A", 40.0, -70.0), ("B", 41.0, -71.0), ("C", 42.0, -72.0)] {
            sqlx::query(
                "INSERT INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(call)
            .bind("cwop")
            .bind(lat)
            .bind(lon)
            .bind(&now)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let results = db.lookup_positions_batch(&["A", "C", "MISSING"]).await.unwrap();
        assert_eq!(results.len(), 2);
        let calls: Vec<&str> = results.iter().map(|r| r.callsign.as_str()).collect();
        assert!(calls.contains(&"A"));
        assert!(calls.contains(&"C"));
    }

    #[tokio::test]
    async fn test_lookup_missing() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let pos = db.lookup_position("NONEXISTENT").await.unwrap();
        assert!(pos.is_none());
    }

    #[tokio::test]
    async fn test_geo_query() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        // Portland ME area
        for (call, lat, lon) in [
            ("NEAR1", 43.7, -70.3),  // Portland ME
            ("NEAR2", 43.5, -70.1),  // Nearby
            ("FAR1", 34.0, -118.2),  // Los Angeles
        ] {
            sqlx::query(
                "INSERT INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(call)
            .bind("cwop")
            .bind(lat)
            .bind(lon)
            .bind(&now)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let filter = RangeFilter::new(43.66, -70.26, 50.0); // 50km around Portland
        let results = db.query_positions_within(&filter).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_sync_log() {
        let db = ReferenceDb::open_memory().await.unwrap();

        db.record_sync("cwop", Some("ME"), 137, 500).await.unwrap();
        db.record_sync("cwop", Some("ME"), 140, 450).await.unwrap();

        let last = db.last_sync("cwop", Some("ME")).await.unwrap().unwrap();
        assert_eq!(last.station_count, Some(140));
        assert_eq!(last.duration_ms, Some(450));

        // No sync for a different region
        let none = db.last_sync("cwop", Some("CA")).await.unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn test_count_by_source() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        for (call, src) in [("A", "cwop"), ("B", "cwop"), ("C", "fcc")] {
            sqlx::query(
                "INSERT INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(call)
            .bind(src)
            .bind(40.0)
            .bind(-70.0)
            .bind(&now)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let counts = db.count_by_source().await.unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0], ("cwop".to_string(), 2));
        assert_eq!(counts[1], ("fcc".to_string(), 1));
    }

    #[tokio::test]
    async fn test_batch_lookup_empty() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let results = db.lookup_positions_batch(&[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_batch_lookup_all_missing() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let results = db.lookup_positions_batch(&["X", "Y", "Z"]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_geo_query_empty_results() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        // Insert a station in Maine
        sqlx::query(
            "INSERT INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind("MAINE1")
        .bind("cwop")
        .bind(43.66)
        .bind(-70.26)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        // Query in Los Angeles area — should find nothing
        let filter = RangeFilter::new(34.05, -118.24, 50.0);
        let results = db.query_positions_within(&filter).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_same_callsign_different_sources() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        // Same callsign, different source — should both exist (composite PK)
        for src in ["cwop", "fcc"] {
            sqlx::query(
                "INSERT INTO positions (callsign, source, lat, lon, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind("KD1KE")
            .bind(src)
            .bind(44.489)
            .bind(-69.35)
            .bind(&now)
            .execute(db.pool())
            .await
            .unwrap();
        }

        assert_eq!(db.total_count().await.unwrap(), 2);

        // lookup_position returns first match (LIMIT 1)
        let pos = db.lookup_position("KD1KE").await.unwrap();
        assert!(pos.is_some());
    }

    #[tokio::test]
    async fn test_sync_log_no_region() {
        let db = ReferenceDb::open_memory().await.unwrap();

        db.record_sync("cwop", None, 1000, 5000).await.unwrap();

        let last = db.last_sync("cwop", None).await.unwrap().unwrap();
        assert_eq!(last.source, "cwop");
        assert!(last.region.is_none());
        assert_eq!(last.station_count, Some(1000));
    }

    #[tokio::test]
    async fn test_sync_log_region_isolation() {
        let db = ReferenceDb::open_memory().await.unwrap();

        db.record_sync("cwop", Some("ME"), 137, 500).await.unwrap();
        db.record_sync("cwop", Some("CA"), 900, 2000).await.unwrap();

        let me = db.last_sync("cwop", Some("ME")).await.unwrap().unwrap();
        assert_eq!(me.station_count, Some(137));

        let ca = db.last_sync("cwop", Some("CA")).await.unwrap().unwrap();
        assert_eq!(ca.station_count, Some(900));

        // No-region query should find nothing (separate from per-region)
        let none = db.last_sync("cwop", None).await.unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn test_count_by_source_empty() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let counts = db.count_by_source().await.unwrap();
        assert!(counts.is_empty());
    }

    #[tokio::test]
    async fn test_default_db_path() {
        let path = default_db_path();
        assert!(path.to_str().unwrap().contains("reference.db"));
    }
}
