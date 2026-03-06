use std::path::PathBuf;

use clap::{Parser, Subcommand};
use fcc_data::db::{default_db_path, FccDb};
use fcc_data::download::Day;
use fcc_data::models::{OperatorClass, SearchQuery};

#[derive(Parser)]
#[command(name = "fcc-tool", about = "FCC Amateur Radio License Database tool")]
struct Cli {
    /// Database file path (default: ~/.local/share/packet-radio/fcc.db)
    #[arg(long)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync FCC database
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },

    /// Geocode license addresses
    Geocode {
        /// Only geocode PO Box addresses (ZIP centroid)
        #[arg(long)]
        po_box_only: bool,
    },

    /// Load ZIP code centroids from SimpleMaps CSV
    LoadZipCentroids {
        /// Path to the ZIP code CSV file
        csv: PathBuf,
    },

    /// Search licenses
    Search {
        /// Callsign (prefix match)
        #[arg(long)]
        call: Option<String>,

        /// Name (partial match)
        #[arg(long)]
        name: Option<String>,

        /// City (exact match)
        #[arg(long)]
        city: Option<String>,

        /// State (2-letter code)
        #[arg(long)]
        state: Option<String>,

        /// ZIP code (prefix match)
        #[arg(long)]
        zip: Option<String>,

        /// Operator class (T/G/E/A/N)
        #[arg(long, name = "class")]
        operator_class: Option<String>,

        /// License status (A=Active, E=Expired, C=Cancelled, T=Terminated)
        #[arg(long, default_value = "A")]
        status: Option<String>,

        /// Max results
        #[arg(long, default_value = "25")]
        limit: i64,
    },

    /// Show database statistics
    Stats,

    /// Interactive TUI
    Tui,
}

#[derive(Subcommand)]
enum SyncAction {
    /// Download and ingest full FCC database (~160MB download)
    Full,

    /// Download and apply daily update
    Daily {
        /// Day of week (mon, tue, wed, thu, fri, sat, sun)
        #[arg(long, default_value = "mon")]
        day: String,
    },

    /// Apply daily updates since last sync (auto-detects; does full if needed)
    Catchup,

    /// Show sync history
    Status,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fcc_data=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(default_db_path);
    let db = FccDb::open(&db_path).await?;

