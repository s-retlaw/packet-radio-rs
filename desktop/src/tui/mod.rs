//! TUI module — terminal user interface for the desktop TNC.
//!
//! The TUI is the default mode. It provides a tabbed interface for
//! monitoring decoded frames, APRS stations, and configuring settings.

pub mod event;
pub mod state;
pub mod ui;
pub mod widgets;

use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::TncConfig;
use self::event::{Event, EventHandler};
use self::state::*;
use self::widgets::{FilePickerState, SelectableList};

/// Audio device info collected at enumeration time.
#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub name: String,
    /// Human-readable capability summary (e.g. "1ch, 8000-48000 Hz, I16/F32")
    pub description: String,
    /// Sample rates supported by BOTH the device AND our demodulator
    pub supported_rates: Vec<u32>,
}

/// Demodulator-supported rates we expose to users.
const USER_SAMPLE_RATES: [u32; 4] = [11025, 22050, 44100, 48000];

/// The main TUI application.
pub struct App {
    pub tab: Tab,
    pub processing: ProcessingState,
    pub config: TncConfig,
    pub config_path: std::path::PathBuf,
    pub frames: SelectableList<DecodedFrameInfo>,
    pub aprs_stations: SelectableList<AprsStation>,
    pub stats: Stats,
    pub settings: SettingsFormState,
    pub should_quit: bool,
    pub show_quit_dialog: bool,
    pub quit_selected: usize,
    pub show_error_dialog: bool,
    pub error_message: Option<String>,
    pub start_time: Instant,
    /// Set to true when user requests audio processing to start.
    pub start_requested: bool,
    /// Audio device info for device-aware settings.
    pub devices: Vec<AudioDeviceInfo>,
    /// Active file picker modal (None = closed).
    pub file_picker: Option<FilePickerState>,
    /// Last directory used in the file picker (for persistence across opens).
    pub last_file_picker_dir: Option<std::path::PathBuf>,
    /// Show a detail popup for the selected packet/station.
    pub show_detail_dialog: bool,
    /// APRS search mode active.
    pub aprs_search_active: bool,
    /// APRS search text input.
    pub aprs_search_input: widgets::TextInputState,
    /// Shared counter of connected KISS TCP clients.
    pub kiss_client_count: Arc<AtomicU32>,
}

impl App {
    /// Create a new App from config.
    pub fn new(config: TncConfig, config_path: std::path::PathBuf, devices: Vec<AudioDeviceInfo>, kiss_client_count: Arc<AtomicU32>) -> Self {
        let settings = SettingsFormState::from_config(&config, &devices);
        Self {
            tab: Tab::Packets,
            processing: ProcessingState::Stopped,
            config,
            config_path,
            frames: SelectableList::new(),
            aprs_stations: SelectableList::new(),
            stats: Stats::default(),
            settings,
            should_quit: false,
            show_quit_dialog: false,
            quit_selected: 1, // default to "No"
            show_error_dialog: false,
            error_message: None,
            start_time: Instant::now(),
            start_requested: false,
            devices,
            file_picker: None,
            last_file_picker_dir: None,
            show_detail_dialog: false,
            aprs_search_active: false,
            aprs_search_input: widgets::TextInputState::new(),
            kiss_client_count,
        }
    }

    /// Create a minimal App for unit tests — no audio, no cpal, no tokio.
    #[cfg(test)]
    pub fn new_for_testing() -> Self {
        let config = TncConfig::default();
        let devices = vec![
            AudioDeviceInfo {
                name: "default".to_string(),
                description: "1ch, 8000-48000 Hz, I16".to_string(),
                supported_rates: vec![11025, 22050, 44100, 48000],
            },
            AudioDeviceInfo {
                name: "test_device".to_string(),
                description: "2ch, 44100-48000 Hz, F32".to_string(),
                supported_rates: vec![44100, 48000],
            },
        ];
        Self::new(config, std::path::PathBuf::from("./test-config.toml"), devices, Arc::new(AtomicU32::new(0)))
    }

    /// Returns true if the Audio Source is set to WAV File (field 0, selected index 1).
    pub fn is_wav_source(&self) -> bool {
        if let Some(field) = self.settings.fields.first() {
            matches!(&field.kind, FieldKind::Dropdown { selected, .. } if *selected == 1)
        } else {
            false
        }
    }

    /// Handle a key event, mutating state. Returns true if the event was consumed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Error dialog takes highest priority
        if self.show_error_dialog {
            return self.handle_error_dialog_key(key);
        }

        // Quit dialog takes priority
        if self.show_quit_dialog {
            return self.handle_quit_dialog_key(key);
        }

