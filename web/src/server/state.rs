use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use super::config::WebConfig;

/// Shared application state available to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub packet_tx: broadcast::Sender<String>,
    pub config: Arc<RwLock<WebConfig>>,
    /// Config file path for saving changes.
    pub config_path: String,
    /// Watch channel to notify background tasks of config changes.
    pub config_notify: Arc<tokio::sync::watch::Sender<()>>,
}
