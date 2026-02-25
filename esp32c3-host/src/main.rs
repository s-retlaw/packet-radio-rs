//! ESP32-C3 Host Tool
//!
//! Streams WAV audio to ESP32-C3 test harness firmware over USB serial,
//! collects decoded frames and cycle counts, optionally compares with
//! local MiniDecoder output.

#[allow(dead_code)]
mod protocol;

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use clap::Parser;
use serialport::SerialPort;

use packet_radio_core::modem::multi::MiniDecoder;
use packet_radio_core::modem::DemodConfig;

use protocol::*;

/// Chunk size in samples for audio streaming.
const CHUNK_SAMPLES: usize = 512;

/// Serial read timeout.
const SERIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// PING timeout.
const PING_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Parser)]
#[command(name = "esp32c3-host", about = "ESP32-C3 packet radio test harness host tool")]
struct Cli {
    /// Serial port path (e.g., /dev/ttyACM0)
    #[arg(short, long, default_value = "/dev/ttyACM0")]
    port: String,

    /// Baud rate — must match firmware UART_BAUD (921600 default)
    #[arg(short, long, default_value_t = 921600)]
    baud: u32,

    /// WAV file to stream
    #[arg(short, long)]
    wav: Option<String>,

    /// Just send PING and verify connectivity
    #[arg(long)]
    ping: bool,

    /// Compare ESP32 decode results with local MiniDecoder
    #[arg(long)]
    compare: bool,

    /// Decoder mode: fast, quality, or mini
    #[arg(short, long, default_value = "mini")]
    mode: String,

    /// Sample rate override
    #[arg(long, default_value_t = 11025)]
    sample_rate: u32,

    /// CPU frequency in MHz (160 for ESP32-C6, 125 for RP2040)
    #[arg(long, default_value_t = 160)]
    cpu_freq: u32,

    /// Skip DTR/RTS board reset (needed for RP2040 USB-CDC which has no reset lines)
    #[arg(long)]
    no_reset: bool,
}

/// Collected frame from ESP32.
struct EspFrame {
    #[allow(dead_code)]
    seq: u16,
    data: Vec<u8>,
}

/// Send a protocol message over serial.
fn send_msg(port: &mut Box<dyn SerialPort>, msg_type: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut buf = [0u8; MAX_MSG_SIZE];
    let len = build_msg(msg_type, 0, payload, &mut buf);
    port.write_all(&buf[..len])?;
    port.flush()?;
    Ok(())
}

/// Read exactly `n` bytes from serial with timeout.
fn read_exact(port: &mut Box<dyn SerialPort>, buf: &mut [u8]) -> std::io::Result<()> {
    let mut pos = 0;
    let deadline = Instant::now() + SERIAL_TIMEOUT;
    while pos < buf.len() {
        if Instant::now() > deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "serial read timeout",
            ));
        }
        match port.read(&mut buf[pos..]) {
            Ok(n) => pos += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Valid message types for resync detection.
fn is_valid_msg_type(b: u8) -> bool {
    matches!(b, MSG_PONG | MSG_READY | MSG_FRAME | MSG_CHUNK_ACK | MSG_STATS | MSG_ERROR)
}

/// Read one protocol message with resync on invalid data.
/// If the header looks invalid (bad msg_type or oversized payload), skip bytes
/// until we find a valid-looking message type, then re-read the header.
fn read_msg(port: &mut Box<dyn SerialPort>) -> std::io::Result<(u8, Vec<u8>)> {
    let mut hdr_buf = [0u8; HEADER_SIZE];
    read_exact(port, &mut hdr_buf)?;

    // Resync loop: if header looks invalid, shift bytes and scan
    let mut resync_count = 0;
    loop {
        let hdr = Header::parse(&hdr_buf);
        if is_valid_msg_type(hdr.msg_type) && (hdr.payload_len as usize) <= MAX_PAYLOAD {
            // Valid header
            let mut payload = vec![0u8; hdr.payload_len as usize];
            if !payload.is_empty() {
                read_exact(port, &mut payload)?;
            }
            if resync_count > 0 {
                eprintln!("  (resynced after {} bytes)", resync_count);
            }
            return Ok((hdr.msg_type, payload));
        }

        // Invalid header — skip first byte and read one more
        if resync_count == 0 {
            eprintln!(
                "Warning: invalid header [{:02x} {:02x} {:02x} {:02x}], resyncing...",
                hdr_buf[0], hdr_buf[1], hdr_buf[2], hdr_buf[3]
            );
        }
        resync_count += 1;
        hdr_buf[0] = hdr_buf[1];
        hdr_buf[1] = hdr_buf[2];
        hdr_buf[2] = hdr_buf[3];
        read_exact(port, &mut hdr_buf[3..4])?;

        if resync_count > 4096 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "resync failed after 4096 bytes — firmware may have panicked",
            ));
        }
    }
}

