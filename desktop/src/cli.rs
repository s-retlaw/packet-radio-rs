//! CLI argument parsing for the desktop TNC.

use std::path::PathBuf;
use clap::Parser;

/// Demodulator mode for 1200/300 baud AFSK.
#[derive(Clone, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum DemodMode {
    /// Single Goertzel + Bresenham. Lowest CPU, basic decode.
    #[default]
    Fast,
    /// Goertzel + LLR soft decode. Slight improvement over fast.
    Quality,
    /// 38 parallel decoders. Best decode rate, highest CPU.
    Multi,
    /// 3 optimal decoders. ~97% of multi at 8% CPU cost.
    Smart3,
    /// Delay-multiply discriminator + Gardner PLL timing.
    Dm,
    /// DireWolf-style mixer demodulator + soft HDLC.
    Corr,
    /// Correlation + 24 gain/frequency diversity slicers.
    CorrSlicer,
    /// Correlation mixer with Gardner PLL clock recovery.
    CorrPll,
    /// Binary XOR correlator. Twist-immune, amplitude-invariant.
    Xor,
}

impl DemodMode {
    /// Convert to the string key used by config and process loops.
    pub fn as_str(&self) -> &'static str {
        match self {
            DemodMode::Fast => "fast",
            DemodMode::Quality => "quality",
            DemodMode::Multi => "multi",
            DemodMode::Smart3 => "smart3",
            DemodMode::Dm => "dm",
            DemodMode::Corr => "corr",
            DemodMode::CorrSlicer => "corr-slicer",
            DemodMode::CorrPll => "corr-pll",
            DemodMode::Xor => "xor",
        }
    }

    /// Parse from a config string. Returns `Fast` for unknown values.
    pub fn from_config_str(s: &str) -> Self {
        match s {
            "fast" => DemodMode::Fast,
            "quality" => DemodMode::Quality,
            "multi" => DemodMode::Multi,
            "smart3" => DemodMode::Smart3,
            "dm" => DemodMode::Dm,
            "corr" => DemodMode::Corr,
            "corr-slicer" => DemodMode::CorrSlicer,
            "corr-pll" => DemodMode::CorrPll,
            "xor" => DemodMode::Xor,
            _ => DemodMode::Fast,
        }
    }
}

#[derive(Parser)]
#[command(name = "packet-radio-tnc", about = "Packet radio TNC — AFSK modem with KISS TCP")]
pub struct Cli {
    /// Audio input device name (or "default")
    #[arg(short = 'd', long, default_value = "default")]
    pub device: String,

    /// Decode from WAV file instead of live audio
    #[arg(long)]
    pub wav: Option<PathBuf>,

    /// List available audio devices and exit
    #[arg(long)]
    pub list_devices: bool,

    /// KISS TCP port (0 to disable)
    #[arg(short = 'k', long, default_value = "8001")]
    pub kiss_port: u16,

    /// Sample rate
    #[arg(short = 's', long, default_value = "11025")]
    pub sample_rate: u32,

    /// Demodulator mode
    #[arg(short = 'm', long, value_enum, default_value_t = DemodMode::Fast)]
    pub mode: DemodMode,

    /// Write TX audio to WAV file (modulated output from KISS frames received via TCP)
    #[arg(long)]
    pub tx_wav: Option<PathBuf>,

    /// RX pipe mode: output KISS frames to stdout (binary). Audio from --wav or stdin.
    #[arg(long)]
    pub rx_pipe: bool,

    /// TX pipe mode: read KISS from stdin, output raw i16 LE PCM to stdout.
    #[arg(long)]
    pub tx_pipe: bool,

    /// Baud rate: 300 (HF), 1200 (VHF, default), or 9600 (G3RUH FSK)
    #[arg(short = 'B', long, default_value = "1200")]
    pub baud: u32,

    /// 9600 baud algorithm: direwolf, gardner, early-late, mm, rrc
    #[arg(long = "9600-algo")]
    pub algo_9600: Option<String>,

    /// Use Mini9600 decoder (6 MCU-optimal decoders for 9600 baud)
    #[arg(long)]
    pub mini9600: bool,

    /// Auto-baud: decode both 1200 and 9600 simultaneously
    #[arg(long)]
    pub auto_baud: bool,

    /// Disable TUI — headless mode (log to stdout like before)
    #[arg(long)]
    pub no_tui: bool,

    /// Config file path (default: ./packet-radio.toml)
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Verbose output (repeat for more: -v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl Cli {
    /// Returns true if TUI should be bypassed (pipe modes, WAV decode, --no-tui).
    pub fn is_headless(&self) -> bool {
        self.no_tui || self.rx_pipe || self.tx_pipe || self.wav.is_some() || self.list_devices
    }
}
