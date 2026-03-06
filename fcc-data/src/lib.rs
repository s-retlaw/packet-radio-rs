pub mod db;
pub mod download;
pub mod error;
pub mod geo;
pub mod geocode;
pub mod ingest;
pub mod models;
pub mod parse;
pub mod tui;

// Re-exports for library consumers
pub use db::{default_db_path, FccDb};
pub use models::{GeoQuery, LicenseRecord, SearchQuery};
