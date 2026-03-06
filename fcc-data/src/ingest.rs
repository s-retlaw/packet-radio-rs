use chrono::{Datelike, NaiveDate, Utc};
use indicatif::{ProgressBar, ProgressStyle};
use tracing::{info, warn};

use crate::db::FccDb;
use crate::download::{self, Day, ExtractedData};
use crate::error::Result;
use crate::geocode;
use crate::parse::{parse_am_line, parse_co_line, parse_en_line, parse_hd_line, parse_hs_line};

const BATCH_SIZE: usize = 5000;

/// Full sync: download the complete FCC database and ingest all records.
pub async fn sync_full(db: &FccDb) -> Result<i64> {
    let log_id = db.start_sync_log("full").await?;

    match do_full_sync(db).await {
        Ok(count) => {
            db.finish_sync_log(log_id, "completed", Some(count), None)
                .await?;
            Ok(count)
        }
        Err(e) => {
            db.finish_sync_log(log_id, "failed", None, Some(&e.to_string()))
                .await?;
            Err(e)
        }
    }
}

async fn do_full_sync(db: &FccDb) -> Result<i64> {
    let data = download::download_full().await?;
    let count = ingest_data(db, &data).await?;
    run_geocoding(db).await?;
    Ok(count)
}

/// Daily sync: download the daily update for a specific day and apply upserts.
pub async fn sync_daily(db: &FccDb, day: Day) -> Result<i64> {
    let log_id = db.start_sync_log(&format!("daily_{}", day.suffix())).await?;

    match do_daily_sync(db, day).await {
        Ok(count) => {
            db.finish_sync_log(log_id, "completed", Some(count), None)
                .await?;
            Ok(count)
        }
        Err(e) => {
            db.finish_sync_log(log_id, "failed", None, Some(&e.to_string()))
                .await?;
            Err(e)
        }
    }
}

async fn do_daily_sync(db: &FccDb, day: Day) -> Result<i64> {
    let data = download::download_daily(day).await?;
    let count = ingest_data(db, &data).await?;
    run_geocoding(db).await?;
    Ok(count)
}

/// Catchup sync: if no prior sync exists, do a full sync. Otherwise, apply
/// each daily file from the day after the last sync through yesterday.
///
/// FCC publishes daily files named by day-of-week (l_am_mon.zip through
/// l_am_sun.zip). Each file covers changes from that calendar day. Files
/// are overwritten weekly, so catchup only works within a ~7-day window.
/// If the last sync is older than 7 days we fall back to a full sync.
pub async fn sync_catchup(db: &FccDb) -> Result<CatchupResult> {
    let last_sync = db.last_successful_sync().await?;

    let last_date = match last_sync {
        None => {
            println!("No previous sync found — running full sync");
            let count = sync_full(db).await?;
            return Ok(CatchupResult {
                strategy: CatchupStrategy::Full,
                total_records: count,
                days_applied: Vec::new(),
            });
        }
        Some(ref ts) => {
            // Parse the RFC3339 timestamp to get the date
            match chrono::DateTime::parse_from_rfc3339(ts) {
                Ok(dt) => dt.date_naive(),
                Err(_) => {
                    // Try parsing just the date portion
                    match NaiveDate::parse_from_str(&ts[..10], "%Y-%m-%d") {
                        Ok(d) => d,
                        Err(_) => {
                            println!("Cannot parse last sync date '{}' — running full sync", ts);
                            let count = sync_full(db).await?;
                            return Ok(CatchupResult {
                                strategy: CatchupStrategy::Full,
                                total_records: count,
                                days_applied: Vec::new(),
                            });
                        }
                    }
                }
            }
        }
    };

    let today = Utc::now().date_naive();
    let days_behind = (today - last_date).num_days();

    if days_behind <= 0 {
        println!("Already up to date (last sync: {})", last_date);
        return Ok(CatchupResult {
            strategy: CatchupStrategy::AlreadyCurrent,
            total_records: 0,
            days_applied: Vec::new(),
        });
    }

    if days_behind > 7 {
        println!(
            "Last sync was {} days ago ({}) — daily files only cover 7 days, running full sync",
            days_behind, last_date
        );
        let count = sync_full(db).await?;
        return Ok(CatchupResult {
            strategy: CatchupStrategy::Full,
            total_records: count,
            days_applied: Vec::new(),
        });
    }

    // Apply daily files from (last_date + 1) through yesterday
    // (today's file may not be published yet)
    let end_date = today - chrono::Duration::days(1);
    let mut current = last_date + chrono::Duration::days(1);
    let mut total_records: i64 = 0;
    let mut days_applied = Vec::new();

    println!(
        "Catching up from {} to {} ({} days)",
        current, end_date, days_behind.min(7)
    );

    while current <= end_date {
        let day = Day::from_chrono_weekday(current.weekday());
        println!("  Applying {} ({})...", current, day.suffix());

        match sync_daily(db, day).await {
            Ok(count) => {
                println!("    {} records", count);
                total_records += count;
                days_applied.push((current, day, count));
            }
            Err(e) => {
                warn!("Failed to sync {} ({}): {}", current, day.suffix(), e);
                println!("    Failed: {} (skipping)", e);
            }
        }

        current += chrono::Duration::days(1);
    }

    println!(
        "Catchup complete: {} daily files, {} total records",
        days_applied.len(),
        total_records
    );

    Ok(CatchupResult {
        strategy: CatchupStrategy::Daily,
        total_records,
        days_applied,
    })
}

