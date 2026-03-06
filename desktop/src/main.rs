//! Desktop Packet Radio TNC
//!
//! A full-featured TNC that runs on Linux, macOS, and Windows.
//! Uses the sound card for audio I/O and provides a KISS TCP
//! interface for connecting to APRS client software.

mod audio;
mod cli;
mod config;
mod kiss_server;
mod tui;

use clap::Parser;
use packet_radio_core::modem::demod::{CorrelationDemodulator, DemodSymbol, DmDemodulator, FastDemodulator, QualityDemodulator};
use packet_radio_core::modem::binary_xor::BinaryXorDemodulator;
use packet_radio_core::modem::corr_slicer::CorrSlicerDecoder;
use packet_radio_core::modem::multi::{MiniDecoder, MultiDecoder};
use packet_radio_core::modem::soft_hdlc::{SoftHdlcDecoder, FrameResult};
use packet_radio_core::modem::{DemodConfig, ModConfig};
use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::ax25::Frame;
use packet_radio_core::tnc::{AfskModulateAdapter, Fsk9600ModulateAdapter, NullDemod, TncConfig, TncEngine, TncPlatform};
use packet_radio_core::modem::demod_9600::Demod9600Config;
use packet_radio_core::modem::mod_9600::Mod9600Config;
use packet_radio_core::modem::multi_9600::{Mini9600Decoder, Multi9600Decoder, Single9600Decoder};
use packet_radio_core::kiss;
use packet_radio_core::aprs;
use packet_radio_core::SampleSource;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::sync::broadcast;

fn main() {
    let cli = cli::Cli::parse();

    // Init tracing — suppress stdout in TUI mode (TUI owns the terminal)
    let level = match cli.verbose {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };

    if !cli.is_headless() {
        tracing_subscriber::fmt()
            .with_max_level(level)
            .with_writer(std::io::sink)
            .init();
        run_tui_mode(cli);
    } else if cli.rx_pipe || cli.tx_pipe {
        tracing_subscriber::fmt()
            .with_max_level(level)
            .with_writer(std::io::stderr)
            .init();
        run_headless(cli);
    } else {
        tracing_subscriber::fmt()
            .with_max_level(level)
            .init();
        run_headless(cli);
    }
}

// ── TUI Mode ───────────────────────────────────────────────────────────

/// Launch the TUI — the default mode when no pipe/wav flags are set.
fn run_tui_mode(cli: cli::Cli) {
    // Resolve config path
    let config_path = config::TncConfig::config_path(cli.config.as_deref());
    let mut tnc_config = config::TncConfig::load_or_default(&config_path);

    // CLI flags override config file values
    if cli.device != "default" {
        tnc_config.audio.device = cli.device.clone();
    }
    if cli.sample_rate != 11025 {
        tnc_config.audio.sample_rate = cli.sample_rate;
    }
    if cli.mode != cli::DemodMode::Fast {
        // Fast is the CLI default — only override if user explicitly chose something
        tnc_config.modem.mode = cli.mode.as_str().to_string();
    }
    if cli.kiss_port != 8001 {
        tnc_config.kiss.port = cli.kiss_port;
    }

    // Enumerate audio devices with capability info
    let devices = tui::enumerate_audio_devices();

    // Channels: audio thread → TUI (crossbeam) and KISS broadcast (tokio)
    let (async_tx, async_rx) = crossbeam_channel::unbounded::<tui::state::AsyncEvent>();
    let (kiss_frame_tx, _) = broadcast::channel::<Vec<u8>>(64);
    let (kiss_in_tx, _kiss_in_rx) = crossbeam_channel::bounded::<Vec<u8>>(64);

    // Tokio runtime for KISS TCP + TUI event loop
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("fatal: failed to create async runtime: {e}");
            std::process::exit(1);
        }
    };

    // Start KISS TCP server
    let kiss_client_count = Arc::new(AtomicU32::new(0));
    if tnc_config.kiss.port > 0 {
        let tx = kiss_frame_tx.clone();
        let port = tnc_config.kiss.port;
        let kiss_in = kiss_in_tx.clone();
        let client_count = kiss_client_count.clone();
        rt.spawn(async move {
            kiss_server::run_bidirectional(port, tx, kiss_in, client_count).await;
        });
    }

    // Build the App
    let app = tui::App::new(tnc_config, config_path, devices, kiss_client_count);

    // Closure: spawn audio processing thread on demand
    let start_audio = {
        let async_tx = async_tx.clone();
        let kiss_tx = kiss_frame_tx.clone();
        move |cfg: &config::TncConfig| -> Result<(std::thread::JoinHandle<()>, Arc<AtomicBool>), String> {
            spawn_audio_thread(cfg, async_tx.clone(), kiss_tx.clone())
        }
    };

    // Run the TUI event loop (blocks until quit)
    if let Err(e) = rt.block_on(tui::run_tui(app, async_rx, start_audio)) {
        eprintln!("TUI error: {e}");
    }
}

