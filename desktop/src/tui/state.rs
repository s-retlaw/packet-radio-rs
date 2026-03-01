//! TUI application state types.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

/// Navigation tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Packets,
    Aprs,
    Settings,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Tab::Packets => Tab::Aprs,
            Tab::Aprs => Tab::Settings,
            Tab::Settings => Tab::Packets,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Tab::Packets => Tab::Settings,
            Tab::Aprs => Tab::Packets,
            Tab::Settings => Tab::Aprs,
        }
    }

    pub fn from_number(n: u8) -> Option<Self> {
        match n {
            1 => Some(Tab::Packets),
            2 => Some(Tab::Aprs),
            3 => Some(Tab::Settings),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Packets => "Packets",
            Tab::Aprs => "APRS",
            Tab::Settings => "Settings",
        }
    }

    pub fn number(self) -> u8 {
        match self {
            Tab::Packets => 1,
            Tab::Aprs => 2,
            Tab::Settings => 3,
        }
    }
}

/// View mode within a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Main,
    Detail,
}

/// Audio processing state -- either stopped or running.
pub enum ProcessingState {
    Stopped,
    Running {
        audio_handle: JoinHandle<()>,
        stop_signal: Arc<AtomicBool>,
    },
}

impl ProcessingState {
    pub fn is_running(&self) -> bool {
        matches!(self, ProcessingState::Running { .. })
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self, ProcessingState::Stopped)
    }
}

/// A decoded AX.25 frame for display.
#[derive(Debug, Clone)]
pub struct DecodedFrameInfo {
    pub frame_number: u64,
    pub timestamp: String,
    pub source: String,
    pub dest: String,
    pub via: String,
    pub info: String,
    pub aprs_summary: Option<String>,
    pub raw_len: usize,
}

/// An APRS station aggregated from decoded frames.
#[derive(Debug, Clone)]
pub struct AprsStation {
    pub callsign: String,
    /// "Position", "Mic-E", "Message", "Other"
    pub station_type: String,
    pub last_heard: String,
    /// (lat, lon)
    pub position: Option<(f64, f64)>,
    pub comment: String,
    pub packet_count: u32,
    /// knots
    pub speed: Option<u16>,
    /// degrees
    pub course: Option<u16>,
}

/// Runtime statistics.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub total_frames: u64,
    pub unique_frames: u64,
    pub soft_saves: u32,
    pub kiss_clients: u32,
    pub uptime_secs: u64,
}

impl Stats {
    pub fn uptime_display(&self) -> String {
        let h = self.uptime_secs / 3600;
        let m = (self.uptime_secs % 3600) / 60;
        let s = self.uptime_secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }
}

/// Settings form state for the Settings tab.
#[derive(Debug, Clone)]
pub struct SettingsFormState {
    pub selected_field: usize,
    pub editing: bool,
    pub fields: Vec<SettingsField>,
}

