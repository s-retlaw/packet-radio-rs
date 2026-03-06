pub mod state;
pub mod ui;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::prelude::*;
use std::io;
use std::time::Duration;
use tracing::error;

use crate::db::FccDb;
use crate::geo::haversine_km;
use crate::models::{GeoQuery, LicenseRecord};
use state::{App, Tab};

/// Run the interactive TUI.
pub async fn run(db: FccDb) -> crate::error::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(db);
    let result = run_loop(&mut terminal, &mut app).await;

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> crate::error::Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Ctrl-C always quits
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    return Ok(());
                }
                // q quits unless editing a text field
                if key.code == KeyCode::Char('q') && !app.is_editing() {
                    return Ok(());
                }

                // Tab switching (F-keys always work, Tab/BackTab only when not editing)
                match key.code {
                    KeyCode::F(1) => {
                        app.tab = Tab::Search;
                        continue;
                    }
                    KeyCode::F(2) => {
                        app.tab = Tab::Results;
                        continue;
                    }
                    KeyCode::F(3) => {
                        app.tab = Tab::Detail;
                        continue;
                    }
                    KeyCode::Tab if !app.is_editing() => {
                        app.tab = app.tab.next();
                        continue;
                    }
                    KeyCode::BackTab if !app.is_editing() => {
                        app.tab = app.tab.prev();
                        continue;
                    }
                    _ => {}
                }

                match app.tab {
                    Tab::Search => handle_search_keys(app, key).await,
                    Tab::Results => handle_results_keys(app, key).await,
                    Tab::Detail => handle_detail_keys(app, key).await,
                }
            }
        }
    }
}

async fn handle_search_keys(app: &mut App, key: event::KeyEvent) {
    match key.code {
        KeyCode::Up => app.search.prev_field(),
        KeyCode::Down => app.search.next_field(),
        KeyCode::Tab => {
            if app.search.editing {
                app.search.editing = false;
                app.search.next_field();
            } else {
                app.search.next_field();
            }
        }
        KeyCode::Enter => {
            if app.search.active_field == state::SearchField::Submit {
                let query = app.search.to_query();
                match app.db.search(&query).await {
                    Ok(results) => {
                        let count = results.len();
                        app.results = results;
                        if !app.results.is_empty() {
                            app.result_table_state.select(Some(0));
                        }
                        app.tab = Tab::Results;
                        app.status = format!("Found {} results", count);
                    }
                    Err(e) => {
                        error!("Search error: {}", e);
                        app.status = format!("Error: {}", e);
                    }
                }
            } else {
                app.search.toggle_editing();
            }
        }
        KeyCode::Char(c) if app.search.editing => {
            app.search.insert_char(c);
        }
        KeyCode::Backspace if app.search.editing => {
            app.search.delete_char();
        }
        KeyCode::Esc if app.search.editing => {
            app.search.editing = false;
        }
        _ => {}
    }
}

async fn handle_results_keys(app: &mut App, key: event::KeyEvent) {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
        KeyCode::Home | KeyCode::Char('g') => app.select_first(),
        KeyCode::End | KeyCode::Char('G') => app.select_last(),
        KeyCode::PageDown => {
            for _ in 0..20 {
                app.select_next();
            }
        }
        KeyCode::PageUp => {
            for _ in 0..20 {
                app.select_prev();
            }
        }
        KeyCode::Enter => {
            if !app.results.is_empty() {
                load_detail(app).await;
                app.detail_scroll = 0;
                app.tab = Tab::Detail;
            }
        }
        _ => {}
    }
}

/// Load history, related records, and callsign chain for the currently selected license.
async fn load_detail(app: &mut App) {
    let license = match app.selected_license() {
        Some(l) => l.clone(),
        None => return,
    };
    load_detail_for(app, &license).await;
}