/// Read messages until we get the expected type, collecting FRAMEs along the way.
fn read_until(
    port: &mut Box<dyn SerialPort>,
    expected: u8,
    frames: &mut Vec<EspFrame>,
) -> std::io::Result<Vec<u8>> {
    loop {
        let (msg_type, payload) = read_msg(port)?;
        match msg_type {
            MSG_FRAME => {
                if let (Some(seq), Some(data)) =
                    (FramePayload::parse_seq(&payload), FramePayload::parse_data(&payload))
                {
                    frames.push(EspFrame {
                        seq,
                        data: data.to_vec(),
                    });
                }
            }
            MSG_ERROR => {
                let msg = String::from_utf8_lossy(&payload);
                eprintln!("ESP32 error: {}", msg);
            }
            t if t == expected => return Ok(payload),
            _ => {
                // Unexpected message type — skip
            }
        }
    }
}

/// FNV-1a hash for frame dedup (matches core implementation).
fn frame_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn decoder_mode_byte(mode: &str) -> u8 {
    match mode {
        "fast" => MODE_FAST,
        "quality" => MODE_QUALITY,
        "mini" => MODE_MINI,
        "corr3" => MODE_CORR3,
        "tnc" => MODE_TNC,
        _ => {
            eprintln!("Unknown mode '{}', using mini", mode);
            MODE_MINI
        }
    }
}

fn do_ping(port: &mut Box<dyn SerialPort>) -> std::io::Result<bool> {
    port.set_timeout(PING_TIMEOUT)?;
    send_msg(port, MSG_PING, &[])?;

    let start = Instant::now();
    let (msg_type, _) = read_msg(port)?;
    let rtt = start.elapsed();

    if msg_type == MSG_PONG {
        println!("PONG received in {:.1}ms", rtt.as_secs_f64() * 1000.0);
        Ok(true)
    } else {
        eprintln!("Expected PONG (0x81), got 0x{:02x}", msg_type);
        Ok(false)
    }
}

