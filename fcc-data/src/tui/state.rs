use ratatui::widgets::TableState;

use crate::db::FccDb;
use crate::models::{LicenseRecord, SearchQuery};

/// History code descriptions for common FCC action codes.
pub fn describe_history_code(code: &str) -> &str {
    match code {
        // Current FCC codes (found in real data)
        "LIISS" => "License issued",
        "LIREN" => "License renewed",
        "LIAUA" => "Amateur upgrade",
        "LIMOD" => "License modified",
        "LICAN" => "License cancelled",
        "LIEXP" => "License expired",
        "LITIN" => "License terminated",
        "VANGRT" => "Vanity callsign granted",
        "SYSGRT" => "System granted",
        "COR" => "Correction",
        "AUTHPR" => "Authorization printed",
        "LTSFRN" => "FRN transfer",
        "LTSFND" => "FRN transfer done",
        "ESCFRN" => "FRN escrow",
        "ESUFRN" => "FRN escrow update",
        // Legacy LIS* codes (older records)
        "LISNEW" => "New license issued",
        "LISREN" => "License renewed",
        "LISMOD" => "License modified",
        "LISCAN" => "License cancelled",
        "LISEXP" => "License expired",
        "LISVNW" => "Vanity callsign — new",
        "LISVGR" => "Vanity callsign — granted",
        "LISUPG" => "Upgrade",
        "LISAUT" => "Automatic renewal",
        "LISBLK" => "License blocked",
        "ENRMOD" => "Entity record modified",
        "AMTADD" => "Amateur record added",
        "AMTMOD" => "Amateur record modified",
        "AMTREN" => "Amateur record renewed",
        "SYSADM" => "System administration",
        _ => "",
    }
}

/// Active tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Results,
    Detail,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Tab::Search => Tab::Results,
            Tab::Results => Tab::Detail,
            Tab::Detail => Tab::Search,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Tab::Search => Tab::Detail,
            Tab::Results => Tab::Search,
            Tab::Detail => Tab::Results,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Tab::Search => 0,
            Tab::Results => 1,
            Tab::Detail => 2,
        }
    }
}

/// Search form fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    CallSign,
    Name,
    City,
    State,
    ZipCode,
    OperatorClass,
    Status,
    Submit,
}

impl SearchField {
    pub const ALL: &[SearchField] = &[
        SearchField::CallSign,
        SearchField::Name,
        SearchField::City,
        SearchField::State,
        SearchField::ZipCode,
        SearchField::OperatorClass,
        SearchField::Status,
        SearchField::Submit,
    ];

    pub fn label(&self) -> &str {
        match self {
            Self::CallSign => "Callsign",
            Self::Name => "Name",
            Self::City => "City",
            Self::State => "State",
            Self::ZipCode => "ZIP Code",
            Self::OperatorClass => "Class (T/G/E/A/N)",
            Self::Status => "Status (A/E/C/T)",
            Self::Submit => "[ Search ]",
        }
    }
}

/// Search form state.
pub struct SearchFormState {
    pub call_sign: String,
    pub name: String,
    pub city: String,
    pub state: String,
    pub zip_code: String,
    pub operator_class: String,
    pub status: String,
    pub active_field: SearchField,
    pub editing: bool,
}

impl Default for SearchFormState {
    fn default() -> Self {
        Self {
            call_sign: String::new(),
            name: String::new(),
            city: String::new(),
            state: String::new(),
            zip_code: String::new(),
            operator_class: String::new(),
            status: "A".to_string(),
            active_field: SearchField::CallSign,
            editing: false,
        }
    }
}

impl SearchFormState {

    pub fn next_field(&mut self) {
        let fields = SearchField::ALL;
        let pos = fields.iter().position(|f| *f == self.active_field).unwrap_or(0);
        self.active_field = fields[(pos + 1) % fields.len()];
        self.editing = false;
    }

    pub fn prev_field(&mut self) {
        let fields = SearchField::ALL;
        let pos = fields.iter().position(|f| *f == self.active_field).unwrap_or(0);
        self.active_field = fields[(pos + fields.len() - 1) % fields.len()];
        self.editing = false;
    }

    pub fn toggle_editing(&mut self) {
        if self.active_field != SearchField::Submit {
            self.editing = !self.editing;
        }
    }

    pub fn active_value_mut(&mut self) -> Option<&mut String> {
        match self.active_field {
            SearchField::CallSign => Some(&mut self.call_sign),
            SearchField::Name => Some(&mut self.name),
            SearchField::City => Some(&mut self.city),
            SearchField::State => Some(&mut self.state),
            SearchField::ZipCode => Some(&mut self.zip_code),
            SearchField::OperatorClass => Some(&mut self.operator_class),
            SearchField::Status => Some(&mut self.status),
            SearchField::Submit => None,
        }
    }