/// Load detail data for a specific license record.
/// Runs all independent queries concurrently.
async fn load_detail_for(app: &mut App, license: &LicenseRecord) {
    let usi = license.usi;
    let call_sign = license.call_sign.clone();
    let lat_lon = license.lat.zip(license.lon);

    let nearby_query = async {
        if let Some((lat, lon)) = lat_lon {
            app.db
                .stations_near(&GeoQuery {
                    lat,
                    lon,
                    radius_km: 25.0,
                    limit: Some(20),
                })
                .await
        } else {
            Ok(Vec::new())
        }
    };

    let (history_r, comments_r, related_r, chain_r, nearby_r) = tokio::join!(
        app.db.get_history(usi),
        app.db.get_comments(usi),
        app.db.related_by_licensee(usi),
        app.db.callsign_history_chain(&call_sign),
        nearby_query,
    );

    app.history = history_r.unwrap_or_else(|e| {
        error!("History error: {}", e);
        Vec::new()
    });
    app.comments = comments_r.unwrap_or_else(|e| {
        error!("Comments error: {}", e);
        Vec::new()
    });
    app.related = related_r.unwrap_or_else(|e| {
        error!("Related records error: {}", e);
        Vec::new()
    });
    app.callsign_chain = chain_r.unwrap_or_else(|e| {
        error!("Callsign chain error: {}", e);
        Vec::new()
    });

    match nearby_r {
        Ok(stations) => {
            if let Some((lat, lon)) = lat_lon {
                let total_before_filter = stations.len();
                app.nearby = stations
                    .into_iter()
                    .filter(|s| s.usi != usi)
                    .map(|s| {
                        let d = haversine_km(lat, lon, s.lat.unwrap_or(0.0), s.lon.unwrap_or(0.0));
                        (s, d)
                    })
                    .collect();
                app.nearby
                    .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                tracing::debug!(
                    "Nearby query for {} ({:.4}, {:.4}): {} from DB, {} after self-filter",
                    call_sign, lat, lon, total_before_filter, app.nearby.len()
                );
            } else {
                app.nearby = Vec::new();
            }
        }
        Err(e) => {
            error!("Nearby error: {}", e);
            app.nearby = Vec::new();
        }
    }

    app.nearby_browsing = false;
    app.nearby_cursor = 0;
    app.nearby_section_line = None;
}

/// Navigate the detail view to a different license (pushes current onto stack).
async fn navigate_to_license(app: &mut App, target: LicenseRecord) {
    // Push current license onto back-stack
    if let Some(current) = app.selected_license().cloned() {
        app.detail_stack.push(current);
    }
    // Replace the current results entry with the target so selected_license() returns it
    let idx = app.selected_index();
    if idx < app.results.len() {
        app.results[idx] = target.clone();
    }
    app.detail_scroll = 0;
    load_detail_for(app, &target).await;
    app.status = format!("Viewing {} (Backspace=back)", target.call_sign);
}

