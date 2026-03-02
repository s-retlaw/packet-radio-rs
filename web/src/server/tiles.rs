use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::state::AppState;

/// Embedded US base map (placeholder — replace with extracted PMTiles).
/// To generate a real basemap:
/// ```bash
/// pmtiles extract https://build.protomaps.com/20260301.pmtiles web/assets/us-base.pmtiles \
///   --bbox=-125.0,24.0,-66.0,50.0 --maxzoom=8
/// ```
static EMBEDDED_US_BASE: &[u8] = include_bytes!("../../assets/us-base.pmtiles");

/// Parse a Range header value into (start, end) byte positions.
/// Supports: `bytes=START-END` and `bytes=START-`
pub fn parse_range_header(range_str: &str, file_size: u64) -> Option<(u64, u64)> {
    let range_str = range_str.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range_str.splitn(2, '-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start: u64 = parts[0].parse().ok()?;
    let end = if parts[1].is_empty() {
        file_size.saturating_sub(1)
    } else {
        parts[1].parse().ok()?
    };

    if start > end || start >= file_size {
        return None;
    }

    Some((start, end.min(file_size - 1)))
}

/// Check a path for directory traversal attacks.
pub fn is_safe_path(path: &str) -> bool {
    !path.contains("..") && !path.starts_with('/') && !path.contains('\\')
}

/// Serve bytes from an in-memory buffer with range-request support.
fn serve_bytes(data: &[u8], headers: &HeaderMap) -> Response {
    let file_size = data.len() as u64;

    if let Some(range_value) = headers.get(header::RANGE) {
        let range_str = match range_value.to_str() {
            Ok(s) => s,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid range header").into_response(),
        };

        if let Some((start, end)) = parse_range_header(range_str, file_size) {
            let length = end - start + 1;
            let buf = data[start as usize..=end as usize].to_vec();

            Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, file_size),
                )
                .header(header::CONTENT_LENGTH, length.to_string())
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(Body::from(buf))
                .unwrap()
                .into_response()
        } else {
            (StatusCode::RANGE_NOT_SATISFIABLE, "Invalid range").into_response()
        }
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, file_size.to_string())
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Body::from(data.to_vec()))
            .unwrap()
            .into_response()
    }
}

/// Serve a PMTiles file with range-request support.
/// Falls back to embedded US base map if `us-base.pmtiles` is not found on disk.
pub async fn tiles_handler(
    Path(path): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if !is_safe_path(&path) {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    let maps_dir = {
        let cfg = state.config.read().await;
        cfg.maps_dir.clone()
    };
    let file_path = std::path::Path::new(&maps_dir).join(&path);

    // Try filesystem first
    match tokio::fs::File::open(&file_path).await {
        Ok(mut file) => {
            let metadata = match file.metadata().await {
                Ok(m) => m,
                Err(_) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file")
                        .into_response()
                }
            };
            let file_size = metadata.len();

            if let Some(range_value) = headers.get(header::RANGE) {
                let range_str = match range_value.to_str() {
                    Ok(s) => s,
                    Err(_) => {
                        return (StatusCode::BAD_REQUEST, "Invalid range header").into_response()
                    }
                };

                if let Some((start, end)) = parse_range_header(range_str, file_size) {
                    let length = end - start + 1;
                    let mut buf = vec![0u8; length as usize];

                    if file
                        .seek(std::io::SeekFrom::Start(start))
                        .await
                        .is_err()
                    {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Seek failed").into_response();
                    }

                    if file.read_exact(&mut buf).await.is_err() {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Read failed").into_response();
                    }

                    Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(
                            header::CONTENT_RANGE,
                            format!("bytes {}-{}/{}", start, end, file_size),
                        )
                        .header(header::CONTENT_LENGTH, length.to_string())
                        .header(header::CONTENT_TYPE, "application/octet-stream")
                        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                        .body(Body::from(buf))
                        .unwrap()
                        .into_response()
                } else {
                    (StatusCode::RANGE_NOT_SATISFIABLE, "Invalid range").into_response()
                }
            } else {
                let mut buf = Vec::with_capacity(file_size as usize);
                if file.read_to_end(&mut buf).await.is_err() {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Read failed").into_response();
                }

                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(header::CONTENT_LENGTH, file_size.to_string())
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(Body::from(buf))
                    .unwrap()
                    .into_response()
            }
        }
        Err(_) => {
            // Fall back to embedded base map for us-base.pmtiles
            if path == "us-base.pmtiles" {
                serve_bytes(EMBEDDED_US_BASE, &headers)
            } else {
                (StatusCode::NOT_FOUND, "File not found").into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_basic() {
        assert_eq!(parse_range_header("bytes=0-99", 1024), Some((0, 99)));
    }

    #[test]
    fn test_parse_range_open_end() {
        assert_eq!(parse_range_header("bytes=100-", 1024), Some((100, 1023)));
    }

    #[test]
    fn test_parse_range_full_file() {
        assert_eq!(parse_range_header("bytes=0-1023", 1024), Some((0, 1023)));
    }

    #[test]
    fn test_parse_range_clamp() {
        // End beyond file size should be clamped
        assert_eq!(parse_range_header("bytes=0-9999", 1024), Some((0, 1023)));
    }

    #[test]
    fn test_parse_range_invalid() {
        assert_eq!(parse_range_header("bytes=500-100", 1024), None);
        assert_eq!(parse_range_header("bytes=2000-3000", 1024), None);
        assert_eq!(parse_range_header("invalid", 1024), None);
        assert_eq!(parse_range_header("bytes=abc-def", 1024), None);
    }

    #[test]
    fn test_is_safe_path() {
        assert!(is_safe_path("file.pmtiles"));
        assert!(is_safe_path("subdir/file.pmtiles"));
        assert!(!is_safe_path("../../../etc/passwd"));
        assert!(!is_safe_path("/absolute/path"));
        assert!(!is_safe_path("path\\with\\backslash"));
        assert!(!is_safe_path("sub/../escape"));
    }

    #[test]
    fn test_embedded_us_base_exists() {
        assert!(!EMBEDDED_US_BASE.is_empty());
        // PMTiles v3 magic: "PMTiles" + version 3
        assert_eq!(&EMBEDDED_US_BASE[..7], b"PMTiles");
        assert_eq!(EMBEDDED_US_BASE[7], 3);
    }

    #[test]
    fn test_serve_bytes_full() {
        let data = b"Hello, World!";
        let headers = HeaderMap::new();
        let response = serve_bytes(data, &headers);
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_serve_bytes_range() {
        let data = b"Hello, World!";
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=0-4".parse().unwrap());
        let response = serve_bytes(data, &headers);
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    }

    #[test]
    fn test_serve_bytes_invalid_range() {
        let data = b"Hello";
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=10-20".parse().unwrap());
        let response = serve_bytes(data, &headers);
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }
}
