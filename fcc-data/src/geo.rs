const EARTH_RADIUS_KM: f64 = 6371.0;

/// Haversine great-circle distance between two points in kilometers.
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let lat1_r = lat1.to_radians();
    let lat2_r = lat2.to_radians();

    let a =
        (dlat / 2.0).sin().powi(2) + lat1_r.cos() * lat2_r.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    EARTH_RADIUS_KM * c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_haversine_same_point() {
        assert!((haversine_km(45.0, -69.0, 45.0, -69.0)).abs() < 0.001);
    }

    #[test]
    fn test_haversine_portland_boston() {
        let d = haversine_km(43.66, -70.26, 42.36, -71.06);
        assert!((d - 155.0).abs() < 10.0, "Portland-Boston: {d} km");
    }
}