fn do_stream(
    port: &mut Box<dyn SerialPort>,
    wav_path: &str,
    mode: &str,
    sample_rate: u32,
    compare: bool,
    cpu_freq_mhz: u32,
) -> std::io::Result<()> {
    // Open WAV file
    let reader = hound::WavReader::open(wav_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("WAV open: {}", e)))?;
    let spec = reader.spec();
    println!(
        "WAV: {} Hz, {} ch, {} bits",
        spec.sample_rate, spec.channels, spec.bits_per_sample
    );

    if spec.sample_rate != sample_rate {
        eprintln!(
            "Warning: WAV sample rate {} != expected {}",
            spec.sample_rate, sample_rate
        );
    }

    // Collect all samples (mono, i16)
    let all_samples: Vec<i16> = reader
        .into_samples::<i16>()
        .step_by(spec.channels as usize)
        .filter_map(|s| s.ok())
        .collect();

    println!("Loaded {} samples ({:.2}s)", all_samples.len(),
        all_samples.len() as f64 / sample_rate as f64);

    // Configure decoder on ESP32
    port.set_timeout(SERIAL_TIMEOUT)?;
    let mode_byte = decoder_mode_byte(mode);
    let mut config_payload = [0u8; 5];
    let cfg = ConfigPayload {
        decoder_mode: mode_byte,
        sample_rate,
    };
    cfg.encode(&mut config_payload);
    send_msg(port, MSG_CONFIG, &config_payload)?;

    // Wait for READY
    let mut esp_frames: Vec<EspFrame> = Vec::new();
    read_until(port, MSG_READY, &mut esp_frames)?;
    println!("ESP32 ready (mode={})", mode);

    // Stream audio chunks
    let total_chunks = (all_samples.len() + CHUNK_SAMPLES - 1) / CHUNK_SAMPLES;
    let stream_start = Instant::now();

    for (chunk_idx, chunk) in all_samples.chunks(CHUNK_SAMPLES).enumerate() {
        let seq = chunk_idx as u16;

        // Build AUDIO_CHUNK payload: seq:u16 + samples:N×i16
        let payload_len = 2 + chunk.len() * 2;
        let mut payload = vec![0u8; payload_len];
        payload[0..2].copy_from_slice(&seq.to_le_bytes());
        for (i, &s) in chunk.iter().enumerate() {
            let bytes = s.to_le_bytes();
            payload[2 + i * 2] = bytes[0];
            payload[2 + i * 2 + 1] = bytes[1];
        }

        send_msg(port, MSG_AUDIO_CHUNK, &payload)?;

        // Wait for CHUNK_ACK (collecting any FRAMEs along the way)
        let _ack_payload = read_until(port, MSG_CHUNK_ACK, &mut esp_frames)?;

        // Progress every 100 chunks
        if (chunk_idx + 1) % 100 == 0 || chunk_idx + 1 == total_chunks {
            eprint!(
                "\r  Chunk {}/{} ({} frames so far)",
                chunk_idx + 1,
                total_chunks,
                esp_frames.len()
            );
        }
    }
    eprintln!();

    // Send STREAM_END
    send_msg(port, MSG_STREAM_END, &[])?;
    let stats_payload = read_until(port, MSG_STATS, &mut esp_frames)?;

    let stream_elapsed = stream_start.elapsed();

    // Print ESP32 results
    println!("\n=== ESP32 Results ===");
    println!("  Frames decoded: {}", esp_frames.len());
    println!("  Chunks processed: {}", total_chunks);

    if let Some(stats) = StatsPayload::parse(&stats_payload) {
        let avg = if stats.chunks > 0 {
            stats.total_cycles / stats.chunks as u64
        } else {
            0
        };
        let cycles_per_sample = if stats.chunks > 0 {
            avg as f64 / CHUNK_SAMPLES as f64
        } else {
            0.0
        };
        let cpu_freq = cpu_freq_mhz as f64 * 1_000_000.0;
        let samples_per_sec = sample_rate as f64;
        let cycles_available = cpu_freq / samples_per_sec;
        let utilization = cycles_per_sample / cycles_available * 100.0;

        println!("  Total cycles: {}", stats.total_cycles);
        println!(
            "  Cycles/chunk: min={} avg={} max={}",
            stats.min_cycles, avg, stats.max_cycles
        );
        println!("  Cycles/sample: {:.1}", cycles_per_sample);
        println!(
            "  CPU utilization: {:.1}% of {:.0} MHz @ {} Hz",
            utilization,
            cpu_freq / 1e6,
            sample_rate
        );
        println!(
            "  Real-time headroom: {:.1}x",
            if utilization > 0.0 {
                100.0 / utilization
            } else {
                f64::INFINITY
            }
        );
    }

    println!(
        "  Host wall time: {:.2}s (includes serial I/O)",
        stream_elapsed.as_secs_f64()
    );

    // Deduplicate ESP32 frames by hash
    let mut esp_hashes: Vec<u32> = Vec::new();
    let mut unique_esp_frames: Vec<Vec<u8>> = Vec::new();
    for f in &esp_frames {
        let h = frame_hash(&f.data);
        if !esp_hashes.contains(&h) {
            esp_hashes.push(h);
            unique_esp_frames.push(f.data.clone());
        }
    }
    println!("  Unique frames (by hash): {}", unique_esp_frames.len());

    // Local comparison
    if compare {
        println!("\n=== Local Comparison ===");
        let config = DemodConfig {
            sample_rate,
            ..DemodConfig::default_1200()
        };

        let mut local_mini = MiniDecoder::new(config);
        let mut local_frames: Vec<Vec<u8>> = Vec::new();

        for chunk in all_samples.chunks(CHUNK_SAMPLES) {
            let output = local_mini.process_samples(chunk);
            for i in 0..output.len() {
                local_frames.push(output.frame(i).to_vec());
            }
        }

        println!("  Local MiniDecoder: {} frames", local_frames.len());

        // Compare frame sets by hash
        let local_hashes: Vec<u32> = local_frames.iter().map(|f| frame_hash(f)).collect();

        let esp_only: Vec<u32> = esp_hashes
            .iter()
            .filter(|h| !local_hashes.contains(h))
            .copied()
            .collect();
        let local_only: Vec<u32> = local_hashes
            .iter()
            .filter(|h| !esp_hashes.contains(h))
            .copied()
            .collect();
        let common = esp_hashes
            .iter()
            .filter(|h| local_hashes.contains(h))
            .count();

        println!("  Common frames: {}", common);
        println!("  ESP32-only: {} frames", esp_only.len());
        println!("  Local-only: {} frames", local_only.len());

        if esp_only.is_empty() && local_only.is_empty() {
            println!("  MATCH: ESP32 and local produce identical frame sets");
        } else {
            println!(
                "  DIFF: {} frame(s) differ (see BPF coefficient note in docs)",
                esp_only.len() + local_only.len()
            );
        }
    }

    Ok(())
}

