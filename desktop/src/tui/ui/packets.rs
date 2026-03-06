//! Packets tab — AX.25 frame list with detail pane.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, Wrap};
use super::DrawContext;
use crate::tui::state::AprsData;

pub fn draw_packets(frame: &mut Frame, area: Rect, ctx: &mut DrawContext) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(5)])
        .split(area);

    draw_frame_table(frame, chunks[0], ctx);
    draw_detail(frame, chunks[1], ctx);
}

fn draw_frame_table(frame: &mut Frame, area: Rect, ctx: &mut DrawContext) {
    let header = Row::new(vec![
        Cell::from(" # "),
        Cell::from("Time"),
        Cell::from("Source"),
        Cell::from("Dest"),
        Cell::from("Via"),
        Cell::from("Info"),
    ])
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    .bottom_margin(0);

    let rows: Vec<Row> = ctx.frames.items().iter().rev().map(|f| {
        Row::new(vec![
            Cell::from(format!("{:>4}", f.frame_number)),
            Cell::from(f.timestamp.clone()),
            Cell::from(f.source.clone()),
            Cell::from(f.dest.clone()),
            Cell::from(f.via.clone()),
            Cell::from(truncate(&f.info, 40)),
        ])
    }).collect();

    let total = rows.len();

    let widths = [
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(18),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().title(" Decoded Frames ").borders(Borders::ALL))
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    // Rows are rendered in reverse (newest first) but SelectableList tracks
    // data indices (oldest = 0). Map data index → visual row for highlighting.
    let data_idx = ctx.frames.selected_index();
    let visual_idx = total.saturating_sub(1 + data_idx);
    let state = ctx.frames.table_state_mut();
    let saved_selected = state.selected();
    state.select(Some(visual_idx));
    frame.render_stateful_widget(table, area, state);
    // Restore data index so selected_item() still works correctly
    state.select(saved_selected);

    // Scrollbar
    let visible = area.height.saturating_sub(3) as usize; // borders + header
    if total > visible {
        let position = visual_idx;
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        let mut scrollbar_state = ScrollbarState::new(total.saturating_sub(visible))
            .position(position);
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn draw_detail(frame: &mut Frame, area: Rect, ctx: &DrawContext) {
    let content = if let Some(f) = ctx.frames.selected_item() {
        let via_str = if f.via.is_empty() {
            String::new()
        } else {
            format!(",{}", f.via)
        };
        let mut lines = vec![
            Line::from(format!(
                "#{} {}>{}{}: {}",
                f.frame_number, f.source, f.dest, via_str, f.info,
            )),
        ];
        if let Some(ref summary) = f.aprs_summary {
            lines.push(Line::from(format!("APRS: {}", summary)));
        }
        lines.push(Line::from(format!("{} bytes", f.raw_len)));
        lines
    } else {
        vec![Line::from("No frame selected")]
    };

    let detail = Paragraph::new(content)
        .block(Block::default().title(" Detail ").borders(Borders::ALL));
    frame.render_widget(detail, area);
}

pub fn draw_packet_detail_popup(frame: &mut Frame, ctx: &DrawContext) {
    let f = match ctx.frames.selected_item() {
        Some(f) => f,
        None => return,
    };

    let via_display = if f.via.is_empty() { "none".to_string() } else { f.via.clone() };

    let mut text = vec![
        Line::from(format!("Frame:    #{}", f.frame_number)),
        Line::from(format!("Time:     {}", f.timestamp)),
        Line::from(format!("Source:   {}", f.source)),
        Line::from(format!("Dest:     {}", f.dest)),
        Line::from(format!("Via:      {}", via_display)),
        Line::from(format!("Size:     {} bytes", f.raw_len)),
        Line::from(""),
        Line::from(format!("Info:     {}", f.info)),
    ];

    // Show structured APRS data if available
    if let Some(ref data) = f.aprs_data {
        text.push(Line::from(""));
        text.push(Line::from(Span::styled("--- APRS ---", Style::default().fg(Color::Cyan))));
        match data {
            AprsData::Position { lat, lon, symbol, comment, weather, timestamp, altitude, compressed_extra } => {
                text.push(Line::from("Type:     Position".to_string()));
                text.push(Line::from(format!("Lat/Lon:  {:.4}, {:.4}", lat, lon)));
                text.push(Line::from(format!("Symbol:   {}{}", symbol.0 as char, symbol.1 as char)));
                if let Some(ref ts) = timestamp {
                    text.push(Line::from(format!("Time:     {}", ts)));
                }
                if let Some(alt) = altitude {
                    text.push(Line::from(format!("Altitude: {} ft", alt)));
                }
                if let Some(ref extra) = compressed_extra {
                    text.push(Line::from(format!("Compress: {}", extra)));
                }
                if let Some(ref wx) = weather {
                    format_weather_lines(wx, &mut text);
                }
                if !comment.is_empty() {
                    text.push(Line::from(format!("Comment:  {}", comment)));
                }
            }
            AprsData::MicE { lat, lon, speed, course, symbol } => {
                text.push(Line::from("Type:     Mic-E".to_string()));
                text.push(Line::from(format!("Lat/Lon:  {:.4}, {:.4}", lat, lon)));
                text.push(Line::from(format!("Speed:    {} kts", speed)));
                text.push(Line::from(format!("Course:   {} deg", course)));
                text.push(Line::from(format!("Symbol:   {}{}", symbol.0 as char, symbol.1 as char)));
            }
            AprsData::Message { addressee, text: msg_text, message_no, message_type } => {
                text.push(Line::from(format!("Type:     Message ({})", message_type)));
                text.push(Line::from(format!("To:       {}", addressee)));
                text.push(Line::from(format!("Text:     {}", msg_text)));
                if let Some(ref no) = message_no {
                    text.push(Line::from(format!("Msg No:   {}", no)));
                }
            }
            AprsData::Weather { weather, comment } => {
                text.push(Line::from("Type:     Weather".to_string()));
                format_weather_lines(weather, &mut text);
                if !comment.is_empty() {
                    text.push(Line::from(format!("Comment:  {}", comment)));
                }
            }
            AprsData::Object { name, live, lat, lon, symbol, comment, timestamp } => {
                let status = if *live { "live" } else { "killed" };
                text.push(Line::from(format!("Type:     Object ({})", status)));
                text.push(Line::from(format!("Name:     {}", name)));
                text.push(Line::from(format!("Lat/Lon:  {:.4}, {:.4}", lat, lon)));
                text.push(Line::from(format!("Symbol:   {}{}", symbol.0 as char, symbol.1 as char)));
                if let Some(ref ts) = timestamp {
                    text.push(Line::from(format!("Time:     {}", ts)));
                }
                if !comment.is_empty() {
                    text.push(Line::from(format!("Comment:  {}", comment)));
                }
            }
            AprsData::Item { name, live, lat, lon, symbol, comment } => {
                let status = if *live { "live" } else { "killed" };
                text.push(Line::from(format!("Type:     Item ({})", status)));
                text.push(Line::from(format!("Name:     {}", name)));
                text.push(Line::from(format!("Lat/Lon:  {:.4}, {:.4}", lat, lon)));
                text.push(Line::from(format!("Symbol:   {}{}", symbol.0 as char, symbol.1 as char)));
                if !comment.is_empty() {
                    text.push(Line::from(format!("Comment:  {}", comment)));
                }
            }
            AprsData::Status { text: status_text, timestamp, maidenhead } => {
                text.push(Line::from("Type:     Status".to_string()));
                if let Some(ref ts) = timestamp {
                    text.push(Line::from(format!("Time:     {}", ts)));
                }
                if let Some(ref mh) = maidenhead {
                    text.push(Line::from(format!("Grid:     {}", mh)));
                }
                text.push(Line::from(format!("Text:     {}", status_text)));
            }
            AprsData::Telemetry { sequence, analog, digital } => {
                text.push(Line::from("Type:     Telemetry".to_string()));
                text.push(Line::from(format!("Seq:      #{}", sequence)));
                let analog_strs: Vec<String> = analog.iter().map(|a| {
                    a.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string())
                }).collect();
                text.push(Line::from(format!("Analog:   {}", analog_strs.join(", "))));
                text.push(Line::from(format!("Digital:  {:08b}", digital)));
            }
            AprsData::Query { query_type } => {
                text.push(Line::from("Type:     Query".to_string()));
                text.push(Line::from(format!("Query:    {}", query_type)));
            }
            AprsData::Capabilities { data } => {
                text.push(Line::from("Type:     Capabilities".to_string()));
                text.push(Line::from(format!("Data:     {}", data)));
            }
            AprsData::ThirdParty { data } => {
                text.push(Line::from("Type:     Third-party".to_string()));
                text.push(Line::from(format!("Data:     {}", data)));
            }
            AprsData::RawGps { data, position, speed, course, altitude, satellites, fix_valid } => {
                text.push(Line::from(format!("Type:     Raw GPS{}", if *fix_valid { "" } else { " (no fix)" })));
                text.push(Line::from(format!("NMEA:     {}", data)));
                if let Some((lat, lon)) = position {
                    text.push(Line::from(format!("Position: {lat:.6}, {lon:.6}")));
                }
                if let Some(spd) = speed {
                    text.push(Line::from(format!("Speed:    {spd:.1} kts")));
                }
                if let Some(crs) = course {
                    text.push(Line::from(format!("Course:   {crs:.1}°")));
                }
                if let Some(alt) = altitude {
                    text.push(Line::from(format!("Altitude: {alt:.1} m")));
                }
                if let Some(sats) = satellites {
                    text.push(Line::from(format!("Sats:     {sats}")));
                }
            }
            AprsData::UserDefined { data } => {
                text.push(Line::from("Type:     User-defined".to_string()));
                text.push(Line::from(format!("Data:     {}", data)));
            }
            AprsData::Unknown { dti } => {
                text.push(Line::from(format!("Type:     Unknown (DTI 0x{:02X})", dti)));
            }
        }
    } else if let Some(ref summary) = f.aprs_summary {
        text.push(Line::from(""));
        text.push(Line::from(format!("APRS:     {}", summary)));
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
                .title(" Packet Detail ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
    frame.render_widget(popup, popup_area);
}

/// Append weather data lines to a text vector.
fn format_weather_lines(wx: &crate::tui::state::WeatherInfo, text: &mut Vec<Line<'_>>) {
    if let Some(t) = wx.temperature {
        let c = (t as i32 - 32) * 5 / 9;
        text.push(Line::from(format!("Temp:     {} F ({} C)", t, c)));
    }
    if let Some(dir) = wx.wind_direction {
        let speed = wx.wind_speed.unwrap_or(0);
        text.push(Line::from(format!("Wind:     {} deg @ {} mph", dir, speed)));
    }
    if let Some(gust) = wx.wind_gust {
        text.push(Line::from(format!("Gusts:    {} mph", gust)));
    }
    if let Some(h) = wx.humidity {
        text.push(Line::from(format!("Humidity: {}%", h)));
    }
    if let Some(bp) = wx.barometric_pressure {
        text.push(Line::from(format!("Pressure: {:.1} mb", bp as f64 / 10.0)));
    }
    if let Some(rain) = wx.rain_last_hour {
        text.push(Line::from(format!("Rain/hr:  {:.2} in", rain as f64 / 100.0)));
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
