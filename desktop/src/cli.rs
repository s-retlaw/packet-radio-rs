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

    /// Use multi-decoder (9 parallel decoders with filter/timing diversity)
    #[arg(long)]
    pub multi: bool,

    /// Verbose output (repeat for more: -v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}