    match cli.command {
        Commands::Sync { action } => match action {
            SyncAction::Full => {
                println!("Starting full FCC database sync...");
                let count = fcc_data::ingest::sync_full(&db).await?;
                println!("Sync complete: {} records ingested", count);
            }
            SyncAction::Daily { day } => {
                let day = Day::parse(&day).ok_or_else(|| {
                    format!("Invalid day: {}. Use: mon, tue, wed, thu, fri, sat, sun", day)
                })?;
                println!("Starting daily sync ({})...", day.suffix());
                let count = fcc_data::ingest::sync_daily(&db, day).await?;
                println!("Daily sync complete: {} records", count);
            }
            SyncAction::Catchup => {
                let result = fcc_data::ingest::sync_catchup(&db).await?;
                match result.strategy {
                    fcc_data::ingest::CatchupStrategy::Full => {
                        println!("Full sync: {} records", result.total_records);
                    }
                    fcc_data::ingest::CatchupStrategy::AlreadyCurrent => {}
                    fcc_data::ingest::CatchupStrategy::Daily => {
                        for (date, day, count) in &result.days_applied {
                            println!("  {} ({}): {} records", date, day.suffix(), count);
                        }
                    }
                }
            }
            SyncAction::Status => {
                let history = db.sync_history(10).await?;
                if history.is_empty() {
                    println!("No sync history found. Run 'fcc-tool sync full' first.");
                } else {
                    println!("{:<5} {:<12} {:<25} {:<10} {:>10} Error",
                        "ID", "Type", "Started", "Status", "Records");
                    println!("{}", "-".repeat(80));
                    for entry in &history {
                        println!("{:<5} {:<12} {:<25} {:<10} {:>10} {}",
                            entry.id,
                            entry.sync_type,
                            entry.started_at,
                            entry.status,
                            entry.records_processed.map(|n| n.to_string()).unwrap_or_default(),
                            entry.error_message.as_deref().unwrap_or(""),
                        );
                    }
                }
            }
        },

        Commands::Geocode { po_box_only } => {
            println!("Geocoding all pending records{}...",
                if po_box_only { " (PO Box only)" } else { "" });
            let (geocoded, failed) =
                fcc_data::geocode::geocode_batch(&db, po_box_only).await?;
            println!("Geocoded: {}, Failed: {}", geocoded, failed);
        }

        Commands::LoadZipCentroids { csv } => {
            let count =
                fcc_data::geocode::zip_centroid::load_csv(&db, &csv).await?;
            println!("Loaded {} ZIP centroids", count);
        }

        Commands::Search {
            call, name, city, state, zip, operator_class, status, limit,
        } => {
            let query = SearchQuery {
                call_sign: call,
                name,
                city,
                state,
                zip_code: zip,
                operator_class,
                license_status: status,
                limit: Some(limit),
            };

            let results = db.search(&query).await?;

            if results.is_empty() {
                println!("No results found.");
            } else {
                println!("{:<10} {:<25} {:<12} {:<15} {:<3} {:<7}",
                    "CALL", "NAME", "CLASS", "CITY", "ST", "STATUS");
                println!("{}", "-".repeat(75));
                for r in &results {
                    let class = OperatorClass::from_code(&r.operator_class).to_string();
                    println!("{:<10} {:<25} {:<12} {:<15} {:<3} {:<7}",
                        r.call_sign,
                        truncate(&r.display_name(), 24),
                        class,
                        truncate(&r.city, 14),
                        r.state,
                        r.license_status,
                    );
                }
                println!("\n{} results", results.len());
            }
        }

        Commands::Stats => {
            println!("FCC Database Statistics");
            println!("{}", "=".repeat(40));
            println!("Path: {}", db.path().display());

            if db.path().exists() {
                let meta = std::fs::metadata(db.path())?;
                let size_mb = meta.len() as f64 / (1024.0 * 1024.0);
                println!("Size: {:.1} MB", size_mb);
            }

            println!();
            let counts = db.table_counts().await?;
            println!("Table counts:");
            for (table, count) in &counts {
                println!("  {:<15} {:>10}", table, count);
            }

            println!();
            let by_class = db.count_by_class().await?;
            if !by_class.is_empty() {
                println!("Active licenses by class:");
                for (class, count) in &by_class {
                    let display = OperatorClass::from_code(class).to_string();
                    println!("  {:<15} {:>10}", display, count);
                }
            }

            println!();
            let (total, geocoded, stale) = db.geocode_stats().await?;
            println!("Geocoding:");
            println!("  Active licenses:  {:>10}", total);
            println!("  Geocoded:         {:>10} ({:.1}%)", geocoded,
                if total > 0 { geocoded as f64 / total as f64 * 100.0 } else { 0.0 });
            println!("  Stale:            {:>10}", stale);

            println!();
            let history = db.sync_history(3).await?;
            if !history.is_empty() {
                println!("Recent syncs:");
                for entry in &history {
                    println!("  {} - {} ({}, {} records)",
                        entry.started_at,
                        entry.sync_type,
                        entry.status,
                        entry.records_processed.map_or_else(|| "?".to_string(), |n| n.to_string()),
                    );
                }
            }
        }

        Commands::Tui => {
            fcc_data::tui::run(db).await?;
        }
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end = max.saturating_sub(3);
        let boundary = s
            .char_indices()
            .map(|(i, _)| i)
            .take(end)
            .last()
            .unwrap_or(0);
        format!("{}...", &s[..boundary])
    }
}
