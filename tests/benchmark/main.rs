//! WA8LMF TNC Test CD Benchmark Runner
//!
//! Processes WAV files through both demodulator paths and reports packet
//! counts, decode rates, and comparative performance against Dire Wolf.
//!
//! Usage:
//!   cargo run --release -p benchmark -- wav track1.wav
//!   cargo run --release -p benchmark -- suite tests/wav/
//!   cargo run --release -p benchmark -- compare track1.wav
//!   cargo run --release -p benchmark -- synthetic

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod common;
mod suite;
mod compare;
mod synthetic;
mod dm;
mod corr;
mod smart3;
mod diff;
mod attribution;
mod export;
mod soft_diag;
mod xor;
mod twist;
mod window;
mod pll_300;
mod fusion;
mod baud9600;

/// Packet Radio RS — Benchmark Runner
#[derive(Parser)]
#[command(name = "benchmark", about = "WA8LMF TNC Test CD benchmark runner")]
struct Cli {
    /// Set baud rate (default: 1200)
    #[arg(long, short = 'B', global = true, default_value_t = 1200)]
    baud: u32,

    /// Run suite at specific sample rate (e.g. 22050)
    #[arg(long, global = true)]
    rate: Option<u32>,

    /// Run suite at all discovered sample rates + best summary
    #[arg(long, global = true)]
    all_rates: bool,

