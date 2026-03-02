use async_trait::async_trait;

/// Trait for data source fetching — separated from parsing and storage.
///
/// Implementations provide raw content (HTML, JSON, CSV, etc.) for a given
/// region identifier. The parser layer handles converting raw content to
/// typed records.
#[async_trait]
pub trait DataFetcher: Send + Sync {
    /// Fetch raw content for a region (returns HTML, JSON, CSV, whatever).
    async fn fetch_region(&self, region: &str) -> Result<String, FetchError>;

    /// List available regions.
    fn regions(&self) -> Vec<String>;
}

/// Errors that can occur during fetching.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("HTTP error for region '{region}': {source}")]
    Http {
        region: String,
        source: reqwest::Error,
    },

    #[error("Region not found: {0}")]
    NotFound(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
