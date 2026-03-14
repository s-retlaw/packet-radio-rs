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
    }
}
