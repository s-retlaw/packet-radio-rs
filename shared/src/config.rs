//! Configuration — TNC settings, audio config, network config.
//!
//! TODO: Define configuration structure and parsing (TOML or JSON)

/// Top-level TNC configuration
pub struct TncConfig {
    /// Station callsign (e.g., "N0CALL-1")
    pub callsign: String,
    /// Audio device name or index
    pub audio_device: String,
    /// Audio sample rate
    pub sample_rate: u32,
    /// Number of parallel demodulators
    pub num_decoders: u8,
    /// KISS TCP listen port (0 = disabled)
    pub kiss_tcp_port: u16,
    /// AGW listen port (0 = disabled)
    pub agw_port: u16,
    /// APRS-IS configuration (None = IGate disabled)
    pub aprs_is: Option<AprsIsConfig>,
}

/// APRS-IS connection configuration
pub struct AprsIsConfig {
    /// Server hostname (e.g., "rotate.aprs2.net")
    pub server: String,
    /// Server port (typically 14580)
    pub port: u16,
    /// Callsign for APRS-IS login
    pub callsign: String,
    /// Passcode (computed from callsign)
    pub passcode: i16,
    /// Filter string (e.g., "r/35.0/-106.0/100")
    pub filter: String,
}

impl Default for TncConfig {
    fn default() -> Self {
        Self {
            callsign: "N0CALL".to_string(),
            audio_device: "default".to_string(),
            sample_rate: 11025,
            num_decoders: 3,
            kiss_tcp_port: 8001,
            agw_port: 8000,
            aprs_is: None,
        }
    }
}