/// Spawn an audio processing thread. Returns the thread handle and stop signal.
///
/// If `cfg.audio.wav_path` is set, opens a WAV file source. Otherwise opens
/// the live audio device (cpal). The WAV source breaks on EOF; live audio
/// sleeps and retries.
fn spawn_audio_thread(
    cfg: &config::TncConfig,
    async_tx: crossbeam_channel::Sender<tui::state::AsyncEvent>,
    kiss_frame_tx: broadcast::Sender<Vec<u8>>,
) -> Result<(std::thread::JoinHandle<()>, Arc<AtomicBool>), String> {
    let stop_signal = Arc::new(AtomicBool::new(false));
    let stop = stop_signal.clone();

    let device_name = cfg.audio.device.clone();
    let sample_rate = cfg.audio.sample_rate;
    let baud_rate = cfg.modem.baud_rate;
    let mode = cfg.modem.mode.clone();
    let wav_path = cfg.audio.wav_path.clone();

    // Oneshot to report whether audio opened successfully
    let (result_tx, result_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);

    let handle = std::thread::spawn(move || {
        // For 9600 baud, enforce minimum sample rate
        let effective_rate = if baud_rate == 9600 && sample_rate < 44100 { 48000 } else { sample_rate };

        let is_wav = wav_path.is_some();
        let (source, effective_rate): (Box<dyn SampleSource>, u32) = if let Some(ref path) = wav_path {
            match audio::WavSource::open(path, effective_rate) {
                Ok(src) => {
                    // Use the WAV file's native sample rate for the demodulator
                    let wav_rate = src.sample_rate();
                    let _ = result_tx.send(Ok(()));
                    (Box::new(src), wav_rate)
                }
                Err(e) => {
                    let _ = result_tx.send(Err(e));
                    return;
                }
            }
        } else {
            match audio::CpalSource::open(&device_name, effective_rate) {
                Ok(src) => {
                    let _ = result_tx.send(Ok(()));
                    (Box::new(src), effective_rate)
                }
                Err(e) => {
                    let _ = result_tx.send(Err(e));
                    return;
                }
            }
        };

        if baud_rate == 9600 {
            let config_9600 = Demod9600Config::with_sample_rate(effective_rate);
            run_audio_loop_9600(source, config_9600, &mode, is_wav, &stop, &async_tx, &kiss_frame_tx);
        } else {
            let config = demod_config_for_rate(effective_rate, baud_rate);
            run_audio_loop(source, config, &mode, is_wav, &stop, &async_tx, &kiss_frame_tx);
        }
        let _ = async_tx.send(tui::state::AsyncEvent::AudioDone);
    });

    // Wait for the thread to report whether audio opened successfully
    match result_rx.recv() {
        Ok(Ok(())) => Ok((handle, stop_signal)),
        Ok(Err(e)) => {
            let _ = handle.join();
            Err(e)
        }
        Err(_) => Err("audio thread exited before reporting status".to_string()),
    }
}

/// Audio processing loop — runs in a background thread, checks `stop` flag.
/// When `is_wav` is true, break on EOF instead of sleeping.
fn run_audio_loop(
    mut source: Box<dyn SampleSource>,
    config: DemodConfig,
    mode: &str,
    is_wav: bool,
    stop: &AtomicBool,
    async_tx: &crossbeam_channel::Sender<tui::state::AsyncEvent>,
    kiss_frame_tx: &broadcast::Sender<Vec<u8>>,
) {
    let mut frame_count: u64 = 0;
    let mut audio_buf = [0i16; 1024];
    let demod_mode = cli::DemodMode::from_config_str(mode);
    let mut decoder = UnifiedDecoder::new(&demod_mode, config);

    loop {
        if stop.load(Ordering::Relaxed) { break; }
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        decoder.process(&audio_buf[..n], &mut |data| {
            frame_count += 1;
            emit_tui_frame(frame_count, data, async_tx, kiss_frame_tx);
        });
    }
}

/// Audio processing loop for 9600 baud G3RUH — runs in a background thread.
fn run_audio_loop_9600(
    mut source: Box<dyn SampleSource>,
    config: Demod9600Config,
    mode: &str,
    is_wav: bool,
    stop: &AtomicBool,
    async_tx: &crossbeam_channel::Sender<tui::state::AsyncEvent>,
    kiss_frame_tx: &broadcast::Sender<Vec<u8>>,
) {
    let mut frame_count: u64 = 0;
    let mut audio_buf = [0i16; 1024];

    match mode {
        "multi" => {
            let mut decoder = Multi9600Decoder::new(config);
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let output = decoder.process_samples(&audio_buf[..n]);
                for i in 0..output.len() {
                    frame_count += 1;
                    emit_tui_frame(frame_count, output.frame(i), async_tx, kiss_frame_tx);
                }
            }
        }
        "mini" => {
            let mut decoder = Mini9600Decoder::new(config);
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let output = decoder.process_samples(&audio_buf[..n]);
                for i in 0..output.len() {
                    frame_count += 1;
                    emit_tui_frame(frame_count, output.frame(i), async_tx, kiss_frame_tx);
                }
            }
        }
        _ => {
            // Single algorithm: direwolf, gardner, early-late, mm, rrc
            let mut decoder = match mode {
                "gardner" => Single9600Decoder::gardner(config),
                "early-late" => Single9600Decoder::early_late(config),
                "mm" => Single9600Decoder::mueller_muller(config),
                "rrc" => Single9600Decoder::rrc(config),
                _ => Single9600Decoder::direwolf(config),
            };
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let output = decoder.process_samples(&audio_buf[..n]);
                for i in 0..output.len() {
                    let (buf, len) = output.frame(i);
                    frame_count += 1;
                    emit_tui_frame(frame_count, &buf[..*len], async_tx, kiss_frame_tx);
                }
            }
        }
    }
}

/// Convert a decoded frame to `DecodedFrameInfo` and send to TUI + KISS.
fn emit_tui_frame(
    count: u64,
    data: &[u8],
    async_tx: &crossbeam_channel::Sender<tui::state::AsyncEvent>,
    kiss_frame_tx: &broadcast::Sender<Vec<u8>>,
) {
    let info = make_frame_info(count, data);
    let _ = async_tx.try_send(tui::state::AsyncEvent::FrameDecoded(Box::new(info)));
    let _ = kiss_frame_tx.send(data.to_vec());
}

