//! CLI argument parsing for the desktop TNC.

use std::path::PathBuf;
use clap::Parser;

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

    /// Use quality demodulator (default: fast)
    #[arg(long)]
    pub quality: bool,

    /// Use multi-decoder (32+ parallel decoders with filter/timing diversity)
    #[arg(long)]
    pub multi: bool,

    /// Use delay-multiply demodulator (BPF → delay-multiply → Bresenham)
    #[arg(long)]
    pub dm: bool,

    /// Use Smart3 mini-decoder (3 attribution-optimal parallel decoders)
    #[arg(long)]
    pub smart3: bool,

    /// Use correlation (mixer) demodulator (DireWolf-style tone detection)
    #[arg(long)]
    pub corr: bool,

    /// Use correlation demodulator + multi-slicer (8 gain levels, single demod)
    #[arg(long)]
    pub corr_slicer: bool,

    /// Use correlation demodulator + Gardner PLL timing recovery
    #[arg(long)]
    pub corr_pll: bool,

    /// Use binary XOR correlator (twist-immune, amplitude-invariant)
    #[arg(long)]
    pub xor: bool,

    /// Write TX audio to WAV file (modulated output from KISS frames received via TCP)
    #[arg(long)]
    pub tx_wav: Option<PathBuf>,

    /// RX pipe mode: output KISS frames to stdout (binary). Audio from --wav or stdin.
    #[arg(long)]
    pub rx_pipe: bool,

    /// TX pipe mode: read KISS from stdin, output raw i16 LE PCM to stdout.
    #[arg(long)]
    pub tx_pipe: bool,

    /// Baud rate: 300 (HF) or 1200 (VHF, default)
    #[arg(short = 'B', long, default_value = "1200")]
    pub baud: u32,

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

    /// Determine the demod mode string from CLI flags.
    pub fn demod_mode(&self) -> &str {
        if self.multi { "multi" }
        else if self.smart3 { "smart3" }
        else if self.corr_slicer { "corr-slicer" }
        else if self.corr_pll { "corr-pll" }
        else if self.corr { "corr" }
        else if self.dm { "dm" }
        else if self.xor { "xor" }
        else if self.quality { "quality" }
        else { "fast" }
    }
}
