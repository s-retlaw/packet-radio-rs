use clap::{Parser, Subcommand};
use reference::cwop::db::{count_cwop_by_region, get_cwop_station};
use reference::cwop::fetcher::HttpFetcher;
use reference::cwop::{CwopError, CwopSource};
use reference::db::{default_db_path, ReferenceDb};
use reference::geo::RangeFilter;
use std::path::PathBuf;
use std::time::Duration;

/// Top-level error type for the reference-tool CLI.
#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Db(#[from] sqlx::Error),

    #[error("{0}")]
    Cwop(#[from] CwopError),

    #[error("{0}")]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Parser)]
#[command(name = "reference-tool", about = "Manage APRS reference data")]
struct Cli {
    /// Database file path (default: ~/.local/share/packet-radio/reference.db)
    #[arg(long)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync reference data from external sources
    Sync {
        /// Data source to sync
        #[command(subcommand)]
        source: SyncSource,
    },

    /// Look up a station by callsign
    Lookup {
        /// Callsign to look up
        callsign: String,
    },

    /// Query stations within a geographic area
    Query {
        /// Center latitude
        #[arg(long, allow_hyphen_values = true)]
        lat: f64,

        /// Center longitude
        #[arg(long, allow_hyphen_values = true)]
        lon: f64,

        /// Search radius in kilometers
        #[arg(long)]
        radius: f64,
    },

    /// Show database statistics
    Stats,

    /// Show database info (path, size, table counts)
    Info,
}

#[derive(Subcommand)]
enum SyncSource {
    /// Sync CWOP weather station data from wxqa.com
    Cwop {
        /// Only sync a single region (e.g., ME, CA, canada)
        #[arg(long)]
        region: Option<String>,

        /// Only sync if data is older than this many hours (default: always sync)
        #[arg(long)]
        max_age_hours: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> Result<(), CliError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "reference=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(default_db_path);

    let db = ReferenceDb::open(&db_path).await?;

    match cli.command {
        Commands::Sync { source } => match source {
            SyncSource::Cwop { region, max_age_hours } => {
                let fetcher = HttpFetcher::new()?;
                let source = CwopSource::new(fetcher, db);

                if let Some(region) = region {
                    let count = source.sync_region(&region).await?;
                    println!("Synced {} stations from region {}", count, region);
                } else if let Some(hours) = max_age_hours {
                    let result = source
                        .sync_if_stale(Duration::from_secs(hours * 3600))
                        .await?;
                    match result {
                        Some(r) => println!(
                            "Synced {} stations from {} regions in {:.1}s",
                            r.total_stations,
                            r.total_regions,
                            r.duration.as_secs_f64()
                        ),
                        None => println!("Data is fresh, skipping sync"),
                    }
                } else {
                    let result = source.sync_all().await?;
                    println!(
                        "Synced {} stations from {} regions in {:.1}s",
                        result.total_stations,
                        result.total_regions,
                        result.duration.as_secs_f64()
                    );
                    if !result.errors.is_empty() {
                        println!("\nErrors:");
                        for (region, err) in &result.errors {
                            println!("  {}: {}", region, err);
                        }
                    }
                }
            }
        },

        Commands::Lookup { callsign } => {
            // Try CWOP-specific lookup first
            if let Some(cwop) = get_cwop_station(&db, &callsign).await? {
                println!("Station: {}", cwop.callsign);
                println!("  Source:    CWOP");
                println!("  Position:  {:.5}, {:.5}", cwop.lat, cwop.lon);
                if let Some(city) = &cwop.city {
                    println!("  City:      {}", city);
                }
                println!("  Region:    {}", cwop.region);
                if let Some(elev) = cwop.elevation_m {
                    println!("  Elevation: {:.1} m", elev);
                }
                if let Some(nwsid) = &cwop.nwsid {
                    println!("  NWSID:     {}", nwsid);
                }
            } else if let Some(pos) = db.lookup_position(&callsign).await? {
                println!("Station: {}", pos.callsign);
                println!("  Source:   {}", pos.source);
                println!("  Position: {:.5}, {:.5}", pos.lat, pos.lon);
            } else {
                println!("Station {} not found", callsign);
            }
        }

        Commands::Query { lat, lon, radius } => {
            let filter = RangeFilter::new(lat, lon, radius);
            let positions = db.query_positions_within(&filter).await?;

            if positions.is_empty() {
                println!("No stations found within {} km of ({}, {})", radius, lat, lon);
            } else {
                println!(
                    "Found {} stations within {} km of ({}, {}):\n",
                    positions.len(),
                    radius,
                    lat,
                    lon
                );
                println!("{:<12} {:>10} {:>11} {}", "CALLSIGN", "LAT", "LON", "SOURCE");
                println!("{}", "-".repeat(50));
                for pos in &positions {
                    println!(
                        "{:<12} {:>10.5} {:>11.5} {}",
                        pos.callsign, pos.lat, pos.lon, pos.source
                    );
                }
            }
        }

        Commands::Stats => {
            let total = db.total_count().await?;
            let by_source = db.count_by_source().await?;

            println!("Reference Database Statistics");
            println!("{}", "=".repeat(40));
            println!("Total stations: {}", total);
            println!();

            if !by_source.is_empty() {
                println!("By source:");
                for (source, count) in &by_source {
                    println!("  {:<10} {}", source, count);
                }
                println!();
            }

            // CWOP region breakdown
            let cwop_regions = count_cwop_by_region(&db).await?;
            if !cwop_regions.is_empty() {
                println!("CWOP by region:");
                for (region, count) in &cwop_regions {
                    println!("  {:<20} {}", region, count);
                }
                println!();
            }

            // Last sync info
            if let Some(entry) = db.last_sync("cwop", None).await? {
                println!("Last full CWOP sync: {}", entry.synced_at);
                if let Some(count) = entry.station_count {
                    println!("  Stations: {}", count);
                }
                if let Some(ms) = entry.duration_ms {
                    println!("  Duration: {:.1}s", ms as f64 / 1000.0);
                }
            }
        }

        Commands::Info => {
            println!("Database path: {}", db.path().display());

            if db.path().exists() {
                let metadata = std::fs::metadata(db.path())?;
                let size_kb = metadata.len() as f64 / 1024.0;
                if size_kb > 1024.0 {
                    println!("Database size: {:.1} MB", size_kb / 1024.0);
                } else {
                    println!("Database size: {:.1} KB", size_kb);
                }
            }

            let total = db.total_count().await?;
            println!("Total positions: {}", total);
        }
    }

    Ok(())
}
