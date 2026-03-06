use packet_radio_core::modem::demod_9600::Demod9600Config;
use packet_radio_core::SampleSource;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::broadcast;

use crate::audio;
use crate::cli;
use crate::decoder::demod_config_for_rate;
use crate::kiss_server;
use crate::processing::{
    process_loop, process_loop_auto_baud, process_loop_rx_pipe, process_loop_tx_pipe,
    process_loop_9600_mini, process_loop_9600_multi, process_loop_9600_single,
};
use crate::tx::TxPipeline;

/// Original processing path — console output, no TUI.
pub fn run_headless(cli: cli::Cli) {
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
