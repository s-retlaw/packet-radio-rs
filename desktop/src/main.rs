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
use std::sync::atomic::{AtomicBool, Ordering};
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
    let cli_mode = cli.demod_mode();
    if cli_mode != "fast" {
        // "fast" is the CLI default — only override if user explicitly chose something
        tnc_config.modem.mode = cli_mode.to_string();
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
    if tnc_config.kiss.port > 0 {
        let tx = kiss_frame_tx.clone();
        let port = tnc_config.kiss.port;
        let kiss_in = kiss_in_tx.clone();
        rt.spawn(async move {
            kiss_server::run_bidirectional(port, tx, kiss_in).await;
        });
    }

    // Build the App
    let app = tui::App::new(tnc_config, config_path, devices);

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

    match mode {
        "multi" => {
            let mut multi = MultiDecoder::new(config);
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let output = multi.process_samples(&audio_buf[..n]);
                for i in 0..output.len() {
                    frame_count += 1;
                    emit_tui_frame(frame_count, output.frame(i), async_tx, kiss_frame_tx);
                }
            }
        }
        "smart3" => {
            let mut mini = MiniDecoder::new(config);
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let output = mini.process_samples(&audio_buf[..n]);
                for i in 0..output.len() {
                    frame_count += 1;
                    emit_tui_frame(frame_count, output.frame(i), async_tx, kiss_frame_tx);
                }
            }
        }
        "corr-slicer" => {
            let mut decoder = CorrSlicerDecoder::new(config).with_adaptive_gain();
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
        "dm" => {
            let mut demod = DmDemodulator::with_bpf_pll(config);
            let mut soft_hdlc = SoftHdlcDecoder::new();
            let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                        frame_count += 1;
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        emit_tui_frame(frame_count, data, async_tx, kiss_frame_tx);
                    }
                }
            }
        }
        "xor" => {
            let mut demod = BinaryXorDemodulator::new(config);
            let mut soft_hdlc = SoftHdlcDecoder::new();
            let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                        frame_count += 1;
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        emit_tui_frame(frame_count, data, async_tx, kiss_frame_tx);
                    }
                }
            }
        }
        "corr" => {
            let mut demod = CorrelationDemodulator::new(config)
                .with_adaptive_gain()
                .with_energy_llr();
            let mut soft_hdlc = SoftHdlcDecoder::new();
            let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                        frame_count += 1;
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        emit_tui_frame(frame_count, data, async_tx, kiss_frame_tx);
                    }
                }
            }
        }
        "corr-pll" => {
            let mut demod = CorrelationDemodulator::new(config)
                .with_adaptive_gain()
                .with_energy_llr()
                .with_pll();
            let mut soft_hdlc = SoftHdlcDecoder::new();
            let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                        frame_count += 1;
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        emit_tui_frame(frame_count, data, async_tx, kiss_frame_tx);
                    }
                }
            }
        }
        "quality" => {
            let mut demod = QualityDemodulator::new(config);
            let mut soft_hdlc = SoftHdlcDecoder::new();
            let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                        frame_count += 1;
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        emit_tui_frame(frame_count, data, async_tx, kiss_frame_tx);
                    }
                }
            }
        }
        _ => {
            // Default: fast demodulator
            let mut demod = FastDemodulator::new(config);
            let mut hdlc = HdlcDecoder::new();
            let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(f) = hdlc.feed_bit(symbols[i].bit) {
                        frame_count += 1;
                        emit_tui_frame(frame_count, f, async_tx, kiss_frame_tx);
                    }
                }
            }
        }
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
    let _ = async_tx.try_send(tui::state::AsyncEvent::FrameDecoded(info));
    let _ = kiss_frame_tx.send(data.to_vec());
}

