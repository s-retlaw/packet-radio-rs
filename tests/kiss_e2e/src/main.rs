//! KISS E2E Tests — validates KISS interop with our TNC.
//!
//! Test A: RX — spawns TNC in WAV mode, connects kiss-tnc client, reads frames.
//! Test B: TCP TX — sends a KISS frame via TCP, captures TX WAV, decodes it.
//! Test C: Pipe loopback — tx-pipe | rx-pipe round-trip.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use packet_radio_core::kiss::{Command as KissCommand, KissDecoder};

const WAV_PATH: &str = "tests/wav/03_100-mic-e-bursts-flat.wav";
const KISS_PORT_RX: u16 = 18901;
const KISS_PORT_TX: u16 = 18902;
const MIN_FRAMES: usize = 85;
const TIMEOUT_SECS: u64 = 30;

#[tokio::main]
async fn main() {
    let mut passed = 0;
    let mut failed = 0;

    // Test A: RX via KISS TCP
    if Path::new(WAV_PATH).exists() {
        if test_rx_tcp().await {
            passed += 1;
        } else {
            failed += 1;
        }
    } else {
        println!("SKIP test_rx_tcp: {WAV_PATH} not found");
    }

    // Test B: TX via KISS TCP + --tx-wav
    if Path::new(WAV_PATH).exists() {
        if test_tx_tcp().await {
            passed += 1;
        } else {
            failed += 1;
        }
    } else {
        println!("SKIP test_tx_tcp: {WAV_PATH} not found");
    }

    // Test C: Pipe loopback (no WAV files needed)
    if test_pipe_loopback().await {
        passed += 1;
    } else {
        failed += 1;
    }

    println!();
    println!("=== E2E Summary: {passed} passed, {failed} failed ===");
    if failed > 0 {
        std::process::exit(1);
    }
}

// -- Helpers ------------------------------------------------------------------

fn tnc_binary() -> std::path::PathBuf {
    Path::new("target/release/packet-radio-desktop").to_path_buf()
}

fn build_tnc() {
    println!("Building desktop TNC...");
    let status = std::process::Command::new("cargo")
        .args(["build", "--release", "-p", "packet-radio-desktop"])
        .status()
        .expect("failed to run cargo build");
    assert!(status.success(), "cargo build failed");
    assert!(tnc_binary().exists(), "binary not found");
}

/// Build a test AX.25 frame (raw, no HDLC).
fn build_test_ax25() -> Vec<u8> {
    use packet_radio_core::ax25::frame::build_test_frame;
    let (buf, len) = build_test_frame("TEST-1", "CQ", b"!E2E pipe loopback test");
    buf[..len].to_vec()
}

/// KISS-encode a data frame for port 0.
fn kiss_encode(ax25_data: &[u8]) -> Vec<u8> {
    let mut buf = [0u8; 1024];
    let len = packet_radio_core::kiss::encode_frame(0, ax25_data, &mut buf)
        .expect("KISS encode failed");
    buf[..len].to_vec()
}

/// Wait for TNC stdout to contain a marker line, return remaining stdout reader.
async fn wait_for_ready(
    tnc: &mut tokio::process::Child,
    marker: &str,
    timeout_secs: u64,
) -> BufReader<tokio::process::ChildStdout> {
    let stdout = tnc.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let start = Instant::now();

    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        if line.contains(marker) {
                            println!("TNC ready ({}ms)", start.elapsed().as_millis());
                            // Return the inner reader
                            return lines.into_inner();
                        }
                    }
                    Ok(None) => {
                        panic!("TNC exited before ready signal");
                    }
                    Err(e) => {
                        panic!("stdout read error: {e}");
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                tnc.kill().await.ok();
                panic!("TNC startup timeout ({timeout_secs}s)");
            }
        }
    }
}

/// Validate basic AX.25 frame structure: check destination callsign (bytes 0..6)
/// and source callsign (bytes 7..13) contain printable ASCII when right-shifted.
/// Bytes 6 and 13 are SSID bytes and are not checked.
fn validate_ax25(data: &[u8]) -> bool {
    if data.len() < 15 {
        return false;
    }
    // Destination callsign (6 bytes)
    for &b in &data[0..6] {
        let ch = b >> 1;
        if !(0x20..=0x7E).contains(&ch) {
            return false;
        }
    }
    // Source callsign (6 bytes)
    for &b in &data[7..13] {
        let ch = b >> 1;
        if !(0x20..=0x7E).contains(&ch) {
            return false;
        }
    }
    true
}

// -- Test A: RX via KISS TCP --------------------------------------------------

