use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::Result;
use crate::geo::haversine_km;
use crate::models::{GeoQuery, LicenseRecord, SearchQuery, SyncLogEntry};

/// Extract a LicenseRecord from a SqliteRow with the standard SELECT column order.
fn row_to_license(row: &sqlx::sqlite::SqliteRow) -> LicenseRecord {
    LicenseRecord {
        usi: row.get(0),
        call_sign: row.get(1),
        license_status: row.get(2),
        operator_class: row.get(3),
        first_name: row.get(4),
        last_name: row.get(5),
        entity_name: row.get(6),
        street_address: row.get(7),
        city: row.get(8),
        state: row.get(9),
        zip_code: row.get(10),
        grant_date: row.get(11),
        expired_date: row.get(12),
        previous_call_sign: row.get(13),
        lat: row.get(14),
        lon: row.get(15),
        geo_source: row.get(16),
        frn: row.get(17),
        licensee_id: row.get(18),
        mi: row.get(19),
        suffix: row.get(20),
        previous_operator_class: row.get(21),
        cancellation_date: row.get(22),
        last_action_date: row.get(23),
        radio_service_code: row.get(24),
        region_code: row.get(25),
        entity_type: row.get(26),
        geo_quality: row.get(27),
    }
}

/// Column list shared by all license queries. Must match `row_to_license` field order.
macro_rules! license_columns {
    () => {
        "h.usi, h.call_sign, h.license_status, COALESCE(a.operator_class, ''),
            COALESCE(e.first_name, ''), COALESCE(e.last_name, ''),
            COALESCE(e.entity_name, ''), COALESCE(e.street_address, ''),
            COALESCE(e.city, ''), COALESCE(e.state, ''), COALESCE(e.zip_code, ''),
            h.grant_date, h.expired_date, COALESCE(a.previous_call_sign, ''),
            g.lat, g.lon, g.geo_source,
            COALESCE(e.frn, ''), COALESCE(e.licensee_id, ''),
            COALESCE(e.mi, ''), COALESCE(e.suffix, ''),
            COALESCE(a.previous_operator_class, ''),
            h.cancellation_date, h.last_action_date,
            h.radio_service_code, COALESCE(a.region_code, ''),
            COALESCE(e.entity_type, ''), g.geo_quality"
    };
}

const LICENSE_SELECT: &str = concat!(
    "SELECT ", license_columns!(),
    " FROM hd h
     LEFT JOIN en e ON e.usi = h.usi
     LEFT JOIN am a ON a.usi = h.usi
     LEFT JOIN geocodes g ON g.usi = h.usi"
);

/// The FCC license database.
pub struct FccDb {
    pool: SqlitePool,
    path: PathBuf,
}

