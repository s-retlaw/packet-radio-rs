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
    /// `devices` is the list of available audio device names.
    pub fn from_config(config: &crate::config::TncConfig, devices: Vec<String>) -> Self {
        // Find device index
        let device_idx = devices
            .iter()
            .position(|d| *d == config.audio.device)
            .unwrap_or(0);

        // Sample rate options
        let rate_options = vec![
            "11025".into(),
            "22050".into(),
            "44100".into(),
            "48000".into(),
        ];
        let rate_str = config.audio.sample_rate.to_string();
        let rate_idx = rate_options
            .iter()
            .position(|r| *r == rate_str)
            .unwrap_or(0);

        // Mode options
        let mode_options: Vec<String> = crate::config::TncConfig::available_modes()
            .iter()
            .map(|(_, label, _)| label.to_string())
            .collect();
        let mode_descriptions: Vec<String> = crate::config::TncConfig::available_modes()
            .iter()
            .map(|(_, _, desc)| desc.to_string())
            .collect();
        let mode_values: Vec<&str> = crate::config::TncConfig::available_modes()
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
                    options: devices,
                    selected: device_idx,
                    descriptions: Vec::new(),
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
        // Mode (field 2) -- need to map label back to value
        if let Some(field) = self.fields.get(2) {
            if let FieldKind::Dropdown { selected, .. } = &field.kind {
                let modes = crate::config::TncConfig::available_modes();
                if let Some((val, _, _)) = modes.get(*selected) {
                    config.modem.mode = val.to_string();
                }
            }
        }
        // KISS port (field 3)
        if let Some(val) = self.field_value(3) {
            config.kiss.port = val.parse().unwrap_or(8001);
        }
        // Callsign (field 4)
        if let Some(val) = self.field_value(4) {
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
        let devices = vec!["default".to_string(), "hw:1,0".to_string()];
        let mut form = SettingsFormState::from_config(&config, devices);

        assert_eq!(form.selected_field, 0);
        assert_eq!(form.fields.len(), 5);

        // select_next cycles forward
        form.select_next();
        assert_eq!(form.selected_field, 1);
        form.select_next();
        assert_eq!(form.selected_field, 2);
        form.select_next();
        assert_eq!(form.selected_field, 3);
        form.select_next();
        assert_eq!(form.selected_field, 4);
        form.select_next(); // wraps
        assert_eq!(form.selected_field, 0);

        // select_prev cycles backward
        form.select_prev(); // wraps to end
        assert_eq!(form.selected_field, 4);
        form.select_prev();
        assert_eq!(form.selected_field, 3);
    }

    #[test]
    fn test_settings_form_cycle_dropdown() {
        let config = crate::config::TncConfig::default();
        let devices = vec!["default".to_string(), "pulse".to_string()];
        let mut form = SettingsFormState::from_config(&config, devices);

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
        let devices = vec!["default".to_string()];
        let form = SettingsFormState::from_config(&config, devices);

        // Field 3 = KISS TCP Port (text)
        assert_eq!(form.field_value(3), Some("9600".to_string()));
        // Field 4 = Callsign (text)
        assert_eq!(form.field_value(4), Some("W1AW".to_string()));
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
        let devices = vec!["default".to_string(), "pulse".to_string()];
        let form = SettingsFormState::from_config(&config, devices);
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
        let devices = vec!["default".to_string()];
        let mut form = SettingsFormState::from_config(&config, devices);

        // Move to text field (field 3 = KISS port)
        form.selected_field = 3;
        let before = form.field_value(3);
        form.cycle_dropdown(); // should be a no-op on text fields
        let after = form.field_value(3);
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
}