async fn test_rx_tcp() -> bool {
    println!();
    println!("=== Test A: RX via KISS TCP ===");
    build_tnc();

    let mut tnc = Command::new(tnc_binary())
        .args([
            "--wav", WAV_PATH,
            "--kiss-port", &KISS_PORT_RX.to_string(),
            "--smart3",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn TNC");

    // Drain stderr
    let stderr = tnc.stderr.take().unwrap();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(_)) = reader.next_line().await {}
    });

    let stdout = wait_for_ready(&mut tnc, "KISS TCP server listening", 10).await;
    // Continue draining stdout
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(_)) = lines.next_line().await {}
    });

    let addr = format!("127.0.0.1:{KISS_PORT_RX}");
    println!("Connecting kiss-tnc client to {addr}...");
    let mut client = kiss_tnc::Tnc::connect_tcp(&addr)
        .await
        .expect("failed to connect to TNC");

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut malformed = 0u32;
    let deadline = Instant::now() + Duration::from_secs(TIMEOUT_SECS);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { break; }
        match tokio::time::timeout(remaining, client.read_frame()).await {
            Ok(Ok((_port, data))) => {
                if validate_ax25(&data) {
                    frames.push(data);
                } else {
                    malformed += 1;
                }
            }
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }

    let _ = tnc.wait().await;
    let distinct: HashSet<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();

    println!("Frames received: {} (distinct: {}), malformed: {malformed}", frames.len(), distinct.len());

    let pass = frames.len() >= MIN_FRAMES && malformed == 0;
    if pass {
        println!("PASS test_rx_tcp");
    } else {
        eprintln!("FAIL test_rx_tcp: {} frames, {malformed} malformed", frames.len());
    }
    pass
}

// -- Test B: TX via KISS TCP --------------------------------------------------

async fn test_tx_tcp() -> bool {
    println!();
    println!("=== Test B: TX via KISS TCP + tx-wav ===");
    build_tnc();

    let tx_wav = "/tmp/kiss_e2e_tx.wav";

    // Remove stale TX WAV
    let _ = std::fs::remove_file(tx_wav);

    let mut tnc = Command::new(tnc_binary())
        .args([
            "--wav", WAV_PATH,
            "--kiss-port", &KISS_PORT_TX.to_string(),
            "--smart3",
            "--tx-wav", tx_wav,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn TNC");

    // Drain stderr
    let stderr = tnc.stderr.take().unwrap();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(_)) = reader.next_line().await {}
    });

    let stdout = wait_for_ready(&mut tnc, "KISS TCP server listening", 10).await;
    // Continue draining stdout
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(_)) = lines.next_line().await {}
    });

    // Connect and send a test frame
    let addr = format!("127.0.0.1:{KISS_PORT_TX}");
    println!("Connecting to TNC at {addr}...");

    // Small delay for server to be fully ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut stream = tokio::net::TcpStream::connect(&addr)
        .await
        .expect("failed to connect to TNC");

    let test_frame = build_test_ax25();
    let kiss_data = kiss_encode(&test_frame);
    println!("Sending test frame ({} KISS bytes)...", kiss_data.len());
    stream.write_all(&kiss_data).await.expect("write failed");

    // Give TNC time to process the KISS frame and modulate
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Close connection and wait for TNC to finish
    drop(stream);
    let status = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), tnc.wait())
        .await
        .expect("TNC exit timeout")
        .expect("wait failed");
    println!("TNC exited with {status}");

    // Verify TX WAV exists and has samples
    if !Path::new(tx_wav).exists() {
        eprintln!("FAIL test_tx_tcp: {tx_wav} not created");
        return false;
    }

    let reader = hound::WavReader::open(tx_wav).expect("failed to open TX WAV");
    let spec = reader.spec();
    let sample_count = reader.len();
    println!("TX WAV: {} samples, {} Hz, {} ch", sample_count, spec.sample_rate, spec.channels);

    if sample_count == 0 {
        eprintln!("FAIL test_tx_tcp: TX WAV is empty");
        return false;
    }

    // Decode the TX WAV with MiniDecoder to verify our test frame round-trips
    let reader2 = hound::WavReader::open(tx_wav).expect("failed to open TX WAV for decode");
    let samples: Vec<i16> = reader2.into_samples::<i16>().filter_map(|s| s.ok()).collect();
    let config = packet_radio_core::modem::DemodConfig::default_1200();
    let mut mini = packet_radio_core::modem::multi::MiniDecoder::new(config);
    let mut decoded_frames: Vec<Vec<u8>> = Vec::new();

    // Process in chunks
    for chunk in samples.chunks(1024) {
        let output = mini.process_samples(chunk);
        for i in 0..output.len() {
            decoded_frames.push(output.frame(i).to_vec());
        }
    }

    println!("Decoded {} frames from TX WAV", decoded_frames.len());

    if decoded_frames.is_empty() {
        eprintln!("FAIL test_tx_tcp: no frames decoded from TX WAV");
        return false;
    }

    // Check that at least one decoded frame matches our test frame
    let found = decoded_frames.iter().any(|f| f == &test_frame);
    if found {
        println!("PASS test_tx_tcp (test frame round-tripped through TX WAV)");
        true
    } else {
        eprintln!("FAIL test_tx_tcp: test frame not found in decoded output");
        eprintln!("  Expected: {:02X?}", &test_frame[..test_frame.len().min(40)]);
        for (i, f) in decoded_frames.iter().enumerate() {
            eprintln!("  Decoded[{i}]: {:02X?}", &f[..f.len().min(40)]);
        }
        false
    }
}