#[derive(Debug, Clone)]
pub struct SettingsField {
    pub label: String,
    pub kind: FieldKind,
    /// Optional input filter: transforms/rejects characters. None = accept all.
    pub filter: Option<fn(char) -> Option<char>>,
    /// Maximum text length. None = unlimited.
    pub max_len: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum FieldKind {
    Dropdown {
        options: Vec<String>,
        selected: usize,
        /// Optional descriptions corresponding to each option.
        descriptions: Vec<String>,
    },
    Text {
        value: super::widgets::TextInputState,
    },
}

impl SettingsFormState {
    /// Create settings form from a TncConfig.
    /// `devices` is the list of available audio devices with capability info.
    pub fn from_config(config: &crate::config::TncConfig, devices: &[super::AudioDeviceInfo]) -> Self {
        // Find device index
        let device_idx = devices
            .iter()
            .position(|d| d.name == config.audio.device)
            .unwrap_or(0);

        let device_names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
        let device_descriptions: Vec<String> = devices.iter().map(|d| d.description.clone()).collect();

        // Sample rate options from the selected device's supported rates
        let supported_rates = devices
            .get(device_idx)
            .map(|d| &d.supported_rates[..])
            .unwrap_or(&[11025, 22050, 44100, 48000]);
        let rate_options: Vec<String> = supported_rates.iter().map(|r| r.to_string()).collect();
        let rate_str = config.audio.sample_rate.to_string();
        let rate_idx = rate_options
            .iter()
            .position(|r| *r == rate_str)
            .unwrap_or_else(|| Self::closest_rate_index(supported_rates, config.audio.sample_rate));

        // Baud rate options
        let baud_options = vec!["300".to_string(), "1200".to_string(), "9600".to_string()];
        let baud_descriptions = vec![
            "HF AFSK (1600/1800 Hz tones)".to_string(),
            "VHF AFSK (1200/2200 Hz tones)".to_string(),
            "UHF G3RUH FSK (9600 baud baseband)".to_string(),
        ];
        let baud_idx = match config.modem.baud_rate {
            300 => 0,
            9600 => 2,
            _ => 1, // 1200 default
        };

        // Mode options — use baud-aware mode list
        let modes = crate::config::available_modes_for_baud(config.modem.baud_rate);
        let mode_options: Vec<String> = modes
            .iter()
            .map(|(_, label, _)| label.to_string())
            .collect();
        let mode_descriptions: Vec<String> = modes
            .iter()
            .map(|(_, _, desc)| desc.to_string())
            .collect();
        let mode_values: Vec<&str> = modes
            .iter()
            .map(|(val, _, _)| *val)
            .collect();
        let mode_idx = mode_values
            .iter()
            .position(|v| *v == config.modem.mode)
            .unwrap_or(0);

        let fields = vec![
            SettingsField {
                label: "Audio Device".into(),
                kind: FieldKind::Dropdown {
                    options: device_names,
                    selected: device_idx,
                    descriptions: device_descriptions,
                },
                filter: None,
                max_len: None,
            },
            SettingsField {
                label: "Sample Rate".into(),
                kind: FieldKind::Dropdown {
                    options: rate_options,
                    selected: rate_idx,
                    descriptions: Vec::new(),
                },
                filter: None,
                max_len: None,
            },
            SettingsField {
                label: "Baud Rate".into(),
                kind: FieldKind::Dropdown {
                    options: baud_options,
                    selected: baud_idx,
                    descriptions: baud_descriptions,
                },
                filter: None,
                max_len: None,
            },
            SettingsField {
                label: "Demod Mode".into(),
                kind: FieldKind::Dropdown {
                    options: mode_options,
                    selected: mode_idx,
                    descriptions: mode_descriptions,
                },
                filter: None,
                max_len: None,
            },
            SettingsField {
                label: "KISS TCP Port".into(),
                kind: FieldKind::Text {
                    value: super::widgets::TextInputState::with_value(
                        &config.kiss.port.to_string(),
                    ),
                },
                filter: Some(|c| if c.is_ascii_digit() { Some(c) } else { None }),
                max_len: Some(5), // max u16 is 65535 (5 digits)
            },
            SettingsField {
                label: "Callsign".into(),
                kind: FieldKind::Text {
                    value: super::widgets::TextInputState::with_value(&config.station.callsign),
                },
                filter: Some(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        Some(c.to_ascii_uppercase())
                    } else {
                        None
                    }
                }),
                max_len: Some(9), // e.g. W1AW-15
            },
        ];

