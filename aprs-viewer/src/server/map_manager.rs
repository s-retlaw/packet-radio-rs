use crate::models::MapPack;
use super::error::ServerError;
use std::path::Path;

/// Allowed domains for map tile downloads.
const ALLOWED_DOWNLOAD_DOMAINS: &[&str] = &[
    "build.protomaps.com",
    "maps.protomaps.com",
    "r2-public.protomaps.com",
];

/// Check whether a URL points to an allowed download domain.
fn is_allowed_url(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    // Must be HTTPS
    if parsed.scheme() != "https" {
        return false;
    }

    match parsed.host_str() {
        Some(host) => ALLOWED_DOWNLOAD_DOMAINS.contains(&host),
        None => false,
    }
}

/// List installed PMTiles packs in the maps directory.
pub async fn list_installed_packs(
    maps_dir: &str,
) -> Result<Vec<MapPack>, ServerError> {
    let mut packs = Vec::new();
    let dir = Path::new(maps_dir);

    if !dir.exists() {
        return Ok(packs);
    }

    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "pmtiles" {
                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let metadata = entry.metadata().await?;
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                packs.push(MapPack {
                    id: filename.clone(),
                    name,
                    filename,
                    size_bytes: metadata.len(),
                    installed: true,
                });
            }
        }
    }

    packs.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(packs)
}

/// Delete a map pack file. Rejects paths with traversal characters.
pub async fn delete_pack(
    maps_dir: &str,
    filename: &str,
) -> Result<(), ServerError> {
    // Safety: reject traversal
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(ServerError::InvalidInput("Invalid filename".into()));
    }

    if !filename.ends_with(".pmtiles") {
        return Err(ServerError::InvalidInput("Can only delete .pmtiles files".into()));
    }

    let path = Path::new(maps_dir).join(filename);
    if !path.exists() {
        return Err(ServerError::InvalidInput("File not found".into()));
    }

    tokio::fs::remove_file(&path).await?;
    Ok(())
}

/// Download a map pack from a URL to the maps directory.
///
/// Only allows downloads from known tile server domains to prevent SSRF.
pub async fn download_pack(
    url: &str,
    filename: &str,
    maps_dir: &str,
) -> Result<(), ServerError> {
    // Safety: reject traversal
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(ServerError::InvalidInput("Invalid filename".into()));
    }

    if !filename.ends_with(".pmtiles") {
        return Err(ServerError::InvalidInput("Filename must end with .pmtiles".into()));
    }

    // SSRF protection: only allow known tile server domains
    if !is_allowed_url(url) {
        return Err(ServerError::InvalidInput(
            "URL not allowed: must be HTTPS from a known tile server domain".into(),
        ));
    }

    // Create maps dir if needed
    tokio::fs::create_dir_all(maps_dir).await?;

    let dest = Path::new(maps_dir).join(filename);

    // Download using reqwest
    let response = reqwest::get(url).await?;
    if !response.status().is_success() {
        return Err(ServerError::InvalidInput(format!("HTTP {}", response.status())));
    }

    let bytes = response.bytes().await?;
    tokio::fs::write(&dest, &bytes).await?;

    tracing::info!("Downloaded {} ({} bytes)", filename, bytes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_dir_with_files(files: &[(&str, &[u8])]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (name, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
        }
        dir
    }

    #[tokio::test]
    async fn test_list_installed_packs_empty_dir() {
        let dir = TempDir::new().unwrap();
        let packs = list_installed_packs(dir.path().to_str().unwrap())
            .await
            .unwrap();
        assert!(packs.is_empty());
    }

    #[tokio::test]
    async fn test_list_installed_packs_nonexistent_dir() {
        let packs = list_installed_packs("/nonexistent/path/maps")
            .await
            .unwrap();
        assert!(packs.is_empty());
    }

    #[tokio::test]
    async fn test_list_installed_packs_with_files() {
        let dir = create_temp_dir_with_files(&[
            ("us-base.pmtiles", b"PMTiles\x03fake"),
            ("northeast.pmtiles", b"PMTiles\x03data"),
            ("readme.txt", b"not a pmtiles file"),
        ]);
        let packs = list_installed_packs(dir.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(packs.len(), 2);
        assert!(packs.iter().any(|p| p.filename == "northeast.pmtiles"));
        assert!(packs.iter().any(|p| p.filename == "us-base.pmtiles"));
        // Non-pmtiles files ignored
        assert!(!packs.iter().any(|p| p.filename == "readme.txt"));
    }

    #[tokio::test]
    async fn test_delete_pack_success() {
        let dir = create_temp_dir_with_files(&[("test.pmtiles", b"data")]);
        delete_pack(dir.path().to_str().unwrap(), "test.pmtiles")
            .await
            .unwrap();
        assert!(!dir.path().join("test.pmtiles").exists());
    }

    #[tokio::test]
    async fn test_delete_pack_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let result = delete_pack(dir.path().to_str().unwrap(), "../escape.pmtiles").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));
    }

    #[tokio::test]
    async fn test_delete_pack_non_pmtiles_rejected() {
        let dir = create_temp_dir_with_files(&[("test.txt", b"data")]);
        let result = delete_pack(dir.path().to_str().unwrap(), "test.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_pack_not_found() {
        let dir = TempDir::new().unwrap();
        let result = delete_pack(dir.path().to_str().unwrap(), "missing.pmtiles").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_is_allowed_url_valid() {
        assert!(is_allowed_url("https://build.protomaps.com/20260301.pmtiles"));
        assert!(is_allowed_url("https://maps.protomaps.com/some/path.pmtiles"));
        assert!(is_allowed_url("https://r2-public.protomaps.com/file.pmtiles"));
    }

    #[test]
    fn test_is_allowed_url_rejects_http() {
        assert!(!is_allowed_url("http://build.protomaps.com/file.pmtiles"));
    }

    #[test]
    fn test_is_allowed_url_rejects_unknown_domain() {
        assert!(!is_allowed_url("https://evil.com/file.pmtiles"));
        assert!(!is_allowed_url("https://localhost/file.pmtiles"));
        assert!(!is_allowed_url("https://127.0.0.1/file.pmtiles"));
        assert!(!is_allowed_url("https://192.168.1.1/file.pmtiles"));
    }

    #[test]
    fn test_is_allowed_url_rejects_invalid() {
        assert!(!is_allowed_url("not-a-url"));
        assert!(!is_allowed_url(""));
    }

    #[tokio::test]
    async fn test_download_pack_ssrf_rejected() {
        let dir = TempDir::new().unwrap();
        let result = download_pack(
            "https://evil.com/malicious.pmtiles",
            "test.pmtiles",
            dir.path().to_str().unwrap(),
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("URL not allowed"));
    }
}