/// Result of a catchup sync operation.
pub struct CatchupResult {
    pub strategy: CatchupStrategy,
    pub total_records: i64,
    pub days_applied: Vec<(NaiveDate, Day, i64)>,
}

/// What strategy the catchup used.
#[derive(Debug)]
pub enum CatchupStrategy {
    /// No prior sync — ran full download
    Full,
    /// Already current, nothing to do
    AlreadyCurrent,
    /// Applied one or more daily files
    Daily,
}

/// Ingest extracted FCC data into the database.
pub async fn ingest_data(db: &FccDb, data: &ExtractedData) -> Result<i64> {
    let mut total: i64 = 0;

    if let Some(ref hd_text) = data.hd_data {
        let count = ingest_hd(db, hd_text).await?;
        info!("Ingested {} HD records", count);
        total += count;
    }

    if let Some(ref en_text) = data.en_data {
        let count = ingest_en(db, en_text).await?;
        info!("Ingested {} EN records", count);
        total += count;
    }

    if let Some(ref am_text) = data.am_data {
        let count = ingest_am(db, am_text).await?;
        info!("Ingested {} AM records", count);
        total += count;
    }

    if let Some(ref hs_text) = data.hs_data {
        let count = ingest_hs(db, hs_text).await?;
        info!("Ingested {} HS records", count);
        total += count;
    }

    if let Some(ref co_text) = data.co_data {
        let count = ingest_co(db, co_text).await?;
        info!("Ingested {} CO records", count);
        total += count;
    }

    Ok(total)
}

async fn ingest_hd(db: &FccDb, text: &str) -> Result<i64> {
    let lines: Vec<&str> = text.lines().collect();
    let pb = make_progress_bar(lines.len() as u64, "HD records");
    let mut count: i64 = 0;

    for chunk in lines.chunks(BATCH_SIZE) {
        let mut tx = db.pool().begin().await?;
        for line in chunk {
            if let Some(rec) = parse_hd_line(line) {
                sqlx::query(
                    "INSERT INTO hd (usi, call_sign, license_status, radio_service_code, grant_date, expired_date, cancellation_date, last_action_date)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(usi) DO UPDATE SET
                       call_sign=?2, license_status=?3, radio_service_code=?4,
                       grant_date=?5, expired_date=?6, cancellation_date=?7, last_action_date=?8"
                )
                .bind(rec.usi)
                .bind(&rec.call_sign)
                .bind(&rec.license_status)
                .bind(&rec.radio_service_code)
                .bind(&rec.grant_date)
                .bind(&rec.expired_date)
                .bind(&rec.cancellation_date)
                .bind(&rec.last_action_date)
                .execute(&mut *tx)
                .await?;
                count += 1;
            }
        }
        tx.commit().await?;
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!("{} HD records", count));
    Ok(count)
}

