//! APRS tab — station list with detail pane.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, Wrap};
use super::DrawContext;

pub fn draw_aprs(frame: &mut Frame, area: Rect, ctx: &mut DrawContext) {
    let has_search = ctx.aprs_search_active || !ctx.aprs_search_text.is_empty();
    let constraints = if has_search {
        vec![Constraint::Length(1), Constraint::Min(8), Constraint::Length(5)]
    } else {
        vec![Constraint::Min(8), Constraint::Length(5)]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    if has_search {
        draw_search_bar(frame, chunks[0], ctx);
        draw_station_table(frame, chunks[1], ctx);
        draw_station_detail(frame, chunks[2], ctx);
    } else {
        draw_station_table(frame, chunks[0], ctx);
        draw_station_detail(frame, chunks[1], ctx);
    }
}

fn draw_search_bar(frame: &mut Frame, area: Rect, ctx: &DrawContext) {
    let label = " / ";
    let cursor_indicator = if ctx.aprs_search_active { "_" } else { "" };
    let text = format!("{}{}{}", label, ctx.aprs_search_text, cursor_indicator);
    let match_count = ctx.aprs_filtered_indices.len();
    let total = ctx.aprs_stations.items().len();
    let status = if ctx.aprs_search_text.is_empty() {
        String::new()
    } else {
        format!("  ({}/{})", match_count, total)
    };

    let spans = vec![
        Span::styled(label, Style::default().fg(Color::Yellow)),
        Span::raw(ctx.aprs_search_text),
        if ctx.aprs_search_active {
            Span::styled("_", Style::default().fg(Color::Yellow).add_modifier(Modifier::SLOW_BLINK))
        } else {
            Span::raw("")
        },
        Span::styled(status, Style::default().fg(Color::DarkGray)),
    ];
    let _ = text; // used only for sizing logic above
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_station_table(frame: &mut Frame, area: Rect, ctx: &mut DrawContext) {
    let header = Row::new(vec![
        Cell::from("Call"),
        Cell::from("Type"),
        Cell::from("Last Heard"),
        Cell::from("Position"),
        Cell::from("Comment"),
    ])
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let all_stations = ctx.aprs_stations.items();
    let rows: Vec<Row> = ctx.aprs_filtered_indices.iter().map(|&i| {
        let s = &all_stations[i];
        let pos = s.position.map(|(lat, lon)| format!("{:.4}, {:.4}", lat, lon))
            .unwrap_or_default();
        let display_name = if let Some(ref name) = s.object_name {
            format!("{} [{}]", s.callsign, name)
        } else {
            s.callsign.clone()
        };
        Row::new(vec![
            Cell::from(display_name),
            Cell::from(s.station_type.clone()),
            Cell::from(s.last_heard.clone()),
            Cell::from(pos),
            Cell::from(truncate_comment(s)),
        ])
    }).collect();

    let total = rows.len();
    let title = if ctx.aprs_search_text.is_empty() {
        format!(" APRS Stations ({}) ", all_stations.len())
    } else {
        format!(" APRS Stations ({}/{}) ", total, all_stations.len())
    };

    let widths = [
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(12),
        Constraint::Length(22),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().title(title).borders(Borders::ALL))
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_stateful_widget(table, area, ctx.aprs_stations.table_state_mut());

    // Scrollbar
    let visible = area.height.saturating_sub(3) as usize; // borders + header
    if total > visible {
        let selected = ctx.aprs_stations.selected_index();
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        let mut scrollbar_state = ScrollbarState::new(total.saturating_sub(visible))
            .position(selected);
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

/// Build a compact comment string for the table column, incorporating weather if present.
fn truncate_comment(s: &crate::tui::state::AprsStation) -> String {
    if let Some(ref wx) = s.weather {
        let mut parts = Vec::new();
        if let Some(t) = wx.temperature {
            let c = (t as i32 - 32) * 5 / 9;
            parts.push(format!("{}F/{}C", t, c));
        }
        if let Some(w) = wx.wind_speed {
            let dir = wx.wind_direction.unwrap_or(0);
            parts.push(format!("Wind {}@{}mph", dir, w));
        }
        if let Some(h) = wx.humidity {
            parts.push(format!("{}%RH", h));
        }
        if !parts.is_empty() {
            return parts.join(" ");
        }
    }
    s.comment.clone()
}

fn draw_station_detail(frame: &mut Frame, area: Rect, ctx: &DrawContext) {
    let content = if let Some(s) = ctx.aprs_stations.selected_item() {
        let mut lines = vec![
            Line::from(format!("{}  {}  Packets: {}", s.callsign, s.station_type, s.packet_count)),
        ];
        if let Some(ref name) = s.object_name {
            lines.push(Line::from(format!("Name: {}", name)));
        }
        if !s.last_via.is_empty() {
            lines.push(Line::from(format!("Via: {}", s.last_via)));
        }
        if let Some((lat, lon)) = s.position {
            lines.push(Line::from(format!("Position: {:.4}, {:.4}", lat, lon)));
        }
        if let Some(speed) = s.speed {
            let course = s.course.unwrap_or(0);
            lines.push(Line::from(format!("Speed: {} kts  Course: {} deg", speed, course)));
        }
        if let Some(ref wx) = s.weather {
            lines.push(Line::from(format_weather_short(wx)));
        }
        if !s.comment.is_empty() && s.weather.is_none() {
            lines.push(Line::from(s.comment.clone()));
        }
        lines
    } else {
        vec![Line::from("No station selected")]
    };

    let detail = Paragraph::new(content)
        .block(Block::default().title(" Station Detail ").borders(Borders::ALL));
    frame.render_widget(detail, area);
}

pub fn draw_station_detail_popup(frame: &mut Frame, ctx: &DrawContext) {
    let s = match ctx.aprs_stations.selected_item() {
        Some(s) => s,
        None => return,
    };

    let mut text = vec![
        Line::from(format!("Callsign:   {}", s.callsign)),
        Line::from(format!("Type:       {}", s.station_type)),
        Line::from(format!("Last Heard: {}", s.last_heard)),
        Line::from(format!("Packets:    {}", s.packet_count)),
    ];

    if !s.last_via.is_empty() {
        text.push(Line::from(format!("Path:       {}", s.last_via)));
    }

    if let Some(ref name) = s.object_name {
        text.push(Line::from(format!("Name:       {}", name)));
    }

    if let Some((table, code)) = s.symbol {
        text.push(Line::from(format!("Symbol:     {} ({}{})", symbol_description(table, code), table as char, code as char)));
    }

    if let Some((lat, lon)) = s.position {
        text.push(Line::from(format!("Position:   {:.4}, {:.4}", lat, lon)));
    } else {
        text.push(Line::from("Position:   unknown"));
    }

    if let Some(speed) = s.speed {
        text.push(Line::from(format!("Speed:      {} kts", speed)));
    }
    if let Some(course) = s.course {
        text.push(Line::from(format!("Course:     {} deg", course)));
    }

    // Weather section
    if let Some(ref wx) = s.weather {
        text.push(Line::from(""));
        text.push(Line::from(Span::styled("--- Weather ---", Style::default().fg(Color::Cyan))));
        if let Some(t) = wx.temperature {
            let c = (t as i32 - 32) * 5 / 9;
            text.push(Line::from(format!("Temp:       {} F ({} C)", t, c)));
        }
        if let Some(dir) = wx.wind_direction {
            let speed = wx.wind_speed.unwrap_or(0);
            text.push(Line::from(format!("Wind:       {} deg @ {} mph", dir, speed)));
        }
        if let Some(gust) = wx.wind_gust {
            text.push(Line::from(format!("Gusts:      {} mph", gust)));
        }
        if let Some(h) = wx.humidity {
            text.push(Line::from(format!("Humidity:   {}%", h)));
        }
        if let Some(bp) = wx.barometric_pressure {
            text.push(Line::from(format!("Pressure:   {:.1} mb", bp as f64 / 10.0)));
        }
        if let Some(rain) = wx.rain_last_hour {
            text.push(Line::from(format!("Rain/hr:    {:.2} in", rain as f64 / 100.0)));
        }
        if let Some(rain) = wx.rain_24h {
            text.push(Line::from(format!("Rain/24h:   {:.2} in", rain as f64 / 100.0)));
        }
        if let Some(lum) = wx.luminosity {
            text.push(Line::from(format!("Luminosity: {} W/m2", lum)));
        }
        if let Some(snow) = wx.snowfall {
            text.push(Line::from(format!("Snow:       {} in", snow)));
        }
    }

    if !s.comment.is_empty() {
        text.push(Line::from(""));
        text.push(Line::from(format!("Comment:    {}", s.comment)));
    }

    text.push(Line::from(""));
    text.push(Line::from(
        Span::styled("Esc/Enter: Close", Style::default().fg(Color::DarkGray))
    ));

    let height = (text.len() as u16).min(30) + 2; // +2 for borders, cap at 32
    let popup_area = crate::tui::widgets::centered_rect(70, height, frame.area());

    frame.render_widget(Clear, popup_area);
    let popup = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Station Detail ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
    frame.render_widget(popup, popup_area);
}

/// One-line weather summary for the detail pane.
fn format_weather_short(wx: &crate::tui::state::WeatherInfo) -> String {
    let mut parts = Vec::new();
    if let Some(t) = wx.temperature {
        let c = (t as i32 - 32) * 5 / 9;
        parts.push(format!("{}F/{}C", t, c));
    }
    if let Some(w) = wx.wind_speed {
        let dir = wx.wind_direction.unwrap_or(0);
        parts.push(format!("Wind {}@{}mph", dir, w));
    }
    if let Some(g) = wx.wind_gust {
        parts.push(format!("Gust {}mph", g));
    }
    if let Some(h) = wx.humidity {
        parts.push(format!("{}%RH", h));
    }
    if let Some(bp) = wx.barometric_pressure {
        parts.push(format!("{:.1}mb", bp as f64 / 10.0));
    }
    format!("WX: {}", parts.join(" "))
}

/// Map APRS symbol table+code to a human-readable description.
fn symbol_description(table: u8, code: u8) -> &'static str {
    if table == b'/' {
        match code {
            b'!' => "Police",
            b'#' => "Digi",
            b'$' => "Phone",
            b'&' => "HF Gateway",
            b'-' => "House",
            b'.' => "X",
            b'/' => "Dot",
            b'>' => "Car",
            b'?' => "Server",
            b'H' => "Hotel",
            b'I' => "TCP/IP",
            b'K' => "School",
            b'O' => "Balloon",
            b'R' => "RV",
            b'S' => "Shuttle",
            b'T' => "SSTV",
            b'U' => "Bus",
            b'W' => "NWS Site",
            b'X' => "Helicopter",
            b'Y' => "Yacht",
            b'[' => "Runner",
            b'^' => "Aircraft",
            b'_' => "WX Station",
            b'a' => "Ambulance",
            b'b' => "Bike",
            b'f' => "Fire Truck",
            b'j' => "Jeep",
            b'k' => "Truck",
            b'n' => "Node",
            b'p' => "Rover",
            b'r' => "Antenna",
            b's' => "Ship",
            b'u' => "Truck (18)",
            b'v' => "Van",
            b'y' => "House (Yagi)",
            _ => "Station",
        }
    } else if table == b'\\' {
        match code {
            b'#' => "Digi (alt)",
            b'>' => "Car (alt)",
            b'_' => "WX Station (alt)",
            b'n' => "Node (alt)",
            _ => "Station (alt)",
        }
    } else {
        "Station"
    }
}
