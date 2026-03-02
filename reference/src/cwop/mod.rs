pub mod db;
pub mod fetcher;
pub mod parser;

use crate::db::ReferenceDb;
use crate::source::DataFetcher;
use std::time::Duration;
use tracing::{info, warn};

/// CWOP data source: orchestrates fetch → parse → store.
pub struct CwopSource<F: DataFetcher> {
    fetcher: F,
    db: ReferenceDb,
}

impl<F: DataFetcher> CwopSource<F> {
    pub fn new(fetcher: F, db: ReferenceDb) -> Self {
        Self { fetcher, db }
    }

    /// Sync all available regions.
    pub async fn sync_all(&self) -> Result<SyncResult, CwopError> {
        let regions = self.fetcher.regions();
        let start = std::time::Instant::now();
        let mut total_stations = 0;
        let mut total_regions = 0;
        let mut errors = Vec::new();

        for region in &regions {
            match self.sync_region(region).await {
                Ok(count) => {
                    total_stations += count;
                    total_regions += 1;
                }
                Err(e) => {
                    warn!("Failed to sync region {}: {}", region, e);
                    errors.push((region.clone(), e.to_string()));
                }
            }
        }

        let duration = start.elapsed();
        self.db
            .record_sync("cwop", None, total_stations as i64, duration.as_millis() as i64)
            .await
            .map_err(CwopError::Db)?;

        info!(
            "CWOP sync complete: {} stations from {} regions in {:.1}s",
            total_stations,
            total_regions,
            duration.as_secs_f64()
        );

        Ok(SyncResult {
            total_stations,
            total_regions,
            duration,
            errors,
        })
    }

    /// Sync a single region.
    pub async fn sync_region(&self, region: &str) -> Result<usize, CwopError> {
        let start = std::time::Instant::now();

        let html = self
            .fetcher
            .fetch_region(region)
            .await
            .map_err(CwopError::Fetch)?;

        let stations =
            parser::parse_state_page(&html, region).map_err(CwopError::Parse)?;

        let count =
            db::upsert_cwop_stations(&self.db, &stations)
                .await
                .map_err(CwopError::Db)?;

        let duration = start.elapsed();
        self.db
            .record_sync("cwop", Some(region), count as i64, duration.as_millis() as i64)
            .await
            .map_err(CwopError::Db)?;

        info!("Synced CWOP region {}: {} stations in {:.1}s", region, count, duration.as_secs_f64());
        Ok(count)
    }

    /// Only sync if the last sync is older than `max_age`.
    pub async fn sync_if_stale(&self, max_age: Duration) -> Result<Option<SyncResult>, CwopError> {
        let last = self
            .db
            .last_sync("cwop", None)
            .await
            .map_err(CwopError::Db)?;

        if let Some(entry) = last {
            if let Ok(synced) = chrono::DateTime::parse_from_rfc3339(&entry.synced_at) {
                let age = chrono::Utc::now().signed_duration_since(synced);
                if age.to_std().unwrap_or(Duration::ZERO) < max_age {
                    info!(
                        "CWOP data is fresh (synced {} ago), skipping",
                        format_duration(age.to_std().unwrap_or(Duration::ZERO))
                    );
                    return Ok(None);
                }
            }
        }

        self.sync_all().await.map(Some)
    }

    /// Get a reference to the underlying database.
    pub fn db(&self) -> &ReferenceDb {
        &self.db
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Result of a full sync operation.
#[derive(Debug)]
pub struct SyncResult {
    pub total_stations: usize,
    pub total_regions: usize,
    pub duration: Duration,
    pub errors: Vec<(String, String)>,
}

/// Errors from CWOP operations.
#[derive(Debug, thiserror::Error)]
pub enum CwopError {
    #[error("Fetch error: {0}")]
    Fetch(#[from] crate::source::FetchError),

    #[error("Parse error: {0}")]
    Parse(#[from] parser::ParseError),

    #[error("Database error: {0}")]
    Db(sqlx::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cwop::fetcher::MockFetcher;
    use std::path::Path;

    fn fixtures_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
    }

    #[tokio::test]
    async fn test_sync_pipeline() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let fetcher = MockFetcher::new(fixtures_dir());
        let source = CwopSource::new(fetcher, db);

        let result = source.sync_all().await.unwrap();
        assert!(result.total_stations > 0);
        assert!(result.total_regions > 0);
        assert!(result.errors.is_empty());

        // Verify data in DB — count may be less than total_stations due to
        // duplicate callsigns across regions (DB deduplicates by callsign+source)
        let count = source.db().total_count().await.unwrap();
        assert!(count > 0);
        assert!(count as usize <= result.total_stations);
    }

    #[tokio::test]
    async fn test_sync_single_region() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let fetcher = MockFetcher::new(fixtures_dir());
        let source = CwopSource::new(fetcher, db);

        let count = source.sync_region("ME").await.unwrap();
        assert!(count >= 100);

        // Check a specific station
        let pos = source.db().lookup_position("KD1KE").await.unwrap();
        assert!(pos.is_some());
    }

    #[tokio::test]
    async fn test_sync_if_stale_skips_fresh() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let fetcher = MockFetcher::new(fixtures_dir());
        let source = CwopSource::new(fetcher, db);

        // First sync
        source.sync_all().await.unwrap();

        // Should skip — data is fresh
        let result = source
            .sync_if_stale(Duration::from_secs(3600))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_sync_if_stale_syncs_old() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let fetcher = MockFetcher::new(fixtures_dir());
        let source = CwopSource::new(fetcher, db);

        // First sync
        source.sync_all().await.unwrap();

        // Should sync — max_age is 0
        let result = source
            .sync_if_stale(Duration::from_secs(0))
            .await
            .unwrap();
        assert!(result.is_some());
    }
}