    /// Only run MCU-feasible decoders (Fast, Quality, Smart3, TwistMini, DM)
    #[arg(long, global = true)]
    mcu_only: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode a single WAV file (all decoders)
    Wav {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode all WAV files in directory, compare with Dire Wolf
    Suite {
        /// Path to directory containing WAV files
        directory: PathBuf,
    },
    /// Compare fast vs. quality path frame-by-frame
    #[command(alias = "compare")]
    CompareApproaches {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode using delay-multiply demodulator
    Dm {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Run synthetic signal benchmark
    Synthetic,
    /// DM+PLL with all variant combinations
    DmPll {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Sweep PLL alpha/beta parameters
    DmPllSweep {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Dump DM discriminator diagnostics to CSV
    DmDebug {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Two-stage parameter tuning (Gardner shift, smoothing, LLR)
    DmPllTune {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Export decoded frames to files
    Export {
        /// Path to WAV file
        file: PathBuf,
        /// Output directory
        output_dir: PathBuf,
    },
    /// Frame-level diff against Dire Wolf reference
    Diff {
        /// Path to WAV file
        file: PathBuf,
        /// Path to reference packets file
        #[arg(long)]
        reference: Option<PathBuf>,
    },
    /// Per-decoder attribution analysis (multi-decoder)
    Attribution {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode using Smart3 mini-decoder (3 optimal decoders)
    Smart3 {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Soft decode diagnostics (per-frame LLR analysis)
    SoftDiag {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode using correlation (mixer) demodulator
    Corr {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Sweep correlation LPF cutoff (400-1000 Hz, 50 Hz steps)
    CorrLpfSweep {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode using correlation multi-slicer (8 gain levels)
    CorrSlicer {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Correlation + Gardner PLL timing recovery
    CorrPll {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Sweep Corr+PLL alpha/error_shift parameters
    CorrPllSweep {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode using binary XOR correlator
    Xor {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Sweep twist-tuned decoder configurations
    Twist {
        /// Path to WAV file
        file: PathBuf,
    },
    /// TwistMini multi-rate comparison
    TwistMini {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Sweep Smart3 decoder configurations
    Smart3Sweep {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Sweep Goertzel window types (ISI reduction)
    Window {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Sweep 300-baud DM+PLL alpha values
    #[command(name = "pll-300")]
    Pll300 {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Cross-architecture Goertzel+Corr LLR fusion
    Fusion {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode 9600 baud WAV (all algorithms)
    #[command(name = "9600")]
    Baud9600 {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Side-by-side 9600 baud algorithm comparison
    #[command(name = "9600-compare")]
    Baud9600Compare {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode using Multi9600 ensemble decoder
    #[command(name = "9600-multi")]
    Baud9600Multi {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Grid: all algorithms x all 9600 WAVs vs DireWolf
    #[command(name = "9600-suite")]
    Baud9600Suite {
        /// Path to directory containing WAV files
        directory: PathBuf,
    },
    /// 9600 baud diagnostics
    #[command(name = "9600-diag")]
    Baud9600Diag {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Decode using Mini9600 (6-decoder MCU ensemble)
    #[command(name = "9600-mini")]
    Baud9600Mini {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Grid search: LPF x timing x slicer x cascaded
    #[command(name = "9600-tune")]
    Baud9600Tune {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Per-decoder attribution + greedy set-cover
    #[command(name = "9600-attribution")]
    Baud9600Attribution {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Test adaptive pre-emphasis single decoder
    Preemph {
        /// Path to WAV file
        file: PathBuf,
    },
    /// Validate FX.25 decode against a Dire Wolf-generated FX.25 WAV file
    Fx25 {
        /// Path to FX.25 WAV file (generate with: gen_packets -X 16 -n 10 -o file.wav)
        file: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    // Store baud rate in a thread-local for decode functions
    common::BAUD_RATE.with(|b| b.set(cli.baud));

    match cli.command {
        Command::Wav { file } => {
            suite::run_single_wav(&file.to_string_lossy(), cli.mcu_only);
        }
        Command::Suite { directory } => {
            suite::run_suite(&directory.to_string_lossy(), cli.rate, cli.all_rates, cli.mcu_only);
        }
        Command::CompareApproaches { file } => {
            compare::run_compare_approaches(&file.to_string_lossy());
        }
        Command::Dm { file } => {
            dm::run_dm_single(&file.to_string_lossy());
        }
        Command::Synthetic => {
            synthetic::run_synthetic_benchmark();
        }
        Command::DmPll { file } => {
            dm::run_dm_pll(&file.to_string_lossy());
        }
        Command::DmPllSweep { file } => {
            dm::run_dm_pll_sweep(&file.to_string_lossy());
        }
        Command::DmDebug { file } => {
            dm::run_dm_debug(&file.to_string_lossy());
        }
        Command::DmPllTune { file } => {
            dm::run_dm_pll_tune(&file.to_string_lossy());
        }
        Command::Export { file, output_dir } => {
            export::run_export(&file.to_string_lossy(), &output_dir.to_string_lossy());
        }
        Command::Diff { file, reference } => {
            diff::run_diff(&file.to_string_lossy(), reference.as_ref().map(|p| p.to_str().unwrap_or("")));
        }
        Command::Attribution { file } => {
            attribution::run_attribution(&file.to_string_lossy());
        }
        Command::Smart3 { file } => {
            smart3::run_smart3(&file.to_string_lossy());
        }
        Command::SoftDiag { file } => {
            soft_diag::run_soft_diag(&file.to_string_lossy());
        }
        Command::Corr { file } => {
            corr::run_corr(&file.to_string_lossy());
        }
        Command::CorrLpfSweep { file } => {
            corr::run_corr_lpf_sweep(&file.to_string_lossy());
        }
        Command::CorrSlicer { file } => {
            corr::run_corr_slicer(&file.to_string_lossy());
        }
        Command::CorrPll { file } => {
            corr::run_corr_pll(&file.to_string_lossy());
        }
        Command::CorrPllSweep { file } => {
            corr::run_corr_pll_sweep(&file.to_string_lossy());
        }
        Command::Xor { file } => {
            xor::run_xor(&file.to_string_lossy());
        }
        Command::Twist { file } => {
            twist::run_twist_sweep(&file.to_string_lossy());
        }
        Command::TwistMini { file } => {
            twist::run_twist_mini(&file.to_string_lossy());
        }
        Command::Smart3Sweep { file } => {
            smart3::run_smart3_sweep(&file.to_string_lossy());
        }
        Command::Window { file } => {
            window::run_window_sweep(&file.to_string_lossy());
        }
        Command::Pll300 { file } => {
            pll_300::run_pll_300(&file.to_string_lossy());
        }
        Command::Fusion { file } => {
            fusion::run_fusion(&file.to_string_lossy());
        }
        Command::Baud9600 { file } => {
            baud9600::run_9600_single(&file.to_string_lossy());
        }
        Command::Baud9600Compare { file } => {
            baud9600::run_9600_compare(&file.to_string_lossy());
        }
        Command::Baud9600Multi { file } => {
            baud9600::run_9600_multi(&file.to_string_lossy());
        }
        Command::Baud9600Suite { directory } => {
            baud9600::run_9600_suite(&directory.to_string_lossy());
        }
        Command::Baud9600Diag { file } => {
            baud9600::run_9600_diag(&file.to_string_lossy());
        }
        Command::Baud9600Mini { file } => {
            baud9600::run_9600_mini(&file.to_string_lossy());
        }
        Command::Baud9600Tune { file } => {
            baud9600::run_9600_tune(&file.to_string_lossy());
        }
        Command::Baud9600Attribution { file } => {
            baud9600::run_9600_attribution(&file.to_string_lossy());
        }
        Command::Preemph { file } => {
            let path = file.to_string_lossy();
            let (sr, samples) = common::read_wav_file(&path).unwrap();
            let fast = common::decode_fast(&samples, sr);
            let (quality, _) = common::decode_quality(&samples, sr);
            let (preemph, soft_p) = common::decode_fast_preemph(&samples, sr);
            println!("Fast:       {} frames", fast.frames.len());
            println!("Quality:    {} frames", quality.frames.len());
            println!("Preemph:    {} frames ({} soft)", preemph.frames.len(), soft_p);
        }
        Command::Fx25 { file } => {
            let path = file.to_string_lossy();
            if path == "loopback" {
                run_fx25_loopback();
            } else {
                run_fx25_validate(&path);
            }
        }
    }
}

fn run_fx25_loopback() {
    use packet_radio_core::modem::afsk::AfskModulator;
    use packet_radio_core::modem::demod::{FastDemodulator, DemodSymbol};
    use packet_radio_core::modem::{DemodConfig, ModConfig};
    use packet_radio_core::fx25::decode::Fx25Decoder;
    use packet_radio_core::fx25::encode::fx25_encode;

    println!("=== FX.25 Loopback Test ===");

    // Create a test frame (with CRC)
    let mut frame = [0u8; 18];
    for (i, &c) in b"TEST  ".iter().enumerate() { frame[i] = c << 1; }
    frame[6] = 0x60;
    for (i, &c) in b"SRC   ".iter().enumerate() { frame[7 + i] = c << 1; }
    frame[13] = 0x61;
    frame[14] = 0x03;
    frame[15] = 0xF0;
    frame[16] = b'H';
    frame[17] = b'i';

    // Add CRC
    let crc = packet_radio_core::ax25::crc16_ccitt(&frame);
    let mut frame_crc = [0u8; 20];
    frame_crc[..18].copy_from_slice(&frame);
    frame_crc[18] = crc as u8;
    frame_crc[19] = (crc >> 8) as u8;

    // FX.25 encode
    let block = fx25_encode(&frame_crc, 16).expect("encode failed");
    println!("Encoded: tag_idx={}, {} bits", block.tag_index, block.bit_count);

    // Modulate: preamble flags + FX.25 block + postamble
    let mod_config = ModConfig::default_1200();
    let mut modulator = AfskModulator::new(mod_config);
    let mut audio = Vec::new();
    let mut sym_buf = [0i16; 64];

    // 50 preamble flags
    for _ in 0..50 {
        for &flag_bit in &[false, true, true, true, true, true, true, false] {
            let n = modulator.modulate_bit(flag_bit, &mut sym_buf);
            audio.extend_from_slice(&sym_buf[..n]);
        }
    }

    // FX.25 block bits
    for bit in block.iter_bits() {
        let n = modulator.modulate_bit(bit, &mut sym_buf);
        audio.extend_from_slice(&sym_buf[..n]);
    }

    // 10 postamble flags
    for _ in 0..10 {
        for &flag_bit in &[false, true, true, true, true, true, true, false] {
            let n = modulator.modulate_bit(flag_bit, &mut sym_buf);
            audio.extend_from_slice(&sym_buf[..n]);
        }
    }

    println!("Generated {} audio samples at {} Hz", audio.len(), 11025);

    // Demodulate
    let demod_config = DemodConfig::default_1200();
    let mut demod = FastDemodulator::new(demod_config).with_adaptive_gain();
    let mut fx25 = Fx25Decoder::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0, sample_idx: 0, raw_bit: false }; 1024];

    let mut fx25_count = 0u32;
    let mut shift_reg: u64 = 0;
    let mut bit_idx: u64 = 0;
    let mut best_hamming = 64u32;

    for chunk in audio.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for sym in &symbols[..n] {
            // Use NRZI-decoded bits (same as DW convention)
            shift_reg = (shift_reg >> 1) | ((sym.bit as u64) << 63);
            bit_idx += 1;
            if bit_idx >= 64 {
                for t in packet_radio_core::fx25::FX25_TAGS.iter() {
                    let dist = (shift_reg ^ t.tag).count_ones();
                    if dist < best_hamming {
                        best_hamming = dist;
                    }
                }
            }

            if let Some(f) = fx25.feed_bit(sym.bit) {
                fx25_count += 1;
                println!("  Decoded frame: {} bytes", f.len());
            }
        }
    }

    println!("Results: FX.25 frames={}, tags={}, best_hamming={}",
        fx25_count, fx25.stats_tags_detected, best_hamming);
    println!("  RS clean={}, corrected={}, failed={}",
        fx25.stats_rs_clean, fx25.stats_rs_corrected, fx25.stats_rs_failed);
}

fn run_fx25_validate(path: &str) {
    use packet_radio_core::modem::demod::{FastDemodulator, DemodSymbol};
    use packet_radio_core::modem::hdlc_bank::AnyHdlc;
    use packet_radio_core::fx25::decode::Fx25Decoder;

    let (sr, samples) = common::read_wav_file(path).expect("failed to read WAV");
    println!("=== FX.25 Validation: {} ===", path);
    println!("Sample rate: {} Hz, {} samples ({:.1}s)",
        sr, samples.len(), samples.len() as f64 / sr as f64);

    let config = common::config_for_rate(sr, common::get_baud());
    // Use QualityAdapter-style demod for better bit recovery
    let mut demod = FastDemodulator::new(config).with_adaptive_gain().with_energy_llr();
    let mut hdlc = AnyHdlc::new();
    let mut fx25 = Fx25Decoder::new();
    let mut symbols = [DemodSymbol { bit: false, llr: 0, sample_idx: 0, raw_bit: false }; 1024];

    let mut hdlc_count = 0u32;
    let mut fx25_count = 0u32;

    // Also run a raw tag scan: check DW's actual tag values
    let mut shift_reg: u64 = 0;
    let mut bit_idx: u64 = 0;
    let mut best_hamming = 64u32;
    let mut best_tag_val: u64 = 0;
    let mut best_bit_pos: u64 = 0;

    for chunk in samples.chunks(1024) {
        let n = demod.process_samples(chunk, &mut symbols);
        for sym in &symbols[..n] {
            if hdlc.feed(sym.bit, sym.llr).is_some() {
                hdlc_count += 1;
            }

            // Track shift register: NRZI-decoded, right-shift (DW convention)
            shift_reg = (shift_reg >> 1) | ((sym.bit as u64) << 63);
            bit_idx += 1;
            if bit_idx >= 64 {
                // Check min hamming across ALL tags with generous threshold
                for (ti, t) in packet_radio_core::fx25::FX25_TAGS.iter().enumerate() {
                    let dist = (shift_reg ^ t.tag).count_ones();
                    if dist < best_hamming {
                        best_hamming = dist;
                        best_tag_val = shift_reg;
                        best_bit_pos = bit_idx;
                        if dist <= 12 {
                            println!("  [bit {}] Near match: tag[{}] hamming={}, reg=0x{:016X} vs tag=0x{:016X}",
                                bit_idx, ti, dist, shift_reg, t.tag);
                        }
                    }
                }
                if let Some((idx, dist)) = packet_radio_core::fx25::match_tag(shift_reg, 10) {
                    if dist < best_hamming {
                        best_hamming = dist;
                        best_tag_val = shift_reg;
                        best_bit_pos = bit_idx;
                        if dist <= 5 {
                            println!("  [bit {}] Tag match: idx={}, hamming={}, reg=0x{:016X}",
                                bit_idx, idx, dist, shift_reg);
                        }
                    }
                }
            }

            if let Some(frame) = fx25.feed_bit(sym.bit) {
                fx25_count += 1;
                let preview: String = frame.iter().take(20)
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>().join(" ");
                println!("  FX.25 frame {}: {} bytes [{}...]", fx25_count, frame.len(), preview);
            }
        }
    }

    println!();
    println!("Results:");
    println!("  HDLC frames:         {}", hdlc_count);
    println!("  FX.25 frames:        {}", fx25_count);
    println!("  Tags detected:       {}", fx25.stats_tags_detected);
    println!("  RS clean (0 errors): {}", fx25.stats_rs_clean);
    println!("  RS corrected:        {}", fx25.stats_rs_corrected);
    println!("  RS failed:           {}", fx25.stats_rs_failed);
    println!("  Best tag hamming:    {} (at bit {}, reg=0x{:016X})",
        best_hamming, best_bit_pos, best_tag_val);
}