        // Detail popup takes priority
        if self.show_detail_dialog {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.show_detail_dialog = false;
                }
                _ => {}
            }
            return true; // consume all keys while detail dialog is shown
        }

        // File picker modal takes priority
        if self.file_picker.is_some() {
            return self.handle_file_picker_key(key);
        }

        // When editing a text field or searching, bypass global key bindings
        let editing = (self.tab == Tab::Settings && self.settings.editing)
            || (self.tab == Tab::Aprs && self.aprs_search_active);

        match key.code {
            KeyCode::Char('q') if !editing => {
                self.show_quit_dialog = true;
                self.quit_selected = 1; // default "No"
                true
            }
            KeyCode::Char('o') if !editing && !self.processing.is_running() => {
                if self.is_wav_source() {
                    self.file_picker = Some(FilePickerState::new(
                        self.last_file_picker_dir.as_deref(),
                    ));
                }
                true
            }
            KeyCode::Char('s') if !editing && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.is_wav_source() && !self.processing.is_running() {
                    // In WAV mode, 's' re-decodes the last file (or shows error)
                    if self.config.audio.wav_path.is_some() {
                        self.start_requested = true;
                    } else {
                        self.error_message = Some("No WAV file selected. Press 'o' to open a file.".to_string());
                        self.show_error_dialog = true;
                    }
                } else {
                    self.toggle_processing();
                }
                true
            }
            // Tab switching by number
            KeyCode::Char(c @ '1'..='3') if !editing => {
                if let Some(tab) = Tab::from_number(c as u8 - b'0') {
                    self.tab = tab;
                }
                true
            }
            KeyCode::Tab => {
                self.tab = self.tab.next();
                true
            }
            KeyCode::BackTab => {
                self.tab = self.tab.prev();
                true
            }
            // Per-tab key handling
            _ => self.handle_tab_key(key),
        }
    }

    fn handle_quit_dialog_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.quit_selected = 0;
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.quit_selected = 1;
                true
            }
            KeyCode::Enter => {
                if self.quit_selected == 0 {
                    self.should_quit = true;
                }
                self.show_quit_dialog = false;
                true
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                self.show_quit_dialog = false;
                true
            }
            KeyCode::Char('y') => {
                self.should_quit = true;
                self.show_quit_dialog = false;
                true
            }
            _ => false,
        }
    }

    fn handle_error_dialog_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                self.show_error_dialog = false;
                self.error_message = None;
                true
            }
            _ => true, // consume all keys while error dialog is shown
        }
    }

    fn handle_file_picker_key(&mut self, key: KeyEvent) -> bool {
        let picker = match self.file_picker.as_mut() {
            Some(p) => p,
            None => return false,
        };

        match key.code {
            KeyCode::Esc => {
                self.file_picker = None;
            }
            KeyCode::Enter => {
                if let Some(path) = picker.enter() {
                    // File selected — store dir for next time, set wav_path, trigger decode
                    self.last_file_picker_dir = path.parent().map(|p| p.to_path_buf());
                    self.config.audio.wav_path = Some(path);
                    self.file_picker = None;
                    self.start_requested = true;
                }
                // If enter() returned None, we navigated into a directory — picker stays open
            }
            KeyCode::Backspace => {
                picker.go_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.select_next();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                picker.select_prev();
            }
            KeyCode::Home => {
                picker.select_first();
            }
            KeyCode::End => {
                picker.select_last();
            }
            KeyCode::Char('a') => {
                picker.toggle_filter();
            }
            _ => {}
        }
        true // consume all keys while picker is open
    }

    fn handle_tab_key(&mut self, key: KeyEvent) -> bool {
        match self.tab {
            Tab::Packets => self.handle_packets_key(key),
            Tab::Aprs => self.handle_aprs_key(key),
            Tab::Settings => self.handle_settings_key(key),
        }
    }

    fn handle_packets_key(&mut self, key: KeyEvent) -> bool {
        // Navigation is inverted because the table renders items in reverse
        // (newest at top via iter().rev()): data index 0 = oldest = visual bottom.
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.frames.select_prev();
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.frames.select_next();
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                if !self.frames.is_empty() {
                    self.frames.select(self.frames.len() - 1);
                }
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.frames.select(0);
                true
            }
            KeyCode::Enter => {
                if !self.frames.is_empty() {
                    self.show_detail_dialog = true;
                }
                true
            }
            _ => false,
        }
    }

    fn handle_aprs_key(&mut self, key: KeyEvent) -> bool {
        if self.aprs_search_active {
            return self.handle_aprs_search_key(key);
        }
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.aprs_stations.select_next();
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.aprs_stations.select_prev();
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.aprs_stations.select(0);
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                if !self.aprs_stations.is_empty() {
                    self.aprs_stations.select(self.aprs_stations.len() - 1);
                }
                true
            }
            KeyCode::Enter => {
                if !self.aprs_stations.is_empty() {
                    self.show_detail_dialog = true;
                }
                true
            }
            KeyCode::Char('/') => {
                self.aprs_search_active = true;
                true
            }
            _ => false,
        }
    }

    fn handle_aprs_search_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.aprs_search_active = false;
                self.aprs_search_input.clear();
                true
            }
            KeyCode::Enter => {
                self.aprs_search_active = false;
                true
            }
            KeyCode::Backspace => {
                self.aprs_search_input.backspace();
                true
            }
            KeyCode::Delete => {
                self.aprs_search_input.delete();
                true
            }
            KeyCode::Left => {
                self.aprs_search_input.move_left();
                true
            }
            KeyCode::Right => {
                self.aprs_search_input.move_right();
                true
            }
            KeyCode::Home => {
                self.aprs_search_input.home();
                true
            }
            KeyCode::End => {
                self.aprs_search_input.end();
                true
            }
            KeyCode::Char(c) => {
                self.aprs_search_input.insert(c);
                true
            }
            _ => false,
        }
    }

    /// Returns indices of APRS stations matching the current search filter.
    fn filtered_aprs_indices(&self) -> Vec<usize> {
        let query = self.aprs_search_input.value();
        if query.is_empty() {
            return (0..self.aprs_stations.len()).collect();
        }
        let query_lower = query.to_ascii_lowercase();
        self.aprs_stations.items().iter().enumerate()
            .filter(|(_, s)| {
                s.callsign.to_ascii_lowercase().contains(&query_lower)
                    || s.object_name.as_ref().is_some_and(|n| n.to_ascii_lowercase().contains(&query_lower))
                    || s.comment.to_ascii_lowercase().contains(&query_lower)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn handle_settings_key(&mut self, key: KeyEvent) -> bool {
        if self.processing.is_running() {
            return false; // read-only when running
        }

        if self.settings.editing {
            return self.handle_settings_edit_key(key);
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings.select_next();
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings.select_prev();
                true
            }
            KeyCode::Enter => {
                // Enter edit mode for text fields, cycle for dropdowns
                if let Some(field) = self.settings.fields.get(self.settings.selected_field) {
                    match field.kind {
                        FieldKind::Dropdown { .. } => {
                            let field_idx = self.settings.selected_field;
                            self.settings.cycle_dropdown();
                            if field_idx == 1 {
                                if let Some(msg) = self.settings.on_device_changed(&self.devices) {
                                    self.error_message = Some(msg);
                                    self.show_error_dialog = true;
                                }
                            }
                            if field_idx == 3 {
                                if let Some(msg) = self.settings.on_baud_changed(&self.devices) {
                                    self.error_message = Some(msg);
                                    self.show_error_dialog = true;
                                }
                            }
                        }
                        FieldKind::Text { .. } => {
                            self.settings.editing = true;
                        }
                    }
                }
                true
            }
            KeyCode::Char(' ') => {
                let field_idx = self.settings.selected_field;
                self.settings.cycle_dropdown();
                if field_idx == 1 {
                    if let Some(msg) = self.settings.on_device_changed(&self.devices) {
                        self.error_message = Some(msg);
                        self.show_error_dialog = true;
                    }
                }
                if field_idx == 3 {
                    if let Some(msg) = self.settings.on_baud_changed(&self.devices) {
                        self.error_message = Some(msg);
                        self.show_error_dialog = true;
                    }
                }
                true
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+S: validate and save config
                if let Some(err) = self.validate_settings() {
                    self.error_message = Some(err);
                    self.show_error_dialog = true;
                } else {
                    let new_config = self.settings.to_config();
                    if let Err(e) = new_config.save(&self.config_path) {
                        self.error_message = Some(format!("failed to save: {e}"));
                        self.show_error_dialog = true;
                    } else {
                        tracing::info!("config saved to {}", self.config_path.display());
                        self.config = new_config;
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn handle_settings_edit_key(&mut self, key: KeyEvent) -> bool {
        let idx = self.settings.selected_field;
        if let Some(field) = self.settings.fields.get_mut(idx) {
            let filter = field.filter;
            let max_len = field.max_len;
            if let FieldKind::Text { value } = &mut field.kind {
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => {
                        self.settings.editing = false;
                    }
                    KeyCode::Char(c) => {
                        // Enforce max length
                        if let Some(max) = max_len {
                            if value.len() >= max {
                                return true;
                            }
                        }
                        let c = filter.map_or(Some(c), |f| f(c));
                        if let Some(c) = c {
                            value.insert(c);
                        }
                    }
                    KeyCode::Backspace => {
                        value.backspace();
                    }
                    KeyCode::Delete => {
                        value.delete();
                    }
                    KeyCode::Left => {
                        value.move_left();
                    }
                    KeyCode::Right => {
                        value.move_right();
                    }
                    KeyCode::Home => {
                        value.home();
                    }
                    KeyCode::End => {
                        value.end();
                    }
                    _ => return false,
                }
                return true;
            }
        }
        false
    }

    /// Validate settings before saving. Returns error message if invalid.
    fn validate_settings(&self) -> Option<String> {
        // Validate KISS TCP port (field 5)
        if let Some(val) = self.settings.field_value(5) {
            match val.parse::<u16>() {
                Ok(0) => return Some("port must be between 1 and 65535".to_string()),
                Err(_) => return Some(format!("invalid port: '{}'. Must be 0\u{2013}65535.", val)),
                _ => {}
            }
        }
        // Validate callsign is non-empty (field 6)
        if let Some(val) = self.settings.field_value(6) {
            if val.trim().is_empty() {
                return Some("callsign cannot be empty".to_string());
            }
        }
        None
    }

    /// Toggle between running and stopped states.
    fn toggle_processing(&mut self) {
        match &self.processing {
            ProcessingState::Stopped => {
                self.start_requested = true;
            }
            ProcessingState::Running { stop_signal, .. } => {
                stop_signal.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Process an async event from the audio thread.
    pub fn handle_async_event(&mut self, evt: AsyncEvent) {
        match evt {
            AsyncEvent::FrameDecoded(frame) => {
                // Update APRS station list if applicable
                if let Some(ref summary) = frame.aprs_summary {
                    self.update_aprs_station(&frame, summary);
                }
                self.stats.unique_frames += 1;
                self.stats.total_frames += 1;
                self.frames.items_mut().push(frame);
                // Keep selection at the newest frame
                let len = self.frames.len();
                self.frames.select(len.saturating_sub(1));
            }
            AsyncEvent::StatsUpdate(stats) => {
                self.stats = stats;
            }
            AsyncEvent::AudioDone => {
                // Join the audio thread
                let old = std::mem::replace(&mut self.processing, ProcessingState::Stopped);
                if let ProcessingState::Running { audio_handle, .. } = old {
                    let _ = audio_handle.join();
                }
            }
        }
    }

    fn update_aprs_station(&mut self, frame: &DecodedFrameInfo, _summary: &str) {
        // Find existing station or create new one
        let existing = self.aprs_stations.items_mut().iter_mut()
            .find(|s| s.callsign == frame.source);
        if let Some(station) = existing {
            station.packet_count += 1;
            station.last_heard = frame.timestamp.clone();
            station.last_frame_number = frame.frame_number;
            station.last_via = frame.via.clone();
            if let Some(ref data) = frame.aprs_data {
                apply_aprs_data(station, data);
            }
        } else {
            let mut station = AprsStation {
                callsign: frame.source.clone(),
                station_type: "Unknown".to_string(),
                last_heard: frame.timestamp.clone(),
                position: None,
                comment: String::new(),
                packet_count: 1,
                last_frame_number: frame.frame_number,
                last_via: frame.via.clone(),
                speed: None,
                course: None,
                weather: None,
                symbol: None,
                object_name: None,
            };
            if let Some(ref data) = frame.aprs_data {
                apply_aprs_data(&mut station, data);
            }
            self.aprs_stations.items_mut().push(station);
        }
        // Re-sort: most recently heard first, stable within same second
        self.aprs_stations.items_mut().sort_by(|a, b| {
            b.last_heard.cmp(&a.last_heard)
                .then_with(|| b.last_frame_number.cmp(&a.last_frame_number))
        });
    }
}

/// Populate an AprsStation's fields from structured APRS data.
fn apply_aprs_data(station: &mut AprsStation, data: &state::AprsData) {
    use state::AprsData;
    match data {
        AprsData::Position { lat, lon, symbol, comment, weather, .. } => {
            station.station_type = "Position".to_string();
            station.position = Some((*lat, *lon));
            station.symbol = Some(*symbol);
            if !comment.is_empty() {
                station.comment = comment.clone();
            }
            if let Some(w) = weather {
                station.weather = Some(w.clone());
            }
        }
        AprsData::MicE { lat, lon, speed, course, symbol } => {
            station.station_type = "Mic-E".to_string();
            station.position = Some((*lat, *lon));
            station.speed = Some(*speed);
            station.course = Some(*course);
            station.symbol = Some(*symbol);
        }
        AprsData::Message { .. } => {
            station.station_type = "Message".to_string();
        }
        AprsData::Weather { weather, comment } => {
            station.station_type = "Weather".to_string();
            station.weather = Some(weather.clone());
            if !comment.is_empty() {
                station.comment = comment.clone();
            }
        }
        AprsData::Object { name, live, lat, lon, symbol, comment, .. } => {
            station.station_type = if *live { "Object" } else { "Object (killed)" }.to_string();
            station.position = Some((*lat, *lon));
            station.symbol = Some(*symbol);
            station.object_name = Some(name.clone());
            if !comment.is_empty() {
                station.comment = comment.clone();
            }
        }
        AprsData::Item { name, live, lat, lon, symbol, comment } => {
            station.station_type = if *live { "Item" } else { "Item (killed)" }.to_string();
            station.position = Some((*lat, *lon));
            station.symbol = Some(*symbol);
            station.object_name = Some(name.clone());
            if !comment.is_empty() {
                station.comment = comment.clone();
            }
        }
        AprsData::Status { text, .. } => {
            station.station_type = "Status".to_string();
            if !text.is_empty() {
                station.comment = text.clone();
            }
        }
        AprsData::Telemetry { .. } => {
            station.station_type = "Telemetry".to_string();
        }
        AprsData::Query { query_type } => {
            station.station_type = "Query".to_string();
            station.comment = query_type.clone();
        }
        AprsData::Capabilities { data } => {
            station.station_type = "Capabilities".to_string();
            station.comment = data.clone();
        }
        AprsData::ThirdParty { data } => {
            station.station_type = "Third-party".to_string();
            station.comment = data.clone();
        }
        AprsData::RawGps { data, position, speed, course, fix_valid, .. } => {
            station.station_type = if *fix_valid { "GPS" } else { "GPS (no fix)" }.to_string();
            if let Some(pos) = position {
                station.position = Some(*pos);
            }
            if let Some(spd) = speed {
                station.speed = Some(*spd as u16);
            }
            if let Some(crs) = course {
                station.course = Some(*crs as u16);
            }
            if !data.is_empty() {
                station.comment = data.clone();
            }
        }
        AprsData::UserDefined { data } => {
            station.station_type = "User-defined".to_string();
            station.comment = data.clone();
        }
        AprsData::Unknown { .. } => {
            station.station_type = "Unknown".to_string();
        }
    }
}

/// Run the TUI event loop. This is the main entry point for TUI mode.
///
/// `start_audio` is called whenever the user requests processing to start.
/// It should open the audio device, spawn a processing thread, and return the
/// thread handle + stop signal. The spawned thread sends `AsyncEvent` messages
/// through its own sender; the matching receiver is `async_rx`.
pub async fn run_tui<F>(
    mut app: App,
    async_rx: crossbeam_channel::Receiver<AsyncEvent>,
    start_audio: F,
) -> io::Result<()>
where
    F: Fn(&crate::config::TncConfig) -> Result<(std::thread::JoinHandle<()>, Arc<AtomicBool>), String>,
{
    // Redirect stderr to /dev/null while TUI is active.
    // ALSA (and other libs) write diagnostics directly to fd 2, which
    // corrupts the alternate-screen buffer. We dup the original fd so
    // we can restore it on exit.
    use rustix::io::dup;
    use rustix::stdio::dup2_stderr;

    let saved_stderr: Option<rustix::fd::OwnedFd> = {
        let devnull = std::fs::File::open("/dev/null").ok();
        devnull.and_then(|f| {
            let saved = dup(rustix::stdio::stderr()).ok();
            dup2_stderr(&f).ok();
            saved
        })
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut events = EventHandler::new(Duration::from_millis(100), Some(async_rx));

    // Main event loop
    loop {
        // Process start request from 's' key
        if app.start_requested {
            app.start_requested = false;
            match start_audio(&app.config) {
                Ok((handle, stop)) => {
                    app.processing = ProcessingState::Running {
                        audio_handle: handle,
                        stop_signal: stop,
                    };
                    app.start_time = Instant::now();
                }
                Err(e) => {
                    tracing::error!("failed to start audio: {e}");
                    app.error_message = Some(e);
                    app.show_error_dialog = true;
                }
            }
        }

        // Update uptime and KISS client count
        if app.processing.is_running() {
            app.stats.uptime_secs = app.start_time.elapsed().as_secs();
        }
        app.stats.kiss_clients = app.kiss_client_count.load(Ordering::Relaxed);

        // Check if audio thread has finished
        let should_stop = if let ProcessingState::Running { audio_handle, .. } = &app.processing {
            audio_handle.is_finished()
        } else {
            false
        };
        if should_stop {
            let old = std::mem::replace(&mut app.processing, ProcessingState::Stopped);
            if let ProcessingState::Running { audio_handle, .. } = old {
                let _ = audio_handle.join();
            }
        }

        // Draw
        let is_wav = app.is_wav_source();
        let wav_filename: Option<String> = app.config.audio.wav_path.as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string());
        let filtered_indices = app.filtered_aprs_indices();
        terminal.draw(|frame| {
            let mut ctx = ui::DrawContext {
                tab: app.tab,
                processing: &app.processing,
                config: &app.config,
                frames: &mut app.frames,
                aprs_stations: &mut app.aprs_stations,
                stats: &app.stats,
                settings: &app.settings,
                show_quit_dialog: app.show_quit_dialog,
                quit_selected: app.quit_selected,
                show_error_dialog: app.show_error_dialog,
                error_message: &app.error_message,
                file_picker: app.file_picker.as_mut(),
                wav_file: wav_filename.as_deref(),
                is_wav_source: is_wav,
                show_detail_dialog: app.show_detail_dialog,
                aprs_search_text: app.aprs_search_input.value(),
                aprs_search_active: app.aprs_search_active,
                aprs_filtered_indices: &filtered_indices,
            };
            ui::draw(frame, &mut ctx);
        })?;

        if app.should_quit {
            break;
        }

        // Handle events
        if let Some(event) = events.next().await {
            match event {
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Async(async_evt) => {
                    app.handle_async_event(async_evt);
                }
                Event::Tick => {}
                Event::Resize(_, _) => {}
            }
        }
    }

    // Cleanup
    // Stop audio if running
    if let ProcessingState::Running { stop_signal, .. } = &app.processing {
        stop_signal.store(true, Ordering::Relaxed);
    }
    let old = std::mem::replace(&mut app.processing, ProcessingState::Stopped);
    if let ProcessingState::Running { audio_handle, .. } = old {
        let _ = audio_handle.join();
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Restore original stderr (OwnedFd closes automatically on drop)
    if let Some(saved) = saved_stderr {
        dup2_stderr(&saved).ok();
    }

    Ok(())
}

/// Enumerate available audio input devices with capability info.
pub fn enumerate_audio_devices() -> Vec<AudioDeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    use cpal::SampleFormat;
    let host = cpal::default_host();
    let mut seen_names = Vec::new();
    let mut devices = Vec::new();

    // Collect devices, default first
    let mut raw_devices = Vec::new();
    if let Some(dev) = host.default_input_device() {
        if let Ok(name) = dev.name() {
            seen_names.push(name);
            raw_devices.push(dev);
        }
    }
    if let Ok(input_devices) = host.input_devices() {
        for dev in input_devices {
            if let Ok(name) = dev.name() {
                if !seen_names.contains(&name) {
                    seen_names.push(name);
                    raw_devices.push(dev);
                }
            }
        }
    }

    for dev in &raw_devices {
        let name = dev.name().unwrap_or_else(|_| "unknown".into());
        let (description, supported_rates) = match dev.supported_input_configs() {
            Ok(configs) => {
                let configs: Vec<_> = configs.collect();
                if configs.is_empty() {
                    ("no config info".to_string(), USER_SAMPLE_RATES.to_vec())
                } else {
                    // Collect channel counts, rate range, sample formats
                    let mut min_rate = u32::MAX;
                    let mut max_rate = 0u32;
                    let mut channels = std::collections::BTreeSet::new();
                    let mut formats = std::collections::BTreeSet::new();

                    for cfg in &configs {
                        channels.insert(cfg.channels());
                        let lo = cfg.min_sample_rate().0;
                        let hi = cfg.max_sample_rate().0;
                        if lo < min_rate { min_rate = lo; }
                        if hi > max_rate { max_rate = hi; }
                        formats.insert(match cfg.sample_format() {
                            SampleFormat::I8 => "I8",
                            SampleFormat::I16 => "I16",
                            SampleFormat::I32 => "I32",
                            SampleFormat::I64 => "I64",
                            SampleFormat::U8 => "U8",
                            SampleFormat::U16 => "U16",
                            SampleFormat::U32 => "U32",
                            SampleFormat::U64 => "U64",
                            SampleFormat::F32 => "F32",
                            SampleFormat::F64 => "F64",
                            _ => "?",
                        });
                    }

                    let ch_str: Vec<String> = channels.iter().map(|c| format!("{}ch", c)).collect();
                    let fmt_str: Vec<&str> = formats.into_iter().collect();
                    let desc = format!(
                        "{}, {}-{} Hz, {}",
                        ch_str.join("/"),
                        min_rate,
                        max_rate,
                        fmt_str.join("/"),
                    );

                    // Intersect USER_SAMPLE_RATES with device-supported ranges
                    let mut rates: Vec<u32> = USER_SAMPLE_RATES
                        .iter()
                        .copied()
                        .filter(|&rate| {
                            configs.iter().any(|cfg| {
                                rate >= cfg.min_sample_rate().0
                                    && rate <= cfg.max_sample_rate().0
                            })
                        })
                        .collect();

                    // Fallback: if no intersection, show all rates
                    if rates.is_empty() {
                        rates = USER_SAMPLE_RATES.to_vec();
                    }

                    (desc, rates)
                }
            }
            Err(_) => {
                ("unable to query".to_string(), USER_SAMPLE_RATES.to_vec())
            }
        };

        devices.push(AudioDeviceInfo {
            name,
            description,
            supported_rates,
        });
    }

    if devices.is_empty() {
        devices.push(AudioDeviceInfo {
            name: "default".to_string(),
            description: "no devices found".to_string(),
            supported_rates: USER_SAMPLE_RATES.to_vec(),
        });
    }
    devices
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, KeyEventKind, KeyEventState};

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_key_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_tab_switching_by_number() {
        let mut app = App::new_for_testing();
        assert_eq!(app.tab, Tab::Packets);

        app.handle_key(make_key(KeyCode::Char('2')));
        assert_eq!(app.tab, Tab::Aprs);

        app.handle_key(make_key(KeyCode::Char('3')));
        assert_eq!(app.tab, Tab::Settings);

        app.handle_key(make_key(KeyCode::Char('1')));
        assert_eq!(app.tab, Tab::Packets);
    }

    #[test]
    fn test_tab_cycling() {
        let mut app = App::new_for_testing();
        assert_eq!(app.tab, Tab::Packets);

        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Aprs);

        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Settings);

        app.handle_key(make_key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Packets);

        app.handle_key(make_key(KeyCode::BackTab));
        assert_eq!(app.tab, Tab::Settings);
    }

    #[test]
    fn test_quit_dialog() {
        let mut app = App::new_for_testing();
        assert!(!app.show_quit_dialog);

        app.handle_key(make_key(KeyCode::Char('q')));
        assert!(app.show_quit_dialog);
        assert_eq!(app.quit_selected, 1); // default No

        // Press 'n' to cancel
        app.handle_key(make_key(KeyCode::Char('n')));
        assert!(!app.show_quit_dialog);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_quit_dialog_yes() {
        let mut app = App::new_for_testing();
        app.handle_key(make_key(KeyCode::Char('q')));
        assert!(app.show_quit_dialog);

        app.handle_key(make_key(KeyCode::Char('y')));
        assert!(app.should_quit);
    }

    #[test]
    fn test_quit_dialog_select_and_enter() {
        let mut app = App::new_for_testing();
        app.handle_key(make_key(KeyCode::Char('q')));
        assert!(app.show_quit_dialog);
        assert_eq!(app.quit_selected, 1); // No

        app.handle_key(make_key(KeyCode::Left));
        assert_eq!(app.quit_selected, 0); // Yes

        app.handle_key(make_key(KeyCode::Enter));
        assert!(app.should_quit);
    }

    #[test]
    fn test_packets_scroll() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Packets;

        // Add some frames (stored oldest-first, displayed newest-first via .rev())
        for i in 1..=5 {
            app.frames.items_mut().push(DecodedFrameInfo {
                frame_number: i,
                timestamp: "00:00:00".into(),
                source: format!("SRC{i}"),
                dest: "DEST".into(),
                via: String::new(),
                info: "test".into(),
                aprs_summary: None,
                aprs_data: None,
                raw_len: 10,
            });
        }
        // Start at visual top = newest = data index 4
        app.frames.select(4);

        // Down (visual) = select_prev = decrement data index
        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.frames.selected_index(), 3);

        app.handle_key(make_key(KeyCode::Char('j')));
        assert_eq!(app.frames.selected_index(), 2);

        // Up (visual) = select_next = increment data index
        app.handle_key(make_key(KeyCode::Up));
        assert_eq!(app.frames.selected_index(), 3);

        // Home/g = visual top = newest = data index 4
        app.handle_key(make_key(KeyCode::Home));
        assert_eq!(app.frames.selected_index(), 4);

        // End/G = visual bottom = oldest = data index 0
        app.handle_key(make_key(KeyCode::End));
        assert_eq!(app.frames.selected_index(), 0);
    }

    #[test]
    fn test_settings_not_editable_when_running() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        // Simulate running state with a mock
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        app.processing = ProcessingState::Running {
            audio_handle: handle,
            stop_signal: stop.clone(),
        };

        // Arrow keys should not navigate in settings when running
        let consumed = app.handle_settings_key(make_key(KeyCode::Down));
        assert!(!consumed);

        // Clean up
        stop.store(true, Ordering::Relaxed);
        let old = std::mem::replace(&mut app.processing, ProcessingState::Stopped);
        if let ProcessingState::Running { audio_handle, .. } = old {
            audio_handle.join().unwrap();
        }
    }

    #[test]
    fn test_settings_navigation() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        assert_eq!(app.settings.selected_field, 0);

        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.settings.selected_field, 1);

        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.settings.selected_field, 2);

        app.handle_key(make_key(KeyCode::Up));
        assert_eq!(app.settings.selected_field, 1);
    }

    #[test]
    fn test_settings_dropdown_cycle() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        // Field 0 is Audio Source dropdown
        assert_eq!(app.settings.field_value(0), Some("Live Audio".to_string()));

        app.handle_key(make_key(KeyCode::Char(' '))); // space cycles dropdown
        assert_eq!(app.settings.field_value(0), Some("WAV File".to_string()));
    }

    #[test]
    fn test_settings_text_edit() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        // Navigate to KISS port (field 5)
        app.settings.selected_field = 5;

        // Enter edit mode
        app.handle_key(make_key(KeyCode::Enter));
        assert!(app.settings.editing);

        // Type a character
        app.handle_key(make_key(KeyCode::Char('9')));

        // Exit edit mode
        app.handle_key(make_key(KeyCode::Esc));
        assert!(!app.settings.editing);
    }

    #[test]
    fn test_async_frame_event() {
        let mut app = App::new_for_testing();
        assert!(app.frames.is_empty());

        app.handle_async_event(AsyncEvent::FrameDecoded(DecodedFrameInfo {
            frame_number: 1,
            timestamp: "12:00:00".into(),
            source: "W1AW".into(),
            dest: "APRS".into(),
            via: "WIDE1-1".into(),
            info: "test packet".into(),
            aprs_summary: Some("Position: 49.0N 72.0W".into()),
            aprs_data: Some(state::AprsData::Position {
                lat: 49.058,
                lon: -72.029,
                symbol: (b'/', b'-'),
                comment: String::new(),
                weather: None,
                timestamp: None,
                altitude: None,
                compressed_extra: None,
            }),
            raw_len: 50,
        }));

        assert_eq!(app.frames.len(), 1);
        assert_eq!(app.stats.unique_frames, 1);
        // APRS station should be created
        assert_eq!(app.aprs_stations.len(), 1);
        assert_eq!(app.aprs_stations.items()[0].callsign, "W1AW");
    }

    #[test]
    fn test_s_key_sets_start_requested() {
        let mut app = App::new_for_testing();
        assert!(!app.start_requested);

        app.handle_key(make_key(KeyCode::Char('s')));
        assert!(app.start_requested);
    }

    #[test]
    fn test_ctrl_s_does_not_toggle_processing() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        assert!(!app.start_requested);

        // Ctrl+S should NOT toggle processing — it should fall through to save
        app.handle_key(make_key_mod(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(!app.start_requested);
    }

    #[test]
    fn test_s_key_not_intercepted_during_text_edit() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        app.settings.selected_field = 5; // KISS port (text field)

        // Enter edit mode
        app.handle_key(make_key(KeyCode::Enter));
        assert!(app.settings.editing);

        // 's' should be filtered out (port only accepts digits), NOT toggle processing
        app.handle_key(make_key(KeyCode::Char('s')));
        assert!(!app.start_requested);
    }

    #[test]
    fn test_port_rejects_non_digits() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        app.settings.selected_field = 5; // KISS port
        app.handle_key(make_key(KeyCode::Enter)); // enter edit mode

        let before = app.settings.field_value(5).unwrap();
        // Letters should be rejected
        app.handle_key(make_key(KeyCode::Char('a')));
        app.handle_key(make_key(KeyCode::Char('!')));
        app.handle_key(make_key(KeyCode::Char(' ')));
        assert_eq!(app.settings.field_value(5).unwrap(), before);

        // Digits should be accepted
        app.handle_key(make_key(KeyCode::Char('9')));
        assert!(app.settings.field_value(5).unwrap().ends_with('9'));
    }

    #[test]
    fn test_callsign_filter_uppercase_and_reject() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        app.settings.selected_field = 6; // Callsign
        app.handle_key(make_key(KeyCode::Enter)); // enter edit mode

        // Clear existing value
        for _ in 0..10 {
            app.handle_key(make_key(KeyCode::Backspace));
        }

        // Lowercase should be uppercased
        app.handle_key(make_key(KeyCode::Char('w')));
        app.handle_key(make_key(KeyCode::Char('1')));
        app.handle_key(make_key(KeyCode::Char('a')));
        app.handle_key(make_key(KeyCode::Char('w')));
        assert_eq!(app.settings.field_value(6).unwrap(), "W1AW");

        // Hyphen should be accepted (for SSID)
        app.handle_key(make_key(KeyCode::Char('-')));
        app.handle_key(make_key(KeyCode::Char('1')));
        assert_eq!(app.settings.field_value(6).unwrap(), "W1AW-1");

        // Special chars should be rejected
        app.handle_key(make_key(KeyCode::Char('!')));
        app.handle_key(make_key(KeyCode::Char('@')));
        app.handle_key(make_key(KeyCode::Char(' ')));
        assert_eq!(app.settings.field_value(6).unwrap(), "W1AW-1");
    }

    #[test]
    fn test_error_dialog_show_and_dismiss() {
        let mut app = App::new_for_testing();
        assert!(!app.show_error_dialog);

        // Simulate audio error
        app.error_message = Some("ALSA error: no such device".to_string());
        app.show_error_dialog = true;

        // Error dialog should consume all keys
        assert!(app.handle_key(make_key(KeyCode::Char('q'))));
        assert!(app.show_error_dialog); // still shown
        assert!(!app.should_quit); // q didn't trigger quit

        // Enter dismisses it
        app.handle_key(make_key(KeyCode::Enter));
        assert!(!app.show_error_dialog);
        assert!(app.error_message.is_none());
    }

    #[test]
    fn test_error_dialog_dismiss_with_esc() {
        let mut app = App::new_for_testing();
        app.error_message = Some("test error".to_string());
        app.show_error_dialog = true;

        app.handle_key(make_key(KeyCode::Esc));
        assert!(!app.show_error_dialog);
        assert!(app.error_message.is_none());
    }

    #[test]
    fn test_mode_description_available() {
        let app = App::new_for_testing();
        // Field 4 is Demod Mode dropdown — should have descriptions
        let desc = app.settings.field_description(4);
        assert!(desc.is_some());
        assert!(!desc.unwrap().is_empty());
    }

    #[test]
    fn test_port_max_length_enforced() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        app.settings.selected_field = 5; // KISS port
        app.handle_key(make_key(KeyCode::Enter)); // enter edit mode

        // Clear existing value
        for _ in 0..10 {
            app.handle_key(make_key(KeyCode::Backspace));
        }

        // Type 5 digits — should be accepted
        for c in ['6', '5', '5', '3', '5'] {
            app.handle_key(make_key(KeyCode::Char(c)));
        }
        assert_eq!(app.settings.field_value(5).unwrap(), "65535");

        // 6th digit should be rejected (max_len = 5)
        app.handle_key(make_key(KeyCode::Char('9')));
        assert_eq!(app.settings.field_value(5).unwrap(), "65535");
    }

    #[test]
    fn test_validate_settings_port_zero() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        app.settings.selected_field = 5;
        app.handle_key(make_key(KeyCode::Enter));

        // Clear and type "0"
        for _ in 0..10 {
            app.handle_key(make_key(KeyCode::Backspace));
        }
        app.handle_key(make_key(KeyCode::Char('0')));
        app.handle_key(make_key(KeyCode::Esc));

        let err = app.validate_settings();
        assert!(err.is_some());
        assert!(err.unwrap().contains("port"));
    }

    #[test]
    fn test_validate_settings_empty_callsign() {
        let mut app = App::new_for_testing();
        app.tab = Tab::Settings;
        app.settings.selected_field = 6; // Callsign
        app.handle_key(make_key(KeyCode::Enter));

        // Clear callsign
        for _ in 0..10 {
            app.handle_key(make_key(KeyCode::Backspace));
        }
        app.handle_key(make_key(KeyCode::Esc));

        let err = app.validate_settings();
        assert!(err.is_some());
        assert!(err.unwrap().contains("callsign"));
    }

    #[test]
    fn test_validate_settings_valid() {
        let app = App::new_for_testing();
        // Default config should be valid
        assert!(app.validate_settings().is_none());
    }

    #[test]
    fn test_is_wav_source_default_false() {
        let app = App::new_for_testing();
        assert!(!app.is_wav_source());
    }

    #[test]
    fn test_is_wav_source_after_toggle() {
        let mut app = App::new_for_testing();
        // Field 0 = Audio Source: cycle to "WAV File" (index 1)
        app.settings.selected_field = 0;
        app.settings.cycle_dropdown();
        assert!(app.is_wav_source());

        // Cycle back to "Live Audio"
        app.settings.cycle_dropdown();
        assert!(!app.is_wav_source());
    }

    #[test]
    fn test_o_key_opens_picker_when_wav_source() {
        let mut app = App::new_for_testing();
        // Set source to WAV File
        app.settings.selected_field = 0;
        app.settings.cycle_dropdown();
        assert!(app.is_wav_source());

        // Press 'o' — should open picker
        assert!(app.file_picker.is_none());
        app.handle_key(make_key(KeyCode::Char('o')));
        assert!(app.file_picker.is_some());
    }

    #[test]
    fn test_o_key_noop_when_live_source() {
        let mut app = App::new_for_testing();
        assert!(!app.is_wav_source());

        // Press 'o' — should NOT open picker (source is Live Audio)
        app.handle_key(make_key(KeyCode::Char('o')));
        assert!(app.file_picker.is_none());
    }

    #[test]
    fn test_o_key_noop_when_running() {
        let mut app = App::new_for_testing();
        // Set source to WAV File
        app.settings.selected_field = 0;
        app.settings.cycle_dropdown();

        // Simulate running state
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        app.processing = ProcessingState::Running {
            audio_handle: handle,
            stop_signal: stop.clone(),
        };

        // Press 'o' — should NOT open picker when running
        app.handle_key(make_key(KeyCode::Char('o')));
        assert!(app.file_picker.is_none());

        // Clean up
        stop.store(true, Ordering::Relaxed);
        let old = std::mem::replace(&mut app.processing, ProcessingState::Stopped);
        if let ProcessingState::Running { audio_handle, .. } = old {
            audio_handle.join().unwrap();
        }
    }

    #[test]
    fn test_file_picker_esc_closes() {
        let mut app = App::new_for_testing();
        // Set source to WAV File and open picker
        app.settings.selected_field = 0;
        app.settings.cycle_dropdown();
        app.handle_key(make_key(KeyCode::Char('o')));
        assert!(app.file_picker.is_some());

        // Esc should close it
        app.handle_key(make_key(KeyCode::Esc));
        assert!(app.file_picker.is_none());
    }

    #[test]
    fn test_s_key_wav_mode_no_file_shows_error() {
        let mut app = App::new_for_testing();
        // Set source to WAV File
        app.settings.selected_field = 0;
        app.settings.cycle_dropdown();
        assert!(app.is_wav_source());
        assert!(app.config.audio.wav_path.is_none());

        // Press 's' — should show error since no file selected
        app.handle_key(make_key(KeyCode::Char('s')));
        assert!(!app.start_requested);
        assert!(app.show_error_dialog);
        assert!(app.error_message.as_ref().unwrap().contains("No WAV file"));
    }

    #[test]
    fn test_s_key_wav_mode_with_file_starts() {
        let mut app = App::new_for_testing();
        // Set source to WAV File
        app.settings.selected_field = 0;
        app.settings.cycle_dropdown();
        // Set a wav path
        app.config.audio.wav_path = Some(std::path::PathBuf::from("/tmp/test.wav"));

        // Press 's' — should start
        app.handle_key(make_key(KeyCode::Char('s')));
        assert!(app.start_requested);
    }
}