        Self {
            selected_field: 0,
            editing: false,
            fields,
        }
    }

    /// Called when the audio device dropdown changes. Updates the sample rate
    /// dropdown to only show rates supported by the new device.
    /// Returns a notification message if the rate was changed.
    pub fn on_device_changed(&mut self, devices: &[super::AudioDeviceInfo]) -> Option<String> {
        // Get the newly selected device index from field 0
        let device_idx = match &self.fields[0].kind {
            FieldKind::Dropdown { selected, .. } => *selected,
            _ => return None,
        };

        let supported_rates = match devices.get(device_idx) {
            Some(dev) => &dev.supported_rates,
            None => return None,
        };

        // Get the currently selected rate before changing
        let old_rate: Option<u32> = self.field_value(1).and_then(|v| v.parse().ok());

        // Replace field 1's options
        if let Some(field) = self.fields.get_mut(1) {
            if let FieldKind::Dropdown { options, selected, .. } = &mut field.kind {
                let new_options: Vec<String> = supported_rates.iter().map(|r| r.to_string()).collect();

                // Try to keep the same rate selected
                let old_rate_str = old_rate.map(|r| r.to_string()).unwrap_or_default();
                let new_idx = new_options.iter().position(|r| *r == old_rate_str);

                *options = new_options;

                if let Some(idx) = new_idx {
                    *selected = idx;
                    return None; // rate unchanged
                }

                // Rate not available — pick closest
                let closest = Self::closest_rate_index(supported_rates, old_rate.unwrap_or(11025));
                *selected = closest;
                let new_rate = supported_rates.get(closest).copied().unwrap_or(11025);
                return Some(format!("Sample rate changed to {} Hz (device constraint)", new_rate));
            }
        }

        None
    }

    /// Called when the baud rate dropdown changes. Updates sample rate options
    /// (9600 requires >=44100) and swaps the mode list. Returns a notification
    /// message if the sample rate was changed.
    pub fn on_baud_changed(&mut self, devices: &[super::AudioDeviceInfo]) -> Option<String> {
        // Get the newly selected baud rate from field 2
        let new_baud: u32 = match &self.fields[2].kind {
            FieldKind::Dropdown { options, selected, .. } => {
                options.get(*selected).and_then(|v| v.parse().ok()).unwrap_or(1200)
            }
            _ => return None,
        };

        // --- Update sample rate options (field 1) ---
        // Get the device's supported rates from field 0
        let device_idx = match &self.fields[0].kind {
            FieldKind::Dropdown { selected, .. } => *selected,
            _ => 0,
        };
        let device_rates = match devices.get(device_idx) {
            Some(dev) => &dev.supported_rates,
            None => return None,
        };

        // For 9600 baud, filter to only rates >= 44100
        let filtered_rates: Vec<u32> = if new_baud == 9600 {
            device_rates.iter().copied().filter(|&r| r >= 44100).collect()
        } else {
            device_rates.to_vec()
        };
        let filtered_rates = if filtered_rates.is_empty() { device_rates.clone() } else { filtered_rates };

        let old_rate: Option<u32> = self.field_value(1).and_then(|v| v.parse().ok());
        let mut rate_msg = None;

        if let Some(field) = self.fields.get_mut(1) {
            if let FieldKind::Dropdown { options, selected, .. } = &mut field.kind {
                let new_options: Vec<String> = filtered_rates.iter().map(|r| r.to_string()).collect();
                let old_rate_str = old_rate.map(|r| r.to_string()).unwrap_or_default();
                let new_idx = new_options.iter().position(|r| *r == old_rate_str);

                *options = new_options;

                if let Some(idx) = new_idx {
                    *selected = idx;
                } else {
                    let closest = Self::closest_rate_index(&filtered_rates, old_rate.unwrap_or(11025));
                    *selected = closest;
                    let new_rate = filtered_rates.get(closest).copied().unwrap_or(11025);
                    rate_msg = Some(format!("Sample rate changed to {} Hz (baud rate constraint)", new_rate));
                }
            }
        }

        // --- Swap mode list (field 3) ---
        let modes = crate::config::available_modes_for_baud(new_baud);
        if let Some(field) = self.fields.get_mut(3) {
            if let FieldKind::Dropdown { options, selected, descriptions } = &mut field.kind {
                *options = modes.iter().map(|(_, label, _)| label.to_string()).collect();
                *descriptions = modes.iter().map(|(_, _, desc)| desc.to_string()).collect();
                *selected = 0; // reset to first (best default)
            }
        }

        rate_msg
    }

    /// Find the index of the closest rate in a sorted list.
    fn closest_rate_index(rates: &[u32], target: u32) -> usize {
        if rates.is_empty() {
            return 0;
        }
        rates
            .iter()
            .enumerate()
            .min_by_key(|(_, &r)| (r as i64 - target as i64).unsigned_abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    pub fn select_next(&mut self) {
        if !self.fields.is_empty() {
            self.selected_field = (self.selected_field + 1) % self.fields.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.fields.is_empty() {
            if self.selected_field == 0 {
                self.selected_field = self.fields.len() - 1;
            } else {
                self.selected_field -= 1;
            }
        }
    }

    /// Cycle the dropdown option for the current field.
    pub fn cycle_dropdown(&mut self) {
        if let Some(field) = self.fields.get_mut(self.selected_field) {
            if let FieldKind::Dropdown { options, selected, .. } = &mut field.kind {
                if !options.is_empty() {
                    *selected = (*selected + 1) % options.len();
                }
            }
        }
    }

    /// Get the current value string for a field.
    pub fn field_value(&self, idx: usize) -> Option<String> {
        self.fields.get(idx).map(|f| match &f.kind {
            FieldKind::Dropdown { options, selected, .. } => {
                options.get(*selected).cloned().unwrap_or_default()
            }
            FieldKind::Text { value } => value.value().to_string(),
        })
    }

    /// Get the description for the currently selected dropdown option, if any.
    pub fn field_description(&self, idx: usize) -> Option<&str> {
        self.fields.get(idx).and_then(|f| match &f.kind {
            FieldKind::Dropdown { selected, descriptions, .. } => {
                descriptions.get(*selected).map(|s| s.as_str()).filter(|s| !s.is_empty())
            }
            _ => None,
        })
    }

    /// Apply settings back to a TncConfig.
    pub fn to_config(&self) -> crate::config::TncConfig {
        let mut config = crate::config::TncConfig::default();

        // Audio device (field 0)
        if let Some(val) = self.field_value(0) {
            config.audio.device = val;
        }
        // Sample rate (field 1)
        if let Some(val) = self.field_value(1) {
            config.audio.sample_rate = val.parse().unwrap_or(11025);
        }
        // Baud rate (field 2)
        if let Some(val) = self.field_value(2) {
            config.modem.baud_rate = val.parse().unwrap_or(1200);
        }
        // Mode (field 3) -- need to map label back to value
        if let Some(field) = self.fields.get(3) {
            if let FieldKind::Dropdown { selected, .. } = &field.kind {
                let modes = crate::config::available_modes_for_baud(config.modem.baud_rate);
                if let Some((val, _, _)) = modes.get(*selected) {
                    config.modem.mode = val.to_string();
                }
            }
        }
        // KISS port (field 4)
        if let Some(val) = self.field_value(4) {
            config.kiss.port = val.parse().unwrap_or(8001);
        }
        // Callsign (field 5)
        if let Some(val) = self.field_value(5) {
            config.station.callsign = val;
        }

        config
    }
}

/// Async events from the audio thread to the TUI.
#[derive(Debug, Clone)]
pub enum AsyncEvent {
    FrameDecoded(DecodedFrameInfo),
    StatsUpdate(Stats),
    AudioDone,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::AudioDeviceInfo;

    /// Helper: build test AudioDeviceInfo from name strings (all rates supported).
    fn test_devices(names: &[&str]) -> Vec<AudioDeviceInfo> {
        names.iter().map(|n| AudioDeviceInfo {
            name: n.to_string(),
            description: format!("1ch, 8000-48000 Hz, I16"),
            supported_rates: vec![11025, 22050, 44100, 48000],
        }).collect()
    }

    /// Helper: build a device with restricted rates.
    fn test_device_with_rates(name: &str, rates: &[u32]) -> AudioDeviceInfo {
        AudioDeviceInfo {
            name: name.to_string(),
            description: "test".to_string(),
            supported_rates: rates.to_vec(),
        }
    }

    #[test]
    fn test_tab_next() {
        assert_eq!(Tab::Packets.next(), Tab::Aprs);
        assert_eq!(Tab::Aprs.next(), Tab::Settings);
        assert_eq!(Tab::Settings.next(), Tab::Packets);
    }

    #[test]
    fn test_tab_prev() {
        assert_eq!(Tab::Packets.prev(), Tab::Settings);
        assert_eq!(Tab::Aprs.prev(), Tab::Packets);
        assert_eq!(Tab::Settings.prev(), Tab::Aprs);
    }

    #[test]
    fn test_tab_from_number() {
        assert_eq!(Tab::from_number(1), Some(Tab::Packets));
        assert_eq!(Tab::from_number(2), Some(Tab::Aprs));
        assert_eq!(Tab::from_number(3), Some(Tab::Settings));
        assert_eq!(Tab::from_number(0), None);
        assert_eq!(Tab::from_number(4), None);
        assert_eq!(Tab::from_number(255), None);
    }

    #[test]
    fn test_tab_label_and_number() {
        for tab in [Tab::Packets, Tab::Aprs, Tab::Settings] {
            // Round-trip: number -> from_number -> same tab
            assert_eq!(Tab::from_number(tab.number()), Some(tab));
            // Label is non-empty
            assert!(!tab.label().is_empty());
        }
        assert_eq!(Tab::Packets.label(), "Packets");
        assert_eq!(Tab::Aprs.label(), "APRS");
        assert_eq!(Tab::Settings.label(), "Settings");
    }

    #[test]
    fn test_tab_full_cycle() {
        let start = Tab::Packets;
        let after_three_next = start.next().next().next();
        assert_eq!(start, after_three_next);

        let after_three_prev = start.prev().prev().prev();
        assert_eq!(start, after_three_prev);
    }

    #[test]
    fn test_view_variants() {
        let main = View::Main;
        let detail = View::Detail;
        assert_eq!(main, View::Main);
        assert_eq!(detail, View::Detail);
        assert_ne!(main, detail);
    }

    #[test]
    fn test_processing_state_is_running() {
        use std::sync::atomic::Ordering;

        let stopped = ProcessingState::Stopped;
        assert!(stopped.is_stopped());
        assert!(!stopped.is_running());

        let stop_signal = Arc::new(AtomicBool::new(false));
        let signal_clone = stop_signal.clone();
        let handle = std::thread::spawn(move || {
            while !signal_clone.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });

        let running = ProcessingState::Running {
            audio_handle: handle,
            stop_signal: stop_signal.clone(),
        };
        assert!(running.is_running());
        assert!(!running.is_stopped());

        // Clean up the thread
        stop_signal.store(true, std::sync::atomic::Ordering::Relaxed);
        if let ProcessingState::Running { audio_handle, .. } = running {
            audio_handle.join().unwrap();
        }
    }

    #[test]
    fn test_processing_state_is_stopped() {
        let state = ProcessingState::Stopped;
        assert!(state.is_stopped());
        assert!(!state.is_running());
    }

    #[test]
    fn test_stats_uptime_display() {
        let stats = Stats {
            uptime_secs: 0,
            ..Default::default()
        };
        assert_eq!(stats.uptime_display(), "00:00:00");

        let stats = Stats {
            uptime_secs: 61,
            ..Default::default()
        };
        assert_eq!(stats.uptime_display(), "00:01:01");

        let stats = Stats {
            uptime_secs: 3661,
            ..Default::default()
        };
        assert_eq!(stats.uptime_display(), "01:01:01");

        let stats = Stats {
            uptime_secs: 86399,
            ..Default::default()
        };
        assert_eq!(stats.uptime_display(), "23:59:59");

        let stats = Stats {
            uptime_secs: 90061,
            ..Default::default()
        };
        assert_eq!(stats.uptime_display(), "25:01:01");
    }

    #[test]
    fn test_stats_default() {
        let stats = Stats::default();
        assert_eq!(stats.total_frames, 0);
        assert_eq!(stats.unique_frames, 0);
        assert_eq!(stats.soft_saves, 0);
        assert_eq!(stats.kiss_clients, 0);
        assert_eq!(stats.uptime_secs, 0);
    }

    #[test]
    fn test_settings_form_nav() {
        let config = crate::config::TncConfig::default();
        let devices = test_devices(&["default", "hw:1,0"]);
        let mut form = SettingsFormState::from_config(&config, &devices);

        assert_eq!(form.selected_field, 0);
        assert_eq!(form.fields.len(), 6);

        // select_next cycles forward
        form.select_next();
        assert_eq!(form.selected_field, 1);
        form.select_next();
        assert_eq!(form.selected_field, 2);
        form.select_next();
        assert_eq!(form.selected_field, 3);
        form.select_next();
        assert_eq!(form.selected_field, 4);
        form.select_next();
        assert_eq!(form.selected_field, 5);
        form.select_next(); // wraps
        assert_eq!(form.selected_field, 0);

        // select_prev cycles backward
        form.select_prev(); // wraps to end
        assert_eq!(form.selected_field, 5);
        form.select_prev();
        assert_eq!(form.selected_field, 4);
    }

    #[test]
    fn test_settings_form_cycle_dropdown() {
        let config = crate::config::TncConfig::default();
        let devices = test_devices(&["default", "pulse"]);
        let mut form = SettingsFormState::from_config(&config, &devices);

        // Field 0 is Audio Device dropdown with 2 options
        assert_eq!(form.selected_field, 0);
        assert_eq!(form.field_value(0), Some("default".to_string()));

        form.cycle_dropdown();
        assert_eq!(form.field_value(0), Some("pulse".to_string()));

        form.cycle_dropdown(); // wraps back
        assert_eq!(form.field_value(0), Some("default".to_string()));
    }

    #[test]
    fn test_settings_form_field_value() {
        let config = crate::config::TncConfig {
            kiss: crate::config::KissConfig { port: 9600 },
            station: crate::config::StationConfig {
                callsign: "W1AW".to_string(),
            },
            ..Default::default()
        };
        let devices = test_devices(&["default"]);
        let form = SettingsFormState::from_config(&config, &devices);

        // Field 4 = KISS TCP Port (text)
        assert_eq!(form.field_value(4), Some("9600".to_string()));
        // Field 5 = Callsign (text)
        assert_eq!(form.field_value(5), Some("W1AW".to_string()));
        // Out of bounds
        assert_eq!(form.field_value(99), None);
    }

    #[test]
    fn test_settings_form_to_config_roundtrip() {
        let config = crate::config::TncConfig {
            audio: crate::config::AudioConfig {
                device: "pulse".to_string(),
                sample_rate: 44100,
            },
            modem: crate::config::ModemConfig {
                mode: "smart3".to_string(),
                ..Default::default()
            },
            kiss: crate::config::KissConfig { port: 9600 },
            station: crate::config::StationConfig {
                callsign: "W1AW".to_string(),
            },
        };
        let devices = test_devices(&["default", "pulse"]);
        let form = SettingsFormState::from_config(&config, &devices);
        let result = form.to_config();

        assert_eq!(result.audio.device, "pulse");
        assert_eq!(result.audio.sample_rate, 44100);
        assert_eq!(result.modem.mode, "smart3");
        assert_eq!(result.kiss.port, 9600);
        assert_eq!(result.station.callsign, "W1AW");
    }

    #[test]
    fn test_settings_form_cycle_on_text_field_is_noop() {
        let config = crate::config::TncConfig::default();
        let devices = test_devices(&["default"]);
        let mut form = SettingsFormState::from_config(&config, &devices);

        // Move to text field (field 4 = KISS port)
        form.selected_field = 4;
        let before = form.field_value(4);
        form.cycle_dropdown(); // should be a no-op on text fields
        let after = form.field_value(4);
        assert_eq!(before, after);
    }

    #[test]
    fn test_decoded_frame_info() {
        let frame = DecodedFrameInfo {
            frame_number: 1,
            timestamp: "12:34:56".to_string(),
            source: "W1AW".to_string(),
            dest: "APRS".to_string(),
            via: "WIDE1-1".to_string(),
            info: "!4903.50N/07201.75W-".to_string(),
            aprs_summary: Some("Position: 49.058N 72.029W".to_string()),
            raw_len: 64,
        };
        assert_eq!(frame.frame_number, 1);
        assert!(frame.aprs_summary.is_some());
    }

    #[test]
    fn test_aprs_station() {
        let station = AprsStation {
            callsign: "W1AW".to_string(),
            station_type: "Position".to_string(),
            last_heard: "12:34:56".to_string(),
            position: Some((49.058, -72.029)),
            comment: "Hiram Percy Maxim Memorial Station".to_string(),
            packet_count: 5,
            speed: Some(0),
            course: None,
        };
        assert_eq!(station.callsign, "W1AW");
        assert_eq!(station.packet_count, 5);
        assert!(station.position.is_some());
        assert!(station.course.is_none());
    }

    #[test]
    fn test_async_event_variants() {
        let frame_evt = AsyncEvent::FrameDecoded(DecodedFrameInfo {
            frame_number: 1,
            timestamp: "00:00:00".to_string(),
            source: "TEST".to_string(),
            dest: "APRS".to_string(),
            via: String::new(),
            info: String::new(),
            aprs_summary: None,
            raw_len: 0,
        });
        assert!(matches!(frame_evt, AsyncEvent::FrameDecoded(_)));

        let stats_evt = AsyncEvent::StatsUpdate(Stats::default());
        assert!(matches!(stats_evt, AsyncEvent::StatsUpdate(_)));

        let done_evt = AsyncEvent::AudioDone;
        assert!(matches!(done_evt, AsyncEvent::AudioDone));
    }

    #[test]
    fn test_on_device_changed_rate_stays() {
        let devices = vec![
            test_device_with_rates("dev_a", &[11025, 22050, 44100, 48000]),
            test_device_with_rates("dev_b", &[11025, 22050, 44100, 48000]),
        ];
        let config = crate::config::TncConfig::default(); // 11025
        let mut form = SettingsFormState::from_config(&config, &devices);
        // Select dev_b
        form.selected_field = 0;
        form.cycle_dropdown();
        let msg = form.on_device_changed(&devices);
        assert!(msg.is_none()); // rate 11025 still available
        assert_eq!(form.field_value(1), Some("11025".to_string()));
    }

    #[test]
    fn test_on_device_changed_rate_forced() {
        let devices = vec![
            test_device_with_rates("dev_a", &[11025, 22050, 44100, 48000]),
            test_device_with_rates("dev_b", &[44100, 48000]),
        ];
        let config = crate::config::TncConfig::default(); // 11025
        let mut form = SettingsFormState::from_config(&config, &devices);
        assert_eq!(form.field_value(1), Some("11025".to_string()));

        // Switch to dev_b (only supports 44100/48000)
        form.selected_field = 0;
        form.cycle_dropdown();
        let msg = form.on_device_changed(&devices);
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("44100")); // closest to 11025

        // Rate options should now be [44100, 48000]
        if let FieldKind::Dropdown { options, .. } = &form.fields[1].kind {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0], "44100");
            assert_eq!(options[1], "48000");
        } else {
            panic!("expected dropdown");
        }
    }

    #[test]
    fn test_device_description_shown() {
        let devices = vec![AudioDeviceInfo {
            name: "hw:0,0".to_string(),
            description: "1ch, 8000-48000 Hz, I16".to_string(),
            supported_rates: vec![11025, 22050, 44100, 48000],
        }];
        let config = crate::config::TncConfig::default();
        let form = SettingsFormState::from_config(&config, &devices);
        // Field 0 (Audio Device) should have a description
        let desc = form.field_description(0);
        assert!(desc.is_some());
        assert!(desc.unwrap().contains("8000-48000"));
    }

    #[test]
    fn test_closest_rate_index() {
        assert_eq!(SettingsFormState::closest_rate_index(&[44100, 48000], 11025), 0);
        assert_eq!(SettingsFormState::closest_rate_index(&[44100, 48000], 48000), 1);
        assert_eq!(SettingsFormState::closest_rate_index(&[11025, 48000], 22050), 0);
        assert_eq!(SettingsFormState::closest_rate_index(&[], 11025), 0);
    }

    #[test]
    fn test_initial_rate_filtered_by_device() {
        // Device only supports 48000
        let devices = vec![test_device_with_rates("narrow_dev", &[48000])];
        let mut config = crate::config::TncConfig::default();
        config.audio.sample_rate = 11025; // not supported by device
        config.audio.device = "narrow_dev".to_string();
        let form = SettingsFormState::from_config(&config, &devices);

        // Rate should be auto-selected to closest (48000)
        assert_eq!(form.field_value(1), Some("48000".to_string()));
        // Only 1 rate option
        if let FieldKind::Dropdown { options, .. } = &form.fields[1].kind {
            assert_eq!(options.len(), 1);
        }
    }

    #[test]
    fn test_on_baud_changed_updates_rates() {
        let devices = vec![
            test_device_with_rates("dev_a", &[11025, 22050, 44100, 48000]),
        ];
        let config = crate::config::TncConfig::default(); // 1200 baud, 11025 Hz
        let mut form = SettingsFormState::from_config(&config, &devices);

        // Switch to 9600 baud (field 2, index 2)
        form.selected_field = 2;
        if let FieldKind::Dropdown { selected, .. } = &mut form.fields[2].kind {
            *selected = 2; // "9600"
        }
        let msg = form.on_baud_changed(&devices);
        // 11025 Hz is not valid for 9600 baud, should be changed
        assert!(msg.is_some());
        let msg_str = msg.unwrap();
        assert!(msg_str.contains("44100") || msg_str.contains("48000"));

        // Rate options should only include >= 44100
        if let FieldKind::Dropdown { options, .. } = &form.fields[1].kind {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0], "44100");
            assert_eq!(options[1], "48000");
        }
    }

    #[test]
    fn test_on_baud_changed_updates_modes() {
        let devices = test_devices(&["default"]);
        let config = crate::config::TncConfig::default(); // 1200 baud
        let mut form = SettingsFormState::from_config(&config, &devices);

        // Mode list should be AFSK modes initially
        if let FieldKind::Dropdown { options, .. } = &form.fields[3].kind {
            assert!(options.iter().any(|o| o.contains("Goertzel") || o.contains("single")));
        }

        // Switch to 9600 baud
        form.selected_field = 2;
        if let FieldKind::Dropdown { selected, .. } = &mut form.fields[2].kind {
            *selected = 2; // "9600"
        }
        form.on_baud_changed(&devices);

        // Mode list should now be 9600 modes
        if let FieldKind::Dropdown { options, selected, .. } = &form.fields[3].kind {
            assert_eq!(*selected, 0); // reset to first
            assert!(options.iter().any(|o| o.contains("DireWolf")));
            assert!(!options.iter().any(|o| o.contains("Goertzel") || o.contains("single")));
        }
    }

    #[test]
    fn test_to_config_baud_roundtrip() {
        let config = crate::config::TncConfig {
            modem: crate::config::ModemConfig {
                mode: "direwolf".to_string(),
                baud_rate: 9600,
            },
            audio: crate::config::AudioConfig {
                sample_rate: 48000,
                ..Default::default()
            },
            ..Default::default()
        };
        let devices = test_devices(&["default"]);
        let form = SettingsFormState::from_config(&config, &devices);
        let result = form.to_config();

        assert_eq!(result.modem.baud_rate, 9600);
        assert_eq!(result.modem.mode, "direwolf");
        assert_eq!(result.audio.sample_rate, 48000);
    }
}
