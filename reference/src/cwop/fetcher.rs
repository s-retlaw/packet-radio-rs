use async_trait::async_trait;
use crate::source::{DataFetcher, FetchError};

/// All CWOP region codes (US states + international).
const US_REGIONS: &[&str] = &[
    "AK", "AL", "AR", "AZ", "CA", "CO", "CT", "DC", "DE", "FL",
    "GA", "GU", "HI", "IA", "ID", "IL", "IN", "KS", "KY", "LA",
    "MA", "MD", "ME", "MI", "MN", "MO", "MP", "MS", "MT", "NC",
    "ND", "NE", "NH", "NJ", "NM", "NV", "NY", "OH", "OK", "OR",
    "PA", "PR", "RI", "SC", "SD", "TN", "TX", "UT", "VA", "VI",
    "VT", "WA", "WI", "WV", "WY",
];

const INTL_REGIONS: &[(&str, &str)] = &[
    ("austria", "AT"),
    ("australia", "AU"),
    ("belgium", "BE"),
    ("canada", "CA-intl"),
    ("switzerland", "CH"),
    ("germany", "DE-intl"),
    ("denmark", "DK"),
    ("spain", "ES"),
    ("finland", "FI"),
    ("france", "FR"),
    ("greece", "GR"),
    ("italy", "IT"),
    ("japan", "JP"),
    ("mexico", "MX"),
    ("netherlands", "NL"),
    ("norway", "NO"),
    ("newzealand", "NZ"),
    ("poland", "PL"),
    ("portugal", "PT"),
    ("sweden", "SE"),
    ("unitedkingdom", "UK"),
    ("caribbean", "CRB"),
    ("southamerica", "SA"),
    ("countries_other", "OTH"),
];

/// Map a region identifier to the wxqa.com URL path segment.
/// US states use their abbreviation directly; international uses country name.
fn region_to_url_path(region: &str) -> &str {
    // Check if it's an international region name (already lowercase path)
    for (path, _code) in INTL_REGIONS {
        if region == *path || region.eq_ignore_ascii_case(path) {
            return path;
        }
    }
    // Otherwise it's a US state code, use as-is
    region
}

/// HTTP fetcher using reqwest.
pub struct HttpFetcher {
    client: reqwest::Client,
    delay: std::time::Duration,
}

impl HttpFetcher {
    pub fn new() -> Result<Self, reqwest::Error> {
        let mut builder = reqwest::Client::builder();

        // The CWOP source (wxqa.com) uses plain HTTP, so TLS cert validation
        // is not normally relevant. If a future source requires accepting
        // invalid certs (e.g., self-signed), set REFERENCE_ACCEPT_INVALID_CERTS=1.
        if std::env::var("REFERENCE_ACCEPT_INVALID_CERTS").is_ok_and(|v| v == "1") {
            builder = builder.danger_accept_invalid_certs(true);
        }

        Ok(Self {
            client: builder.build()?,
            delay: std::time::Duration::from_millis(200),
        })
    }

    pub fn with_delay(mut self, delay: std::time::Duration) -> Self {
        self.delay = delay;
        self
    }
}

#[async_trait]
impl DataFetcher for HttpFetcher {
    async fn fetch_region(&self, region: &str) -> Result<String, FetchError> {
        let path = region_to_url_path(region);
        let url = format!("http://www.wxqa.com/states/{}.html", path);

        // Rate limiting
        tokio::time::sleep(self.delay).await;

        let resp = self.client.get(&url).send().await.map_err(|e| FetchError::Http {
            region: region.to_string(),
            source: e,
        })?;

        if !resp.status().is_success() {
            return Err(FetchError::NotFound(format!(
                "HTTP {} for region {}",
                resp.status(),
                region
            )));
        }

        // The pages use iso-8859-1 encoding — reqwest may misdetect as UTF-8
        let bytes = resp.bytes().await.map_err(|e| FetchError::Http {
            region: region.to_string(),
            source: e,
        })?;

        // Try UTF-8 first, fall back to latin1
        match String::from_utf8(bytes.to_vec()) {
            Ok(s) => Ok(s),
            Err(_) => Ok(bytes.iter().map(|&b| b as char).collect()),
        }
    }

    fn regions(&self) -> Vec<String> {
        all_regions()
    }
}

/// Mock fetcher that reads from local fixture files.
pub struct MockFetcher {
    fixtures_dir: std::path::PathBuf,
}

impl MockFetcher {
    pub fn new(fixtures_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            fixtures_dir: fixtures_dir.into(),
        }
    }
}

#[async_trait]
impl DataFetcher for MockFetcher {
    async fn fetch_region(&self, region: &str) -> Result<String, FetchError> {
        let path = self.fixtures_dir.join(format!("{}.html", region));
        std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FetchError::NotFound(format!("No fixture for region: {}", region))
            } else {
                FetchError::Io(e)
            }
        })
    }

    fn regions(&self) -> Vec<String> {
        // Only return regions that have fixtures
        let mut regions = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.fixtures_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(region) = name.strip_suffix(".html") {
                    regions.push(region.to_string());
                }
            }
        }
        regions.sort();
        regions
    }
}

/// Get all region identifiers (US + international).
pub fn all_regions() -> Vec<String> {
    let mut regions: Vec<String> = US_REGIONS.iter().map(|s| s.to_string()).collect();
    for (path, _code) in INTL_REGIONS {
        regions.push(path.to_string());
    }
    regions
}

/// Get US region identifiers only.
pub fn us_regions() -> Vec<String> {
    US_REGIONS.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_regions_count() {
        let regions = all_regions();
        assert_eq!(regions.len(), 55 + 24); // 55 US + 24 international
    }

    #[test]
    fn test_region_to_url_path() {
        assert_eq!(region_to_url_path("ME"), "ME");
        assert_eq!(region_to_url_path("CA"), "CA");
        assert_eq!(region_to_url_path("canada"), "canada");
        assert_eq!(region_to_url_path("unitedkingdom"), "unitedkingdom");
    }

    #[tokio::test]
    async fn test_mock_fetcher() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures");
        let fetcher = MockFetcher::new(&fixtures);

        let html = fetcher.fetch_region("ME").await.unwrap();
        assert!(html.contains("MAINE APRSWXNET/CWOP MEMBERS"));

        let err = fetcher.fetch_region("NONEXISTENT").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_mock_fetcher_regions() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures");
        let fetcher = MockFetcher::new(&fixtures);

        let regions = fetcher.regions();
        assert!(regions.contains(&"ME".to_string()));
        assert!(regions.contains(&"CA".to_string()));
        assert!(regions.contains(&"canada".to_string()));
    }
}
