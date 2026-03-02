use std::io::Read;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use clap::Parser;
use packet_radio_core::aprs;
use packet_radio_core::kiss::{Command, KissDecoder};

/// KISS TCP client — connect to a TNC and dump decoded frames to stdout
#[derive(Parser)]
#[command(name = "kiss-dump")]
struct Cli {
    /// TNC KISS TCP address (e.g. localhost:8001)
    address: String,

    /// Show APRS-parsed summary after each frame
    #[arg(short, long)]
    aprs: bool,

    /// Show raw hex bytes instead of TNC2 format
    #[arg(short, long)]
    raw: bool,

    /// Print frame count to stderr on exit
    #[arg(short, long)]
    count: bool,

    /// Suppress connection status messages
    #[arg(short, long)]
    quiet: bool,
}

fn main() {
    let cli = Cli::parse();

    let running = Arc::new(AtomicBool::new(true));
    let frame_count = Arc::new(AtomicU32::new(0));

    // Ctrl+C handler
    {
        let running = running.clone();
        let frame_count = frame_count.clone();
        let show_count = cli.count;
        ctrlc::set_handler(move || {
            if show_count {
                eprintln!("{} frames", frame_count.load(Ordering::Relaxed));
            }
            running.store(false, Ordering::Relaxed);
            std::process::exit(0);
        })
        .expect("Failed to set Ctrl+C handler");
    }

    if !cli.quiet {
        eprintln!("Connecting to {}...", cli.address);
    }

    let mut stream = match TcpStream::connect(&cli.address) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to {}: {}", cli.address, e);
            std::process::exit(1);
        }
    };

    if !cli.quiet {
        eprintln!("Connected.");
    }

    let mut decoder = KissDecoder::new();
    let mut buf = [0u8; 4096];

    loop {
        if !running.load(Ordering::Relaxed) {
            break;
        }

        let n = match stream.read(&mut buf) {
            Ok(0) => break, // EOF — TNC closed connection
            Ok(n) => n,
            Err(e) => {
                if !cli.quiet {
                    eprintln!("Read error: {}", e);
                }
                break;
            }
        };

        for &byte in &buf[..n] {
            if let Some((_port, cmd, data)) = decoder.feed_byte(byte) {
                if cmd != Command::DataFrame {
                    continue;
                }
                frame_count.fetch_add(1, Ordering::Relaxed);

                if cli.raw {
                    let hex: Vec<String> = data.iter().map(|b| format!("{:02X}", b)).collect();
                    println!("{}", hex.join(" "));
                } else if let Some(tnc2) = frame_to_tnc2(data) {
                    if cli.aprs {
                        let aprs_summary = parse_aprs_summary(data);
                        println!("{}{}", tnc2, aprs_summary);
                    } else {
                        println!("{}", tnc2);
                    }
                } else {
                    let hex: Vec<String> = data.iter().map(|b| format!("{:02X}", b)).collect();
                    println!("[raw] {}", hex.join(" "));
                }
            }
        }
    }

    if cli.count {
        eprintln!("{} frames", frame_count.load(Ordering::Relaxed));
    }
}

// ─── TNC2 Formatting ────────────────────────────────────────────────────────

/// Convert raw AX.25 frame bytes to TNC2 format string.
fn frame_to_tnc2(frame: &[u8]) -> Option<String> {
    if frame.len() < 16 {
        return None;
    }

    let dst = parse_callsign_tnc2(&frame[0..7]);
    let src = parse_callsign_tnc2(&frame[7..14]);

    let mut result = format!("{}>{}", src, dst);

    struct ViaEntry {
        callsign: String,
        h_bit: bool,
    }
    let mut vias = Vec::new();
    let mut pos = 14;
    let mut addr_end = (frame[13] & 0x01) != 0;

    while !addr_end && pos + 7 <= frame.len() {
        let h_bit = (frame[pos + 6] & 0x80) != 0;
        let callsign = parse_callsign_tnc2(&frame[pos..pos + 7]);
        vias.push(ViaEntry { callsign, h_bit });
        addr_end = (frame[pos + 6] & 0x01) != 0;
        pos += 7;
    }

    // TNC2 format: `*` only on the last digipeater with H-bit set
    let last_h = vias.iter().rposition(|v| v.h_bit);
    for (i, via) in vias.iter().enumerate() {
        result.push(',');
        result.push_str(&via.callsign);
        if Some(i) == last_h {
            result.push('*');
        }
    }

    if pos + 2 > frame.len() {
        return None;
    }
    pos += 2; // skip control + PID

    result.push(':');

    // Strip control characters to match Dire Wolf packets.txt format
    let info = &frame[pos..];
    let cleaned: Vec<u8> = info
        .iter()
        .copied()
        .filter(|&b| b >= 0x20 || b == 0x09)
        .collect();
    let info_str = String::from_utf8_lossy(&cleaned);
    result.push_str(&info_str);
    Some(result)
}

