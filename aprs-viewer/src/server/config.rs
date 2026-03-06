use serde::{Deserialize, Serialize};

/// Web application configuration, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default)]
    pub tnc: TncConnection,
    #[serde(default)]
    pub aprs_is: AprsIsConfig,
    #[serde(default)]
    pub reference: ReferenceConfig,
    #[serde(default = "default_maps_dir")]
    pub maps_dir: String,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_max_station_age_hours")]
    pub max_station_age_hours: u32,
    #[serde(default = "default_max_track_age_hours")]
    pub max_track_age_hours: u32,
    #[serde(default = "default_map_download_url")]
    pub map_download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TncConnection {
    #[serde(default = "default_tnc_host")]
    pub host: String,
    #[serde(default = "default_tnc_port")]
    pub port: u16,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AprsIsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_aprs_is_host")]
    pub host: String,
    #[serde(default = "default_aprs_is_port")]
    pub port: u16,
    #[serde(default)]
    pub callsign: String,
    #[serde(default)]
    pub passcode: String,
    #[serde(default)]
    pub filter: String,
}

/// Reference data configuration (CWOP station positions, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceConfig {
    /// Path to the reference database. Empty string = XDG default
    /// (`~/.local/share/packet-radio/reference.db`).
    #[serde(default)]
    pub db_path: String,
    /// How often to sync CWOP data (hours). 0 = disabled.
    #[serde(default = "default_cwop_sync_interval_hours")]
    pub cwop_sync_interval_hours: u32,
}

impl Default for ReferenceConfig {
    fn default() -> Self {
        Self {
            db_path: String::new(),
            cwop_sync_interval_hours: default_cwop_sync_interval_hours(),
        }
    }
}

fn default_cwop_sync_interval_hours() -> u32 {
    24
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            tnc: TncConnection::default(),
            aprs_is: AprsIsConfig::default(),
            reference: ReferenceConfig::default(),
            maps_dir: default_maps_dir(),
            db_path: default_db_path(),
            max_station_age_hours: default_max_station_age_hours(),
            max_track_age_hours: default_max_track_age_hours(),
            map_download_url: default_map_download_url(),
        }
    }
}

impl Default for TncConnection {
    fn default() -> Self {
        Self {
            host: default_tnc_host(),
            port: default_tnc_port(),
            enabled: false,
        }
    }
}

impl Default for AprsIsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_aprs_is_host(),
            port: default_aprs_is_port(),
            callsign: String::new(),
            passcode: String::new(),
            filter: String::new(),
        }
    }
}

fn default_listen_addr() -> String {
    "127.0.0.1:3000".into()
}
fn default_tnc_host() -> String {
    "127.0.0.1".into()
}
fn default_tnc_port() -> u16 {
    8001
}
fn default_aprs_is_host() -> String {
    "rotate.aprs2.net".into()
}
fn default_aprs_is_port() -> u16 {
    14580
}
fn default_maps_dir() -> String {
    "maps".into()
}
fn default_db_path() -> String {
    "aprs-viewer.db".into()
}
fn default_max_station_age_hours() -> u32 {
    48
}
fn default_max_track_age_hours() -> u32 {
    48
}
fn default_map_download_url() -> String {
    "https://build.protomaps.com".into()
}

impl WebConfig {
    pub fn load(path: &str) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
        toml::from_str(&content).map_err(|e| format!("Failed to parse config: {e}"))
    }

    pub fn load_or_default(path: &str) -> Self {
        Self::load(path).unwrap_or_default()
    }

    /// Save config to a TOML file.
    pub fn save(&self, path: &str) -> Result<(), String> {
        let content =
            toml::to_string_pretty(self).map_err(|e| format!("Failed to serialize config: {e}"))?;
        std::fs::write(path, content).map_err(|e| format!("Failed to write config: {e}"))
    }

    /// Validate config values.
    pub fn validate(&self) -> Result<(), String> {
        if self.tnc.port == 0 {
            return Err("TNC port must be > 0".into());
        }
        if self.aprs_is.port == 0 {
            return Err("APRS-IS port must be > 0".into());
        }
        // Callsign is optional — defaults to N0CALL (receive-only) if empty
        if self.max_station_age_hours == 0 {
            return Err("max_station_age_hours must be > 0".into());
        }
        if self.max_track_age_hours == 0 {
            return Err("max_track_age_hours must be > 0".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WebConfig::default();
        assert_eq!(config.listen_addr, "127.0.0.1:3000");
        assert_eq!(config.tnc.port, 8001);
        assert!(!config.tnc.enabled);
        assert!(!config.aprs_is.enabled);
        assert_eq!(config.max_station_age_hours, 48);
    }

    #[test]
    fn test_parse_minimal_toml() {
        let toml_str = r#"
            listen_addr = "0.0.0.0:8080"
        "#;
        let config: WebConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.listen_addr, "0.0.0.0:8080");
        assert_eq!(config.tnc.port, 8001); // default
    }

    #[test]
    fn test_parse_full_toml() {
        let toml_str = r#"
            listen_addr = "0.0.0.0:9000"
            maps_dir = "/data/maps"
            db_path = "/data/aprs.db"
            max_station_age_hours = 24
            max_track_age_hours = 12

            [tnc]
            host = "192.168.1.100"
            port = 9001
            enabled = true

            [aprs_is]
            enabled = true
            host = "noam.aprs2.net"
            port = 14580
            callsign = "W1AW"
            passcode = "12345"
            filter = "r/42/-71/100"
        "#;
        let config: WebConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.listen_addr, "0.0.0.0:9000");
        assert!(config.tnc.enabled);
        assert_eq!(config.tnc.host, "192.168.1.100");
        assert!(config.aprs_is.enabled);
        assert_eq!(config.aprs_is.callsign, "W1AW");
        assert_eq!(config.max_station_age_hours, 24);
    }

    #[test]
    fn test_roundtrip_toml() {
        let config = WebConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let back: WebConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.listen_addr, back.listen_addr);
        assert_eq!(config.tnc.port, back.tnc.port);
    }

    #[test]
    fn test_invalid_toml() {
        let result: Result<WebConfig, _> = toml::from_str("not valid toml {{{");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_and_reload() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test-config.toml");
        let path_str = path.to_str().unwrap();

        let mut config = WebConfig::default();
        config.tnc.enabled = true;
        config.tnc.host = "10.0.0.1".into();
        config.aprs_is.callsign = "TEST".into();

        config.save(path_str).unwrap();
        let loaded = WebConfig::load(path_str).unwrap();

        assert!(loaded.tnc.enabled);
        assert_eq!(loaded.tnc.host, "10.0.0.1");
        assert_eq!(loaded.aprs_is.callsign, "TEST");
    }

    #[test]
    fn test_validate_ok() {
        let config = WebConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_port() {
        let mut config = WebConfig::default();
        config.tnc.port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_aprs_is_no_callsign_ok() {
        // Callsign is now optional — defaults to N0CALL (receive-only)
        let mut config = WebConfig::default();
        config.aprs_is.enabled = true;
        config.aprs_is.callsign = "".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_station_age() {
        let mut config = WebConfig::default();
        config.max_station_age_hours = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_map_download_url_default() {
        let config = WebConfig::default();
        assert!(!config.map_download_url.is_empty());
    }
}
