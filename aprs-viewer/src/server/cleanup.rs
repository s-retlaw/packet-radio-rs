use sqlx::SqlitePool;

/// Run a single cleanup cycle.
pub async fn run_cleanup_once(pool: SqlitePool, station_max_hours: u32, track_max_hours: u32) {
    match super::db::cleanup_stale_stations(&pool, station_max_hours).await {
        Ok(n) if n > 0 => tracing::info!("Cleaned up {} stale stations", n),
        Err(e) => tracing::error!("Station cleanup error: {}", e),
        _ => {}
    }

    match super::db::cleanup_position_history(&pool, track_max_hours).await {
        Ok(n) if n > 0 => tracing::info!("Cleaned up {} old position history entries", n),
        Err(e) => tracing::error!("Position history cleanup error: {}", e),
        _ => {}
    }
}
