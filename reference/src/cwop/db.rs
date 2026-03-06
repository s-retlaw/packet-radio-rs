use crate::cwop::parser::CwopStation;
use crate::db::ReferenceDb;

/// Full CWOP station record from the database.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CwopStationRow {
    pub callsign: String,
    pub nwsid: Option<String>,
    pub city: Option<String>,
    pub region: String,
    pub elevation_m: Option<f64>,
    pub lat: f64,
    pub lon: f64,
}

/// Bulk upsert CWOP stations into both `positions` and `cwop_stations` tables.
///
/// Uses a transaction to ensure both tables stay consistent.
pub async fn upsert_cwop_stations(
    db: &ReferenceDb,
    stations: &[CwopStation],
) -> Result<usize, sqlx::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let pool = db.pool();

    let mut tx = pool.begin().await?;
    let mut count = 0;

    for station in stations {
        // Upsert into positions (core lookup table)
        sqlx::query(
            "INSERT OR REPLACE INTO positions (callsign, source, lat, lon, updated_at) \
             VALUES (?1, 'cwop', ?2, ?3, ?4)",
        )
        .bind(&station.callsign)
        .bind(station.lat)
        .bind(station.lon)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // Upsert into cwop_stations (source-specific metadata)
        sqlx::query(
            "INSERT OR REPLACE INTO cwop_stations \
             (callsign, nwsid, city, region, elevation_m, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&station.callsign)
        .bind(&station.nwsid)
        .bind(&station.city)
        .bind(&station.region)
        .bind(station.elevation_m)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        count += 1;
    }

    tx.commit().await?;
    Ok(count)
}

/// Look up a single CWOP station with full metadata.
pub async fn get_cwop_station(
    db: &ReferenceDb,
    callsign: &str,
) -> Result<Option<CwopStationRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, String, Option<f64>, f64, f64)>(
        "SELECT c.callsign, c.nwsid, c.city, c.region, c.elevation_m, p.lat, p.lon \
         FROM cwop_stations c \
         JOIN positions p ON p.callsign = c.callsign AND p.source = 'cwop' \
         WHERE c.callsign = ?1",
    )
    .bind(callsign)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|(callsign, nwsid, city, region, elevation_m, lat, lon)| CwopStationRow {
        callsign,
        nwsid,
        city,
        region,
        elevation_m,
        lat,
        lon,
    }))
}

/// Query CWOP stations by region.
pub async fn get_cwop_by_region(
    db: &ReferenceDb,
    region: &str,
) -> Result<Vec<CwopStationRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String, Option<f64>, f64, f64)>(
        "SELECT c.callsign, c.nwsid, c.city, c.region, c.elevation_m, p.lat, p.lon \
         FROM cwop_stations c \
         JOIN positions p ON p.callsign = c.callsign AND p.source = 'cwop' \
         WHERE c.region = ?1 \
         ORDER BY c.callsign",
    )
    .bind(region)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|(callsign, nwsid, city, region, elevation_m, lat, lon)| CwopStationRow {
            callsign,
            nwsid,
            city,
            region,
            elevation_m,
            lat,
            lon,
        })
        .collect())
}