async fn ingest_en(db: &FccDb, text: &str) -> Result<i64> {
    let lines: Vec<&str> = text.lines().collect();
    let pb = make_progress_bar(lines.len() as u64, "EN records");
    let mut count: i64 = 0;

    for chunk in lines.chunks(BATCH_SIZE) {
        let mut tx = db.pool().begin().await?;
        let mut batch_usis = Vec::new();
        for line in chunk {
            if let Some(rec) = parse_en_line(line) {
                let usi = rec.usi;
                sqlx::query(
                    "INSERT INTO en (usi, entity_type, licensee_id, entity_name, first_name, mi, last_name, suffix, street_address, city, state, zip_code, frn)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(usi) DO UPDATE SET
                       entity_type=?2, licensee_id=?3, entity_name=?4, first_name=?5,
                       mi=?6, last_name=?7, suffix=?8, street_address=?9, city=?10,
                       state=?11, zip_code=?12, frn=?13"
                )
                .bind(rec.usi)
                .bind(&rec.entity_type)
                .bind(&rec.licensee_id)
                .bind(&rec.entity_name)
                .bind(&rec.first_name)
                .bind(&rec.mi)
                .bind(&rec.last_name)
                .bind(&rec.suffix)
                .bind(&rec.street_address)
                .bind(&rec.city)
                .bind(&rec.state)
                .bind(&rec.zip_code)
                .bind(&rec.frn)
                .execute(&mut *tx)
                .await?;
                batch_usis.push(usi);
                count += 1;
            }
        }
        tx.commit().await?;

        // Mark geocodes stale for upserted records
        if !batch_usis.is_empty() {
            let stale = db.mark_geocodes_stale_batch(&batch_usis).await?;
            if stale > 0 {
                info!("Marked {} geocodes stale", stale);
            }
        }

        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!("{} EN records", count));
    Ok(count)
}

async fn ingest_am(db: &FccDb, text: &str) -> Result<i64> {
    let lines: Vec<&str> = text.lines().collect();
    let pb = make_progress_bar(lines.len() as u64, "AM records");
    let mut count: i64 = 0;

    for chunk in lines.chunks(BATCH_SIZE) {
        let mut tx = db.pool().begin().await?;
        for line in chunk {
            if let Some(rec) = parse_am_line(line) {
                sqlx::query(
                    "INSERT INTO am (usi, operator_class, group_code, region_code, previous_operator_class, previous_call_sign)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(usi) DO UPDATE SET
                       operator_class=?2, group_code=?3, region_code=?4, previous_operator_class=?5, previous_call_sign=?6"
                )
                .bind(rec.usi)
                .bind(&rec.operator_class)
                .bind(&rec.group_code)
                .bind(&rec.region_code)
                .bind(&rec.previous_operator_class)
                .bind(&rec.previous_call_sign)
                .execute(&mut *tx)
                .await?;
                count += 1;
            }
        }
        tx.commit().await?;
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!("{} AM records", count));
    Ok(count)
}

async fn ingest_hs(db: &FccDb, text: &str) -> Result<i64> {
    let lines: Vec<&str> = text.lines().collect();
    let pb = make_progress_bar(lines.len() as u64, "HS records");
    let mut count: i64 = 0;

    for chunk in lines.chunks(BATCH_SIZE) {
        let mut tx = db.pool().begin().await?;
        for line in chunk {
            if let Some(rec) = parse_hs_line(line) {
                sqlx::query(
                    "INSERT OR IGNORE INTO hs (usi, log_date, code) VALUES (?1, ?2, ?3)"
                )
                .bind(rec.usi)
                .bind(&rec.log_date)
                .bind(&rec.code)
                .execute(&mut *tx)
                .await?;
                count += 1;
            }
        }
        tx.commit().await?;
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!("{} HS records", count));
    Ok(count)
}

async fn ingest_co(db: &FccDb, text: &str) -> Result<i64> {
    let lines: Vec<&str> = text.lines().collect();
    let pb = make_progress_bar(lines.len() as u64, "CO records");
    let mut count: i64 = 0;

    for chunk in lines.chunks(BATCH_SIZE) {
        let mut tx = db.pool().begin().await?;
        for line in chunk {
            if let Some(rec) = parse_co_line(line) {
                sqlx::query(
                    "INSERT INTO co (usi, comment_date, comment, status_code, status_date)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(usi, comment_date, comment) DO UPDATE SET
                       status_code=?4, status_date=?5"
                )
                .bind(rec.usi)
                .bind(&rec.comment_date)
                .bind(&rec.comment)
                .bind(&rec.status_code)
                .bind(&rec.status_date)
                .execute(&mut *tx)
                .await?;
                count += 1;
            }
        }
        tx.commit().await?;
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!("{} CO records", count));
    Ok(count)
}

/// Run geocoding after sync: ensure ZIP centroids loaded, then geocode new/stale records.
async fn run_geocoding(db: &FccDb) -> Result<()> {
    geocode::zip_centroid::ensure_loaded(db).await?;

    let (geocoded, failed) = geocode::geocode_batch(db, false).await?;
    if geocoded > 0 || failed > 0 {
        println!("Geocoded {} records ({} failed)", geocoded, failed);
    }

    Ok(())
}

