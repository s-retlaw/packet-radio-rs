pub mod census;
pub mod zip_centroid;

use indicatif::{ProgressBar, ProgressStyle};
use tracing::{info, warn};

use crate::db::FccDb;
use crate::error::Result;
use crate::models::is_po_box;

/// Batch geocode records that need lat/lon.
///
/// Deduplicates by address — each unique address is geocoded once, then the
/// result is applied to all records at that address via SQL.
///
/// Strategy:
/// 1. Street addresses → Census Bureau batch geocoder (free, 1K/batch)
/// 2. PO Box addresses → ZIP code centroid fallback
/// 3. Census failures → ZIP centroid fallback
pub async fn geocode_batch(db: &FccDb, po_box_only: bool) -> Result<(usize, usize)> {
    let addresses = db.addresses_needing_geocode().await?;

    if addresses.is_empty() {
        info!("No records needing geocoding");
        return Ok((0, 0));
    }

    info!("{} unique addresses to geocode", addresses.len());

    let pb = ProgressBar::new(addresses.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("Geocoding [{bar:40}] {pos}/{len} addrs ({eta})")
            .unwrap()
            .progress_chars("=> "),
    );

    let mut geocoded: usize = 0;
    let mut failed: usize = 0;

    // Split into PO Box vs street addresses
    let (po_box_addrs, street_addrs): (Vec<_>, Vec<_>) = addresses
        .into_iter()
        .partition(|(addr, _, _, _)| is_po_box(addr));

    // PO Box addresses → local lookup first, then ZIP centroid
    for (street, city, state, zip) in &po_box_addrs {
        if let Some((lat, lon, source, quality)) =
            db.lookup_geocode_by_address(street, city, state, zip).await?
        {
            let n = db
                .geocode_by_address(street, city, state, zip, lat, lon, &source, &quality)
                .await?;
            geocoded += n as usize;
        } else if let Some((lat, lon)) = db.lookup_zip_centroid(zip).await? {
            let n = db
                .geocode_by_address(street, city, state, zip, lat, lon, "zip_centroid", "ZIP center")
                .await?;
            geocoded += n as usize;
        } else {
            failed += 1;
        }
        pb.inc(1);
    }

    if po_box_only {
        pb.finish_with_message(format!("Geocoded {} records ({} failed)", geocoded, failed));
        return Ok((geocoded, failed));
    }

    // Street addresses → local lookup first, then Census Bureau batch
    let mut need_census: Vec<(String, String, String, String)> = Vec::new();
    for (street, city, state, zip) in &street_addrs {
        if let Some((lat, lon, source, quality)) =
            db.lookup_geocode_by_address(street, city, state, zip).await?
        {
            let n = db
                .geocode_by_address(street, city, state, zip, lat, lon, &source, &quality)
                .await?;
            geocoded += n as usize;
        } else {
            need_census.push((street.clone(), city.clone(), state.clone(), zip.clone()));
        }
        pb.inc(1);
    }

    // Reset progress for Census batches
    let census_total = need_census.len();
    if census_total > 0 {
        info!(
            "{} addresses resolved locally, {} need Census lookup",
            street_addrs.len() - census_total,
            census_total
        );
        pb.set_length(pb.position() + census_total as u64);
    }

    let batch_size = 1_000;
    for chunk in need_census.chunks(batch_size) {
        let census_addrs: Vec<census::AddressRecord> = chunk
            .iter()
            .enumerate()
            .map(|(i, (street, city, state, zip))| census::AddressRecord {
                id: i as i64,
                street: street.clone(),
                city: city.clone(),
                state: state.clone(),
                zip: zip.clone(),
            })
            .collect();

        match census::batch_geocode(&census_addrs).await {
            Ok(results) => {
                for result in &results {
                    let idx = result.id as usize;
                    let (street, city, state, zip) = &chunk[idx];

                    if let (Some(lat), Some(lon)) = (result.lat, result.lon) {
                        let n = db
                            .geocode_by_address(street, city, state, zip, lat, lon, "census", &result.quality)
                            .await?;
                        geocoded += n as usize;
                    } else if !zip.is_empty() {
                        if let Some((lat, lon)) = db.lookup_zip_centroid(zip).await? {
                            let n = db
                                .geocode_by_address(street, city, state, zip, lat, lon, "zip_centroid", "ZIP center (fallback)")
                                .await?;
                            geocoded += n as usize;
                        } else {
                            failed += 1;
                        }
                    } else {
                        failed += 1;
                    }
                }
            }
            Err(e) => {
                warn!("Census batch geocode failed: {}, trying ZIP fallback", e);
                for (street, city, state, zip) in chunk {
                    if !zip.is_empty() {
                        if let Some((lat, lon)) = db.lookup_zip_centroid(zip).await? {
                            let n = db
                                .geocode_by_address(street, city, state, zip, lat, lon, "zip_centroid", "ZIP center (batch fallback)")
                                .await?;
                            geocoded += n as usize;
                        } else {
                            failed += 1;
                        }
                    } else {
                        failed += 1;
                    }
                }
            }
        }
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!(
        "Geocoded {} records ({} unique addrs, {} failed)",
        geocoded,
        po_box_addrs.len() + street_addrs.len(),
        failed
    ));

    Ok((geocoded, failed))
}
