use std::fmt;

/// Server-level error type for the aprs-viewer application.
#[derive(Debug)]
pub enum ServerError {
    /// Database error from sqlx.
    Db(sqlx::Error),
    /// I/O error (filesystem, network).
    Io(std::io::Error),
    /// HTTP client error (map downloads).
    Http(reqwest::Error),
    /// JSON serialization/deserialization error.
    Json(serde_json::Error),
    /// Validation or business logic error.
    InvalidInput(String),
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerError::Db(e) => write!(f, "database error: {e}"),
            ServerError::Io(e) => write!(f, "I/O error: {e}"),
            ServerError::Http(e) => write!(f, "HTTP error: {e}"),
            ServerError::Json(e) => write!(f, "JSON error: {e}"),
            ServerError::InvalidInput(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ServerError::Db(e) => Some(e),
            ServerError::Io(e) => Some(e),
            ServerError::Http(e) => Some(e),
            ServerError::Json(e) => Some(e),
            ServerError::InvalidInput(_) => None,
        }
    }
}

impl From<sqlx::Error> for ServerError {
    fn from(e: sqlx::Error) -> Self {
        ServerError::Db(e)
    }
}

impl From<std::io::Error> for ServerError {
    fn from(e: std::io::Error) -> Self {
        ServerError::Io(e)
    }
}

impl From<reqwest::Error> for ServerError {
    fn from(e: reqwest::Error) -> Self {
        ServerError::Http(e)
    }
}

impl From<serde_json::Error> for ServerError {
    fn from(e: serde_json::Error) -> Self {
        ServerError::Json(e)
    }
}
