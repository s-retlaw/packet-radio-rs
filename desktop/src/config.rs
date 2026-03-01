//! TNC configuration file — load/save/merge for `packet-radio.toml`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// All known modem modes for 300/1200 baud AFSK.
const AVAILABLE_MODES: &[(&str, &str, &str)] = &[
    ("fast", "Fast (single)", "Single Goertzel + Bresenham. Lowest CPU, basic decode."),
    ("quality", "Quality (soft)", "Goertzel + LLR soft decode. Slight improvement over fast."),
    ("multi", "Multi (38x parallel)", "38 parallel decoders. Best decode rate, highest CPU."),
    ("smart3", "Smart3 (3x mini)", "3 optimal decoders. ~97% of multi at 8% CPU cost."),
    ("dm", "Delay-Multiply", "Delay-multiply discriminator + Gardner PLL timing."),
    ("corr", "Correlation", "DireWolf-style mixer demodulator + soft HDLC."),
    ("corr-slicer", "Corr Slicer", "Correlation + 24 gain/frequency diversity slicers."),
    ("corr-pll", "Corr + PLL", "Correlation mixer with Gardner PLL clock recovery."),
    ("xor", "Binary XOR", "Binary XOR correlator. Twist-immune, amplitude-invariant."),
];

/// All known modem modes for 9600 baud G3RUH FSK.
const AVAILABLE_MODES_9600: &[(&str, &str, &str)] = &[
    ("multi", "Multi (ensemble)", "Parallel 9600 decoders. Best decode rate."),
    ("mini", "Mini (6x MCU)", "6 MCU-optimal decoders. Lower CPU."),
    ("direwolf", "DireWolf", "Single decoder, DireWolf-style matched filter."),
    ("gardner", "Gardner", "Single decoder, Gardner TED timing."),
    ("early-late", "Early-Late", "Single decoder, early-late gate timing."),
    ("mm", "Mueller-Muller", "Single decoder, Mueller-Muller TED."),
    ("rrc", "RRC", "Single decoder, root-raised-cosine matched filter."),
];

/// Return the mode list appropriate for the given baud rate.
pub fn available_modes_for_baud(baud: u32) -> &'static [(&'static str, &'static str, &'static str)] {
    if baud == 9600 { AVAILABLE_MODES_9600 } else { AVAILABLE_MODES }
}

/// Top-level config file structure matching `packet-radio.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TncConfig {
    pub audio: AudioConfig,
    pub modem: ModemConfig,
    pub kiss: KissConfig,
    pub station: StationConfig,
}

/// Audio device and sample rate configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio device name, or "default" for the system default.
    pub device: String,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// Modem / demodulator mode selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModemConfig {
    /// Demodulator mode: fast|quality|multi|smart3|dm|corr|corr-slicer|corr-pll|xor
    pub mode: String,
    /// Baud rate: 300 (HF), 1200 (VHF), or 9600 (UHF). Default: 1200.
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
}

fn default_baud_rate() -> u32 { 1200 }

/// KISS TCP server configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KissConfig {
    /// TCP port for the KISS server.
    pub port: u16,
}

/// Station identification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StationConfig {
    /// Amateur radio callsign.
    pub callsign: String,
}

// ── Default implementations ──────────────────────────────────────────

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: "default".to_string(),
            sample_rate: 11025,
        }
    }
}

impl Default for ModemConfig {
    fn default() -> Self {
        Self {
            mode: "multi".to_string(),
            baud_rate: 1200,
        }
    }
}

impl Default for KissConfig {
    fn default() -> Self {
        Self { port: 8001 }
    }
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            callsign: "N0CALL".to_string(),
        }
    }
}

// ── TncConfig methods ────────────────────────────────────────────────

