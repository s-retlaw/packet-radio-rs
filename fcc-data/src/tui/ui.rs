use ratatui::prelude::*;
use ratatui::widgets::*;

use super::state::{describe_history_code, App, SearchField, Tab};
use crate::models::{LicenseStatus, OperatorClass};

const LABEL_WIDTH: usize = 16;

pub fn render(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Tab bar
            Constraint::Min(0),   // Content
            Constraint::Length(1), // Status bar
        ])
        .split(f.area());

    render_tab_bar(f, app, chunks[0]);

    match app.tab {
        Tab::Search => render_search(f, app, chunks[1]),
        Tab::Results => render_results(f, app, chunks[1]),
        Tab::Detail => render_detail(f, app, chunks[1]),
    }

    render_status_bar(f, app, chunks[2]);
}

fn render_tab_bar(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = ["F1 Search", "F2 Results", "F3 Detail"]
        .iter()
        .map(|t| Line::from(*t))
        .collect();

    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(Style::default().fg(Color::Yellow).bold())
        .divider(" | ");

    f.render_widget(tabs, area);
}

// ── Search Tab ──────────────────────────────────────────────────────

fn render_search(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Search FCC Database ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let fields = SearchField::ALL;
    let constraints: Vec<Constraint> = fields.iter().map(|_| Constraint::Length(3)).collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(constraints)
        .split(inner);

    for (i, field) in fields.iter().enumerate() {
        if i >= chunks.len() {
            break;
        }
        let is_active = app.search.active_field == *field;

        if *field == SearchField::Submit {
            let btn = Paragraph::new(field.label())
                .style(if is_active {
                    Style::default().fg(Color::Black).bg(Color::Yellow).bold()
                } else {
                    Style::default().fg(Color::Cyan)
                })
                .alignment(Alignment::Center);
            f.render_widget(btn, chunks[i]);
        } else {
            let value = match field {
                SearchField::CallSign => &app.search.call_sign,
                SearchField::Name => &app.search.name,
                SearchField::City => &app.search.city,
                SearchField::State => &app.search.state,
                SearchField::ZipCode => &app.search.zip_code,
                SearchField::OperatorClass => &app.search.operator_class,
                SearchField::Status => &app.search.status,
                SearchField::Submit => unreachable!(),
            };

            let is_editing = is_active && app.search.editing;
            let display = if is_editing {
                format!("{}_", value)
            } else {
                value.clone()
            };

            let border_style = if is_editing {
                Style::default().fg(Color::Green)
            } else if is_active {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let pointer = if is_active { ">> " } else { "   " };
            let label = format!("{}{}", pointer, field.label());

            let input = Paragraph::new(display)
                .style(if is_active {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                })
                .block(
                    Block::default()
                        .borders(Borders::BOTTOM)
                        .border_style(border_style)
                        .title(label),
                );
            f.render_widget(input, chunks[i]);
        }
    }
}

// ── Results Tab ─────────────────────────────────────────────────────

fn render_results(f: &mut Frame, app: &mut App, area: Rect) {
    if app.results.is_empty() {
        let msg = Paragraph::new("No results. Use the Search tab (F1) to find licenses.")
            .block(
                Block::default()
                    .title(" Results ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, area);
        return;
    }

    let selected_idx = app.selected_index();

    let header = Row::new(vec![
        Cell::from("Call"),
        Cell::from("Name"),
        Cell::from("Class"),
        Cell::from("City"),
        Cell::from("St"),
        Cell::from("Status"),
    ])
    .style(Style::default().fg(Color::Yellow).bold())
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .results
        .iter()
        .map(|r| {
            let class_display = OperatorClass::from_code(&r.operator_class).to_string();
            let name = r.display_name();
            let status_style = status_color(&r.license_status);
            Row::new(vec![
                Cell::from(r.call_sign.as_str()),
                Cell::from(name),
                Cell::from(class_display),
                Cell::from(r.city.as_str()),
                Cell::from(r.state.as_str()),
                Cell::from(r.license_status.as_str()).style(status_style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Min(20),
            Constraint::Length(12),
            Constraint::Length(15),
            Constraint::Length(3),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
    .block(
        Block::default()
            .title(format!(
                " Results ({}/{}) -- j/k to navigate, Enter for detail ",
                selected_idx + 1,
                app.results.len()
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_stateful_widget(table, area, &mut app.result_table_state);

    // Scrollbar
    let total = app.results.len();
    let visible = area.height.saturating_sub(4) as usize; // borders + header + header margin
    if total > visible {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        let mut scrollbar_state =
            ScrollbarState::new(total.saturating_sub(visible)).position(selected_idx);
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

// ── Detail Tab ──────────────────────────────────────────────────────

fn render_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let license = match app.selected_license() {
        Some(l) => l.clone(),
        None => {
            let msg = Paragraph::new("No license selected. Select a result and press Enter.")
                .block(
                    Block::default()
                        .title(" Detail ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::DarkGray)),
                )
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(msg, area);
            return;
        }
    };

    let label = Style::default().fg(Color::Yellow);
    let section = Style::default().fg(Color::Cyan).bold();
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // ── Identity Section ──
    lines.push(Line::from(Span::styled("Identity", section)));
    lines.push(detail_line(label, "Callsign", &license.call_sign));

    let status_text = match LicenseStatus::from_code(&license.license_status) {
        LicenseStatus::Active => "Active (A)",
        LicenseStatus::Cancelled => "Cancelled (C)",
        LicenseStatus::Expired => "Expired (E)",
        LicenseStatus::Terminated => "Terminated (T)",
        LicenseStatus::Other(_) => &license.license_status,
    };
    lines.push(detail_line(label, "Status", status_text));

    let class_display = OperatorClass::from_code(&license.operator_class);
    lines.push(detail_line(
        label,
        "Class",
        &format!("{} ({})", class_display, license.operator_class),
    ));
    lines.push(detail_line(label, "USI", &license.usi.to_string()));

    if !license.radio_service_code.is_empty() {
        let desc = describe_radio_service(&license.radio_service_code);
        if !desc.is_empty() {
            lines.push(detail_line(
                label,
                "Service",
                &format!("{} ({})", license.radio_service_code, desc),
            ));
        } else {
            lines.push(detail_line(label, "Service", &license.radio_service_code));
        }
    }
    if !license.region_code.is_empty() {
        lines.push(detail_line(label, "Region", &license.region_code));
    }

    if !license.previous_call_sign.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<w$}", "Previous Call", w = LABEL_WIDTH),
                label,
            ),
            Span::styled(
                license.previous_call_sign.clone(),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled("  (p=follow)", Style::default().fg(Color::DarkGray)),
        ]));
    }
    if !license.previous_operator_class.is_empty() {
        let prev_class = OperatorClass::from_code(&license.previous_operator_class);
        lines.push(detail_line(
            label,
            "Previous Class",
            &format!("{} ({})", prev_class, license.previous_operator_class),
        ));
    }

    // ── Name Section ──
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Licensee", section)));

    if !license.entity_name.is_empty() {
        lines.push(detail_line(label, "Entity Name", &license.entity_name));
    }
    let mut name_parts = Vec::new();
    if !license.first_name.is_empty() {
        name_parts.push(license.first_name.clone());
    }
    if !license.mi.is_empty() {
        name_parts.push(format!("{}.", license.mi));
    }
    if !license.last_name.is_empty() {
        name_parts.push(license.last_name.clone());
    }
    if !license.suffix.is_empty() {
        name_parts.push(license.suffix.clone());
    }
    let personal_name = name_parts.join(" ");
    if !personal_name.trim().is_empty() {
        lines.push(detail_line(label, "Name", personal_name.trim()));
    }
    if !license.frn.is_empty() {
        lines.push(detail_line(label, "FRN", &license.frn));
    }
    if !license.licensee_id.is_empty() {
        lines.push(detail_line(label, "Licensee ID", &license.licensee_id));
    }
    if !license.entity_type.is_empty() {
        let et_desc = match license.entity_type.as_str() {
            "L" => "Licensee",
            "CL" => "Controlling Licensee",
            "E" => "Entity",
            "O" => "Owner",
            "T" => "Transferee",
            _ => "",
        };
        if !et_desc.is_empty() {
            lines.push(detail_line(
                label,
                "Entity Type",
                &format!("{} ({})", license.entity_type, et_desc),
            ));
        } else {
            lines.push(detail_line(label, "Entity Type", &license.entity_type));
        }
    }

    // ── Address Section ──
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Address", section)));

    if !license.street_address.is_empty() {
        lines.push(detail_line(label, "Street", &license.street_address));
    }
    let city_line = format!(
        "{}{}{} {}",
        license.city,
        if license.city.is_empty() { "" } else { ", " },
        license.state,
        license.zip_code,
    );
    lines.push(detail_line(label, "City/State/ZIP", city_line.trim()));

    // ── Location Section ──
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Location", section)));

    match (license.lat, license.lon) {
        (Some(lat), Some(lon)) => {
            lines.push(detail_line(label, "Latitude", &format!("{:.6}", lat)));
            lines.push(detail_line(label, "Longitude", &format!("{:.6}", lon)));
            let source = license.geo_source.as_deref().unwrap_or("unknown");
            let quality = license.geo_quality.as_deref().unwrap_or("");
            if !quality.is_empty() {
                lines.push(detail_line(
                    label,
                    "Source",
                    &format!("{}: {}", source, quality),
                ));
            } else {
                lines.push(detail_line(label, "Source", source));
            }
        }
        _ => {
            lines.push(detail_line(dim, "Coordinates", "Not geocoded"));
        }
    }

    // ── Dates Section ──
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Dates", section)));
    if !license.grant_date.is_empty() {
        lines.push(detail_line(label, "Granted", &license.grant_date));
    }
    if !license.expired_date.is_empty() {
        lines.push(detail_line(label, "Expires", &license.expired_date));
    }
    if !license.cancellation_date.is_empty() {
        lines.push(detail_line(label, "Cancelled", &license.cancellation_date));
    }
    if !license.last_action_date.is_empty() {
        lines.push(detail_line(label, "Last Action", &license.last_action_date));
    }

    // ── Comments Section ──
    if !app.comments.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Comments ({} entries)", app.comments.len()),
            section,
        )));
        for (date, comment, status_code) in &app.comments {
            let mut spans = vec![
                Span::styled(
                    format!("{:<LABEL_WIDTH$}", date),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(comment.to_string()),
            ];
            if !status_code.is_empty() {
                spans.push(Span::styled(
                    format!("  [{}]", status_code),
                    Style::default().fg(Color::DarkGray).italic(),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    // ── History Section ──
    if !app.history.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("History ({} entries)", app.history.len()),
            section,
        )));
        for (date, code) in &app.history {
            let desc = describe_history_code(code);
            let mut spans = vec![
                Span::styled(
                    format!("{:<LABEL_WIDTH$}", date),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(code.to_string()),
            ];
            if !desc.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", desc),
                    Style::default().fg(Color::DarkGray).italic(),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    // ── Related Records Section ──
    if !app.related.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "Related Licenses ({} records — press 1-{} to navigate)",
                app.related.len(),
                app.related.len().min(9)
            ),
            section,
        )));
        for (i, rel) in app.related.iter().enumerate() {
            let class_display = OperatorClass::from_code(&rel.operator_class).to_string();
            let status_style = status_color(&rel.license_status);
            let key_hint = if i < 9 {
                format!("[{}] ", i + 1)
            } else {
                "    ".to_string()
            };
            lines.push(Line::from(vec![
                Span::styled(key_hint, Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("{:<10}", rel.call_sign),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{:<12}", class_display),
                    Style::default(),
                ),
                Span::styled(rel.license_status.clone(), status_style),
                Span::styled(
                    format!("  {} — {}", rel.grant_date, rel.expired_date),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    // ── Nearby Stations Section ──
    if license.lat.is_some() && license.lon.is_some() {
        lines.push(Line::from(""));
        // Record the line index where the nearby section header starts
        app.nearby_section_line = Some(lines.len() as u16);
        if app.nearby.is_empty() {
            lines.push(Line::from(Span::styled(
                "Nearby Stations (none within 25 km)",
                dim,
            )));
        } else {
            let nav_hint = if app.nearby_browsing {
                " — j/k=move Enter=select Esc=exit"
            } else {
                " — press n to browse"
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "Nearby Stations ({} within 25 km{})",
                    app.nearby.len(),
                    nav_hint,
                ),
                section,
            )));
            for (i, (station, dist)) in app.nearby.iter().enumerate() {
                let class_display =
                    OperatorClass::from_code(&station.operator_class).to_string();
                let status_style = status_color(&station.license_status);
                let is_cursor = app.nearby_browsing && i == app.nearby_cursor;
                let marker = if is_cursor { "> " } else { "  " };
                let row_style = if is_cursor {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(marker, if is_cursor { row_style } else { Style::default().fg(Color::Cyan) }),
                    Span::styled(
                        format!("{:<10}", station.call_sign),
                        if is_cursor { row_style } else { Style::default().fg(Color::Yellow) },
                    ),
                    Span::styled(format!("{:<12}", class_display), row_style),
                    Span::styled(station.license_status.clone(), if is_cursor { row_style } else { status_style }),
                    Span::styled(
                        format!("  {:>6.1} km", dist),
                        if is_cursor { row_style } else { Style::default().fg(Color::Green) },
                    ),
                    Span::styled(
                        format!("   {}", station.city),
                        if is_cursor { row_style } else { Style::default().fg(Color::DarkGray) },
                    ),
                ]));
            }
        }
    } else {
        app.nearby_section_line = None;
    }

    // ── Callsign Chain Section ──
    if !app.callsign_chain.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Callsign History Chain ({})", license.call_sign),
            section,
        )));
        for (call, name, status, grant) in &app.callsign_chain {
            let status_style = status_color(status);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<10}", call),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(format!("{:<25}", name), Style::default()),
                Span::styled(status.clone(), status_style),
                Span::styled(
                    format!("  {}", grant),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    // Track line count for scroll bounds
    let total_lines = lines.len() as u16;
    let stack_indicator = if !app.detail_stack.is_empty() {
        format!(" ({}← deep)", app.detail_stack.len())
    } else {
        String::new()
    };
    let block = Block::default()
        .title(format!(
            " {} -- Detail{} ",
            license.call_sign, stack_indicator
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner_height = block.inner(area).height;
    app.detail_line_count = total_lines;
    app.detail_visible_height = inner_height;

    // Clamp scroll
    let max_scroll = total_lines.saturating_sub(inner_height);
    if app.detail_scroll > max_scroll {
        app.detail_scroll = max_scroll;
    }

    // Scroll indicator
    let scroll_info = if total_lines > inner_height {
        format!(
            " lines {}-{}/{} ",
            app.detail_scroll + 1,
            (app.detail_scroll + inner_height).min(total_lines),
            total_lines,
        )
    } else {
        String::new()
    };

    let block = block.title_bottom(Line::from(scroll_info).alignment(Alignment::Right));

    let detail = Paragraph::new(lines)
        .block(block)
        .scroll((app.detail_scroll, 0));

    f.render_widget(detail, area);

    // Scrollbar
    if total_lines > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        let mut scrollbar_state = ScrollbarState::new(max_scroll as usize)
            .position(app.detail_scroll as usize);
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

// ── Status Bar ──────────────────────────────────────────────────────

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let help = match app.tab {
        Tab::Search => {
            if app.search.editing {
                "Type to edit | Esc=stop editing | Tab=next field | Enter=search"
            } else {
                "Up/Down=navigate | Enter=edit field | Tab=next | q=quit"
            }
        }
        Tab::Results => "j/k=navigate | Enter=detail | Home/End | PgUp/PgDn | q=quit",
        Tab::Detail => "j/k=scroll | [/]=prev/next | p=prev call | 1-9=related | n=nearby | Bksp=back | q=quit",
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let status = Paragraph::new(app.status.as_str()).style(Style::default().fg(Color::Cyan));
    let help_text = Paragraph::new(help)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Right);

    f.render_widget(status, chunks[0]);
    f.render_widget(help_text, chunks[1]);
}

// ── Helpers ─────────────────────────────────────────────────────────

fn detail_line(label_style: Style, label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {:<w$}", label, w = LABEL_WIDTH),
            label_style,
        ),
        Span::raw(value.to_string()),
    ])
}

fn describe_radio_service(code: &str) -> &str {
    match code {
        "HA" => "Amateur",
        "HV" => "Amateur Vanity",
        _ => "",
    }
}

fn status_color(status: &str) -> Style {
    match status {
        "A" => Style::default().fg(Color::Green),
        "E" => Style::default().fg(Color::Red),
        "C" => Style::default().fg(Color::DarkGray),
        "T" => Style::default().fg(Color::Red),
        _ => Style::default(),
    }
}
