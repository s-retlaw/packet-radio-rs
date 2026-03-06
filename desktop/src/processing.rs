use packet_radio_core::modem::DemodConfig;
use packet_radio_core::modem::demod_9600::Demod9600Config;
use packet_radio_core::modem::multi::MiniDecoder;
use packet_radio_core::modem::multi_9600::{Mini9600Decoder, Multi9600Decoder, Single9600Decoder};
use packet_radio_core::kiss;
use packet_radio_core::SampleSource;
use packet_radio_core::modem::ModConfig;
use packet_radio_core::modem::mod_9600::Mod9600Config;
use packet_radio_core::tnc::{AfskModulateAdapter, Fsk9600ModulateAdapter, NullDemod, TncConfig, TncEngine};
use tokio::sync::broadcast;

use crate::cli;
use crate::decoder::{demod_config_for_rate, create_decoder};
use crate::frame_fmt::print_frame;
use crate::tx::{TxEngine, TxOnlyPlatform, TxPipeline};

/// Main DSP processing loop. Returns the TX pipeline (if any) for WAV writing.
pub fn process_loop(
    mut source: Box<dyn SampleSource>,
    frame_tx: broadcast::Sender<Vec<u8>>,
    is_wav: bool,
    mode: &cli::DemodMode,
    sample_rate: u32,
    baud_rate: u32,
    mut tx_pipeline: Option<TxPipeline>,
) -> Option<TxPipeline> {
    let config = demod_config_for_rate(sample_rate, baud_rate);
    let mut decoder = create_decoder(mode, config);
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

        decoder.process_audio(&audio_buf[..n], &mut |data: &[u8]| {
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

/// RX pipe mode: demodulate audio -> KISS binary on stdout.
pub fn process_loop_rx_pipe(
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

    let mut decoder = create_decoder(mode, config);
    tracing::info!("rx-pipe: {} demodulator", mode.as_str());

    loop {
        let n = source.read_samples(&mut audio_buf);
        if n == 0 {
            if is_wav { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        decoder.process_audio(&audio_buf[..n], &mut |data: &[u8]| emit_frame(data));
    }

    tracing::info!("rx-pipe: done, output {frame_count} frames");
}

/// TX pipe mode: read KISS from stdin, write raw i16 LE PCM to stdout.
pub fn process_loop_tx_pipe(sample_rate: u32, baud: u32) {
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

/// 9600 baud single-algorithm processing loop.
pub fn process_loop_9600_single(
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
pub fn process_loop_9600_multi(
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
pub fn process_loop_9600_mini(
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
pub fn process_loop_auto_baud(
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
