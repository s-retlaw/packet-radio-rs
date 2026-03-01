//! APRS tab — station list with detail pane.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use super::DrawContext;

pub fn draw_aprs(frame: &mut Frame, area: Rect, ctx: &mut DrawContext) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(5)])
        .split(area);

    draw_station_table(frame, chunks[0], ctx);
    draw_station_detail(frame, chunks[1], ctx);
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

    let rows: Vec<Row> = ctx.aprs_stations.items().iter().map(|s| {
        let pos = s.position.map(|(lat, lon)| format!("{:.4}, {:.4}", lat, lon))
            .unwrap_or_default();
        Row::new(vec![
            Cell::from(s.callsign.clone()),
            Cell::from(s.station_type.clone()),
            Cell::from(s.last_heard.clone()),
            Cell::from(pos),
            Cell::from(s.comment.clone()),
        ])
    }).collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(22),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().title(" APRS Stations ").borders(Borders::ALL))
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_stateful_widget(table, area, ctx.aprs_stations.table_state_mut());
}

fn draw_station_detail(frame: &mut Frame, area: Rect, ctx: &DrawContext) {
    let content = if let Some(s) = ctx.aprs_stations.selected_item() {
        let mut lines = vec![
            Line::from(format!("{}  {}  Packets: {}", s.callsign, s.station_type, s.packet_count)),
        ];
        if let Some((lat, lon)) = s.position {
            lines.push(Line::from(format!("Position: {:.4}, {:.4}", lat, lon)));
        }
        if let Some(speed) = s.speed {
            let course = s.course.unwrap_or(0);
            lines.push(Line::from(format!("Speed: {} kts  Course: {} deg", speed, course)));
        }
        if !s.comment.is_empty() {
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