/// Parse raw AX.25 bytes into a `DecodedFrameInfo` for the TUI.
fn make_frame_info(count: u64, data: &[u8]) -> tui::state::DecodedFrameInfo {
    let timestamp = chrono_lite_timestamp();

    if let Some(frame) = Frame::parse(data) {
        let src = core::str::from_utf8(frame.src.callsign_str()).unwrap_or("?");
        let src_ssid = if frame.src.ssid > 0 {
            format!("{src}-{}", frame.src.ssid)
        } else {
            src.to_string()
        };

        let dest = core::str::from_utf8(frame.dest.callsign_str()).unwrap_or("?");
        let dest_ssid = if frame.dest.ssid > 0 {
            format!("{dest}-{}", frame.dest.ssid)
        } else {
            dest.to_string()
        };

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

        let info_str = core::str::from_utf8(frame.info).unwrap_or("<binary>").to_string();

        let aprs_summary = aprs::parse_packet(frame.info, frame.dest.callsign_str())
            .map(|pkt| match pkt {
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
                _ => "APRS".to_string(),
            });

        tui::state::DecodedFrameInfo {
            frame_number: count,
            timestamp,
            source: src_ssid,
            dest: dest_ssid,
            via,
            info: info_str,
            aprs_summary,
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
            raw_len: data.len(),
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
        rt.spawn(async move {
            kiss_server::run_bidirectional(port, tx, kiss_in).await;
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
            cli.quality,
            cli.multi,
            cli.dm,
            cli.smart3,
            cli.corr,
            cli.corr_slicer,
            cli.corr_pll,
            cli.xor,
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
        if cli.multi {
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
            cli.quality,
            cli.multi,
            cli.dm,
            cli.smart3,
            cli.corr,
            cli.corr_slicer,
            cli.corr_pll,
            cli.xor,
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

// ── Process Loops ───────────────────────────────────────────────────────

/// Main DSP processing loop. Returns the TX pipeline (if any) for WAV writing.
fn process_loop(
    source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    use_quality: bool,
    use_multi: bool,
    use_dm: bool,
    use_smart3: bool,
    use_corr: bool,
    use_corr_slicer: bool,
    use_corr_pll: bool,
    use_xor: bool,
    sample_rate: u32,
    baud_rate: u32,
    tx_pipeline: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let config = demod_config_for_rate(sample_rate, baud_rate);

    if use_multi {
        tracing::info!("using multi-decoder ({} parallel decoders)", {
            let m = MultiDecoder::new(config);
            m.num_decoders()
        });
        process_loop_multi(source, frame_tx, is_wav, config, tx_pipeline)
    } else if use_smart3 {
        tracing::info!("using smart3 mini-decoder (3 parallel decoders)");
        process_loop_smart3(source, frame_tx, is_wav, config, tx_pipeline)
    } else if use_corr_slicer {
        tracing::info!("using correlation multi-slicer demodulator ({} slicers)", {
            let d = CorrSlicerDecoder::new(config);
            d.num_slicers()
        });
        process_loop_corr_slicer(source, frame_tx, is_wav, config, tx_pipeline)
    } else if use_corr_pll {
        tracing::info!("using correlation demodulator + Gardner PLL");
        process_loop_corr_pll(source, frame_tx, is_wav, config, tx_pipeline)
    } else if use_corr {
        tracing::info!("using correlation (mixer) demodulator");
        process_loop_corr(source, frame_tx, is_wav, config, tx_pipeline)
    } else if use_dm {
        tracing::info!("using delay-multiply demodulator");
        process_loop_dm(source, frame_tx, is_wav, config, tx_pipeline)
    } else if use_xor {
        tracing::info!("using binary XOR correlator");
        process_loop_xor(source, frame_tx, is_wav, config, tx_pipeline)
    } else {
        process_loop_single(source, frame_tx, is_wav, use_quality, config, tx_pipeline)
    }
}

/// Multi-decoder processing loop.
fn process_loop_multi(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
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

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let output = multi.process_samples(&audio_buf[..n]);
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

/// Smart3 mini-decoder processing loop (3 attribution-optimal decoders).
fn process_loop_smart3(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut mini = MiniDecoder::new(config);
    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    tracing::info!("processing audio at {} Hz", config.sample_rate);

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!(
                    "WAV file complete, decoded {} unique frames ({} total from {} decoders)",
                    mini.total_unique, mini.total_decoded, mini.num_decoders()
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let output = mini.process_samples(&audio_buf[..n]);
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

/// Delay-multiply demodulator processing loop (Gardner PLL + soft HDLC).
fn process_loop_dm(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut demod = DmDemodulator::with_bpf_pll(config);
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut audio_buf = [0i16; 1024];
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
    let mut frame_count: u64 = 0;
    let mut soft_saves: u32 = 0;

    tracing::info!("processing audio at {} Hz (delay-multiply + Gardner PLL + soft HDLC)", config.sample_rate);

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
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let num_symbols = demod.process_samples(&audio_buf[..n], &mut symbols);
        for i in 0..num_symbols {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                if let FrameResult::Recovered { flips, .. } = &result {
                    soft_saves += 1;
                    tracing::debug!("soft recovery: {flips} bit(s) corrected");
                }
                let data = match &result {
                    FrameResult::Valid(d) => *d,
                    FrameResult::Recovered { data, .. } => *data,
                };
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

/// Binary XOR correlator processing loop + soft HDLC.
fn process_loop_xor(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut demod = BinaryXorDemodulator::new(config);
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut audio_buf = [0i16; 1024];
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
    let mut frame_count: u64 = 0;
    let mut soft_saves: u32 = 0;

    tracing::info!("processing audio at {} Hz (binary XOR correlator + soft HDLC)", config.sample_rate);

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
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let num_symbols = demod.process_samples(&audio_buf[..n], &mut symbols);
        for i in 0..num_symbols {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                if let FrameResult::Recovered { flips, .. } = &result {
                    soft_saves += 1;
                    tracing::debug!("soft recovery: {flips} bit(s) corrected");
                }
                let data = match &result {
                    FrameResult::Valid(d) => *d,
                    FrameResult::Recovered { data, .. } => *data,
                };
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

/// Correlation multi-slicer demodulator processing loop.
fn process_loop_corr_slicer(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut decoder = CorrSlicerDecoder::new(config).with_adaptive_gain();
    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    tracing::info!("processing audio at {} Hz (correlation multi-slicer, {} slicers)",
        config.sample_rate, decoder.num_slicers());

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav {
                tracing::info!(
                    "WAV file complete, decoded {} unique frames ({} total from {} slicers)",
                    decoder.total_unique, decoder.total_decoded, decoder.num_slicers()
                );
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

/// Correlation (mixer) demodulator processing loop + soft HDLC.
fn process_loop_corr(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut audio_buf = [0i16; 1024];
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
    let mut frame_count: u64 = 0;
    let mut soft_saves: u32 = 0;

    tracing::info!("processing audio at {} Hz (correlation mixer + soft HDLC)", config.sample_rate);

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
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let num_symbols = demod.process_samples(&audio_buf[..n], &mut symbols);
        for i in 0..num_symbols {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                if let FrameResult::Recovered { flips, .. } = &result {
                    soft_saves += 1;
                    tracing::debug!("soft recovery: {flips} bit(s) corrected");
                }
                let data = match &result {
                    FrameResult::Valid(d) => *d,
                    FrameResult::Recovered { data, .. } => *data,
                };
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

/// Correlation + Gardner PLL demodulator processing loop + soft HDLC.
fn process_loop_corr_pll(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let mut demod = CorrelationDemodulator::new(config)
        .with_adaptive_gain()
        .with_energy_llr()
        .with_pll();
    let mut soft_hdlc = SoftHdlcDecoder::new();
    let mut audio_buf = [0i16; 1024];
    let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
    let mut frame_count: u64 = 0;
    let mut soft_saves: u32 = 0;

    tracing::info!("processing audio at {} Hz (correlation + Gardner PLL + soft HDLC)", config.sample_rate);

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
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let num_symbols = demod.process_samples(&audio_buf[..n], &mut symbols);
        for i in 0..num_symbols {
            if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                if let FrameResult::Recovered { flips, .. } = &result {
                    soft_saves += 1;
                    tracing::debug!("soft recovery: {flips} bit(s) corrected");
                }
                let data = match &result {
                    FrameResult::Valid(d) => *d,
                    FrameResult::Recovered { data, .. } => *data,
                };
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

/// Single-decoder processing loop (fast or quality).
fn process_loop_single(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    use_quality: bool,
    config: DemodConfig,
    mut tx: Option<TxPipeline>,
) -> Option<TxPipeline> {
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

        if let Some(ref mut pipeline) = tx { pipeline.poll(); }

        let num_symbols = match &mut demod {
            Demod::Fast(d) => d.process_samples(&audio_buf[..n], &mut symbols),
            Demod::Quality(d) => d.process_samples(&audio_buf[..n], &mut symbols),
        };

        for i in 0..num_symbols {
            let frame_data = match &mut hdlc {
                Hdlc::Hard(h) => h.feed_bit(symbols[i].bit).map(|f| f.to_vec()),
                Hdlc::Soft(s) => {
                    s.feed_soft_bit(symbols[i].llr).map(|result| {
                        if let FrameResult::Recovered { flips, .. } = &result {
                            soft_saves += 1;
                            tracing::debug!("soft recovery: {flips} bit(s) corrected");
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

    if let Some(ref mut pipeline) = tx { pipeline.poll(); }
    tx
}

// ── RX Pipe Mode ────────────────────────────────────────────────────────

/// RX pipe mode: demodulate audio → KISS binary on stdout.
fn process_loop_rx_pipe(
    mut source: Box<dyn SampleSource>,
    config: DemodConfig,
    is_wav: bool,
    use_quality: bool,
    use_multi: bool,
    use_dm: bool,
    use_smart3: bool,
    use_corr: bool,
    use_corr_slicer: bool,
    use_corr_pll: bool,
    use_xor: bool,
) {
    use std::io::Write;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut kiss_buf = [0u8; 1024];
    let mut audio_buf = [0i16; 1024];
    let mut frame_count: u64 = 0;

    // Callback to KISS-encode and write frame to stdout
    let mut emit_frame = |data: &[u8]| {
        frame_count += 1;
        if let Some(len) = kiss::encode_frame(0, data, &mut kiss_buf) {
            let _ = out.write_all(&kiss_buf[..len]);
            let _ = out.flush();
        }
    };

    if use_multi {
        let mut multi = MultiDecoder::new(config);
        tracing::info!("rx-pipe: multi-decoder ({} decoders)", multi.num_decoders());
        loop {
            let n = source.read_samples(&mut audio_buf);
            if n == 0 {
                if is_wav { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let output = multi.process_samples(&audio_buf[..n]);
            for i in 0..output.len() {
                emit_frame(output.frame(i));
            }
        }
    } else if use_smart3 {
        let mut mini = MiniDecoder::new(config);
        tracing::info!("rx-pipe: smart3 mini-decoder");
        loop {
            let n = source.read_samples(&mut audio_buf);
            if n == 0 {
                if is_wav { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let output = mini.process_samples(&audio_buf[..n]);
            for i in 0..output.len() {
                emit_frame(output.frame(i));
            }
        }
    } else if use_corr_slicer {
        let mut decoder = CorrSlicerDecoder::new(config).with_adaptive_gain();
        tracing::info!("rx-pipe: correlation multi-slicer ({} slicers)", decoder.num_slicers());
        loop {
            let n = source.read_samples(&mut audio_buf);
            if n == 0 {
                if is_wav { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let output = decoder.process_samples(&audio_buf[..n]);
            for i in 0..output.len() {
                emit_frame(output.frame(i));
            }
        }
    } else if use_corr_pll {
        let mut demod = CorrelationDemodulator::new(config).with_adaptive_gain().with_energy_llr().with_pll();
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        tracing::info!("rx-pipe: correlation + PLL");
        loop {
            let n = source.read_samples(&mut audio_buf);
            if n == 0 {
                if is_wav { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
            for i in 0..ns {
                if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => *d,
                        FrameResult::Recovered { data, .. } => *data,
                    };
                    emit_frame(data);
                }
            }
        }
    } else if use_corr {
        let mut demod = CorrelationDemodulator::new(config).with_adaptive_gain().with_energy_llr();
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        tracing::info!("rx-pipe: correlation mixer");
        loop {
            let n = source.read_samples(&mut audio_buf);
            if n == 0 {
                if is_wav { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
            for i in 0..ns {
                if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => *d,
                        FrameResult::Recovered { data, .. } => *data,
                    };
                    emit_frame(data);
                }
            }
        }
    } else if use_dm {
        let mut demod = DmDemodulator::with_bpf_pll(config);
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        tracing::info!("rx-pipe: delay-multiply + PLL");
        loop {
            let n = source.read_samples(&mut audio_buf);
            if n == 0 {
                if is_wav { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
            for i in 0..ns {
                if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => *d,
                        FrameResult::Recovered { data, .. } => *data,
                    };
                    emit_frame(data);
                }
            }
        }
    } else if use_xor {
        let mut demod = BinaryXorDemodulator::new(config);
        let mut soft_hdlc = SoftHdlcDecoder::new();
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        tracing::info!("rx-pipe: binary XOR correlator");
        loop {
            let n = source.read_samples(&mut audio_buf);
            if n == 0 {
                if is_wav { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
            for i in 0..ns {
                if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => *d,
                        FrameResult::Recovered { data, .. } => *data,
                    };
                    emit_frame(data);
                }
            }
        }
    } else {
        // Default: fast or quality single decoder
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; 1024];
        if use_quality {
            let mut demod = QualityDemodulator::new(config);
            let mut soft_hdlc = SoftHdlcDecoder::new();
            tracing::info!("rx-pipe: quality demodulator");
            loop {
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(result) = soft_hdlc.feed_soft_bit(symbols[i].llr) {
                        let data = match &result {
                            FrameResult::Valid(d) => *d,
                            FrameResult::Recovered { data, .. } => *data,
                        };
                        emit_frame(data);
                    }
                }
            }
        } else {
            let mut demod = FastDemodulator::new(config);
            let mut hdlc = HdlcDecoder::new();
            tracing::info!("rx-pipe: fast demodulator");
            loop {
                let n = source.read_samples(&mut audio_buf);
                if n == 0 {
                    if is_wav { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let ns = demod.process_samples(&audio_buf[..n], &mut symbols);
                for i in 0..ns {
                    if let Some(f) = hdlc.feed_bit(symbols[i].bit) {
                        emit_frame(f);
                    }
                }
            }
        }
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
    for i in 0..count {
        let (h, g) = ring[i];
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