// -- Test C: Pipe Loopback ----------------------------------------------------

async fn test_pipe_loopback() -> bool {
    println!();
    println!("=== Test C: Pipe Loopback (tx-pipe | rx-pipe) ===");
    build_tnc();

    let test_frame = build_test_ax25();
    let kiss_data = kiss_encode(&test_frame);
    println!("Test frame: {} AX.25 bytes, {} KISS bytes", test_frame.len(), kiss_data.len());

    // Step 1: tx-pipe -- feed KISS -> get raw PCM
    println!("Running tx-pipe...");
    let mut tx_proc = Command::new(tnc_binary())
        .args(["--tx-pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn tx-pipe");

    // Write KISS data to stdin, then close stdin
    let mut tx_stdin = tx_proc.stdin.take().unwrap();
    tx_stdin.write_all(&kiss_data).await.expect("tx-pipe write failed");
    drop(tx_stdin);

    // Read all PCM from stdout
    let tx_output = tokio::time::timeout(
        Duration::from_secs(10),
        tx_proc.wait_with_output(),
    )
    .await
    .expect("tx-pipe timeout")
    .expect("tx-pipe wait failed");

    if !tx_output.status.success() {
        let stderr = String::from_utf8_lossy(&tx_output.stderr);
        eprintln!("FAIL: tx-pipe exited with {}: {stderr}", tx_output.status);
        return false;
    }

    let pcm_bytes = &tx_output.stdout;
    let pcm_samples = pcm_bytes.len() / 2;
    println!("tx-pipe produced {} PCM bytes ({} samples)", pcm_bytes.len(), pcm_samples);

    if pcm_samples < 100 {
        eprintln!("FAIL: tx-pipe produced too few samples");
        return false;
    }

    // Step 2: rx-pipe -- feed raw PCM -> get KISS frames
    println!("Running rx-pipe...");
    let mut rx_proc = Command::new(tnc_binary())
        .args(["--rx-pipe", "--smart3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rx-pipe");

    let mut rx_stdin = rx_proc.stdin.take().unwrap();
    rx_stdin.write_all(pcm_bytes).await.expect("rx-pipe write failed");
    drop(rx_stdin);

    let rx_output = tokio::time::timeout(
        Duration::from_secs(10),
        rx_proc.wait_with_output(),
    )
    .await
    .expect("rx-pipe timeout")
    .expect("rx-pipe wait failed");

    let kiss_output = &rx_output.stdout;
    println!("rx-pipe produced {} KISS bytes", kiss_output.len());

    // Parse KISS frames from output using core KissDecoder
    let decoded = parse_kiss_frames(kiss_output);
    println!("Decoded {} frames from rx-pipe output", decoded.len());

    if decoded.is_empty() {
        eprintln!("FAIL: no frames decoded from pipe loopback");
        let stderr = String::from_utf8_lossy(&rx_output.stderr);
        eprintln!("rx-pipe stderr: {}", &stderr[..stderr.len().min(500)]);
        return false;
    }

    // Check that at least one decoded frame matches
    let found = decoded.iter().any(|f| f == &test_frame);
    if found {
        println!("PASS test_pipe_loopback (frame round-tripped through pipes)");
        true
    } else {
        eprintln!("FAIL: test frame not found in pipe loopback output");
        eprintln!("  Expected ({} bytes): {:02X?}", test_frame.len(), &test_frame[..test_frame.len().min(40)]);
        for (i, f) in decoded.iter().enumerate() {
            eprintln!("  Decoded[{i}] ({} bytes): {:02X?}", f.len(), &f[..f.len().min(40)]);
        }
        false
    }
}

/// Parse KISS frames from raw bytes using the core KissDecoder.
fn parse_kiss_frames(data: &[u8]) -> Vec<Vec<u8>> {
    let mut decoder = KissDecoder::new();
    let mut frames = Vec::new();

    for &b in data {
        if let Some((_port, cmd, payload)) = decoder.feed_byte(b) {
            if matches!(cmd, KissCommand::DataFrame) {
                frames.push(payload.to_vec());
            }
        }
    }

    frames
}
