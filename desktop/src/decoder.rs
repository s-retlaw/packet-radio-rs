use packet_radio_core::modem::demod::{CorrelationDemodulator, DemodSymbol, DmDemodulator, FastDemodulator, QualityDemodulator};
use packet_radio_core::modem::binary_xor::BinaryXorDemodulator;
use packet_radio_core::modem::corr_slicer::CorrSlicerDecoder;
use packet_radio_core::modem::multi::{MiniDecoder, MultiDecoder};
use packet_radio_core::modem::soft_hdlc::{SoftHdlcDecoder, FrameResult};
use packet_radio_core::modem::DemodConfig;
use packet_radio_core::ax25::frame::HdlcDecoder;

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

/// Wraps all 1200/300 baud demodulator variants behind a single interface.
/// Callers provide a frame callback; the decoder handles symbol->HDLC internally.
#[allow(clippy::large_enum_variant)]
pub enum UnifiedDecoder {
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
pub enum SoftDemod {
    Quality(QualityDemodulator),
    Dm(DmDemodulator),
    Corr(CorrelationDemodulator),
    CorrPll(CorrelationDemodulator),
    Xor(BinaryXorDemodulator),
}

impl SoftDemod {
    pub fn process_samples(&mut self, samples: &[i16], symbols: &mut [DemodSymbol]) -> usize {
        match self {
            SoftDemod::Quality(d) => d.process_samples(samples, symbols),
            SoftDemod::Dm(d) => d.process_samples(samples, symbols),
            SoftDemod::Corr(d) | SoftDemod::CorrPll(d) => d.process_samples(samples, symbols),
            SoftDemod::Xor(d) => d.process_samples(samples, symbols),
        }
    }
}

impl UnifiedDecoder {
    pub fn new(mode: &cli::DemodMode, config: DemodConfig) -> Self {
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
    pub fn process(&mut self, samples: &[i16], emit: &mut dyn FnMut(&[u8])) {
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
