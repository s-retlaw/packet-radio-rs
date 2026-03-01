//! Packets tab — AX.25 frame list with detail pane.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use super::DrawContext;

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

    frame.render_stateful_widget(table, area, ctx.frames.table_state_mut());
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
