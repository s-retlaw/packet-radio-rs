use scraper::{Html, Selector};

/// CWOP station record parsed from HTML.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CwopStation {
    pub callsign: String,
    pub lat: f64,
    pub lon: f64,
    pub elevation_m: Option<f64>,
    pub city: Option<String>,
    pub region: String,
    pub nwsid: Option<String>,
}

/// Parse a CWOP state/region HTML page into station records.
///
/// This is a pure function — no I/O, fully testable with saved HTML fixtures.
/// Handles the quirky wxqa.com HTML format including malformed anchor tags
/// and variable column counts (US=11, international=12).
pub fn parse_state_page(html: &str, region: &str) -> Result<Vec<CwopStation>, ParseError> {
    let document = Html::parse_document(html);

    // Find the data table by looking for the header row with "Call/CW"
    let th_sel = Selector::parse("th.staffTableHeader").unwrap();
    let headers: Vec<String> = document
        .select(&th_sel)
        .map(|el| el.text().collect::<String>())
        .collect();

    if headers.is_empty() {
        return Ok(Vec::new()); // No table found — empty page
    }

    // Determine column indices based on headers.
    // US pages: 11 cols  [Call/CW, Town/City/Meta, Lat/Lon/Maps, Elev, ...]
    // International: 12 cols [Call/CW, Location/Meta, Lat/Lon/Map, Elev, ..., Weather Data, ..., Near Stns]
    let call_idx = 0;
    let city_idx = 1;
    let latlon_idx = 2;
    let elev_idx = 3;

    // NWSID comes from the QC column — find which column has "CWOP QC" header
    let qc_idx = headers
        .iter()
        .position(|h| h.contains("CWOP QC"))
        .unwrap_or(7); // Default for US pages

    let num_cols = headers.len();

    // Select all data rows
    let tr_sel = Selector::parse("tr").unwrap();
    let td_sel = Selector::parse("td.tblData").unwrap();

    let mut stations = Vec::new();

    for row in document.select(&tr_sel) {
        let cells: Vec<_> = row.select(&td_sel).collect();
        if cells.len() != num_cols {
            continue; // Skip header/footer rows
        }

        // Column 0: Callsign (plain text)
        let callsign = cells[call_idx].text().collect::<String>().trim().to_string();
        if callsign.is_empty() {
            continue;
        }

        // Column 1: City — extract text content.
        // Note: wxqa.com has malformed `<A href="..."<B>text</B></A>` tags where
        // html5ever treats `<B>` as an attribute, so we can't rely on finding <b> elements.
        // Instead, just collect all text content from the cell.
        let city = extract_cell_text(&cells[city_idx]);

        // Column 2: Lat/Lon (text: "44.48867 / -69.35")
        let latlon_text = cells[latlon_idx].text().collect::<String>();
        let latlon_text = latlon_text.trim();
        let (lat, lon) = match parse_latlon(latlon_text) {
            Some((lat, lon)) => (lat, lon),
            None => continue, // Skip rows with unparseable coordinates
        };

        // Sanity check coordinates
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            continue;
        }

        // Column 3: Elevation (plain text, meters)
        let elevation_m = cells[elev_idx]
            .text()
            .collect::<String>()
            .trim()
            .parse::<f64>()
            .ok();

        // QC column: NWSID (the station ID like "AP207")
        let nwsid = if qc_idx < cells.len() {
            extract_cell_text(&cells[qc_idx])
        } else {
            None
        };

        stations.push(CwopStation {
            callsign,
            lat,
            lon,
            elevation_m,
            city,
            region: region.to_string(),
            nwsid,
        });
    }

    Ok(stations)
}

