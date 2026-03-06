/// Errors from FCC data operations.
#[derive(Debug, thiserror::Error)]
pub enum FccError {
    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),

    #[error("Parse error: {field} in record {record_type}: {detail}")]
    Parse {
        record_type: String,
        field: String,
        detail: String,
    },

    #[error("Download error: {0}")]
    Download(String),

    #[error("Geocode error: {0}")]
    Geocode(String),
}

pub type Result<T> = std::result::Result<T, FccError>;
