//! Desktop Packet Radio TNC
//!
//! A full-featured TNC that runs on Linux, macOS, and Windows.
//! Uses the sound card for audio I/O and provides a KISS TCP
//! interface for connecting to APRS client software.

mod audio;
mod cli;
mod config;
mod decoder;
mod frame_fmt;
mod headless;
mod kiss_server;
mod processing;
mod tui;
mod tx;

use clap::Parser;
use packet_radio_core::modem::demod_9600::Demod9600Config;
use packet_radio_core::modem::multi_9600::{Mini9600Decoder, Multi9600Decoder, Single9600Decoder};
use packet_radio_core::SampleSource;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::sync::broadcast;

use decoder::{demod_config_for_rate, create_decoder};
use frame_fmt::make_frame_info;
use headless::run_headless;


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
    let (async_tx, async_rx) = crossbeam_channel::bounded::<tui::state::AsyncEvent>(256);
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
    config: packet_radio_core::modem::DemodConfig,
    mode: &str,
    is_wav: bool,
    stop: &AtomicBool,
    async_tx: &crossbeam_channel::Sender<tui::state::AsyncEvent>,
    kiss_frame_tx: &broadcast::Sender<Vec<u8>>,
) {
    let mut frame_count: u64 = 0;
    let mut audio_buf = [0i16; 1024];
    let demod_mode = cli::DemodMode::from_config_str(mode);
    let mut decoder = create_decoder(&demod_mode, config);

    loop {
        if stop.load(Ordering::Relaxed) { break; }
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        decoder.process_audio(&audio_buf[..n], &mut |data: &[u8]| {
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
    if async_tx.try_send(tui::state::AsyncEvent::FrameDecoded(Box::new(info))).is_err() {
        tracing::trace!("TUI async channel full, dropping frame #{count}");
    }
    let _ = kiss_frame_tx.send(data.to_vec());
}