/// Count CWOP stations by region.
pub async fn count_cwop_by_region(
    db: &ReferenceDb,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT region, COUNT(*) FROM cwop_stations GROUP BY region ORDER BY region",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cwop_upsert_populates_positions() {
        let db = ReferenceDb::open_memory().await.unwrap();

        let stations = vec![
            CwopStation {
                callsign: "KD1KE".to_string(),
                lat: 44.489,
                lon: -69.35,
                elevation_m: Some(238.66),
                city: Some("Freedom".to_string()),
                region: "ME".to_string(),
                nwsid: Some("AP207".to_string()),
            },
            CwopStation {
                callsign: "WA1DLZ".to_string(),
                lat: 44.633,
                lon: -69.893,
                elevation_m: Some(134.97),
                city: Some("Mercer".to_string()),
                region: "ME".to_string(),
                nwsid: Some("AP210".to_string()),
            },
        ];

        let count = upsert_cwop_stations(&db, &stations).await.unwrap();
        assert_eq!(count, 2);

        // Verify positions table
        let pos = db.lookup_position("KD1KE").await.unwrap().unwrap();
        assert_eq!(pos.source, "cwop");
        assert!((pos.lat - 44.489).abs() < 0.001);

        // Verify cwop_stations table
        let cwop = get_cwop_station(&db, "KD1KE").await.unwrap().unwrap();
        assert_eq!(cwop.nwsid.as_deref(), Some("AP207"));
        assert_eq!(cwop.city.as_deref(), Some("Freedom"));
        assert!((cwop.elevation_m.unwrap() - 238.66).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_cwop_upsert_updates() {
        let db = ReferenceDb::open_memory().await.unwrap();

        let station = CwopStation {
            callsign: "TEST1".to_string(),
            lat: 40.0,
            lon: -70.0,
            elevation_m: Some(100.0),
            city: Some("Old City".to_string()),
            region: "ME".to_string(),
            nwsid: Some("AP001".to_string()),
        };
        upsert_cwop_stations(&db, &[station]).await.unwrap();

        // Update with new data
        let station = CwopStation {
            callsign: "TEST1".to_string(),
            lat: 41.0,
            lon: -71.0,
            elevation_m: Some(200.0),
            city: Some("New City".to_string()),
            region: "ME".to_string(),
            nwsid: Some("AP001".to_string()),
        };
        upsert_cwop_stations(&db, &[station]).await.unwrap();

        // Should have updated, not duplicated
        let cwop = get_cwop_station(&db, "TEST1").await.unwrap().unwrap();
        assert_eq!(cwop.city.as_deref(), Some("New City"));
        assert!((cwop.lat - 41.0).abs() < 0.001);

        assert_eq!(db.total_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_query_by_region() {
        let db = ReferenceDb::open_memory().await.unwrap();

        let stations = vec![
            CwopStation {
                callsign: "ME1".to_string(),
                lat: 44.0,
                lon: -69.0,
                elevation_m: None,
                city: None,
                region: "ME".to_string(),
                nwsid: None,
            },
            CwopStation {
                callsign: "ME2".to_string(),
                lat: 45.0,
                lon: -70.0,
                elevation_m: None,
                city: None,
                region: "ME".to_string(),
                nwsid: None,
            },
            CwopStation {
                callsign: "CA1".to_string(),
                lat: 34.0,
                lon: -118.0,
                elevation_m: None,
                city: None,
                region: "CA".to_string(),
                nwsid: None,
            },
        ];
        upsert_cwop_stations(&db, &stations).await.unwrap();

        let me = get_cwop_by_region(&db, "ME").await.unwrap();
        assert_eq!(me.len(), 2);

        let ca = get_cwop_by_region(&db, "CA").await.unwrap();
        assert_eq!(ca.len(), 1);

        let counts = count_cwop_by_region(&db).await.unwrap();
        assert_eq!(counts.len(), 2);
    }

    #[tokio::test]
    async fn test_upsert_empty_slice() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let count = upsert_cwop_stations(&db, &[]).await.unwrap();
        assert_eq!(count, 0);
        assert_eq!(db.total_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_station_with_all_none_optionals() {
        let db = ReferenceDb::open_memory().await.unwrap();

        let station = CwopStation {
            callsign: "BARE1".to_string(),
            lat: 40.0,
            lon: -70.0,
            elevation_m: None,
            city: None,
            region: "ME".to_string(),
            nwsid: None,
        };
        upsert_cwop_stations(&db, &[station]).await.unwrap();

        let cwop = get_cwop_station(&db, "BARE1").await.unwrap().unwrap();
        assert!(cwop.elevation_m.is_none());
        assert!(cwop.city.is_none());
        assert!(cwop.nwsid.is_none());
        assert_eq!(cwop.region, "ME");
    }

    #[tokio::test]
    async fn test_get_cwop_station_missing() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let result = get_cwop_station(&db, "NONEXISTENT").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_query_by_region_empty() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let results = get_cwop_by_region(&db, "ZZ").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_count_by_region_empty() {
        let db = ReferenceDb::open_memory().await.unwrap();
        let counts = count_cwop_by_region(&db).await.unwrap();
        assert!(counts.is_empty());
    }

    #[tokio::test]
    async fn test_query_by_region_sorted() {
        let db = ReferenceDb::open_memory().await.unwrap();

        let stations = vec![
            CwopStation {
                callsign: "ZZZ".to_string(),
                lat: 44.0,
                lon: -69.0,
                elevation_m: None,
                city: None,
                region: "ME".to_string(),
                nwsid: None,
            },
            CwopStation {
                callsign: "AAA".to_string(),
                lat: 45.0,
                lon: -70.0,
                elevation_m: None,
                city: None,
                region: "ME".to_string(),
                nwsid: None,
            },
        ];
        upsert_cwop_stations(&db, &stations).await.unwrap();

        let results = get_cwop_by_region(&db, "ME").await.unwrap();
        assert_eq!(results[0].callsign, "AAA");
        assert_eq!(results[1].callsign, "ZZZ");
    }
}
