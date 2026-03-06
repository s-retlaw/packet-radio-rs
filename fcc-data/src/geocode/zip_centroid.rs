use std::io::{Cursor, Read};
use std::path::Path;

use tracing::info;

use crate::db::FccDb;
use crate::error::Result;

/// Census Bureau ZCTA Gazetteer URL (~800KB ZIP).
const GAZETTEER_URL: &str =
    "https://www2.census.gov/geo/docs/maps-data/data/gazetteer/2024_Gazetteer/2024_Gaz_zcta_national.zip";

/// Ensure the zip_centroids table has data.
///
/// If empty, downloads the Census Bureau ZCTA Gazetteer file and loads it.
/// If already populated, returns immediately.
pub async fn ensure_loaded(db: &FccDb) -> Result<()> {
    let count = db.zip_centroid_count().await?;
    if count > 0 {
        info!("ZIP centroids already loaded ({} entries)", count);
        return Ok(());
    }

    println!("Downloading ZIP centroids from Census Bureau...");
    let zip_bytes = reqwest::get(GAZETTEER_URL).await?.bytes().await?;

    let centroids = parse_gazetteer_zip(&zip_bytes)?;
    let loaded = db.upsert_zip_centroids(&centroids).await?;
    println!("Loaded {} ZIP centroids from Census Gazetteer", loaded);

    Ok(())
}

/// Parse the Census Bureau Gazetteer ZIP file.
///
/// The ZIP contains a single tab-delimited file with columns:
/// GEOID, ..., INTPTLAT, INTPTLONG (last two columns)
fn parse_gazetteer_zip(zip_bytes: &[u8]) -> Result<Vec<(String, f64, f64)>> {
    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)?;

    // Find the first .txt file in the archive
    let mut contents = String::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.name().ends_with(".txt") {
            file.read_to_string(&mut contents)?;
            break;
        }
    }

    parse_gazetteer_text(&contents)
}

/// Parse tab-delimited Census Gazetteer text.
///
/// Format: GEOID\tALAND\tAWATER\tALAND_SQMI\tAWATER_SQMI\tINTPTLAT\tINTPTLONG
/// First line is header.
fn parse_gazetteer_text(text: &str) -> Result<Vec<(String, f64, f64)>> {
    let mut centroids = Vec::new();
    let mut lines = text.lines();

    // Skip header
    let header = lines.next().unwrap_or("");

    // Find column indices from header
    let cols: Vec<&str> = header.split('\t').map(|s| s.trim()).collect();
    let geoid_idx = cols.iter().position(|c| *c == "GEOID").unwrap_or(0);
    let lat_idx = cols.iter().position(|c| *c == "INTPTLAT").unwrap_or(cols.len() - 2);
    let lon_idx = cols.iter().position(|c| *c == "INTPTLONG").unwrap_or(cols.len() - 1);

    for line in lines {
        let fields: Vec<&str> = line.split('\t').map(|s| s.trim()).collect();

        let zip = match fields.get(geoid_idx) {
            Some(z) if z.len() >= 5 => z[..5].to_string(),
            _ => continue,
        };

        let lat: f64 = match fields.get(lat_idx).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };

        let lon: f64 = match fields.get(lon_idx).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };

        centroids.push((zip, lat, lon));
    }

    Ok(centroids)
}

/// Load ZIP code centroids from a SimpleMaps-format CSV.
///
/// Expected columns: zip, lat, lng, city, state_id, ...
/// We only need zip, lat, lng.
pub async fn load_csv(db: &FccDb, csv_path: &Path) -> Result<usize> {
    info!("Loading ZIP centroids from {}", csv_path.display());

    let mut reader = csv::Reader::from_path(csv_path)?;

    let mut centroids = Vec::new();
    for result in reader.records() {
        let record = result?;
        let zip = record.get(0).unwrap_or("").to_string();
        let lat: f64 = match record.get(1).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let lng: f64 = match record.get(2).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };

        if zip.len() >= 5 {
            centroids.push((zip[..5].to_string(), lat, lng));
        }
    }

    let count = db.upsert_zip_centroids(&centroids).await?;
    info!("Loaded {} ZIP centroids", count);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_load_csv() {
        let db = FccDb::open_memory().await.unwrap();

        // Write a temp CSV
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmpfile, "zip,lat,lng,city,state_id").unwrap();
        writeln!(tmpfile, "06111,41.6959,-72.7249,Newington,CT").unwrap();
        writeln!(tmpfile, "04101,43.6591,-70.2568,Portland,ME").unwrap();

        let count = load_csv(&db, tmpfile.path()).await.unwrap();
        assert_eq!(count, 2);

        let (lat, lon) = db.lookup_zip_centroid("06111").await.unwrap().unwrap();
        assert!((lat - 41.6959).abs() < 0.001);
        assert!((lon - (-72.7249)).abs() < 0.001);
    }

    #[test]
    fn test_parse_gazetteer_text() {
        let text = "GEOID\tALAND\tAWATER\tALAND_SQMI\tAWATER_SQMI\tINTPTLAT\tINTPTLONG\n\
                    06111\t1234\t567\t0.5\t0.2\t41.6959\t-72.7249\n\
                    04101\t2345\t678\t0.6\t0.3\t43.6591\t-70.2568\n";

        let centroids = parse_gazetteer_text(text).unwrap();
        assert_eq!(centroids.len(), 2);
        assert_eq!(centroids[0].0, "06111");
        assert!((centroids[0].1 - 41.6959).abs() < 0.001);
        assert!((centroids[0].2 - (-72.7249)).abs() < 0.001);
        assert_eq!(centroids[1].0, "04101");
    }

    #[tokio::test]
    async fn test_ensure_loaded_skips_when_populated() {
        let db = FccDb::open_memory().await.unwrap();

        // Pre-populate
        let centroids = vec![("06111".to_string(), 41.7, -72.7)];
        db.upsert_zip_centroids(&centroids).await.unwrap();

        // Should return Ok without downloading
        ensure_loaded(&db).await.unwrap();

        // Still just one entry
        assert_eq!(db.zip_centroid_count().await.unwrap(), 1);
    }
}