    /// Get the current value of a field (read-only).
    pub fn value_of(&self, field: SearchField) -> Option<&str> {
        match field {
            SearchField::CallSign => Some(&self.call_sign),
            SearchField::Name => Some(&self.name),
            SearchField::City => Some(&self.city),
            SearchField::State => Some(&self.state),
            SearchField::ZipCode => Some(&self.zip_code),
            SearchField::OperatorClass => Some(&self.operator_class),
            SearchField::Status => Some(&self.status),
            SearchField::Submit => None,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        if let Some(val) = self.active_value_mut() {
            val.push(c);
        }
    }

    pub fn delete_char(&mut self) {
        if let Some(val) = self.active_value_mut() {
            val.pop();
        }
    }

    pub fn to_query(&self) -> SearchQuery {
        SearchQuery {
            call_sign: non_empty(&self.call_sign),
            name: non_empty(&self.name),
            city: non_empty(&self.city),
            state: non_empty(&self.state),
            zip_code: non_empty(&self.zip_code),
            operator_class: non_empty(&self.operator_class),
            license_status: non_empty(&self.status),
            limit: Some(500),
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Main application state.
pub struct App {
    pub db: FccDb,
    pub tab: Tab,
    pub search: SearchFormState,
    pub results: Vec<LicenseRecord>,
    pub result_table_state: TableState,
    pub history: Vec<(String, String)>,
    pub comments: Vec<(String, String, String)>,
    pub related: Vec<LicenseRecord>,
    pub nearby: Vec<(LicenseRecord, f64)>,
    pub nearby_browsing: bool,
    pub nearby_cursor: usize,
    pub nearby_section_line: Option<u16>,
    pub callsign_chain: Vec<(String, String, String, String)>,
    pub detail_scroll: u16,
    pub detail_line_count: u16,
    pub detail_visible_height: u16,
    pub status: String,
    /// Stack of previous detail views for back-navigation.
    pub detail_stack: Vec<LicenseRecord>,
}

impl App {
    pub fn new(db: FccDb) -> Self {
        Self {
            db,
            tab: Tab::Search,
            search: SearchFormState::default(),
            results: Vec::new(),
            result_table_state: TableState::default(),
            history: Vec::new(),
            comments: Vec::new(),
            related: Vec::new(),
            nearby: Vec::new(),
            nearby_browsing: false,
            nearby_cursor: 0,
            nearby_section_line: None,
            callsign_chain: Vec::new(),
            detail_scroll: 0,
            detail_line_count: 0,
            detail_visible_height: 0,
            status: "Ready -- Enter search criteria and press Enter on [ Search ]".to_string(),
            detail_stack: Vec::new(),
        }
    }

    pub fn is_editing(&self) -> bool {
        self.tab == Tab::Search && self.search.editing
    }

    pub fn selected_index(&self) -> usize {
        self.result_table_state.selected().unwrap_or(0)
    }

    pub fn selected_license(&self) -> Option<&LicenseRecord> {
        self.results.get(self.selected_index())
    }

    pub fn select_next(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let i = self.selected_index();
        if i + 1 < self.results.len() {
            self.result_table_state.select(Some(i + 1));
        }
    }

    pub fn select_prev(&mut self) {
        let i = self.selected_index();
        if i > 0 {
            self.result_table_state.select(Some(i - 1));
        }
    }

    pub fn select_first(&mut self) {
        if !self.results.is_empty() {
            self.result_table_state.select(Some(0));
        }
    }

    pub fn select_last(&mut self) {
        if !self.results.is_empty() {
            self.result_table_state.select(Some(self.results.len() - 1));
        }
    }

    pub fn detail_scroll_down(&mut self) {
        let max = self.detail_line_count.saturating_sub(self.detail_visible_height);
        if self.detail_scroll < max {
            self.detail_scroll += 1;
        }
    }

    pub fn detail_scroll_up(&mut self) {
        if self.detail_scroll > 0 {
            self.detail_scroll -= 1;
        }
    }

    pub fn detail_page_down(&mut self) {
        let page = self.detail_visible_height.max(1);
        let max = self.detail_line_count.saturating_sub(self.detail_visible_height);
        self.detail_scroll = (self.detail_scroll + page).min(max);
    }

    pub fn detail_page_up(&mut self) {
        let page = self.detail_visible_height.max(1);
        self.detail_scroll = self.detail_scroll.saturating_sub(page);
    }
}