/// Parse callsign from AX.25 address field for TNC2 display.
fn parse_callsign_tnc2(data: &[u8]) -> String {
    if data.len() < 7 {
        return "???".to_string();
    }
    let mut call = String::with_capacity(10);
    for &b in &data[..6] {
        let c = (b >> 1) & 0x7F;
        if c > 0x20 {
            call.push(c as char);
        }
    }
    let ssid = (data[6] >> 1) & 0x0F;
    if ssid > 0 {
        call.push_str(&format!("-{}", ssid));
    }
    call
}

// ─── APRS Summary ───────────────────────────────────────────────────────────

/// Parse APRS info from an AX.25 frame and return a summary string.
fn parse_aprs_summary(frame: &[u8]) -> String {
    if frame.len() < 16 {
        return String::new();
    }

    // Extract destination callsign (raw bytes, shifted)
    let mut dest = [0u8; 6];
    for i in 0..6 {
        dest[i] = (frame[i] >> 1) & 0x7F;
    }

    // Find info field start
    let mut pos = 14;
    let mut addr_end = (frame[13] & 0x01) != 0;
    while !addr_end && pos + 7 <= frame.len() {
        addr_end = (frame[pos + 6] & 0x01) != 0;
        pos += 7;
    }
    if pos + 2 > frame.len() {
        return String::new();
    }
    pos += 2;

    let info = &frame[pos..];
    match aprs::parse_packet(info, &dest) {
        Some(pkt) => format!("  [{}]", aprs_type_summary(&pkt)),
        None => String::new(),
    }
}

/// One-line summary of an APRS packet type.
fn aprs_type_summary(pkt: &aprs::AprsPacket) -> String {
    match pkt {
        aprs::AprsPacket::Position {
            position, comment, ..
        } => {
            let c = String::from_utf8_lossy(comment);
            format!(
                "Pos {:.4},{:.4} {}",
                position.lat, position.lon, c
            )
        }
        aprs::AprsPacket::MicE {
            position,
            speed,
            course,
            ..
        } => {
            format!(
                "Mic-E {:.4},{:.4} {}kn {}deg",
                position.lat, position.lon, speed, course
            )
        }
        aprs::AprsPacket::Message {
            addressee, text, ..
        } => {
            let a = String::from_utf8_lossy(addressee);
            let t = String::from_utf8_lossy(text);
            format!("Msg to {}: {}", a.trim(), t)
        }
        aprs::AprsPacket::Status { text, .. } => {
            let t = String::from_utf8_lossy(text);
            format!("Status: {}", t)
        }
        aprs::AprsPacket::Weather { .. } => "Weather".to_string(),
        aprs::AprsPacket::Object { name, .. } => {
            let n = String::from_utf8_lossy(name);
            format!("Object: {}", n.trim())
        }
        aprs::AprsPacket::Item { name, .. } => {
            let n = String::from_utf8_lossy(name);
            format!("Item: {}", n.trim())
        }
        aprs::AprsPacket::Telemetry { sequence, .. } => format!("Telemetry #{}", sequence),
        aprs::AprsPacket::ThirdParty { .. } => "Third-party".to_string(),
        aprs::AprsPacket::RawGps { .. } => "Raw GPS".to_string(),
        aprs::AprsPacket::Capabilities { .. } => "Capabilities".to_string(),
        aprs::AprsPacket::Query { .. } => "Query".to_string(),
        aprs::AprsPacket::UserDefined { .. } => "User-defined".to_string(),
        aprs::AprsPacket::Unknown { dti, .. } => format!("Unknown DTI 0x{:02X}", dti),
    }
}