async fn handle_detail_keys(app: &mut App, key: event::KeyEvent) {
    // If in nearby browsing mode, intercept keys first
    if app.nearby_browsing {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if !app.nearby.is_empty() {
                    if app.nearby_cursor + 1 < app.nearby.len() {
                        app.nearby_cursor += 1;
                    }
                }
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.nearby_cursor > 0 {
                    app.nearby_cursor -= 1;
                }
                return;
            }
            KeyCode::Enter => {
                if app.nearby_cursor < app.nearby.len() {
                    let target = app.nearby[app.nearby_cursor].0.clone();
                    app.nearby_browsing = false;
                    navigate_to_license(app, target).await;
                }
                return;
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                app.nearby_browsing = false;
                app.status = String::new();
                return;
            }
            // All other keys: exit browsing, then fall through to normal handling
            _ => {
                app.nearby_browsing = false;
            }
        }
    }

    match key.code {
        KeyCode::Esc => app.tab = Tab::Results,
        KeyCode::Backspace => {
            // Pop detail stack if we navigated into a related record
            if let Some(prev) = app.detail_stack.pop() {
                let idx = app.selected_index();
                if idx < app.results.len() {
                    app.results[idx] = prev.clone();
                }
                app.detail_scroll = 0;
                load_detail_for(app, &prev).await;
                app.status = format!("Viewing {}", prev.call_sign);
            } else {
                app.tab = Tab::Results;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => app.detail_scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.detail_scroll_down(),
        KeyCode::PageDown | KeyCode::Char(' ') => app.detail_page_down(),
        KeyCode::PageUp => app.detail_page_up(),
        KeyCode::Home | KeyCode::Char('g') => app.detail_scroll = 0,
        KeyCode::End | KeyCode::Char('G') => {
            app.detail_scroll =
                app.detail_line_count.saturating_sub(app.detail_visible_height);
        }
        // Navigate between results without going back to Results tab
        KeyCode::Left | KeyCode::Char('[') => {
            app.detail_stack.clear(); // Reset stack when browsing results
            app.select_prev();
            app.detail_scroll = 0;
            load_detail(app).await;
        }
        KeyCode::Right | KeyCode::Char(']') => {
            app.detail_stack.clear();
            app.select_next();
            app.detail_scroll = 0;
            load_detail(app).await;
        }
        // Follow previous callsign
        KeyCode::Char('p') => {
            if let Some(license) = app.selected_license().cloned() {
                if !license.previous_call_sign.is_empty() {
                    let call = license.previous_call_sign.clone();
                    if let Ok(Some(target)) = app.db.lookup_callsign(&call).await {
                        navigate_to_license(app, target).await;
                    } else {
                        app.status = format!("Callsign {} not found in database", call);
                    }
                }
            }
        }
        // Number keys 1-9: related records only
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as usize) - ('1' as usize);
            if idx < app.related.len() {
                let target = app.related[idx].clone();
                navigate_to_license(app, target).await;
            }
        }
        // 'n' enters nearby browsing mode
        KeyCode::Char('n') => {
            if !app.nearby.is_empty() {
                app.nearby_browsing = true;
                app.nearby_cursor = 0;
                // Auto-scroll to nearby section if we know where it is
                if let Some(line) = app.nearby_section_line {
                    app.detail_scroll = line;
                    // Clamp to max scroll
                    let max = app.detail_line_count.saturating_sub(app.detail_visible_height);
                    if app.detail_scroll > max {
                        app.detail_scroll = max;
                    }
                }
                app.status = "Nearby: j/k=move  Enter=select  Esc=exit".to_string();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn make_license(usi: i64, call: &str, lat: Option<f64>, lon: Option<f64>) -> LicenseRecord {
        LicenseRecord {
            usi,
            call_sign: call.to_string(),
            license_status: "A".to_string(),
            operator_class: "E".to_string(),
            first_name: String::new(),
            last_name: String::new(),
            entity_name: String::new(),
            street_address: String::new(),
            city: "TESTVILLE".to_string(),
            state: "CT".to_string(),
            zip_code: String::new(),
            grant_date: String::new(),
            expired_date: String::new(),
            previous_call_sign: String::new(),
            lat,
            lon,
            geo_source: lat.map(|_| "test".to_string()),
            frn: String::new(),
            licensee_id: String::new(),
            mi: String::new(),
            suffix: String::new(),
            previous_operator_class: String::new(),
            cancellation_date: String::new(),
            last_action_date: String::new(),
            radio_service_code: "HA".to_string(),
            region_code: String::new(),
            entity_type: String::new(),
            geo_quality: None,
        }
    }

    async fn setup_db_with_nearby() -> (FccDb, LicenseRecord) {
        let db = FccDb::open_memory().await.unwrap();

        // Insert 3 stations: W1AW at center, K1ABC nearby (0.01 deg ~1km), W3XYZ far away
        sqlx::query(
            "INSERT INTO hd (usi, call_sign, license_status, grant_date, expired_date, cancellation_date, last_action_date, radio_service_code)
             VALUES (1, 'W1AW', 'A', '', '', '', '', 'HA'),
                    (2, 'K1ABC', 'A', '', '', '', '', 'HA'),
                    (3, 'W3XYZ', 'A', '', '', '', '', 'HA')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO en (usi, city, state) VALUES (1, 'NEWINGTON', 'CT'), (2, 'HARTFORD', 'CT'), (3, 'PORTLAND', 'ME')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO am (usi, operator_class) VALUES (1, 'E'), (2, 'G'), (3, 'T')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        // Geocodes: W1AW and K1ABC close together, W3XYZ far away
        sqlx::query(
            "INSERT INTO geocodes (usi, lat, lon, geo_source) VALUES
             (1, 41.7000, -72.7000, 'test'),
             (2, 41.7100, -72.7100, 'test'),
             (3, 43.6600, -70.2600, 'test')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let license = db.lookup_callsign("W1AW").await.unwrap().unwrap();
        (db, license)
    }

    #[tokio::test]
    async fn test_load_detail_populates_nearby() {
        let (db, license) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));

        load_detail_for(&mut app, &license).await;

        // Should find K1ABC nearby (within 25 km), but not W3XYZ (far) or self
        assert_eq!(app.nearby.len(), 1, "Expected 1 nearby station, got {}", app.nearby.len());
        assert_eq!(app.nearby[0].0.call_sign, "K1ABC");
        assert!(app.nearby[0].1 < 25.0, "Distance should be < 25 km");
        assert!(app.nearby[0].1 > 0.0, "Distance should be > 0 km");
        assert!(!app.nearby_browsing);
    }

    #[tokio::test]
    async fn test_load_detail_no_geocode_clears_nearby() {
        let (db, _) = setup_db_with_nearby().await;
        let mut app = App::new(db);

        // A license with no coordinates
        let no_geo = make_license(99, "N0GEO", None, None);
        app.results = vec![no_geo.clone()];
        app.result_table_state.select(Some(0));

        // Pre-populate nearby to ensure it gets cleared
        app.nearby = vec![(make_license(1, "STALE", Some(0.0), Some(0.0)), 1.0)];

        load_detail_for(&mut app, &no_geo).await;

        assert!(app.nearby.is_empty(), "Nearby should be empty for non-geocoded station");
    }

    #[tokio::test]
    async fn test_load_detail_excludes_self() {
        let (db, license) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));

        load_detail_for(&mut app, &license).await;

        // Self (W1AW, usi=1) should not appear in nearby
        for (station, _) in &app.nearby {
            assert_ne!(station.usi, license.usi, "Self should be excluded from nearby");
        }
    }

    #[tokio::test]
    async fn test_load_detail_nearby_sorted_by_distance() {
        let db = FccDb::open_memory().await.unwrap();

        // Insert stations at varying distances
        sqlx::query(
            "INSERT INTO hd (usi, call_sign, license_status, grant_date, expired_date, cancellation_date, last_action_date, radio_service_code)
             VALUES (1, 'W1AW', 'A', '', '', '', '', 'HA'),
                    (2, 'NEAR', 'A', '', '', '', '', 'HA'),
                    (3, 'MID', 'A', '', '', '', '', 'HA'),
                    (4, 'FAR', 'A', '', '', '', '', 'HA')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO en (usi, city, state) VALUES (1, 'A', 'CT'), (2, 'B', 'CT'), (3, 'C', 'CT'), (4, 'D', 'CT')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO am (usi, operator_class) VALUES (1, 'E'), (2, 'E'), (3, 'E'), (4, 'E')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO geocodes (usi, lat, lon, geo_source) VALUES
             (1, 41.7000, -72.7000, 'test'),
             (2, 41.7010, -72.7010, 'test'),
             (3, 41.7100, -72.7100, 'test'),
             (4, 41.7500, -72.7500, 'test')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let license = db.lookup_callsign("W1AW").await.unwrap().unwrap();
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));

        load_detail_for(&mut app, &license).await;

        assert_eq!(app.nearby.len(), 3);
        // Verify sorted ascending by distance
        for i in 1..app.nearby.len() {
            assert!(
                app.nearby[i - 1].1 <= app.nearby[i].1,
                "Nearby should be sorted by distance: {} <= {}",
                app.nearby[i - 1].1,
                app.nearby[i].1,
            );
        }
    }

    #[tokio::test]
    async fn test_n_key_enters_browsing_mode() {
        let (db, license) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));
        app.tab = Tab::Detail;

        load_detail_for(&mut app, &license).await;
        assert!(!app.nearby.is_empty(), "Precondition: nearby should be populated");

        // Press 'n'
        handle_detail_keys(&mut app, make_key(KeyCode::Char('n'))).await;
        assert!(app.nearby_browsing, "n key should enter nearby browsing mode");
        assert_eq!(app.nearby_cursor, 0, "Cursor should start at 0");
        assert!(app.status.contains("Nearby"), "Status should mention nearby nav");
    }

    #[tokio::test]
    async fn test_n_key_ignored_when_no_nearby() {
        let (db, _) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        let no_geo = make_license(99, "N0GEO", None, None);
        app.results = vec![no_geo.clone()];
        app.result_table_state.select(Some(0));
        app.tab = Tab::Detail;

        load_detail_for(&mut app, &no_geo).await;
        assert!(app.nearby.is_empty(), "Precondition: nearby should be empty");

        handle_detail_keys(&mut app, make_key(KeyCode::Char('n'))).await;
        assert!(!app.nearby_browsing, "n key should be ignored when no nearby stations");
    }

    #[tokio::test]
    async fn test_nearby_enter_navigates() {
        let (db, license) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));
        app.tab = Tab::Detail;

        load_detail_for(&mut app, &license).await;
        assert_eq!(app.nearby.len(), 1);

        // Press 'n' to enter browsing, then Enter to select
        handle_detail_keys(&mut app, make_key(KeyCode::Char('n'))).await;
        assert!(app.nearby_browsing);

        handle_detail_keys(&mut app, make_key(KeyCode::Enter)).await;
        assert!(!app.nearby_browsing, "Browsing should end after Enter");

        // Should have navigated to K1ABC
        let current = app.selected_license().unwrap();
        assert_eq!(current.call_sign, "K1ABC", "Should navigate to nearby station");
        assert_eq!(app.detail_stack.len(), 1, "Should push original onto stack");
    }

    #[tokio::test]
    async fn test_nearby_esc_exits_browsing() {
        let (db, license) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));
        app.tab = Tab::Detail;

        load_detail_for(&mut app, &license).await;

        // Press 'n' to enter browsing, then Esc to exit
        handle_detail_keys(&mut app, make_key(KeyCode::Char('n'))).await;
        assert!(app.nearby_browsing);

        handle_detail_keys(&mut app, make_key(KeyCode::Esc)).await;
        assert!(!app.nearby_browsing, "Esc should exit nearby browsing");
        // Should still be on Detail tab (not switched to Results)
        assert_eq!(app.tab, Tab::Detail);
    }

    #[tokio::test]
    async fn test_nearby_n_exits_browsing() {
        let (db, license) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));
        app.tab = Tab::Detail;

        load_detail_for(&mut app, &license).await;

        // Press 'n' to enter, then 'n' again to exit
        handle_detail_keys(&mut app, make_key(KeyCode::Char('n'))).await;
        assert!(app.nearby_browsing);

        handle_detail_keys(&mut app, make_key(KeyCode::Char('n'))).await;
        assert!(!app.nearby_browsing, "Second n should exit nearby browsing");
    }

    #[tokio::test]
    async fn test_nearby_cursor_movement() {
        let db = FccDb::open_memory().await.unwrap();

        // Insert center + 3 nearby stations
        sqlx::query(
            "INSERT INTO hd (usi, call_sign, license_status, grant_date, expired_date, cancellation_date, last_action_date, radio_service_code)
             VALUES (1, 'W1AW', 'A', '', '', '', '', 'HA'),
                    (2, 'K1ABC', 'A', '', '', '', '', 'HA'),
                    (3, 'K1DEF', 'A', '', '', '', '', 'HA'),
                    (4, 'K1GHI', 'A', '', '', '', '', 'HA')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query("INSERT INTO en (usi, city, state) VALUES (1, 'A', 'CT'), (2, 'B', 'CT'), (3, 'C', 'CT'), (4, 'D', 'CT')")
            .execute(db.pool()).await.unwrap();

        sqlx::query("INSERT INTO am (usi, operator_class) VALUES (1, 'E'), (2, 'G'), (3, 'T'), (4, 'E')")
            .execute(db.pool()).await.unwrap();

        sqlx::query(
            "INSERT INTO geocodes (usi, lat, lon, geo_source) VALUES
             (1, 41.7000, -72.7000, 'test'),
             (2, 41.7010, -72.7010, 'test'),
             (3, 41.7020, -72.7020, 'test'),
             (4, 41.7030, -72.7030, 'test')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let license = db.lookup_callsign("W1AW").await.unwrap().unwrap();
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));
        app.tab = Tab::Detail;

        load_detail_for(&mut app, &license).await;
        assert_eq!(app.nearby.len(), 3, "Should have 3 nearby stations");

        // Enter browsing mode
        handle_detail_keys(&mut app, make_key(KeyCode::Char('n'))).await;
        assert!(app.nearby_browsing);
        assert_eq!(app.nearby_cursor, 0);

        // Move down
        handle_detail_keys(&mut app, make_key(KeyCode::Char('j'))).await;
        assert_eq!(app.nearby_cursor, 1);

        // Move down again
        handle_detail_keys(&mut app, make_key(KeyCode::Down)).await;
        assert_eq!(app.nearby_cursor, 2);

        // Clamp at end
        handle_detail_keys(&mut app, make_key(KeyCode::Char('j'))).await;
        assert_eq!(app.nearby_cursor, 2, "Should clamp at end");

        // Move up
        handle_detail_keys(&mut app, make_key(KeyCode::Char('k'))).await;
        assert_eq!(app.nearby_cursor, 1);

        // Move up to 0
        handle_detail_keys(&mut app, make_key(KeyCode::Up)).await;
        assert_eq!(app.nearby_cursor, 0);

        // Clamp at 0
        handle_detail_keys(&mut app, make_key(KeyCode::Char('k'))).await;
        assert_eq!(app.nearby_cursor, 0, "Should clamp at 0");
    }

    /// Integration test against real DB — skipped if DB doesn't exist.
    #[tokio::test]
    async fn test_nearby_real_db() {
        use crate::db::default_db_path;
        let path = default_db_path();
        if !path.exists() {
            eprintln!("Skipping test_nearby_real_db — no DB at {:?}", path);
            return;
        }
        let db = FccDb::open(&path).await.unwrap();

        // Look up W1AW — a well-known geocoded station
        let license = match db.lookup_callsign("W1AW").await.unwrap() {
            Some(l) => l,
            None => {
                eprintln!("Skipping — W1AW not in DB");
                return;
            }
        };

        assert!(license.lat.is_some(), "W1AW should be geocoded (lat)");
        assert!(license.lon.is_some(), "W1AW should be geocoded (lon)");

        let lat = license.lat.unwrap();
        let lon = license.lon.unwrap();
        eprintln!("W1AW coords: {:.4}, {:.4}", lat, lon);

        let stations = db
            .stations_near(&GeoQuery {
                lat,
                lon,
                radius_km: 25.0,
                limit: Some(20),
            })
            .await
            .unwrap();

        eprintln!("stations_near returned {} results", stations.len());
        for s in &stations {
            let d = haversine_km(lat, lon, s.lat.unwrap_or(0.0), s.lon.unwrap_or(0.0));
            eprintln!("  {} ({:.4}, {:.4}) — {:.1} km", s.call_sign, s.lat.unwrap_or(0.0), s.lon.unwrap_or(0.0), d);
        }

        assert!(
            stations.len() > 5,
            "W1AW in Newington CT should have many nearby stations, got {}",
            stations.len()
        );

        // Now test through load_detail_for
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));
        load_detail_for(&mut app, &license).await;

        eprintln!("app.nearby has {} entries", app.nearby.len());
        for (s, d) in &app.nearby {
            eprintln!("  {} — {:.1} km", s.call_sign, d);
        }

        assert!(
            !app.nearby.is_empty(),
            "Nearby should be populated for W1AW, got 0"
        );
    }

    #[tokio::test]
    async fn test_digit_without_n_goes_to_related() {
        let (db, license) = setup_db_with_nearby().await;
        let mut app = App::new(db);
        app.results = vec![license.clone()];
        app.result_table_state.select(Some(0));
        app.tab = Tab::Detail;

        load_detail_for(&mut app, &license).await;

        // Add a fake related record
        let related = make_license(50, "REL1", Some(40.0), Some(-70.0));
        app.related = vec![related];

        // Press '1' without 'n' — should go to related, not nearby
        handle_detail_keys(&mut app, make_key(KeyCode::Char('1'))).await;
        let current = app.selected_license().unwrap();
        assert_eq!(current.call_sign, "REL1", "Without n prefix, digit should navigate to related");
    }
}