/// Parse raw AX.25 bytes into a `DecodedFrameInfo` for the TUI.
fn make_frame_info(count: u64, data: &[u8]) -> tui::state::DecodedFrameInfo {
    let timestamp = chrono_lite_timestamp();

    if let Some(frame) = Frame::parse(data) {
        let src_ssid = format_address(&frame.src);
        let dest_ssid = format_address(&frame.dest);
        let via = format_via_path(&frame);

        let info_str = core::str::from_utf8(frame.info).unwrap_or("<binary>").to_string();

        let parsed = aprs::parse_packet(frame.info, frame.dest.callsign_str());

        let aprs_summary = parsed.as_ref().map(|pkt| match pkt {
            aprs::AprsPacket::Position { position, .. } => {
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                format!("Position: {lat:.4}, {lon:.4}")
            }
            aprs::AprsPacket::MicE { position, speed, course, .. } => {
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                format!("Mic-E: {lat:.4}, {lon:.4} {speed}kts {course}°")
            }
            aprs::AprsPacket::Message { addressee, text, .. } => {
                let to = core::str::from_utf8(addressee).unwrap_or("?");
                let msg = core::str::from_utf8(text).unwrap_or("?");
                format!("Msg to {to}: {msg}")
            }
            aprs::AprsPacket::Weather { weather, .. } => {
                let temp = weather.temperature.map(|t| format!("{t}F")).unwrap_or_default();
                let wind = weather.wind_speed.map(|s| format!("{s}mph")).unwrap_or_default();
                format!("Weather: {temp} {wind}")
            }
            aprs::AprsPacket::Object { name, live, position, .. } => {
                let n = core::str::from_utf8(name).unwrap_or("?");
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                let status = if *live { "live" } else { "killed" };
                format!("Object {n} ({status}): {lat:.4}, {lon:.4}")
            }
            aprs::AprsPacket::Item { name, live, position, .. } => {
                let n = core::str::from_utf8(name).unwrap_or("?");
                let lat = position.lat as f64 / 1_000_000.0;
                let lon = position.lon as f64 / 1_000_000.0;
                let status = if *live { "live" } else { "killed" };
                format!("Item {n} ({status}): {lat:.4}, {lon:.4}")
            }
            aprs::AprsPacket::Status { text, .. } => {
                let s = core::str::from_utf8(text).unwrap_or("?");
                format!("Status: {s}")
            }
            aprs::AprsPacket::Telemetry { sequence, .. } => {
                format!("Telemetry #{sequence}")
            }
            aprs::AprsPacket::ThirdParty { .. } => "Third-party".to_string(),
            aprs::AprsPacket::RawGps { parsed, .. } => {
                if let Some(ref nmea) = parsed {
                    if let Some(ref pos) = nmea.position {
                        let lat = pos.lat as f64 / 1_000_000.0;
                        let lon = pos.lon as f64 / 1_000_000.0;
                        format!("GPS: {lat:.4}, {lon:.4}")
                    } else {
                        "Raw GPS (no fix)".to_string()
                    }
                } else {
                    "Raw GPS".to_string()
                }
            }
            aprs::AprsPacket::Capabilities { .. } => "Capabilities".to_string(),
            aprs::AprsPacket::Query { query_type, .. } => {
                let q = core::str::from_utf8(query_type).unwrap_or("?");
                format!("Query: {q}")
            }
            aprs::AprsPacket::UserDefined { .. } => "User-defined".to_string(),
            aprs::AprsPacket::Unknown { .. } => "APRS".to_string(),
        });

        let aprs_data = parsed.map(|pkt| aprs_packet_to_data(&pkt));

        tui::state::DecodedFrameInfo {
            frame_number: count,
            timestamp,
            source: src_ssid,
            dest: dest_ssid,
            via,
            info: info_str,
            aprs_summary,
            aprs_data,
            raw_len: data.len(),
        }
    } else {
        tui::state::DecodedFrameInfo {
            frame_number: count,
            timestamp,
            source: "<raw>".to_string(),
            dest: String::new(),
            via: String::new(),
            info: hex_preview(data, 32),
            aprs_summary: None,
            aprs_data: None,
            raw_len: data.len(),
        }
    }
}

/// Format an APRS timestamp for display.
fn format_timestamp(ts: &aprs::Timestamp) -> String {
    match ts {
        aprs::Timestamp::Dhm { day, hour, minute } => format!("{day:02}{hour:02}{minute:02}z"),
        aprs::Timestamp::Hms { hour, minute, second } => format!("{hour:02}{minute:02}{second:02}h"),
        aprs::Timestamp::DhmLocal { day, hour, minute } => format!("{day:02}{hour:02}{minute:02}/"),
    }
}

/// Format compressed extra data for display.
fn format_compressed_extra(extra: &aprs::CompressedExtra) -> String {
    let mut parts = Vec::new();
    if let Some((cse, spd)) = extra.course_speed {
        parts.push(format!("{cse}°/{spd}kts"));
    }
    if let Some(alt) = extra.altitude {
        parts.push(format!("{alt}ft"));
    }
    if let Some(rng) = extra.range {
        parts.push(format!("{rng}mi"));
    }
    parts.join(" ")
}

/// Format a MessageType for display.
fn format_message_type(mt: &aprs::MessageType) -> &'static str {
    match mt {
        aprs::MessageType::Private => "Private",
        aprs::MessageType::Ack => "Ack",
        aprs::MessageType::Rej => "Rej",
        aprs::MessageType::Bulletin => "Bulletin",
        aprs::MessageType::Announcement => "Announcement",
        aprs::MessageType::Nws => "NWS",
    }
}