/// Reset the ESP32 via DTR/RTS serial control lines.
/// On DevKitC-1: RTS→EN (inverted), DTR→BOOT (inverted).
/// To normal-boot: hold DTR low (BOOT high), pulse RTS to reset.
fn reset_board(port: &mut Box<dyn SerialPort>) {
    let _ = port.write_data_terminal_ready(false); // BOOT = high (normal boot)
    let _ = port.write_request_to_send(true);      // EN = low (reset)
    std::thread::sleep(Duration::from_millis(100));
    let _ = port.write_request_to_send(false);     // EN = high (run)
    std::thread::sleep(Duration::from_millis(500)); // Wait for bootloader + firmware init
}

/// Drain any pending bytes from the serial port (startup text, leftovers).
fn drain_serial(port: &mut Box<dyn SerialPort>) {
    let old_timeout = port.timeout();
    let _ = port.set_timeout(Duration::from_millis(300));
    let mut buf = [0u8; 256];
    let mut total = 0;
    loop {
        match port.read(&mut buf) {
            Ok(n) if n > 0 => total += n,
            _ => break,
        }
    }
    let _ = port.set_timeout(old_timeout);
    if total > 0 {
        eprintln!("Drained {} bytes of pending data", total);
    }
}

fn main() {
    let cli = Cli::parse();

    // Open serial port
    let mut port = serialport::new(&cli.port, cli.baud)
        .timeout(SERIAL_TIMEOUT)
        .open()
        .unwrap_or_else(|e| {
            eprintln!("Failed to open {}: {}", cli.port, e);
            eprintln!("Hint: check that ESP32-C3 is connected and firmware is flashed");
            std::process::exit(1);
        });

    println!("Connected to {}", cli.port);

    // Reset the board to ensure clean state, then drain bootloader output
    // Skip reset for RP2040 USB-CDC (no DTR/RTS reset lines)
    if !cli.no_reset {
        reset_board(&mut port);
    }
    drain_serial(&mut port);

    if cli.ping {
        match do_ping(&mut port) {
            Ok(true) => println!("Connectivity OK"),
            Ok(false) => {
                eprintln!("Ping failed");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Ping error: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    if let Some(wav_path) = &cli.wav {
        match do_stream(&mut port, wav_path, &cli.mode, cli.sample_rate, cli.compare, cli.cpu_freq) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Stream error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("No action specified. Use --ping or --wav <file>");
        std::process::exit(1);
    }
}
