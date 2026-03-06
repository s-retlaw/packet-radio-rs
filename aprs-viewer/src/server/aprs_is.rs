use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

use super::error::ServerError;
use super::ingest::process_raw_frame;

// Re-export shared types for use by other modules in this crate.
pub use packet_radio_shared::aprs_is::{parse_tnc2_line, AprsIsClientConfig, Tnc2Packet};

/// Process a single TNC-2 format line through the ingest pipeline.
pub async fn process_tnc2_line(
    line: &str,
    pool: &SqlitePool,
    tx: &broadcast::Sender<String>,
    reference_db: Option<&reference::ReferenceDb>,
) -> Result<bool, ServerError> {
    let pkt = match parse_tnc2_line(line) {
        Some(p) => p,
        None => return Ok(false),
    };

    let ax25 = packet_radio_shared::aprs_is::tnc2_to_ax25(&pkt);
    process_raw_frame(&ax25, pool, tx, reference_db, "aprs-is").await
}

/// Run the APRS-IS client — connects and processes TNC-2 lines.
pub async fn run_aprs_is_client(
    config: &AprsIsClientConfig,
    pool: SqlitePool,
    tx: broadcast::Sender<String>,
    reference_db: Option<Arc<reference::ReferenceDb>>,
) {
    // Default to receive-only if callsign is empty
    let callsign = if config.callsign.trim().is_empty() {
        "N0CALL"
    } else {
        &config.callsign
    };
    let passcode = if callsign == "N0CALL" { "-1" } else { &config.passcode };

    let mut backoff = std::time::Duration::from_secs(1);
    let max_backoff = std::time::Duration::from_secs(60);

    loop {
        tracing::info!("Connecting to APRS-IS at {}:{}", config.host, config.port);
        match tokio::net::TcpStream::connect((&*config.host, config.port)).await {
            Ok(stream) => {
                tracing::info!("Connected to APRS-IS");
                backoff = std::time::Duration::from_secs(1);

                let (reader, mut writer) = tokio::io::split(stream);
                let mut lines = BufReader::new(reader).lines();

                // Send login
                let login = format!(
                    "user {} pass {} vers aprs-viewer 0.1 filter {}\r\n",
                    callsign, passcode, config.filter
                );
                if let Err(e) = writer.write_all(login.as_bytes()).await {
                    tracing::error!("APRS-IS login write error: {}", e);
                    continue;
                }

                // Spawn keepalive task
                let keepalive_handle = tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                    loop {
                        interval.tick().await;
                        if writer.write_all(b"#keepalive\r\n").await.is_err() {
                            break;
                        }
                    }
                });

                // Read lines
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.len() > 1024 {
                        tracing::warn!("APRS-IS: skipping oversized line ({} bytes)", line.len());
                        continue;
                    }
                    if let Err(e) = process_tnc2_line(&line, &pool, &tx, reference_db.as_deref()).await {
                        tracing::error!("APRS-IS packet processing error: {}", e);
                    }
                }

                keepalive_handle.abort();
                tracing::warn!("APRS-IS connection closed");
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to connect to APRS-IS: {}. Retrying in {:?}",
                    e,
                    backoff
                );
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tnc2_basic() {
        let pkt = parse_tnc2_line("N0CALL>APRS,WIDE1-1:!4903.50N/07201.75W-Test").unwrap();
        assert_eq!(pkt.source, "N0CALL");
        assert_eq!(pkt.dest, "APRS");
        assert_eq!(pkt.path, vec!["WIDE1-1"]);
        assert_eq!(pkt.info, b"!4903.50N/07201.75W-Test");
    }

    #[test]
    fn test_parse_tnc2_no_path() {
        let pkt = parse_tnc2_line("N0CALL>APRS:!4903.50N/07201.75W-").unwrap();
        assert_eq!(pkt.source, "N0CALL");
        assert_eq!(pkt.dest, "APRS");
        assert!(pkt.path.is_empty());
    }

    #[test]
    fn test_parse_tnc2_multiple_digipeaters() {
        let pkt =
            parse_tnc2_line("N0CALL>APRS,DIGI1*,DIGI2,WIDE2-1:!4903.50N/07201.75W-").unwrap();
        assert_eq!(pkt.path.len(), 3);
        assert_eq!(pkt.path[0], "DIGI1*");
        assert_eq!(pkt.path[1], "DIGI2");
        assert_eq!(pkt.path[2], "WIDE2-1");
    }

    #[test]
    fn test_parse_tnc2_comment_line() {
        assert!(parse_tnc2_line("# logresp N0CALL verified").is_none());
    }

    #[test]
    fn test_parse_tnc2_empty_line() {
        assert!(parse_tnc2_line("").is_none());
        assert!(parse_tnc2_line("   ").is_none());
    }

    #[test]
    fn test_parse_tnc2_missing_gt() {
        assert!(parse_tnc2_line("N0CALLAPRS:data").is_none());
    }

    #[test]
    fn test_parse_tnc2_missing_colon() {
        assert!(parse_tnc2_line("N0CALL>APRS").is_none());
    }

    #[test]
    fn test_parse_tnc2_empty_info() {
        let pkt = parse_tnc2_line("N0CALL>APRS:").unwrap();
        assert!(pkt.info.is_empty());
    }

    #[test]
    fn test_parse_tnc2_info_with_colons() {
        let pkt = parse_tnc2_line("N0CALL>APRS::W1AW     :Hello{001").unwrap();
        assert_eq!(pkt.info, b":W1AW     :Hello{001");
    }

    #[tokio::test]
    async fn test_process_tnc2_line_pipeline() {
        let pool = super::super::db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        let result =
            process_tnc2_line("N0CALL>APRS:!4903.50N/07201.75W-Test", &pool, &tx, None)
                .await
                .unwrap();
        assert!(result);

        let stations = super::super::db::get_stations(&pool).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].callsign, "N0CALL");
    }

    #[tokio::test]
    async fn test_process_tnc2_comment_ignored() {
        let pool = super::super::db::test_db().await;
        let (tx, _rx) = broadcast::channel(16);

        let result = process_tnc2_line("# server comment", &pool, &tx, None)
            .await
            .unwrap();
        assert!(!result);

        let packets = super::super::db::get_recent_packets(&pool, 10).await.unwrap();
        assert!(packets.is_empty());
    }
}