impl FccDb {
    /// Open (or create) the database at the given path.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let opts = SqliteConnectOptions::from_str(path.to_str().unwrap_or("fcc.db"))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .pragma("cache_size", "-64000") // 64MB cache
            .pragma("synchronous", "NORMAL");

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        let db = Self { pool, path };
        db.migrate().await?;
        Ok(db)
    }

    /// Open at the default path: `~/.local/share/packet-radio/fcc.db`
    pub async fn open_default() -> Result<Self> {
        Self::open(default_db_path()).await
    }

    /// Open an in-memory database (for testing).
    pub async fn open_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(":memory:")?
            .pragma("cache_size", "-64000")
            .pragma("synchronous", "NORMAL");

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

    async fn migrate(&self) -> Result<()> {
        let sql = include_str!("migrations/001_initial.sql");
        for statement in sql.split(';') {
            let trimmed = statement.trim();
            if !trimmed.is_empty() {
                sqlx::query(trimmed).execute(&self.pool).await?;
            }
        }

        // Migration 002: add fields (uses ALTER TABLE, ignore errors if columns already exist)
        let sql_002 = include_str!("migrations/002_add_fields.sql");
        for statement in sql_002.split(';') {
            let trimmed = statement.trim();
            if !trimmed.is_empty() {
                // Ignore "duplicate column" errors for idempotency
                let _ = sqlx::query(trimmed).execute(&self.pool).await;
            }
        }

        // Migration 003: add indexes for common query patterns
        let sql_003 = include_str!("migrations/003_add_indexes.sql");
        for statement in sql_003.split(';') {
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

    // ── Lookup queries ──────────────────────────────────────────────

    /// Look up a license by callsign. Returns the most recent active license.
    pub async fn lookup_callsign(&self, call_sign: &str) -> Result<Option<LicenseRecord>> {
        let sql = format!(
            "{LICENSE_SELECT}
             WHERE UPPER(h.call_sign) = UPPER(?1)
             ORDER BY h.license_status = 'A' DESC, h.grant_date DESC
             LIMIT 1"
        );

        let row = sqlx::query(&sql)
            .bind(call_sign)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(row_to_license))
    }

    /// Look up a license by USI.
    pub async fn lookup_usi(&self, usi: i64) -> Result<Option<LicenseRecord>> {
        let sql = format!("{LICENSE_SELECT} WHERE h.usi = ?1");
        let row = sqlx::query(&sql)
            .bind(usi)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(row_to_license))
    }

    /// Search licenses with flexible filters.
    pub async fn search(&self, query: &SearchQuery) -> Result<Vec<LicenseRecord>> {
        let mut conditions = Vec::new();
        let mut bind_values: Vec<String> = Vec::new();

        if let Some(ref cs) = query.call_sign {
            bind_values.push(format!("{}%", cs.to_uppercase()));
            conditions.push(format!("UPPER(h.call_sign) LIKE ?{}", bind_values.len()));
        }
        if let Some(ref name) = query.name {
            let pattern = format!("%{}%", name.to_uppercase());
            bind_values.push(pattern.clone());
            conditions.push(format!(
                "(UPPER(e.last_name) LIKE ?{n} OR UPPER(e.first_name) LIKE ?{n} OR UPPER(e.entity_name) LIKE ?{n})",
                n = bind_values.len()
            ));
        }
        if let Some(ref city) = query.city {
            bind_values.push(city.to_uppercase());
            conditions.push(format!("UPPER(e.city) = ?{}", bind_values.len()));
        }
        if let Some(ref state) = query.state {
            bind_values.push(state.to_uppercase());
            conditions.push(format!("UPPER(e.state) = ?{}", bind_values.len()));
        }
        if let Some(ref zip) = query.zip_code {
            bind_values.push(zip.clone());
            conditions.push(format!("e.zip_code LIKE ?{} || '%'", bind_values.len()));
        }
        if let Some(ref oc) = query.operator_class {
            bind_values.push(oc.to_uppercase());
            conditions.push(format!("UPPER(a.operator_class) = ?{}", bind_values.len()));
        }
        if let Some(ref status) = query.license_status {
            bind_values.push(status.to_uppercase());
            conditions.push(format!("UPPER(h.license_status) = ?{}", bind_values.len()));
        }

        let where_clause = if conditions.is_empty() {
            "1=1".to_string()
        } else {
            conditions.join(" AND ")
        };

        let limit = query.limit.unwrap_or(100);
        bind_values.push(limit.to_string());

        let sql = format!(
            "{LICENSE_SELECT}
             WHERE {where_clause}
             ORDER BY h.call_sign
             LIMIT ?{}", bind_values.len()
        );

        let mut q = sqlx::query(&sql);
        for val in &bind_values {
            q = q.bind(val);
        }

        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows.iter().map(row_to_license).collect())
    }

    /// Find stations near a geographic point.
    pub async fn stations_near(&self, query: &GeoQuery) -> Result<Vec<LicenseRecord>> {
        let dlat = query.radius_km / 111.0;
        let dlon = query.radius_km / (111.0 * query.lat.to_radians().cos().max(0.01));

        // Order by approximate squared distance so the SQL LIMIT returns the
        // closest rows rather than arbitrary ones from the bounding box.
        let sql = concat!(
            "SELECT ", license_columns!(),
            " FROM hd h
             JOIN geocodes g ON g.usi = h.usi
             LEFT JOIN en e ON e.usi = h.usi
             LEFT JOIN am a ON a.usi = h.usi
             WHERE g.lat BETWEEN ?1 AND ?2 AND g.lon BETWEEN ?3 AND ?4
             ORDER BY (g.lat - ?5) * (g.lat - ?5) + (g.lon - ?6) * (g.lon - ?6)
             LIMIT ?7"
        );

        let limit = query.limit.unwrap_or(100) as i64 * 2; // overfetch for haversine filter

        let rows = sqlx::query(sql)
            .bind(query.lat - dlat)
            .bind(query.lat + dlat)
            .bind(query.lon - dlon)
            .bind(query.lon + dlon)
            .bind(query.lat)
            .bind(query.lon)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        let actual_limit = query.limit.unwrap_or(100) as usize;

        Ok(rows.iter()
            .filter_map(|r| {
                let rec = row_to_license(r);
                let lat = rec.lat?;
                let lon = rec.lon?;
                if haversine_km(query.lat, query.lon, lat, lon) <= query.radius_km {
                    Some(rec)
                } else {
                    None
                }
            })
            .take(actual_limit)
            .collect())
    }

    // ── History ──────────────────────────────────────────────────────

    /// Get comments for a USI. Returns (date, comment, status_code).
    pub async fn get_comments(&self, usi: i64) -> Result<Vec<(String, String, String)>> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT comment_date, comment, COALESCE(status_code, '') FROM co WHERE usi = ?1 ORDER BY comment_date DESC"
        )
        .bind(usi)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Get history entries for a USI.
    pub async fn get_history(&self, usi: i64) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT log_date, code FROM hs WHERE usi = ?1 ORDER BY log_date DESC"
        )
        .bind(usi)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Find related licenses: same licensee_id (same person, different callsigns).
    /// Excludes the given USI from results.
    pub async fn related_by_licensee(&self, usi: i64) -> Result<Vec<LicenseRecord>> {
        let sql = format!(
            "{LICENSE_SELECT}
             WHERE e.licensee_id != '' AND e.licensee_id IN (
                 SELECT licensee_id FROM en WHERE usi = ?1
             ) AND h.usi != ?1
             ORDER BY h.license_status = 'A' DESC, h.grant_date DESC
             LIMIT 20"
        );

        let rows = sqlx::query(&sql)
            .bind(usi)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(row_to_license).collect())
    }

    /// Follow the previous_call_sign chain for a callsign.
    /// Returns the chain of (call_sign, operator_class, license_status, grant_date).
    pub async fn callsign_history_chain(&self, call_sign: &str) -> Result<Vec<(String, String, String, String)>> {
        let mut chain = Vec::new();
        let mut current = call_sign.to_uppercase();
        let mut seen = std::collections::HashSet::new();

        for _ in 0..20 {
            // Prevent infinite loops
            if !seen.insert(current.clone()) {
                break;
            }

            let row = sqlx::query_as::<_, (String, String, String, String)>(
                "SELECT h.call_sign, COALESCE(a.operator_class, ''), h.license_status, h.grant_date
                 FROM hd h
                 LEFT JOIN am a ON a.usi = h.usi
                 WHERE UPPER(a.previous_call_sign) = ?1
                 ORDER BY h.grant_date DESC
                 LIMIT 1"
            )
            .bind(&current)
            .fetch_optional(&self.pool)
            .await?;

            match row {
                Some(r) => {
                    current = r.0.clone();
                    chain.push(r);
                }
                None => break,
            }
        }

        chain.reverse(); // oldest first
        Ok(chain)
    }

    /// Look up who previously held a callsign (the record whose call_sign matches
    /// and whose previous_call_sign points elsewhere).
    pub async fn previous_holders(&self, call_sign: &str) -> Result<Vec<(String, String, String, String)>> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT h.call_sign, COALESCE(a.previous_call_sign, ''), h.license_status, h.grant_date
             FROM hd h
             LEFT JOIN am a ON a.usi = h.usi
             WHERE UPPER(h.call_sign) = UPPER(?1)
             ORDER BY h.grant_date DESC"
        )
        .bind(call_sign)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ── Statistics ───────────────────────────────────────────────────

    /// Count rows in each main table (single query).
    pub async fn table_counts(&self) -> Result<Vec<(String, i64)>> {
        let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
            "SELECT
                (SELECT COUNT(*) FROM hd),
                (SELECT COUNT(*) FROM en),
                (SELECT COUNT(*) FROM am),
                (SELECT COUNT(*) FROM hs),
                (SELECT COUNT(*) FROM co),
                (SELECT COUNT(*) FROM geocodes),
                (SELECT COUNT(*) FROM zip_centroids)"
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(vec![
            ("hd".to_string(), row.0),
            ("en".to_string(), row.1),
            ("am".to_string(), row.2),
            ("hs".to_string(), row.3),
            ("co".to_string(), row.4),
            ("geocodes".to_string(), row.5),
            ("zip_centroids".to_string(), row.6),
        ])
    }

    /// Count active licenses by operator class.
    pub async fn count_by_class(&self) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT COALESCE(a.operator_class, '?'), COUNT(*)
             FROM hd h LEFT JOIN am a ON a.usi = h.usi
             WHERE h.license_status = 'A'
             GROUP BY a.operator_class ORDER BY COUNT(*) DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Count geocoded vs non-geocoded active licenses (single query).
    pub async fn geocode_stats(&self) -> Result<(i64, i64, i64)> {
        let row = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT
                (SELECT COUNT(*) FROM hd WHERE license_status = 'A'),
                (SELECT COUNT(*) FROM hd h JOIN geocodes g ON g.usi = h.usi WHERE h.license_status = 'A'),
                (SELECT COUNT(*) FROM geocodes WHERE geo_stale = 1)"
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    // ── Sync log ─────────────────────────────────────────────────────

    /// Start a sync log entry (returns the ID).
    pub async fn start_sync_log(&self, sync_type: &str) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO fcc_sync_log (sync_type, started_at, status) VALUES (?1, ?2, 'running')"
        )
        .bind(sync_type)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Finish a sync log entry.
    pub async fn finish_sync_log(
        &self,
        log_id: i64,
        status: &str,
        records: Option<i64>,
        error: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE fcc_sync_log SET finished_at = ?1, status = ?2, records_processed = ?3, error_message = ?4
             WHERE id = ?5"
        )
        .bind(&now)
        .bind(status)
        .bind(records)
        .bind(error)
        .bind(log_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get sync log history.
    pub async fn sync_history(&self, limit: i64) -> Result<Vec<SyncLogEntry>> {
        let rows = sqlx::query_as::<_, (i64, String, String, Option<String>, String, Option<i64>, Option<String>)>(
            "SELECT id, sync_type, started_at, finished_at, status, records_processed, error_message
             FROM fcc_sync_log ORDER BY id DESC LIMIT ?1"
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| SyncLogEntry {
            id: r.0, sync_type: r.1, started_at: r.2, finished_at: r.3,
            status: r.4, records_processed: r.5, error_message: r.6,
        }).collect())
    }

    /// Get the timestamp of the last successful sync (any type).
    pub async fn last_successful_sync(&self) -> Result<Option<String>> {
        let row = sqlx::query_as::<_, (String,)>(
            "SELECT started_at FROM fcc_sync_log
             WHERE status = 'completed'
             ORDER BY id DESC LIMIT 1"
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    // ── Geocode helpers ──────────────────────────────────────────────

    /// Get distinct addresses needing geocoding.
    ///
    /// Returns `(street, city, state, zip)` — unique addresses only.
    pub async fn addresses_needing_geocode(&self) -> Result<Vec<(String, String, String, String)>> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT DISTINCT COALESCE(e.street_address, ''), COALESCE(e.city, ''),
                    COALESCE(e.state, ''), COALESCE(e.zip_code, '')
             FROM hd h
             JOIN en e ON e.usi = h.usi
             LEFT JOIN geocodes g ON g.usi = h.usi
             WHERE h.license_status = 'A'
               AND (g.usi IS NULL OR g.geo_stale = 1)
               AND e.street_address != ''
               AND e.city != ''
               AND e.state != ''"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Apply a geocode result to ALL records matching an address (any status).
    /// The discovery query (addresses_needing_geocode) uses active-only to drive
    /// Census lookups, but once we have a result we apply it everywhere.
    pub async fn geocode_by_address(
        &self,
        street: &str,
        city: &str,
        state: &str,
        zip: &str,
        lat: f64,
        lon: f64,
        source: &str,
        quality: &str,
    ) -> Result<u64> {
        let result = sqlx::query(
            "INSERT OR REPLACE INTO geocodes (usi, lat, lon, geo_source, geo_quality, geo_stale)
             SELECT e.usi, ?5, ?6, ?7, ?8, 0
             FROM en e
             WHERE e.street_address = ?1 AND e.city = ?2 AND e.state = ?3 AND e.zip_code = ?4"
        )
        .bind(street)
        .bind(city)
        .bind(state)
        .bind(zip)
        .bind(lat)
        .bind(lon)
        .bind(source)
        .bind(quality)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Look up an existing non-stale geocode for any record at the same address.
    /// Returns `Some((lat, lon, source, quality))` if a match exists locally.
    pub async fn lookup_geocode_by_address(
        &self,
        street: &str,
        city: &str,
        state: &str,
        zip: &str,
    ) -> Result<Option<(f64, f64, String, String)>> {
        let row = sqlx::query_as::<_, (f64, f64, String, String)>(
            "SELECT g.lat, g.lon, g.geo_source, g.geo_quality
             FROM en e
             JOIN geocodes g ON g.usi = e.usi AND g.geo_stale = 0
             WHERE e.street_address = ?1 AND e.city = ?2 AND e.state = ?3 AND e.zip_code = ?4
             LIMIT 1",
        )
        .bind(street)
        .bind(city)
        .bind(state)
        .bind(zip)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Upsert a geocode result.
    pub async fn upsert_geocode(
        &self,
        usi: i64,
        lat: f64,
        lon: f64,
        source: &str,
        quality: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO geocodes (usi, lat, lon, geo_source, geo_quality, geo_stale)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)
             ON CONFLICT(usi) DO UPDATE SET lat=?2, lon=?3, geo_source=?4, geo_quality=?5, geo_stale=0"
        )
        .bind(usi)
        .bind(lat)
        .bind(lon)
        .bind(source)
        .bind(quality)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Bulk upsert ZIP centroids.
    pub async fn upsert_zip_centroids(&self, centroids: &[(String, f64, f64)]) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        let mut count = 0;
        for (zip, lat, lon) in centroids {
            sqlx::query(
                "INSERT INTO zip_centroids (zip, lat, lon) VALUES (?1, ?2, ?3)
                 ON CONFLICT(zip) DO UPDATE SET lat=?2, lon=?3"
            )
            .bind(zip)
            .bind(lat)
            .bind(lon)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }
        tx.commit().await?;
        Ok(count)
    }

    /// Look up ZIP centroid.
    pub async fn lookup_zip_centroid(&self, zip: &str) -> Result<Option<(f64, f64)>> {
        // Use first 5 digits
        let zip5 = if zip.len() >= 5 { &zip[..5] } else { zip };
        let row = sqlx::query_as::<_, (f64, f64)>(
            "SELECT lat, lon FROM zip_centroids WHERE zip = ?1"
        )
        .bind(zip5)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Mark address-change records as stale for re-geocoding.
    pub async fn mark_all_geocodes_stale(&self) -> Result<u64> {
        // Mark geocodes stale where EN address has changed
        // (This is called after an EN upsert batch)
        let result = sqlx::query(
            "UPDATE geocodes SET geo_stale = 1
             WHERE usi IN (
                SELECT g.usi FROM geocodes g
                JOIN en e ON e.usi = g.usi
                WHERE g.geo_stale = 0
             )"
        )
        .execute(&self.pool)
        .await?;
        // Note: This is a simple approach. A more precise version would
        // compare old vs new address, but for a full re-sync we just
        // re-geocode everything anyway.
        Ok(result.rows_affected())
    }

    /// Mark specific USIs as needing re-geocoding.
    pub async fn mark_geocodes_stale_batch(&self, usis: &[i64]) -> Result<u64> {
        if usis.is_empty() {
            return Ok(0);
        }
        let mut total = 0u64;
        for chunk in usis.chunks(999) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "UPDATE geocodes SET geo_stale = 1 WHERE usi IN ({})",
                placeholders.join(", ")
            );
            let mut q = sqlx::query(&sql);
            for usi in chunk {
                q = q.bind(usi);
            }
            let result = q.execute(&self.pool).await?;
            total += result.rows_affected();
        }
        Ok(total)
    }

    /// Count rows in the zip_centroids table.
    pub async fn zip_centroid_count(&self) -> Result<i64> {
        let (count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM zip_centroids")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }
}

/// Default database path: `~/.local/share/packet-radio/fcc.db`
pub fn default_db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("packet-radio")
        .join("fcc.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_open_memory() {
        let db = FccDb::open_memory().await.unwrap();
        let counts = db.table_counts().await.unwrap();
        for (table, count) in &counts {
            assert_eq!(*count, 0, "Table {} should be empty", table);
        }
    }

    #[tokio::test]
    async fn test_insert_and_lookup() {
        let db = FccDb::open_memory().await.unwrap();

        // Insert HD + EN + AM records
        sqlx::query("INSERT INTO hd (usi, call_sign, license_status, grant_date, expired_date) VALUES (1, 'W1AW', 'A', '2020-01-01', '2030-01-01')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO en (usi, first_name, last_name, entity_name, street_address, city, state, zip_code) VALUES (1, 'ARRL', '', 'ARRL INC', '225 MAIN ST', 'NEWINGTON', 'CT', '06111')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO am (usi, operator_class) VALUES (1, 'E')")
            .execute(db.pool()).await.unwrap();

        let license = db.lookup_callsign("W1AW").await.unwrap().unwrap();
        assert_eq!(license.call_sign, "W1AW");
        assert_eq!(license.city, "NEWINGTON");
        assert_eq!(license.operator_class, "E");
    }

    #[tokio::test]
    async fn test_search_by_state() {
        let db = FccDb::open_memory().await.unwrap();

        sqlx::query("INSERT INTO hd (usi, call_sign, license_status) VALUES (1, 'W1AW', 'A'), (2, 'K1ABC', 'A'), (3, 'W3XYZ', 'A')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO en (usi, first_name, last_name, city, state) VALUES (1, 'A', 'B', 'NEWINGTON', 'CT'), (2, 'C', 'D', 'HARTFORD', 'CT'), (3, 'E', 'F', 'PHILA', 'PA')")
            .execute(db.pool()).await.unwrap();

        let results = db.search(&SearchQuery {
            state: Some("CT".to_string()),
            ..Default::default()
        }).await.unwrap();

        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_sync_log() {
        let db = FccDb::open_memory().await.unwrap();
        let log_id = db.start_sync_log("full").await.unwrap();
        db.finish_sync_log(log_id, "completed", Some(1000), None).await.unwrap();

        let history = db.sync_history(10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "completed");
        assert_eq!(history[0].records_processed, Some(1000));
    }

    #[tokio::test]
    async fn test_zip_centroids() {
        let db = FccDb::open_memory().await.unwrap();
        let centroids = vec![
            ("06111".to_string(), 41.7, -72.7),
            ("04101".to_string(), 43.6, -70.2),
        ];
        let count = db.upsert_zip_centroids(&centroids).await.unwrap();
        assert_eq!(count, 2);

        let (lat, lon) = db.lookup_zip_centroid("06111").await.unwrap().unwrap();
        assert!((lat - 41.7).abs() < 0.001);
        assert!((lon - (-72.7)).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_geo_query() {
        let db = FccDb::open_memory().await.unwrap();

        sqlx::query("INSERT INTO hd (usi, call_sign, license_status) VALUES (1, 'W1AW', 'A')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO en (usi, city, state) VALUES (1, 'NEWINGTON', 'CT')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO geocodes (usi, lat, lon, geo_source, geo_quality) VALUES (1, 41.7, -72.7, 'census', 'Exact')")
            .execute(db.pool()).await.unwrap();

        let results = db.stations_near(&GeoQuery {
            lat: 41.7, lon: -72.7, radius_km: 10.0, limit: Some(10),
        }).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_sign, "W1AW");

        // Far away — no results
        let results = db.stations_near(&GeoQuery {
            lat: 0.0, lon: 0.0, radius_km: 10.0, limit: Some(10),
        }).await.unwrap();
        assert_eq!(results.len(), 0);
    }
}
