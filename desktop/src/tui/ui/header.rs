//! Header (status + tab bar) and footer (key hints) rendering.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use super::DrawContext;
use crate::tui::state::Tab;

pub fn draw_header(frame: &mut Frame, area: Rect, ctx: &DrawContext) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);

    // Status line
    let state_label = if ctx.processing.is_running() {
        Span::styled(" RUNNING ", Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" STOPPED ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD))
    };

    let device = &ctx.config.audio.device;
    let rate = ctx.config.audio.sample_rate;
    let mode = ctx.config.mode_label();
    let status_line = Line::from(vec![
        Span::raw(" "),
        state_label,
        Span::raw(format!("  {} @ {} Hz | Mode: {} | KISS: :{} ({} clients) | Frames: {} | Uptime: {}",
            device, rate, mode,
            ctx.config.kiss.port,
            ctx.stats.kiss_clients,
            ctx.stats.unique_frames,
            ctx.stats.uptime_display(),
        )),
    ]);
    frame.render_widget(Paragraph::new(status_line), chunks[0]);

    // Tab bar
    let tab_titles: Vec<Line> = [Tab::Packets, Tab::Aprs, Tab::Settings]
        .iter()
        .map(|t| {
            let num = t.number();
            Line::from(format!(" {}:{} ", num, t.label()))
        })
        .collect();
    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::BOTTOM))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .select(match ctx.tab {
            Tab::Packets => 0,
            Tab::Aprs => 1,
            Tab::Settings => 2,
        });
    frame.render_widget(tabs, chunks[1]);
}

pub fn draw_footer(frame: &mut Frame, area: Rect, ctx: &DrawContext) {
    let stop_start = if ctx.processing.is_running() { "s:Stop" } else { "s:Start" };
    let hints = format!(" q:Quit  {}  1-3:Tab  Up/Down:Scroll  Enter:Detail", stop_start);
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}