impl TncConfig {
    /// Load configuration from a TOML file at `path`.
    ///
    /// Returns an error string if the file cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, String> {
        let contents =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        toml::from_str(&contents).map_err(|e| format!("failed to parse {}: {}", path.display(), e))
    }

    /// Serialize this configuration to TOML and write it to `path`.
    ///
    /// Creates parent directories if they do not exist.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {}", parent.display(), e))?;
        }
        let contents =
            toml::to_string_pretty(self).map_err(|e| format!("failed to serialize config: {}", e))?;
        std::fs::write(path, contents).map_err(|e| format!("failed to write {}: {}", path.display(), e))
    }

    /// Resolve the configuration file path.
    ///
    /// If `cli_override` is `Some`, that path is returned directly.
    /// Otherwise returns `./packet-radio.toml` in the current directory.
    pub fn config_path(cli_override: Option<&Path>) -> PathBuf {
        match cli_override {
            Some(p) => p.to_path_buf(),
            None => PathBuf::from("./packet-radio.toml"),
        }
    }

    /// Load configuration from `path` if the file exists, otherwise return
    /// the default configuration.
    pub fn load_or_default(path: &Path) -> Self {
        if path.exists() {
            Self::load(path).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    /// Human-readable label for the currently configured modem mode.
    pub fn mode_label(&self) -> &str {
        let modes = available_modes_for_baud(self.modem.baud_rate);
        modes
            .iter()
            .find(|(value, _, _)| *value == self.modem.mode)
            .map(|(_, label, _)| *label)
            .unwrap_or(&self.modem.mode)
    }

    /// Description for the currently configured modem mode.
    #[allow(dead_code)]
    pub fn mode_description(&self) -> &str {
        let modes = available_modes_for_baud(self.modem.baud_rate);
        modes
            .iter()
            .find(|(value, _, _)| *value == self.modem.mode)
            .map(|(_, _, desc)| *desc)
            .unwrap_or("")
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_defaults() {
        let cfg = TncConfig::default();
        assert_eq!(cfg.audio.device, "default");
        assert_eq!(cfg.audio.sample_rate, 11025);
        assert_eq!(cfg.modem.mode, "multi");
        assert_eq!(cfg.kiss.port, 8001);
        assert_eq!(cfg.station.callsign, "N0CALL");
    }

    #[test]
    fn test_roundtrip() {
        let dir = std::env::temp_dir().join("packet-radio-config-test-roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("roundtrip.toml");

        let cfg = TncConfig {
            audio: AudioConfig {
                device: "hw:1,0".to_string(),
                sample_rate: 44100,
            },
            modem: ModemConfig {
                mode: "smart3".to_string(),
                ..Default::default()
            },
            kiss: KissConfig { port: 9600 },
            station: StationConfig {
                callsign: "W1AW".to_string(),
            },
        };

        cfg.save(&path).expect("save should succeed");
        let loaded = TncConfig::load(&path).expect("load should succeed");
        assert_eq!(cfg, loaded);

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_missing_fields() {
        let dir = std::env::temp_dir().join("packet-radio-config-test-missing");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("partial.toml");

        // Write a TOML file with only some fields
        let mut f = std::fs::File::create(&path).expect("create file");
        writeln!(f, "[audio]").unwrap();
        writeln!(f, "device = \"pulse\"").unwrap();
        // omit sample_rate, modem, kiss, station entirely
        drop(f);

        let cfg = TncConfig::load(&path).expect("load should succeed");
        assert_eq!(cfg.audio.device, "pulse");
        assert_eq!(cfg.audio.sample_rate, 11025); // default
        assert_eq!(cfg.modem.mode, "multi"); // default
        assert_eq!(cfg.kiss.port, 8001); // default
        assert_eq!(cfg.station.callsign, "N0CALL"); // default

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_merge_empty() {
        let dir = std::env::temp_dir().join("packet-radio-config-test-empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty.toml");

        std::fs::write(&path, "").expect("write empty file");

        let cfg = TncConfig::load(&path).expect("load should succeed");
        assert_eq!(cfg, TncConfig::default());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_save_creates_file() {
        let dir = std::env::temp_dir().join("packet-radio-config-test-create");
        let path = dir.join("subdir").join("new-config.toml");

        // Ensure target does not exist
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);

        let cfg = TncConfig::default();
        cfg.save(&path).expect("save should create file and parent dirs");

        assert!(path.exists(), "file should exist after save");

        let loaded = TncConfig::load(&path).expect("should load saved file");
        assert_eq!(cfg, loaded);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mode_label() {
        let cases = [
            ("fast", "Fast (single)"),
            ("quality", "Quality (soft)"),
            ("multi", "Multi (38x parallel)"),
            ("smart3", "Smart3 (3x mini)"),
            ("dm", "Delay-Multiply"),
            ("corr", "Correlation"),
            ("corr-slicer", "Corr Slicer"),
            ("corr-pll", "Corr + PLL"),
            ("xor", "Binary XOR"),
        ];

        for (mode, expected_label) in &cases {
            let cfg = TncConfig {
                modem: ModemConfig {
                    mode: mode.to_string(),
                    ..Default::default()
                },
                ..TncConfig::default()
            };
            assert_eq!(cfg.mode_label(), *expected_label, "mode_label for '{}'", mode);
        }

        // Unknown mode returns the mode string itself
        let cfg = TncConfig {
            modem: ModemConfig {
                mode: "experimental".to_string(),
                ..Default::default()
            },
            ..TncConfig::default()
        };
        assert_eq!(cfg.mode_label(), "experimental");
    }

    #[test]
    fn test_config_path_override() {
        let override_path = Path::new("/tmp/my-custom-config.toml");
        let result = TncConfig::config_path(Some(override_path));
        assert_eq!(result, PathBuf::from("/tmp/my-custom-config.toml"));
    }

    #[test]
    fn test_config_path_default() {
        let result = TncConfig::config_path(None);
        assert_eq!(result, PathBuf::from("./packet-radio.toml"));
    }

    #[test]
    fn test_mode_description() {
        let cfg = TncConfig {
            modem: ModemConfig {
                mode: "multi".to_string(),
                ..Default::default()
            },
            ..TncConfig::default()
        };
        assert!(cfg.mode_description().contains("38 parallel"));

        let cfg = TncConfig {
            modem: ModemConfig {
                mode: "experimental".to_string(),
                ..Default::default()
            },
            ..TncConfig::default()
        };
        assert_eq!(cfg.mode_description(), "");
    }

    #[test]
    fn test_available_modes_have_descriptions() {
        // Check both 1200 and 9600 mode lists
        for baud in [1200, 9600] {
            for (val, label, desc) in available_modes_for_baud(baud) {
                assert!(!val.is_empty(), "mode value should not be empty (baud {baud})");
                assert!(!label.is_empty(), "mode label should not be empty (baud {baud})");
                assert!(!desc.is_empty(), "mode '{}' should have a description (baud {baud})", val);
            }
        }
    }
}
