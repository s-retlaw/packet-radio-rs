//! Desktop Packet Radio TNC
//!
//! A full-featured TNC that runs on Linux, macOS, and Windows.
//! Uses the sound card for audio I/O and provides a KISS TCP
//! interface for connecting to APRS client software.

mod audio;
mod cli;
mod kiss_server;

use clap::Parser;
use packet_radio_core::modem::demod::{DemodSymbol, FastDemodulator, QualityDemodulator};
use packet_radio_core::modem::multi::MultiDecoder;
use packet_radio_core::modem::soft_hdlc::{SoftHdlcDecoder, FrameResult};
use packet_radio_core::modem::DemodConfig;
use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::ax25::Frame;
use packet_radio_core::aprs;
use packet_radio_core::SampleSource;
use tokio::sync::broadcast;

fn main() {
    let cli = cli::Cli::parse();

    // Init tracing
    let level = match cli.verbose {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .init();

    // List devices and exit
    if cli.list_devices {
        audio::list_devices();
        return;
    }

    // Build the tokio runtime for KISS TCP server
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    // Frame broadcast channel for KISS clients
    let (frame_tx, _) = broadcast::channel::<Vec<u8>>(64);

    // Start KISS TCP server on the tokio runtime
    if cli.kiss_port > 0 {
        let tx = frame_tx.clone();
        let port = cli.kiss_port;
        rt.spawn(async move {
            kiss_server::run_with_sender(port, tx).await;
        });
    }

    // Open audio source
    let source: Box<dyn SampleSource> = if let Some(ref wav_path) = cli.wav {
        match audio::WavSource::open(wav_path, cli.sample_rate) {
            Ok(src) => Box::new(src),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match audio::CpalSource::open(&cli.device, cli.sample_rate) {
            Ok(src) => Box::new(src),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    };

    // Run the processing loop on the main thread.
    // The KISS TCP server runs on tokio background threads.
    // cpal::Stream is !Send so we can't move the source across threads.
    process_loop(
        source,
        frame_tx,
        cli.wav.is_some(),
        cli.quality,
        cli.multi,
        cli.sample_rate,
    );
}

/// Main DSP processing loop.
///
/// Reads audio samples, demodulates, decodes HDLC frames, prints to
/// console, and broadcasts to KISS TCP clients.
fn process_loop(
    source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    use_quality: bool,
    use_multi: bool,
    sample_rate: u32,
) {
    let config = match sample_rate {
        22050 => DemodConfig::default_1200_22k(),
        44100 => DemodConfig::default_1200_44k(),
        _ => DemodConfig::default_1200(),
    };

    if use_multi {
        tracing::info!("using multi-decoder (9 parallel decoders)");
        process_loop_multi(source, frame_tx, is_wav, config);
    } else {
        process_loop_single(source, frame_tx, is_wav, use_quality, config);
    }
}

/// Multi-decoder processing loop.
fn process_loop_multi(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
) {
    let mut multi = MultiDecoder::new(config);
    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    tracing::info!("processing audio at {} Hz", config.sample_rate);

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!(
                    "WAV file complete, decoded {} unique frames ({} total from {} decoders)",
                    multi.total_unique, multi.total_decoded, multi.num_decoders()
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        let output = multi.process_samples(&audio_buf[..n]);
        for i in 0..output.len() {
            frame_count += 1;
            let frame_data = output.frame(i).to_vec();
            print_frame(frame_count, &frame_data);
            let _ = frame_tx.send(frame_data);
        }
    }
}

/// Single-decoder processing loop (fast or quality).
fn process_loop_single(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    use_quality: bool,
    config: DemodConfig,
) {
    // We use an enum to avoid boxing the demodulator in the hot loop
    enum Demod {
        Fast(FastDemodulator),
        Quality(QualityDemodulator),
    }

    let mut demod = if use_quality {
        tracing::info!("using quality demodulator");
        Demod::Quality(QualityDemodulator::new(config))
    } else {
        tracing::info!("using fast demodulator");
        Demod::Fast(FastDemodulator::new(config))
    };

    // Use an enum to avoid boxing the HDLC decoder
    enum Hdlc {
        Hard(HdlcDecoder),
        Soft(SoftHdlcDecoder),
    }

    let mut hdlc = if use_quality {
        Hdlc::Soft(SoftHdlcDecoder::new())
    } else {
        Hdlc::Hard(HdlcDecoder::new())
    };

    let mut audio_buf = [0i16; 1024];
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
    let mut frame_count: u64 = 0;
    let mut soft_saves: u32 = 0;

    tracing::info!("processing audio at {} Hz", config.sample_rate);

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                if soft_saves > 0 {
                    tracing::info!("WAV file complete, decoded {frame_count} frames ({soft_saves} soft recoveries)");
                } else {
                    tracing::info!("WAV file complete, decoded {frame_count} frames");
                }
                break;
            }
            // Live mode: shouldn't happen, but sleep briefly
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        let num_symbols = match &mut demod {
            Demod::Fast(d) => d.process_samples(&audio_buf[..n], &mut symbols),
            Demod::Quality(d) => d.process_samples(&audio_buf[..n], &mut symbols),
        };

        for i in 0..num_symbols {
            let frame_data = match &mut hdlc {
                Hdlc::Hard(h) => h.feed_bit(symbols[i].bit).map(|f| f.to_vec()),
                Hdlc::Soft(s) => {
                    s.feed_soft_bit(symbols[i].llr).map(|result| {
                        match &result {
                            FrameResult::Recovered { flips, .. } => {
                                soft_saves += 1;
                                tracing::debug!("soft recovery: {} bit(s) corrected", flips);
                            }
                            _ => {}
                        }
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        data.to_vec()
                    })
                }
            };

            if let Some(frame_data) = frame_data {
                frame_count += 1;

                // Print to console
                print_frame(frame_count, &frame_data);

                // Broadcast to KISS clients (ignore error if no receivers)
                let _ = frame_tx.send(frame_data);
            }
        }
    }
}

/// Format and print a decoded frame to the console.
fn print_frame(count: u64, data: &[u8]) {
    let now = chrono_lite_timestamp();

    if let Some(frame) = Frame::parse(data) {
        let src = core::str::from_utf8(frame.src.callsign_str()).unwrap_or("?");
        let dest = core::str::from_utf8(frame.dest.callsign_str()).unwrap_or("?");

        // Build via path
        let mut via = String::new();
        for i in 0..frame.num_digipeaters as usize {
            via.push(',');
            let digi = &frame.digipeaters[i];
            if let Ok(call) = core::str::from_utf8(digi.callsign_str()) {
                via.push_str(call);
            }
            if digi.ssid > 0 {
                via.push('-');
                via.push_str(&digi.ssid.to_string());
            }
            if digi.h_bit {
                via.push('*');
            }
        }

        // Format source SSID
        let src_ssid = if frame.src.ssid > 0 {
            format!("{src}-{}", frame.src.ssid)
        } else {
            src.to_string()
        };

        // Format dest SSID
        let dest_ssid = if frame.dest.ssid > 0 {
            format!("{dest}-{}", frame.dest.ssid)
        } else {
            dest.to_string()
        };

        let info = core::str::from_utf8(frame.info).unwrap_or("<binary>");

        println!("[{now}] #{count} {src_ssid}>{dest_ssid}{via}: {info}");

        // Try APRS parse for extra detail at debug level
        if let Some(pkt) = aprs::parse_packet(frame.info, frame.dest.callsign_str()) {
            match pkt {
                aprs::AprsPacket::Position { position, .. } => {
                    let lat = position.lat as f64 / 1_000_000.0;
                    let lon = position.lon as f64 / 1_000_000.0;
                    tracing::debug!("  APRS position: {lat:.4}, {lon:.4}");
                }
                aprs::AprsPacket::MicE { position, speed, course, .. } => {
                    let lat = position.lat as f64 / 1_000_000.0;
                    let lon = position.lon as f64 / 1_000_000.0;
                    tracing::debug!(
                        "  Mic-E: {lat:.4}, {lon:.4} speed={speed}kts course={course}°"
                    );
                }
                aprs::AprsPacket::Message { addressee, text, .. } => {
                    let to = core::str::from_utf8(addressee).unwrap_or("?");
                    let msg = core::str::from_utf8(text).unwrap_or("?");
                    tracing::debug!("  Message to {to}: {msg}");
                }
                _ => {}
            }
        }
    } else {
        // Couldn't parse AX.25 — show raw hex
        println!("[{now}] #{count} <raw {len} bytes: {hex}>",
            len = data.len(),
            hex = hex_preview(data, 32),
        );
    }
}

/// Simple timestamp without pulling in chrono.
fn chrono_lite_timestamp() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs();
            let hours = (secs / 3600) % 24;
            let mins = (secs / 60) % 60;
            let s = secs % 60;
            format!("{hours:02}:{mins:02}:{s:02}")
        }
        Err(_) => "??:??:??".to_string(),
    }
}

/// Hex preview of bytes (truncated to max_bytes).
fn hex_preview(data: &[u8], max_bytes: usize) -> String {
    let show = data.len().min(max_bytes);
    let mut s = String::with_capacity(show * 3);
    for (i, &b) in data[..show].iter().enumerate() {
        if i > 0 { s.push(' '); }
        s.push_str(&format!("{b:02X}"));
    }
    if data.len() > max_bytes {
        s.push_str("...");
    }
    s
}
