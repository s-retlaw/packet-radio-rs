//! Desktop Packet Radio TNC
//!
//! A full-featured TNC that runs on Linux, macOS, and Windows.
//! Uses the sound card for audio I/O and provides a KISS TCP
//! interface for connecting to APRS client software.

mod audio;
mod cli;
mod kiss_server;

use clap::Parser;
use packet_radio_core::modem::demod::{CorrelationDemodulator, DemodSymbol, DmDemodulator, FastDemodulator, QualityDemodulator};
use packet_radio_core::modem::corr_slicer::CorrSlicerDecoder;
use packet_radio_core::modem::multi::{MiniDecoder, MultiDecoder};
use packet_radio_core::modem::soft_hdlc::{SoftHdlcDecoder, FrameResult};
use packet_radio_core::modem::{DemodConfig, ModConfig};
use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::ax25::Frame;
use packet_radio_core::tnc::{AfskModulateAdapter, NullDemod, TncConfig, TncEngine, TncPlatform};
use packet_radio_core::kiss;
use packet_radio_core::aprs;
use packet_radio_core::SampleSource;
use tokio::sync::broadcast;

fn main() {
    let cli = cli::Cli::parse();

    // Pipe modes: send tracing to stderr so stdout is clean data
    let is_pipe = cli.rx_pipe || cli.tx_pipe;

    // Init tracing
    let level = match cli.verbose {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    if is_pipe {
        tracing_subscriber::fmt()
            .with_max_level(level)
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_max_level(level)
            .init();
    }

    // List devices and exit
    if cli.list_devices {
        audio::list_devices();
        return;
    }

    // TX pipe mode: read KISS from stdin, write raw PCM to stdout
    if cli.tx_pipe {
        process_loop_tx_pipe(cli.sample_rate);
        return;
    }

    // Build the tokio runtime for KISS TCP server
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

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
        TxPipeline::new(kiss_in_rx.clone(), cli.sample_rate)
    });

    let config = match cli.sample_rate {
        22050 => DemodConfig::default_1200_22k(),
        44100 => DemodConfig::default_1200_44k(),
        _ => DemodConfig::default_1200(),
    };

    // Open audio source
    let source: Box<dyn SampleSource> = if let Some(ref wav_path) = cli.wav {
        match audio::WavSource::open(wav_path, cli.sample_rate) {
            Ok(src) => Box::new(src),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else if cli.rx_pipe {
        Box::new(audio::StdinSource::new())
    } else {
        match audio::CpalSource::open(&cli.device, cli.sample_rate) {
            Ok(src) => Box::new(src),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    };

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
        );
        return;
    }

    // Run the processing loop on the main thread.
    let tx_pipeline = process_loop(
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
        cli.sample_rate,
        tx_pipeline,
    );

    // Write TX audio to WAV if requested
    if let (Some(ref tx_wav_path), Some(pipeline)) = (&cli.tx_wav, &tx_pipeline) {
        pipeline.write_wav(tx_wav_path, cli.sample_rate);
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

/// TX pipeline: wraps a TX-only TncEngine, accumulates modulated audio.
struct TxPipeline {
    engine: TncEngine<NullDemod, AfskModulateAdapter>,
    platform: TxOnlyPlatform,
    samples: Vec<i16>,
    kiss_rx: crossbeam_channel::Receiver<Vec<u8>>,
}

impl TxPipeline {
    fn new(kiss_rx: crossbeam_channel::Receiver<Vec<u8>>, sample_rate: u32) -> Self {
        let mod_config = match sample_rate {
            22050 => ModConfig { sample_rate: 22050, ..ModConfig::default_1200() },
            44100 => ModConfig { sample_rate: 44100, ..ModConfig::default_1200() },
            _ => ModConfig::default_1200(),
        };
        let mut tnc_config = TncConfig::default();
        tnc_config.full_duplex = true; // Skip CSMA
        tnc_config.txdelay = 25; // 250ms preamble (shorter for testing)

        Self {
            engine: TncEngine::new(NullDemod, AfskModulateAdapter::new(mod_config), tnc_config),
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
    sample_rate: u32,
    tx_pipeline: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let config = match sample_rate {
        22050 => DemodConfig::default_1200_22k(),
        44100 => DemodConfig::default_1200_44k(),
        _ => DemodConfig::default_1200(),
    };

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
fn process_loop_tx_pipe(sample_rate: u32) {
    use std::io::{Read, Write};

    let mod_config = match sample_rate {
        22050 => ModConfig { sample_rate: 22050, ..ModConfig::default_1200() },
        44100 => ModConfig { sample_rate: 44100, ..ModConfig::default_1200() },
        _ => ModConfig::default_1200(),
    };
    let mut tnc_config = TncConfig::default();
    tnc_config.full_duplex = true;
    tnc_config.txdelay = 25;

    let mut engine = TncEngine::new(NullDemod, AfskModulateAdapter::new(mod_config), tnc_config);
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
