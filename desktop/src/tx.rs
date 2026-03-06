const MAX_TX_SAMPLES: usize = 10 * 60 * 48000;

use packet_radio_core::modem::ModConfig;
use packet_radio_core::modem::mod_9600::Mod9600Config;
use packet_radio_core::tnc::{AfskModulateAdapter, Fsk9600ModulateAdapter, NullDemod, TncConfig, TncEngine, TncPlatform};

/// Platform for TX-only TNC: always clear channel, no PTT, full duplex.
pub struct TxOnlyPlatform;

impl TncPlatform for TxOnlyPlatform {
    fn set_ptt(&mut self, _on: bool) {}
    fn channel_busy(&self) -> bool { false }
    fn random_byte(&self) -> u8 {
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() & 0xFF) as u8
    }
    fn now_ms(&self) -> u32 { 0 }
}

/// Inner TX engine — dispatches between AFSK (300/1200) and 9600 FSK.
pub enum TxEngine {
    Afsk(TncEngine<NullDemod, AfskModulateAdapter>),
    Fsk9600(TncEngine<NullDemod, Fsk9600ModulateAdapter>),
}

impl TxEngine {
    pub fn feed_kiss(&mut self, byte: u8) {
        match self {
            TxEngine::Afsk(e) => e.feed_kiss(byte),
            TxEngine::Fsk9600(e) => e.feed_kiss(byte),
        }
    }

    pub fn poll_tx(&mut self, out: &mut [i16], platform: &mut TxOnlyPlatform) -> usize {
        match self {
            TxEngine::Afsk(e) => e.poll_tx(out, platform),
            TxEngine::Fsk9600(e) => e.poll_tx(out, platform),
        }
    }
}

/// TX pipeline: wraps a TX-only TncEngine, accumulates modulated audio.
pub struct TxPipeline {
    engine: TxEngine,
    platform: TxOnlyPlatform,
    samples: Vec<i16>,
    kiss_rx: crossbeam_channel::Receiver<Vec<u8>>,
}

impl TxPipeline {
    pub fn new(kiss_rx: crossbeam_channel::Receiver<Vec<u8>>, sample_rate: u32, baud: u32) -> Self {
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
    pub fn poll(&mut self) {
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
            if self.samples.len() > MAX_TX_SAMPLES {
                tracing::warn!("TX buffer exceeded {}s limit, truncating", MAX_TX_SAMPLES / 48000);
                self.samples.truncate(MAX_TX_SAMPLES);
                break;
            }
        }
    }

    /// Write accumulated TX audio to WAV file.
    pub fn write_wav(&self, path: &std::path::Path, sample_rate: u32) {
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