fn make_progress_bar(total: u64, msg: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {pos}/{len} ({eta})")
            .unwrap()
            .progress_chars("=> "),
    );
    pb.set_message(msg.to_string());
    pb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::ExtractedData;

    #[tokio::test]
    async fn test_ingest_hd_records() {
        let db = FccDb::open_memory().await.unwrap();
        // Real FCC records: AA0GV and AB7TH
        let data = ExtractedData {
            hd_data: Some(
                "HD|215148|0011928619||AA0GV|A|HA|03/04/2026|05/02/2036||||||||||N||||||||||N||GAIL|E|HURD||||||||||03/04/2026|03/04/2026|||||||||||||||\n\
                 HD|222575|0011924741||AB7TH|A|HA|03/04/2026|04/16/2036||||||||||N||||||||||N||CLIFFORD|E|BEERS||||||||||03/04/2026|03/04/2026|||||||||||||||".to_string()
            ),
            en_data: None,
            am_data: None,
            hs_data: None,
            co_data: None,
        };

        let count = ingest_data(&db, &data).await.unwrap();
        assert_eq!(count, 2);

        let (hd_count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM hd")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(hd_count, 2);
    }

    #[tokio::test]
    async fn test_ingest_full_pipeline() {
        let db = FccDb::open_memory().await.unwrap();
        // Real FCC records: AA0GV with all five file types
        let data = ExtractedData {
            hd_data: Some("HD|215148|0011928619||AA0GV|A|HA|03/04/2026|05/02/2036||||||||||N||||||||||N||GAIL|E|HURD||||||||||03/04/2026|03/04/2026|||||||||||||||".to_string()),
            en_data: Some("EN|215148|||AA0GV|L|L00612755|HURD, GAIL E|GAIL|E|HURD|||||52527 849th Rd|NELIGH|NE|68756|||000|0008143463|I||||||".to_string()),
            am_data: Some("AM|215148|||AA0GV|E|A|10||||||||||".to_string()),
            hs_data: Some("HS|215148||AA0GV|01/17/2003|LIAUA\nHS|215148||AA0GV|03/21/2006|LIREN".to_string()),
            co_data: Some("CO|215148||AA0GV|03/04/2026|Test comment for AA0GV||".to_string()),
        };

        let count = ingest_data(&db, &data).await.unwrap();
        assert_eq!(count, 6); // 1 HD + 1 EN + 1 AM + 2 HS + 1 CO

        // Verify the joined lookup works
        let license = db.lookup_callsign("AA0GV").await.unwrap().unwrap();
        assert_eq!(license.call_sign, "AA0GV");
        assert_eq!(license.city, "NELIGH");
        assert_eq!(license.state, "NE");
        assert_eq!(license.operator_class, "E");

        // Verify history was loaded
        let history = db.get_history(215148).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].1, "LIREN"); // most recent first (ORDER BY DESC)
        assert_eq!(history[1].1, "LIAUA");

        // Verify comments were loaded
        let comments = db.get_comments(215148).await.unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].0, "03/04/2026");
        assert_eq!(comments[0].1, "Test comment for AA0GV");
        assert_eq!(comments[0].2, ""); // no status_code
    }

    #[tokio::test]
    async fn test_ingest_am_previous_callsign() {
        let db = FccDb::open_memory().await.unwrap();
        // AB7TH with previous call KK7CY — verifies index 15 fix
        let data = ExtractedData {
            hd_data: Some("HD|222575|0011924741||AB7TH|A|HA|03/04/2026|04/16/2036||||||||||N||||||||||N||CLIFFORD|E|BEERS||||||||||03/04/2026|03/04/2026|||||||||||||||".to_string()),
            en_data: Some("EN|222575|||AB7TH|L|L00123456|BEERS, CLIFFORD E|CLIFFORD|E|BEERS|||||123 Main St|PORTLAND|OR|97201|||000|0009876543|I||||||".to_string()),
            am_data: Some("AM|222575|||AB7TH|E|A|7||||||||KK7CY|A|".to_string()),
            hs_data: None,
            co_data: None,
        };

        let count = ingest_data(&db, &data).await.unwrap();
        assert_eq!(count, 3); // 1 HD + 1 EN + 1 AM

        let license = db.lookup_callsign("AB7TH").await.unwrap().unwrap();
        assert_eq!(license.previous_call_sign, "KK7CY");
        assert_eq!(license.previous_operator_class, "A");
    }
}
