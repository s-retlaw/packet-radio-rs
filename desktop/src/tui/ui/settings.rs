//! Settings tab — configuration form.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use super::DrawContext;
use crate::tui::state::FieldKind;

pub fn draw_settings(frame: &mut Frame, area: Rect, ctx: &DrawContext) {
    let is_running = ctx.processing.is_running();

    let title = if is_running {
        " Settings (read-only while running) "
    } else {
        " Settings "
    };

    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    for (i, field) in ctx.settings.fields.iter().enumerate() {
        let is_selected = !is_running && i == ctx.settings.selected_field;
        let is_editing = is_selected && ctx.settings.editing;

        let label_style = if is_selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if is_running {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };

        let value_str = match &field.kind {
            FieldKind::Dropdown { options, selected, .. } => {
                let val = options.get(*selected).map(|s| s.as_str()).unwrap_or("?");
                if is_running {
                    format!("  {}", val)
                } else if is_selected {
                    format!("  [ {} \u{25be} ]", val)
                } else {
                    format!("  [ {} ]", val)
                }
            }
            FieldKind::Text { value } => {
                let val = value.value();
                if is_running {
                    format!("  {}", val)
                } else if is_editing {
                    // Show cursor position
                    let before = value.before_cursor();
                    let after = value.after_cursor();
                    format!("  [ {}|{} ]", before, after)
                } else {
                    format!("  [ {} ]", val)
                }
            }
        };

        let pointer = if is_selected { "\u{25b8} " } else { "  " };

        lines.push(Line::from(vec![
            Span::styled(format!("{}{:<16}", pointer, field.label), label_style),
            Span::styled(value_str, if is_running { Style::default().fg(Color::DarkGray) } else { Style::default() }),
        ]));

        // Show inline description for selected dropdown fields
        if is_selected {
            if let Some(desc) = ctx.settings.field_description(i) {
                lines.push(Line::from(Span::styled(
                    format!("{:>18}  {}", "", desc),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "  Config: {}",
        "./packet-radio.toml"
    )));
    lines.push(Line::from(""));

    if is_running {
        lines.push(Line::from(Span::styled(
            "  Press 's' to stop processing and edit settings.",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        )));
    } else {
        // Action buttons hint
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(":Edit  "),
            Span::styled("Space", Style::default().fg(Color::Yellow)),
            Span::raw(":Cycle  "),
            Span::styled("Ctrl+S", Style::default().fg(Color::Yellow)),
            Span::raw(":Save  "),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
