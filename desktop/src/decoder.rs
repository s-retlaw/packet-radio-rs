use packet_radio_core::modem::DemodConfig;
use packet_radio_core::tnc::{
    Demodulate, FastAdapter, QualityAdapter, DmAdapter,
    CorrAdapter, CorrPllAdapter, XorAdapter,
    MiniAdapter, MultiAdapter, CorrSlicerAdapter,
};

use crate::cli;

/// Build a DemodConfig for the given sample rate and baud rate.
pub fn demod_config_for_rate(rate: u32, baud: u32) -> DemodConfig {
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

/// Create a boxed `Demodulate` implementation for the given mode and config.
///
/// All demod→HDLC assembly now lives in `core/src/tnc.rs` adapters.
/// Desktop just selects which adapter to use.
pub fn create_decoder(mode: &cli::DemodMode, config: DemodConfig) -> Box<dyn Demodulate> {
    match mode {
        cli::DemodMode::Fast => Box::new(FastAdapter::new(config)),
        cli::DemodMode::Quality => Box::new(QualityAdapter::new(config)),
        cli::DemodMode::Multi => Box::new(MultiAdapter::new(config)),
        cli::DemodMode::Smart3 => Box::new(MiniAdapter::new(config)),
        cli::DemodMode::Dm => Box::new(DmAdapter::new(config)),
        cli::DemodMode::Corr => Box::new(CorrAdapter::new(config)),
        cli::DemodMode::CorrPll => Box::new(CorrPllAdapter::new(config)),
        cli::DemodMode::CorrSlicer => Box::new(CorrSlicerAdapter::new(config)),
        cli::DemodMode::Xor => Box::new(XorAdapter::new(config)),
    }
}
