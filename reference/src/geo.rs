const EARTH_RADIUS_KM: f64 = 6371.0;

/// Haversine great-circle distance between two points in kilometers.
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let lat1_r = lat1.to_radians();
    let lat2_r = lat2.to_radians();

    let a = (dlat / 2.0).sin().powi(2) + lat1_r.cos() * lat2_r.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    EARTH_RADIUS_KM * c
}

/// A geographic range filter: center point + radius.
#[derive(Debug, Clone)]
pub struct RangeFilter {
    pub lat: f64,
    pub lon: f64,
    pub radius_km: f64,
}

impl RangeFilter {
    pub fn new(lat: f64, lon: f64, radius_km: f64) -> Self {
        Self {
            lat,
            lon,
            radius_km,
        }
    }

    /// Check if a point is within the filter radius.
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        haversine_km(self.lat, self.lon, lat, lon) <= self.radius_km
    }

    /// Quick bounding-box pre-filter (avoids expensive haversine for distant points).
    pub fn bbox_contains(&self, lat: f64, lon: f64) -> bool {
        let dlat = self.radius_km / 111.0; // ~111 km per degree latitude
        let dlon = self.radius_km / (111.0 * self.lat.to_radians().cos().max(0.01));
        if (lat - self.lat).abs() > dlat || (lon - self.lon).abs() > dlon {
            return false;
        }
        self.contains(lat, lon)
    }

    /// Parse an APRS-IS range filter string: `r/lat/lon/km`
    pub fn parse_aprs_is(filter_str: &str) -> Option<Self> {
        let parts: Vec<&str> = filter_str.split('/').collect();
        if parts.len() != 4 || parts[0] != "r" {
            return None;
        }
        let lat = parts[1].parse::<f64>().ok()?;
        let lon = parts[2].parse::<f64>().ok()?;
        let radius_km = parts[3].parse::<f64>().ok()?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) || radius_km <= 0.0 {
            return None;
        }
        Some(Self::new(lat, lon, radius_km))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_haversine_same_point() {
        assert!((haversine_km(45.0, -69.0, 45.0, -69.0) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_haversine_portland_boston() {
        // Portland ME (43.66, -70.26) to Boston (42.36, -71.06) ≈ 160 km
        let d = haversine_km(43.66, -70.26, 42.36, -71.06);
        assert!((d - 155.0).abs() < 10.0, "Portland-Boston: {d} km");
    }

    #[test]
    fn test_haversine_nyc_la() {
        // NYC (40.71, -74.01) to LA (34.05, -118.24) ≈ 3944 km
        let d = haversine_km(40.71, -74.01, 34.05, -118.24);
        assert!((d - 3944.0).abs() < 50.0, "NYC-LA: {d} km");
    }

    #[test]
    fn test_haversine_antipodal() {
        // North pole to south pole ≈ π × R ≈ 20015 km
        let d = haversine_km(90.0, 0.0, -90.0, 0.0);
        assert!((d - 20015.0).abs() < 100.0, "Poles: {d} km");
    }

    #[test]
    fn test_range_filter_contains() {
        let f = RangeFilter::new(43.66, -70.26, 200.0); // Portland ME, 200km
        assert!(f.contains(42.36, -71.06)); // Boston: ~155 km — inside
        assert!(!f.contains(40.71, -74.01)); // NYC: ~500 km — outside
    }

    #[test]
    fn test_range_filter_boundary() {
        let f = RangeFilter::new(0.0, 0.0, 111.2); // ~1 degree at equator
        assert!(f.contains(0.5, 0.5)); // Well inside
        assert!(!f.contains(2.0, 0.0)); // ~222 km, outside
    }

    #[test]
    fn test_bbox_contains() {
        let f = RangeFilter::new(43.66, -70.26, 200.0);
        assert!(f.bbox_contains(42.36, -71.06)); // Boston
        assert!(!f.bbox_contains(34.05, -118.24)); // LA — bbox rejects fast
    }

    #[test]
    fn test_parse_aprs_is_valid() {
        let f = RangeFilter::parse_aprs_is("r/42.36/-71.06/100").unwrap();
        assert!((f.lat - 42.36).abs() < 0.001);
        assert!((f.lon - (-71.06)).abs() < 0.001);
        assert!((f.radius_km - 100.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_aprs_is_invalid() {
        assert!(RangeFilter::parse_aprs_is("b/42/-71/100").is_none()); // wrong prefix
        assert!(RangeFilter::parse_aprs_is("r/42/-71").is_none()); // too few parts
        assert!(RangeFilter::parse_aprs_is("r/999/-71/100").is_none()); // lat out of range
        assert!(RangeFilter::parse_aprs_is("r/42/-71/0").is_none()); // zero radius
        assert!(RangeFilter::parse_aprs_is("r/42/-71/-50").is_none()); // negative radius
    }
}