/// Extract text content from a cell, returning None if empty/whitespace-only.
fn extract_cell_text(element: &scraper::ElementRef) -> Option<String> {
    let text: String = element.text().collect::<String>().trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Parse "lat / lon" text into (f64, f64).
fn parse_latlon(text: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let lat = parts[0].trim().parse::<f64>().ok()?;
    let lon = parts[1].trim().parse::<f64>().ok()?;
    Some((lat, lon))
}

/// Errors from parsing.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("HTML parsing failed: {0}")]
    Html(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e))
    }

    #[test]
    fn test_parse_maine_page() {
        let html = load_fixture("ME.html");
        let stations = parse_state_page(&html, "ME").unwrap();

        // ME should have ~137 stations
        assert!(
            stations.len() >= 100,
            "Expected 100+ ME stations, got {}",
            stations.len()
        );

        // Check first station (KD1KE)
        let kd1ke = stations.iter().find(|s| s.callsign == "KD1KE");
        assert!(kd1ke.is_some(), "KD1KE not found");
        let kd1ke = kd1ke.unwrap();
        assert!((kd1ke.lat - 44.489).abs() < 0.01);
        assert!((kd1ke.lon - (-69.35)).abs() < 0.01);
        assert_eq!(kd1ke.city.as_deref(), Some("Freedom"));
        assert!((kd1ke.elevation_m.unwrap() - 238.66).abs() < 0.01);
        assert_eq!(kd1ke.nwsid.as_deref(), Some("AP207"));
        assert_eq!(kd1ke.region, "ME");
    }

    #[test]
    fn test_parse_california_page() {
        let html = load_fixture("CA.html");
        let stations = parse_state_page(&html, "CA").unwrap();

        // CA is large — should have 900+ stations
        assert!(
            stations.len() >= 900,
            "Expected 900+ CA stations, got {}",
            stations.len()
        );

        // All should have valid lat/lon
        for s in &stations {
            assert!(
                (-90.0..=90.0).contains(&s.lat),
                "{}: lat {} out of range",
                s.callsign,
                s.lat
            );
            assert!(
                (-180.0..=180.0).contains(&s.lon),
                "{}: lon {} out of range",
                s.callsign,
                s.lon
            );
        }
    }

    #[test]
    fn test_parse_international() {
        let html = load_fixture("canada.html");
        let stations = parse_state_page(&html, "canada").unwrap();

        assert!(
            stations.len() >= 300,
            "Expected 300+ Canada stations, got {}",
            stations.len()
        );

        // Canadian stations should all be in northern hemisphere
        for s in &stations {
            assert!(s.region == "canada");
            assert!(s.lat > 0.0, "{}: lat {} should be positive", s.callsign, s.lat);
        }
    }

    #[test]
    fn test_parse_empty_page() {
        let html = "<html><body><table></table></body></html>";
        let stations = parse_state_page(html, "XX").unwrap();
        assert!(stations.is_empty());
    }

    #[test]
    fn test_parse_malformed_latlon() {
        // A page with a row that has bad lat/lon should skip that row
        let html = r#"<html><body>
        <table>
        <tr><th class="staffTableHeader">Call/CW</th>
        <th class="staffTableHeader">Town/City/Meta</th>
        <th class="staffTableHeader">Lat/Lon/Maps</th>
        <th class="staffTableHeader">Elev (m)</th>
        <th class="staffTableHeader">Weather Graphs</th>
        <th class="staffTableHeader">Near Stns</th>
        <th class="staffTableHeader">NOAA MesoMap</th>
        <th class="staffTableHeader">CWOP QC</th>
        <th class="staffTableHeader">Meso West</th>
        <th class="staffTableHeader">email to:</th>
        <th class="staffTableHeader">Web Sites</th></tr>
        <tr>
        <td class="tblData">GOOD1</td>
        <td class="tblData"><B>Town</B></td>
        <td class="tblData"><B>44.5 / -69.3</B></td>
        <td class="tblData">100</td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        <td class="tblData"><B>AP001</B></td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        </tr>
        <tr>
        <td class="tblData">BAD1</td>
        <td class="tblData"><B>Town</B></td>
        <td class="tblData"><B>not a number</B></td>
        <td class="tblData">100</td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        <td class="tblData"><B>AP002</B></td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        <td class="tblData"> </td>
        </tr>
        </table></body></html>"#;

        let stations = parse_state_page(html, "TEST").unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "GOOD1");
    }

    #[test]
    fn test_parse_station_with_ssid() {
        let html = load_fixture("ME.html");
        let stations = parse_state_page(&html, "ME").unwrap();

        // W1LH-12 has an SSID
        let w1lh = stations.iter().find(|s| s.callsign == "W1LH-12");
        assert!(w1lh.is_some(), "W1LH-12 not found");
    }

    #[test]
    fn test_latlon_parsing() {
        assert_eq!(parse_latlon("44.48867 / -69.35"), Some((44.48867, -69.35)));
        assert_eq!(parse_latlon("44.382 / 69.90133"), Some((44.382, 69.90133)));
        assert_eq!(parse_latlon("-33.8688 / 151.2093"), Some((-33.8688, 151.2093)));
        assert_eq!(parse_latlon("garbage"), None);
        assert_eq!(parse_latlon(""), None);
    }
}