/// Convert a parsed core AprsPacket to an owned AprsData for the TUI.
fn aprs_packet_to_data(pkt: &aprs::AprsPacket) -> tui::state::AprsData {
    use tui::state::{AprsData, WeatherInfo};

    match pkt {
        aprs::AprsPacket::Position { position, symbol_table, symbol_code, comment, timestamp, compressed_extra } => {
            let lat = position.lat as f64 / 1_000_000.0;
            let lon = position.lon as f64 / 1_000_000.0;
            let comment_str = core::str::from_utf8(comment).unwrap_or("").to_string();
            let weather = aprs::parse_weather_from_comment(comment)
                .map(|w| WeatherInfo::from_core(&w));
            let comment_fields = aprs::parse_comment_fields(comment);
            AprsData::Position {
                lat,
                lon,
                symbol: (*symbol_table, *symbol_code),
                comment: comment_str,
                weather,
                timestamp: timestamp.as_ref().map(format_timestamp),
                altitude: comment_fields.altitude,
                compressed_extra: compressed_extra.as_ref().map(format_compressed_extra),
            }
        }
        aprs::AprsPacket::MicE { position, speed, course, symbol_table, symbol_code } => {
            AprsData::MicE {
                lat: position.lat as f64 / 1_000_000.0,
                lon: position.lon as f64 / 1_000_000.0,
                speed: *speed,
                course: *course,
                symbol: (*symbol_table, *symbol_code),
            }
        }
        aprs::AprsPacket::Message { addressee, text, message_no, message_type } => {
            AprsData::Message {
                addressee: core::str::from_utf8(addressee).unwrap_or("?").to_string(),
                text: core::str::from_utf8(text).unwrap_or("?").to_string(),
                message_no: message_no.map(|m| core::str::from_utf8(m).unwrap_or("").to_string()),
                message_type: format_message_type(message_type).to_string(),
            }
        }
        aprs::AprsPacket::Weather { weather, comment } => {
            AprsData::Weather {
                weather: WeatherInfo::from_core(weather),
                comment: core::str::from_utf8(comment).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Object { name, live, position, symbol_table, symbol_code, comment, timestamp } => {
            AprsData::Object {
                name: core::str::from_utf8(name).unwrap_or("?").to_string(),
                live: *live,
                lat: position.lat as f64 / 1_000_000.0,
                lon: position.lon as f64 / 1_000_000.0,
                symbol: (*symbol_table, *symbol_code),
                comment: core::str::from_utf8(comment).unwrap_or("").to_string(),
                timestamp: timestamp.as_ref().map(format_timestamp),
            }
        }
        aprs::AprsPacket::Item { name, live, position, symbol_table, symbol_code, comment } => {
            AprsData::Item {
                name: core::str::from_utf8(name).unwrap_or("?").to_string(),
                live: *live,
                lat: position.lat as f64 / 1_000_000.0,
                lon: position.lon as f64 / 1_000_000.0,
                symbol: (*symbol_table, *symbol_code),
                comment: core::str::from_utf8(comment).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Status { text, timestamp, maidenhead } => {
            AprsData::Status {
                text: core::str::from_utf8(text).unwrap_or("").to_string(),
                timestamp: timestamp.as_ref().map(format_timestamp),
                maidenhead: maidenhead.map(|m| core::str::from_utf8(m).unwrap_or("").to_string()),
            }
        }
        aprs::AprsPacket::Telemetry { sequence, analog, digital } => {
            AprsData::Telemetry {
                sequence: *sequence,
                analog: *analog,
                digital: *digital,
            }
        }
        aprs::AprsPacket::ThirdParty { data } => {
            AprsData::ThirdParty {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::RawGps { data, parsed } => {
            let (position, speed, course, altitude, satellites, fix_valid) =
                if let Some(ref nmea) = parsed {
                    let pos = nmea.position.as_ref().map(|p| {
                        (p.lat as f64 / 1_000_000.0, p.lon as f64 / 1_000_000.0)
                    });
                    let spd = nmea.speed_tenths_kts.map(|v| v as f64 / 10.0);
                    let crs = nmea.course_tenths_deg.map(|v| v as f64 / 10.0);
                    let alt = nmea.altitude_dm.map(|v| v as f64 / 10.0);
                    (pos, spd, crs, alt, nmea.satellites, nmea.fix_valid)
                } else {
                    (None, None, None, None, None, false)
                };
            AprsData::RawGps {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
                position,
                speed,
                course,
                altitude,
                satellites,
                fix_valid,
            }
        }
        aprs::AprsPacket::Capabilities { data } => {
            AprsData::Capabilities {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Query { query_type } => {
            AprsData::Query {
                query_type: core::str::from_utf8(query_type).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::UserDefined { data } => {
            AprsData::UserDefined {
                data: core::str::from_utf8(data).unwrap_or("").to_string(),
            }
        }
        aprs::AprsPacket::Unknown { dti, .. } => {
            AprsData::Unknown { dti: *dti }
        }
    }
}

// ── Headless Mode ──────────────────────────────────────────────────────

/// Original processing path — console output, no TUI.
fn run_headless(cli: cli::Cli) {
    // List devices and exit
    if cli.list_devices {
        audio::list_devices();
        return;
    }

    // TX pipe mode: read KISS from stdin, write raw PCM to stdout
    if cli.tx_pipe {
        process_loop_tx_pipe(cli.sample_rate, cli.baud);
        return;
    }

    // Build the tokio runtime for KISS TCP server
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("fatal: failed to create async runtime: {e}");
            std::process::exit(1);
        }
    };

    // Frame broadcast channel for KISS clients
    let (frame_tx, _) = broadcast::channel::<Vec<u8>>(64);

    // Crossbeam channel for client KISS bytes → TX pipeline
    let (kiss_in_tx, kiss_in_rx) = crossbeam_channel::bounded::<Vec<u8>>(64);

    // Start KISS TCP server on the tokio runtime
    if !cli.rx_pipe && cli.kiss_port > 0 {
        let tx = frame_tx.clone();
        let port = cli.kiss_port;
        let kiss_in = kiss_in_tx.clone();
        let client_count = Arc::new(AtomicU32::new(0));
        rt.spawn(async move {
            kiss_server::run_bidirectional(port, tx, kiss_in, client_count).await;
        });
    }

    // Build TX pipeline if --tx-wav is specified
    let tx_pipeline = cli.tx_wav.as_ref().map(|_| {
        let tx_rate = if cli.baud == 9600 && cli.sample_rate == 11025 { 48000 } else { cli.sample_rate };
        TxPipeline::new(kiss_in_rx.clone(), tx_rate, cli.baud)
    });

    // Open audio source (stdin source created first to allow WAV auto-detection)
    let effective_rate;
    let source: Box<dyn SampleSource> = if let Some(ref wav_path) = cli.wav {
        match audio::WavSource::open(wav_path, cli.sample_rate) {
            Ok(src) => {
                effective_rate = src.sample_rate();
                Box::new(src)
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else if cli.rx_pipe {
        let stdin_src = match audio::StdinSource::new() {
            Ok(src) => src,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        };
        if let Some(detected) = stdin_src.detected_sample_rate() {
            if detected != cli.sample_rate {
                tracing::info!(
                    "rx-pipe: detected WAV on stdin ({detected} Hz), overriding -s {}",
                    cli.sample_rate,
                );
            } else {
                tracing::info!("rx-pipe: detected WAV on stdin ({detected} Hz)");
            }
            effective_rate = detected;
        } else {
            tracing::info!("rx-pipe: raw PCM on stdin at {} Hz", cli.sample_rate);
            effective_rate = cli.sample_rate;
        }
        Box::new(stdin_src)
    } else {
        effective_rate = cli.sample_rate;
        match audio::CpalSource::open(&cli.device, cli.sample_rate) {
            Ok(src) => Box::new(src),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    };

    let config = demod_config_for_rate(effective_rate, cli.baud);

    // RX pipe mode: demod → KISS binary on stdout
    if cli.rx_pipe {
        // Always treat as finite source (break on EOF from stdin or WAV)
        process_loop_rx_pipe(
            source,
            config,
            true, // always finite — break on EOF
            &cli.mode,
        );
        return;
    }

    // Run the processing loop on the main thread.
    let tx_pipeline = if cli.auto_baud {
        // Auto-baud: run both 1200 + 9600 mini-decoders in parallel
        let sample_rate = if cli.sample_rate == 11025 { 48000 } else { cli.sample_rate };
        tracing::info!("auto-baud mode (1200+9600, sample rate {})", sample_rate);
        process_loop_auto_baud(source, frame_tx, cli.wav.is_some(), sample_rate, tx_pipeline)
    } else if cli.baud == 9600 {
        let sample_rate = if cli.sample_rate == 11025 { 48000 } else { cli.sample_rate };
        let config_9600 = Demod9600Config::with_sample_rate(sample_rate);
        tracing::info!("9600 baud mode (sample rate {})", sample_rate);
        if cli.mode == cli::DemodMode::Multi {
            process_loop_9600_multi(source, frame_tx, cli.wav.is_some(), config_9600, tx_pipeline)
        } else if cli.mini9600 {
            process_loop_9600_mini(source, frame_tx, cli.wav.is_some(), config_9600, tx_pipeline)
        } else {
            let algo = cli.algo_9600.as_deref().unwrap_or("direwolf");
            tracing::info!("9600 algorithm: {}", algo);
            process_loop_9600_single(source, frame_tx, cli.wav.is_some(), config_9600, algo, tx_pipeline)
        }
    } else {
        process_loop(
            source,
            frame_tx,
            cli.wav.is_some(),
            &cli.mode,
            effective_rate,
            cli.baud,
            tx_pipeline,
        )
    };

    // Give KISS TCP clients time to drain buffered frames before exiting
    if cli.wav.is_some() && cli.kiss_port > 0 {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    // Write TX audio to WAV if requested
    if let (Some(ref tx_wav_path), Some(pipeline)) = (&cli.tx_wav, &tx_pipeline) {
        let tx_rate = if cli.baud == 9600 && cli.sample_rate == 11025 { 48000 } else { cli.sample_rate };
        pipeline.write_wav(tx_wav_path, tx_rate);
    }
}

// ── TX Pipeline ─────────────────────────────────────────────────────────

/// Platform for TX-only TNC: always clear channel, no PTT, full duplex.
struct TxOnlyPlatform;

impl TncPlatform for TxOnlyPlatform {
    fn set_ptt(&mut self, _on: bool) {}
    fn channel_busy(&self) -> bool { false }
    fn random_byte(&self) -> u8 { 42 }
    fn now_ms(&self) -> u32 { 0 }
}

/// Inner TX engine — dispatches between AFSK (300/1200) and 9600 FSK.
enum TxEngine {
    Afsk(TncEngine<NullDemod, AfskModulateAdapter>),
    Fsk9600(TncEngine<NullDemod, Fsk9600ModulateAdapter>),
}

impl TxEngine {
    fn feed_kiss(&mut self, byte: u8) {
        match self {
            TxEngine::Afsk(e) => e.feed_kiss(byte),
            TxEngine::Fsk9600(e) => e.feed_kiss(byte),
        }
    }

    fn poll_tx(&mut self, out: &mut [i16], platform: &mut TxOnlyPlatform) -> usize {
        match self {
            TxEngine::Afsk(e) => e.poll_tx(out, platform),
            TxEngine::Fsk9600(e) => e.poll_tx(out, platform),
        }
    }
}

/// TX pipeline: wraps a TX-only TncEngine, accumulates modulated audio.
struct TxPipeline {
    engine: TxEngine,
    platform: TxOnlyPlatform,
    samples: Vec<i16>,
    kiss_rx: crossbeam_channel::Receiver<Vec<u8>>,
}

impl TxPipeline {
    fn new(kiss_rx: crossbeam_channel::Receiver<Vec<u8>>, sample_rate: u32, baud: u32) -> Self {
        let tnc_config = TncConfig {
            baud_rate: baud,
            full_duplex: true, // Skip CSMA
            txdelay: 25,       // 250ms preamble (shorter for testing)
            ..TncConfig::default()
        };

        let engine = if baud == 9600 {
            let mod_config = match sample_rate {
                44100 => Mod9600Config::default_44k(),
                _ => Mod9600Config::default_48k(),
            };
            TxEngine::Fsk9600(TncEngine::new(NullDemod, Fsk9600ModulateAdapter::new(mod_config), tnc_config))
        } else {
            let base = if baud == 300 { ModConfig::default_300() } else { ModConfig::default_1200() };
            let mod_config = ModConfig { sample_rate, ..base };
            TxEngine::Afsk(TncEngine::new(NullDemod, AfskModulateAdapter::new(mod_config), tnc_config))
        };

        Self {
            engine,
            platform: TxOnlyPlatform,
            samples: Vec::new(),
            kiss_rx,
        }
    }

    /// Drain KISS channel and generate TX audio.
    fn poll(&mut self) {
        // Feed any pending KISS bytes
        while let Ok(data) = self.kiss_rx.try_recv() {
            for &b in &data {
                self.engine.feed_kiss(b);
            }
        }

        // Generate TX audio
        let mut buf = [0i16; 1024];
        loop {
            let n = self.engine.poll_tx(&mut buf, &mut self.platform);
            if n == 0 {
                break;
            }
            self.samples.extend_from_slice(&buf[..n]);
        }
    }

    /// Write accumulated TX audio to WAV file.
    fn write_wav(&self, path: &std::path::Path, sample_rate: u32) {
        if self.samples.is_empty() {
            tracing::info!("no TX audio to write");
            return;
        }

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = match hound::WavWriter::create(path, spec) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("failed to create TX WAV: {e}");
                return;
            }
        };
        for &s in &self.samples {
            writer.write_sample(s).ok();
        }
        writer.finalize().ok();
        tracing::info!("wrote {} TX samples to {}", self.samples.len(), path.display());
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a DemodConfig for the given sample rate and baud rate.
fn demod_config_for_rate(rate: u32, baud: u32) -> DemodConfig {
    match baud {
        300 => {
            match rate {
                8000 => DemodConfig::default_300_8k(),
                _ => {
                    let mut c = DemodConfig::default_300();
                    c.sample_rate = rate;
                    c
                }
            }
        }
        _ => {
            match rate {
                22050 => DemodConfig::default_1200_22k(),
                44100 => DemodConfig::default_1200_44k(),
                _ => {
                    let mut c = DemodConfig::default_1200();
                    c.sample_rate = rate;
                    c
                }
            }
        }
    }
}


/// Format an AX.25 address as "CALL" or "CALL-SSID".
fn format_address(addr: &packet_radio_core::ax25::Address) -> String {
    let call = core::str::from_utf8(addr.callsign_str()).unwrap_or("?");
    if addr.ssid > 0 {
        format!("{call}-{}", addr.ssid)
    } else {
        call.to_string()
    }
}

/// Build a comma-separated digipeater path string from a parsed frame.
/// Each digipeater is formatted as "CALL[-SSID][*]".
fn format_via_path(frame: &Frame) -> String {
    let mut via = String::new();
    for i in 0..frame.num_digipeaters as usize {
        if !via.is_empty() { via.push(','); }
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
    via
}

// ── Process Loops ───────────────────────────────────────────────────────

// ── Unified Decoder ─────────────────────────────────────────────────────

/// Wraps all 1200/300 baud demodulator variants behind a single interface.
/// Callers provide a frame callback; the decoder handles symbol->HDLC internally.
#[allow(clippy::large_enum_variant)]
enum UnifiedDecoder {
    Multi(Box<MultiDecoder>),
    Smart3(Box<MiniDecoder>),
    CorrSlicer(Box<CorrSlicerDecoder>),
    /// Symbol-producing demodulators that feed through SoftHdlcDecoder.
    Soft {
        demod: SoftDemod,
        hdlc: SoftHdlcDecoder,
        symbols: [DemodSymbol; 1024],
    },
    /// Fast demodulator with hard HDLC (no soft decode).
    Fast {
        demod: FastDemodulator,
        hdlc: HdlcDecoder,
        symbols: [DemodSymbol; 1024],
    },
}

/// Symbol-producing demodulators that use soft HDLC.
enum SoftDemod {
    Quality(QualityDemodulator),
    Dm(DmDemodulator),
    Corr(CorrelationDemodulator),
    CorrPll(CorrelationDemodulator),
    Xor(BinaryXorDemodulator),
}

impl SoftDemod {
    fn process_samples(&mut self, samples: &[i16], symbols: &mut [DemodSymbol]) -> usize {
        match self {
            SoftDemod::Quality(d) => d.process_samples(samples, symbols),
            SoftDemod::Dm(d) => d.process_samples(samples, symbols),
            SoftDemod::Corr(d) | SoftDemod::CorrPll(d) => d.process_samples(samples, symbols),
            SoftDemod::Xor(d) => d.process_samples(samples, symbols),
        }
    }
}

impl UnifiedDecoder {
    fn new(mode: &cli::DemodMode, config: DemodConfig) -> Self {
        let zero_sym = DemodSymbol { bit: false, llr: 0 };
        match mode {
            cli::DemodMode::Multi => UnifiedDecoder::Multi(Box::new(MultiDecoder::new(config))),
            cli::DemodMode::Smart3 => UnifiedDecoder::Smart3(Box::new(MiniDecoder::new(config))),
            cli::DemodMode::CorrSlicer => {
                UnifiedDecoder::CorrSlicer(Box::new(CorrSlicerDecoder::new(config).with_adaptive_gain()))
            }
            cli::DemodMode::Quality => UnifiedDecoder::Soft {
                demod: SoftDemod::Quality(QualityDemodulator::new(config)),
                hdlc: SoftHdlcDecoder::new(),
                symbols: [zero_sym; 1024],
            },
            cli::DemodMode::Dm => UnifiedDecoder::Soft {
                demod: SoftDemod::Dm(DmDemodulator::with_bpf_pll(config)),
                hdlc: SoftHdlcDecoder::new(),
                symbols: [zero_sym; 1024],
            },
            cli::DemodMode::Corr => UnifiedDecoder::Soft {
                demod: SoftDemod::Corr(
                    CorrelationDemodulator::new(config).with_adaptive_gain().with_energy_llr(),
                ),
                hdlc: SoftHdlcDecoder::new(),
                symbols: [zero_sym; 1024],
            },
            cli::DemodMode::CorrPll => UnifiedDecoder::Soft {
                demod: SoftDemod::CorrPll(
                    CorrelationDemodulator::new(config)
                        .with_adaptive_gain()
                        .with_energy_llr()
                        .with_pll(),
                ),
                hdlc: SoftHdlcDecoder::new(),
                symbols: [zero_sym; 1024],
            },
            cli::DemodMode::Xor => UnifiedDecoder::Soft {
                demod: SoftDemod::Xor(BinaryXorDemodulator::new(config)),
                hdlc: SoftHdlcDecoder::new(),
                symbols: [zero_sym; 1024],
            },
            cli::DemodMode::Fast => UnifiedDecoder::Fast {
                demod: FastDemodulator::new(config),
                hdlc: HdlcDecoder::new(),
                symbols: [zero_sym; 1024],
            },
        }
    }

    /// Process audio samples and call `emit` for each decoded frame.
    fn process(&mut self, samples: &[i16], emit: &mut dyn FnMut(&[u8])) {
        match self {
            UnifiedDecoder::Multi(dec) => {
                let output = dec.process_samples(samples);
                for i in 0..output.len() {
                    emit(output.frame(i));
                }
            }
            UnifiedDecoder::Smart3(dec) => {
                let output = dec.process_samples(samples);
                for i in 0..output.len() {
                    emit(output.frame(i));
                }
            }
            UnifiedDecoder::CorrSlicer(dec) => {
                let output = dec.process_samples(samples);
                for i in 0..output.len() {
                    emit(output.frame(i));
                }
            }
            UnifiedDecoder::Soft { demod, hdlc, symbols } => {
                let ns = demod.process_samples(samples, symbols);
                for sym in &symbols[..ns] {
                    if let Some(result) = hdlc.feed_soft_bit(sym.llr) {
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        emit(data);
                    }
                }
            }
            UnifiedDecoder::Fast { demod, hdlc, symbols } => {
                let ns = demod.process_samples(samples, symbols);
                for sym in &symbols[..ns] {
                    if let Some(f) = hdlc.feed_bit(sym.bit) {
                        emit(f);
                    }
                }
            }
        }
    }
}

// ── Process Loops ───────────────────────────────────────────────────────

/// Main DSP processing loop. Returns the TX pipeline (if any) for WAV writing.
fn process_loop(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    mode: &cli::DemodMode,
    sample_rate: u32,
    baud_rate: u32,
    mut tx_pipeline: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let config = demod_config_for_rate(sample_rate, baud_rate);
    let mut decoder = UnifiedDecoder::new(mode, config);
    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    tracing::info!("using {} demodulator at {} Hz", mode.as_str(), sample_rate);

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!("WAV file complete, decoded {frame_count} frames");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx_pipeline {
            pipeline.poll();
        }

        decoder.process(&audio_buf[..n], &mut |data| {
            frame_count += 1;
            let frame_data = data.to_vec();
            print_frame(frame_count, &frame_data);
            let _ = frame_tx.send(frame_data);
        });
    }

    if let Some(ref mut pipeline) = tx_pipeline {
        pipeline.poll();
    }
    tx_pipeline
}

// ── RX Pipe Mode ────────────────────────────────────────────────────────

/// RX pipe mode: demodulate audio -> KISS binary on stdout.
fn process_loop_rx_pipe(
    mut source: Box<dyn SampleSource>,
    config: DemodConfig,
    is_wav: bool,
    mode: &cli::DemodMode,
) {
    use std::io::Write;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut kiss_buf = [0u8; 1024];
    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    let mut emit_frame = |data: &[u8]| {
        frame_count += 1;
        if let Some(len) = kiss::encode_frame(0, data, &mut kiss_buf) {
            let _ = out.write_all(&kiss_buf[..len]);
            let _ = out.flush();
        }
    };

    let mut decoder = UnifiedDecoder::new(mode, config);
    tracing::info!("rx-pipe: {} demodulator", mode.as_str());

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        decoder.process(&audio_buf[..n], &mut |data| emit_frame(data));
    }

    tracing::info!("rx-pipe: done, output {frame_count} frames");
}

// ── TX Pipe Mode ────────────────────────────────────────────────────────

/// TX pipe mode: read KISS from stdin, write raw i16 LE PCM to stdout.
fn process_loop_tx_pipe(sample_rate: u32, baud: u32) {
    use std::io::{Read, Write};

    let tnc_config = TncConfig {
        baud_rate: baud,
        full_duplex: true,
        txdelay: 25,
        ..TncConfig::default()
    };

    let mut engine: TxEngine = if baud == 9600 {
        let mod_config = match sample_rate {
            44100 => Mod9600Config::default_44k(),
            _ => Mod9600Config::default_48k(),
        };
        TxEngine::Fsk9600(TncEngine::new(NullDemod, Fsk9600ModulateAdapter::new(mod_config), tnc_config))
    } else {
        let base = if baud == 300 { ModConfig::default_300() } else { ModConfig::default_1200() };
        let mod_config = ModConfig { sample_rate, ..base };
        TxEngine::Afsk(TncEngine::new(NullDemod, AfskModulateAdapter::new(mod_config), tnc_config))
    };
    let mut platform = TxOnlyPlatform;

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();
    let mut read_buf = [0u8; 4096];
    let mut tx_buf = [0i16; 1024];

    tracing::info!("tx-pipe: reading KISS from stdin, writing PCM to stdout ({sample_rate} Hz)");

    loop {
        // Read KISS bytes from stdin
        let n = match stdin_lock.read(&mut read_buf) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(_) => break,
        };

        // Feed all KISS bytes to engine
        for &b in &read_buf[..n] {
            engine.feed_kiss(b);
        }

        // Generate TX audio and write to stdout
        loop {
            let samples = engine.poll_tx(&mut tx_buf, &mut platform);
            if samples == 0 {
                break;
            }
            // Write i16 LE samples as raw bytes
            for &s in &tx_buf[..samples] {
                let _ = stdout_lock.write_all(&s.to_le_bytes());
            }
        }
    }

    // Drain any remaining TX audio
    loop {
        let samples = engine.poll_tx(&mut tx_buf, &mut platform);
        if samples == 0 {
            break;
        }
        for &s in &tx_buf[..samples] {
            let _ = stdout_lock.write_all(&s.to_le_bytes());
        }
    }

    let _ = stdout_lock.flush();
    tracing::info!("tx-pipe: done");
}

// ── 9600 Baud Process Loops ─────────────────────────────────────────────

/// 9600 baud single-algorithm processing loop.
fn process_loop_9600_single(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: Demod9600Config,
    algo: &str,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut decoder = match algo {
        "gardner" => Single9600Decoder::gardner(config),
        "early-late" => Single9600Decoder::early_late(config),
        "mm" => Single9600Decoder::mueller_muller(config),
        "rrc" => Single9600Decoder::rrc(config),
        _ => Single9600Decoder::direwolf(config), // default
    };

    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!("WAV file complete, decoded {frame_count} frames (9600 baud)");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let output = decoder.process_samples(&audio_buf[..n]);
        for i in 0..output.len() {
            let (buf, len) = output.frame(i);
            frame_count += 1;
            let frame_data = buf[..*len].to_vec();
            print_frame(frame_count, &frame_data);
            let _ = frame_tx.send(frame_data);
        }
    }

    if let Some(ref mut pipeline) = tx { pipeline.poll(); }
    tx
}

/// 9600 baud multi-decoder processing loop.
fn process_loop_9600_multi(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: Demod9600Config,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut decoder = Multi9600Decoder::new(config);
    tracing::info!("9600 multi-decoder: {} parallel decoders", decoder.num_decoders());

    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!("WAV file complete, decoded {frame_count} unique frames (9600 baud multi)");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let output = decoder.process_samples(&audio_buf[..n]);
        for i in 0..output.len() {
            frame_count += 1;
            let frame_data = output.frame(i).to_vec();
            print_frame(frame_count, &frame_data);
            let _ = frame_tx.send(frame_data);
        }
    }

    if let Some(ref mut pipeline) = tx { pipeline.poll(); }
    tx
}

/// 9600 baud Mini9600 decoder processing loop (6 MCU-optimal decoders).
fn process_loop_9600_mini(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: Demod9600Config,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut decoder = Mini9600Decoder::new(config);
    tracing::info!("9600 mini-decoder: {} parallel decoders", decoder.num_decoders());

    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!("WAV file complete, decoded {frame_count} unique frames (9600 mini)");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let output = decoder.process_samples(&audio_buf[..n]);
        for i in 0..output.len() {
            frame_count += 1;
            let frame_data = output.frame(i).to_vec();
            print_frame(frame_count, &frame_data);
            let _ = frame_tx.send(frame_data);
        }
    }

    if let Some(ref mut pipeline) = tx { pipeline.poll(); }
    tx
}

/// Auto-baud processing loop: 1200 MiniDecoder + 9600 Mini9600Decoder in parallel.
fn process_loop_auto_baud(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    sample_rate: u32,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    // 1200 baud decoder (MiniDecoder — 3 Goertzel decoders)
    let config_1200 = match sample_rate {
        22050 => DemodConfig::default_1200_22k(),
        44100 => DemodConfig::default_1200_44k(),
        48000 => DemodConfig { sample_rate: 48000, ..DemodConfig::default_1200() },
        _ => DemodConfig::default_1200(),
    };
    let mut decoder_1200 = MiniDecoder::new(config_1200);

    // 9600 baud decoder (Mini9600Decoder — 6 decoders)
    let config_9600 = Demod9600Config::with_sample_rate(sample_rate);
    let mut decoder_9600 = Mini9600Decoder::new(config_9600);

    tracing::info!(
        "auto-baud: {} 1200-baud + {} 9600-baud decoders",
        3, // MiniDecoder is always 3
        decoder_9600.num_decoders(),
    );

    // Cross-architecture dedup ring (FNV-1a hashes + generation)
    let mut recent_hashes: [(u64, u32); 32] = [(0, 0); 32];
    let mut recent_write: usize = 0;
    let mut recent_count: usize = 0;
    let mut generation: u32 = 0;

    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!("WAV file complete, decoded {frame_count} unique frames (auto-baud)");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }
        generation = generation.wrapping_add(1);

        let samples = &audio_buf[..n];

        // Run 1200 baud decoder
        let output_1200 = decoder_1200.process_samples(samples);
        for i in 0..output_1200.len() {
            let data = output_1200.frame(i);
            let hash = fnv1a_hash(data);
            if !is_recent_dup(hash, generation, &recent_hashes, recent_count) {
                recent_hashes[recent_write] = (hash, generation);
                recent_write = (recent_write + 1) % recent_hashes.len();
                if recent_count < recent_hashes.len() { recent_count += 1; }
                frame_count += 1;
                let frame_data = data.to_vec();
                print_frame(frame_count, &frame_data);
                let _ = frame_tx.send(frame_data);
            }
        }

        // Run 9600 baud decoder
        let output_9600 = decoder_9600.process_samples(samples);
        for i in 0..output_9600.len() {
            let data = output_9600.frame(i);
            let hash = fnv1a_hash(data);
            if !is_recent_dup(hash, generation, &recent_hashes, recent_count) {
                recent_hashes[recent_write] = (hash, generation);
                recent_write = (recent_write + 1) % recent_hashes.len();
                if recent_count < recent_hashes.len() { recent_count += 1; }
                frame_count += 1;
                let frame_data = data.to_vec();
                print_frame(frame_count, &frame_data);
                let _ = frame_tx.send(frame_data);
            }
        }
    }

    if let Some(ref mut pipeline) = tx { pipeline.poll(); }
    tx
}

/// FNV-1a 64-bit hash for frame dedup.
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Check if a hash was seen recently (within 3 generations).
fn is_recent_dup(hash: u64, gen: u32, ring: &[(u64, u32); 32], count: usize) -> bool {
    for &(h, g) in &ring[..count] {
        if h == hash && gen.wrapping_sub(g) < 3 {
            return true;
        }
    }
    false
}

// ── Formatting ──────────────────────────────────────────────────────────

/// Format and print a decoded frame to the console.
fn print_frame(count: u64, data: &[u8]) {
    let now = chrono_lite_timestamp();

    if let Some(frame) = Frame::parse(data) {
        let src_ssid = format_address(&frame.src);
        let dest_ssid = format_address(&frame.dest);
        let via = format_via_path(&frame);
        let via_prefix = if via.is_empty() { String::new() } else { format!(",{via}") };

        let info = core::str::from_utf8(frame.info).unwrap_or("<binary>");

        println!("[{now}] #{count} {src_ssid}>{dest_ssid}{via_prefix}: {info}");

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
