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

    #[test]
    fn test_haversine_symmetry() {
        let d1 = haversine_km(43.66, -70.26, 42.36, -71.06);
        let d2 = haversine_km(42.36, -71.06, 43.66, -70.26);
        assert!((d1 - d2).abs() < 0.001, "Haversine should be symmetric: {d1} vs {d2}");
    }

    #[test]
    fn test_haversine_equator_crossing() {
        // Quito (-0.18, -78.47) to Bogota (4.71, -74.07) ≈ 710 km
        let d = haversine_km(-0.18, -78.47, 4.71, -74.07);
        assert!((d - 710.0).abs() < 50.0, "Quito-Bogota: {d} km");
    }

    #[test]
    fn test_haversine_dateline_crossing() {
        // Fiji (−17.7, 178.0) to Samoa (−13.8, −171.8) ≈ 1100 km
        let d = haversine_km(-17.7, 178.0, -13.8, -171.8);
        assert!((d - 1100.0).abs() < 100.0, "Fiji-Samoa across dateline: {d} km");
    }

    #[test]
    fn test_haversine_london_tokyo() {
        // London (51.51, -0.13) to Tokyo (35.68, 139.69) ≈ 9560 km
        let d = haversine_km(51.51, -0.13, 35.68, 139.69);
        assert!((d - 9560.0).abs() < 100.0, "London-Tokyo: {d} km");
    }

    #[test]
    fn test_range_filter_zero_distance() {
        let f = RangeFilter::new(45.0, -70.0, 1.0);
        assert!(f.contains(45.0, -70.0)); // Same point always inside
    }

    #[test]
    fn test_bbox_contains_near_pole() {
        // Near North Pole — longitude degrees are very short
        let f = RangeFilter::new(89.0, 0.0, 50.0);
        assert!(f.bbox_contains(89.0, 10.0)); // Should still work near pole
        assert!(f.bbox_contains(89.0, -10.0));
    }

    #[test]
    fn test_parse_aprs_is_boundary_values() {
        // Exact boundary lat/lon
        let f = RangeFilter::parse_aprs_is("r/90/180/1").unwrap();
        assert!((f.lat - 90.0).abs() < 0.001);
        assert!((f.lon - 180.0).abs() < 0.001);

        let f = RangeFilter::parse_aprs_is("r/-90/-180/0.1").unwrap();
        assert!((f.lat - (-90.0)).abs() < 0.001);
        assert!((f.lon - (-180.0)).abs() < 0.001);
    }

    #[test]
    fn test_parse_aprs_is_lon_out_of_range() {
        assert!(RangeFilter::parse_aprs_is("r/42/181/100").is_none());
        assert!(RangeFilter::parse_aprs_is("r/42/-181/100").is_none());
    }

    #[test]
    fn test_parse_aprs_is_extra_parts() {
        assert!(RangeFilter::parse_aprs_is("r/42/-71/100/extra").is_none());
    }

    #[test]
    fn test_parse_aprs_is_non_numeric() {
        assert!(RangeFilter::parse_aprs_is("r/abc/-71/100").is_none());
        assert!(RangeFilter::parse_aprs_is("r/42/def/100").is_none());
        assert!(RangeFilter::parse_aprs_is("r/42/-71/xyz").is_none());
    }

    #[test]
    fn test_parse_aprs_is_empty_string() {
        assert!(RangeFilter::parse_aprs_is("").is_none());
    }
}
