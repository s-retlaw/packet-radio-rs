use tracing::info;

use crate::error::{FccError, Result};

const CENSUS_BATCH_URL: &str =
    "https://geocoding.geo.census.gov/geocoder/locations/addressbatch";

/// Input address record for Census batch geocoding.
#[derive(Debug, Clone)]
pub struct AddressRecord {
    pub id: i64,
    pub street: String,
    pub city: String,
    pub state: String,
    pub zip: String,
}

/// Output geocode result from Census batch.
#[derive(Debug, Clone)]
pub struct CensusResult {
    pub id: i64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub quality: String,
}

/// Batch geocode addresses via Census Bureau API.
///
/// The Census Bureau accepts POST multipart form data with a CSV file
/// of addresses. Each line: `id,street,city,state,zip`
/// Returns CSV with: `id,input_address,match_status,match_type,matched_address,
///                     lon_lat,tiger_line_id,side`
///
/// Max 10,000 addresses per batch. Free, no API key required.
pub async fn batch_geocode(addresses: &[AddressRecord]) -> Result<Vec<CensusResult>> {
    if addresses.is_empty() {
        return Ok(Vec::new());
    }

    info!("Batch geocoding {} addresses via Census Bureau", addresses.len());

    // Build CSV content
    let mut csv_content = String::new();
    for addr in addresses {
        csv_content.push_str(&format!(
            "{},{},{},{},{}\n",
            addr.id,
            escape_csv(&addr.street),
            escape_csv(&addr.city),
            addr.state,
            addr.zip
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| FccError::Geocode(e.to_string()))?;

    let form = reqwest::multipart::Form::new()
        .text("benchmark", "Public_AR_Current")
        .part(
            "addressFile",
            reqwest::multipart::Part::bytes(csv_content.into_bytes())
                .file_name("addresses.csv")
                .mime_str("text/csv")
                .map_err(|e| FccError::Geocode(e.to_string()))?,
        );

    let resp = client
        .post(CENSUS_BATCH_URL)
        .multipart(form)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(FccError::Geocode(format!(
            "Census API returned HTTP {}",
            resp.status()
        )));
    }

    let body = resp.text().await?;
    parse_census_response(&body)
}

fn parse_census_response(body: &str) -> Result<Vec<CensusResult>> {
    let mut results = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse CSV — fields may be quoted
        let fields = parse_csv_line(line);
        if fields.is_empty() {
            continue;
        }

        let id = match fields[0].trim_matches('"').parse::<i64>() {
            Ok(id) => id,
            Err(_) => continue,
        };

        let match_status = fields.get(2).map(|s| s.trim_matches('"')).unwrap_or("");
        let match_type = fields.get(3).map(|s| s.trim_matches('"')).unwrap_or("");

        if match_status == "Match" {
            // lon/lat field is like "-72.7,41.7"
            if let Some(lonlat) = fields.get(5) {
                let lonlat = lonlat.trim_matches('"');
                let parts: Vec<&str> = lonlat.split(',').collect();
                if parts.len() == 2 {
                    if let (Ok(lon), Ok(lat)) =
                        (parts[0].trim().parse::<f64>(), parts[1].trim().parse::<f64>())
                    {
                        results.push(CensusResult {
                            id,
                            lat: Some(lat),
                            lon: Some(lon),
                            quality: match_type.to_string(),
                        });
                        continue;
                    }
                }
            }
        }

        results.push(CensusResult {
            id,
            lat: None,
            lon: None,
            quality: format!("No_Match ({})", match_status),
        });
    }

    Ok(results)
}

/// Simple CSV line parser that handles quoted fields.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

fn escape_csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_census_match() {
        let response = r#""100","225 MAIN ST, NEWINGTON, CT, 06111","Match","Exact","225 MAIN ST, NEWINGTON, CT, 06111","-72.72,41.69","12345","L"
"200","123 FAKE ST, NOWHERE, XX, 00000","No_Match","","","","",""
"#;
        let results = parse_census_response(response).unwrap();
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].id, 100);
        assert!((results[0].lat.unwrap() - 41.69).abs() < 0.01);
        assert!((results[0].lon.unwrap() - (-72.72)).abs() < 0.01);
        assert_eq!(results[0].quality, "Exact");

        assert_eq!(results[1].id, 200);
        assert!(results[1].lat.is_none());
    }

    #[test]
    fn test_escape_csv() {
        assert_eq!(escape_csv("hello"), "hello");
        assert_eq!(escape_csv("hello, world"), "\"hello, world\"");
    }

    #[test]
    fn test_parse_csv_line() {
        let fields = parse_csv_line("a,\"b,c\",d");
        assert_eq!(fields, vec!["a", "b,c", "d"]);
    }
}
