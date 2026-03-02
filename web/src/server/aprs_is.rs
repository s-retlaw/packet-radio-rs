use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

use super::ingest::process_raw_frame;

/// Parsed TNC-2 format line.
#[derive(Debug, Clone)]
pub struct Tnc2Packet {
    pub source: String,
    pub dest: String,
    pub path: Vec<String>,
    pub info: Vec<u8>,
}

/// Parse a TNC-2 format line into its components.
/// Format: `SOURCE>DEST,PATH1,PATH2:INFO`
pub fn parse_tnc2_line(line: &str) -> Option<Tnc2Packet> {
    let line = line.trim();

    // Skip comments and empty lines
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Find source>dest separator
    let gt_pos = line.find('>')?;
    let source = &line[..gt_pos];
    if source.is_empty() {
        return None;
    }

    let rest = &line[gt_pos + 1..];

    // Find the info separator ':'
    let colon_pos = rest.find(':')?;
    let dest_path = &rest[..colon_pos];
    let info = &rest[colon_pos + 1..];

    // Split dest,path1,path2,...
    let mut parts = dest_path.split(',');
    let dest = parts.next()?.to_string();
    if dest.is_empty() {
        return None;
    }

    let path: Vec<String> = parts.map(|s| s.to_string()).collect();

    Some(Tnc2Packet {
        source: source.to_string(),
        dest,
        path,
        info: info.as_bytes().to_vec(),
    })
}

/// Build a synthetic AX.25 frame from a TNC-2 parsed packet.
/// This allows us to reuse the same process_raw_frame pipeline.
fn tnc2_to_ax25(pkt: &Tnc2Packet) -> Vec<u8> {
    let mut frame = Vec::new();

    // Encode address field (shifted left by 1, space-padded to 6 chars)
    fn encode_address(call_ssid: &str, is_last: bool) -> [u8; 7] {
        let mut bytes = [0x40u8; 7]; // space << 1
        let (callsign, ssid) = if let Some(dash) = call_ssid.find('-') {
            (&call_ssid[..dash], call_ssid[dash + 1..].parse::<u8>().unwrap_or(0))
        } else {
            // Check for H-bit marker
            let clean = call_ssid.trim_end_matches('*');
            (clean, 0u8)
        };

        for (i, &b) in callsign.as_bytes().iter().take(6).enumerate() {
            bytes[i] = b << 1;
        }

        let h_bit = if call_ssid.ends_with('*') { 0x80 } else { 0 };
        bytes[6] = 0x60 | ((ssid & 0x0F) << 1) | h_bit;
        if is_last {
            bytes[6] |= 0x01;
        }
        bytes
    }

    let has_path = !pkt.path.is_empty();

    // Destination
    frame.extend_from_slice(&encode_address(&pkt.dest, false));

    // Source (last if no digipeaters)
    frame.extend_from_slice(&encode_address(&pkt.source, !has_path));

    // Digipeaters
    for (i, digi) in pkt.path.iter().enumerate() {
        let is_last = i == pkt.path.len() - 1;
        frame.extend_from_slice(&encode_address(digi, is_last));
    }

    // Control + PID
    frame.push(0x03); // UI
    frame.push(0xF0); // No L3

    // Info field
    frame.extend_from_slice(&pkt.info);

    frame
}

/// Process a single TNC-2 format line through the ingest pipeline.
pub async fn process_tnc2_line(
    line: &str,
    pool: &SqlitePool,
    tx: &broadcast::Sender<String>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let pkt = match parse_tnc2_line(line) {
        Some(p) => p,
        None => return Ok(false),
    };

    let ax25 = tnc2_to_ax25(&pkt);
    process_raw_frame(&ax25, pool, tx).await
}

/// Run the APRS-IS client — connects and processes TNC-2 lines.
pub async fn run_aprs_is_client(
    host: &str,
    port: u16,
    callsign: &str,
    passcode: &str,
    filter: &str,
    pool: SqlitePool,
    tx: broadcast::Sender<String>,
) {
    // Default to receive-only if callsign is empty
    let callsign = if callsign.trim().is_empty() {
        "N0CALL"
    } else {
        callsign
    };
    let passcode = if callsign == "N0CALL" { "-1" } else { passcode };

    let mut backoff = std::time::Duration::from_secs(1);
    let max_backoff = std::time::Duration::from_secs(60);

    loop {
        tracing::info!("Connecting to APRS-IS at {}:{}", host, port);
        match tokio::net::TcpStream::connect((host, port)).await {
            Ok(stream) => {
                tracing::info!("Connected to APRS-IS");
                backoff = std::time::Duration::from_secs(1);

                let (reader, mut writer) = tokio::io::split(stream);
                let mut lines = BufReader::new(reader).lines();

                // Send login
                let login = format!(
                    "user {} pass {} vers packet-radio-web 0.1 filter {}\r\n",
                    callsign, passcode, filter
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
                    if let Err(e) = process_tnc2_line(&line, &pool, &tx).await {
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
            process_tnc2_line("N0CALL>APRS:!4903.50N/07201.75W-Test", &pool, &tx)
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

        let result = process_tnc2_line("# server comment", &pool, &tx)
            .await
            .unwrap();
        assert!(!result);

        let packets = super::super::db::get_recent_packets(&pool, 10).await.unwrap();
        assert!(packets.is_empty());
    }
}
