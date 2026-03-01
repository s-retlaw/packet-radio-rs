//! UI rendering — dispatches draw calls to per-tab modules.

mod aprs;
mod header;
mod packets;
mod settings;

use ratatui::prelude::*;
use super::state::{Tab, ProcessingState, DecodedFrameInfo, AprsStation, Stats, SettingsFormState};
use super::widgets::SelectableList;

/// All state needed by the rendering layer (borrowed from App).
pub struct DrawContext<'a> {
    pub tab: Tab,
    pub processing: &'a ProcessingState,
    pub config: &'a crate::config::TncConfig,
    pub frames: &'a mut SelectableList<DecodedFrameInfo>,
    pub aprs_stations: &'a mut SelectableList<AprsStation>,
    pub stats: &'a Stats,
    pub settings: &'a SettingsFormState,
    pub show_quit_dialog: bool,
    pub quit_selected: usize,
    pub show_error_dialog: bool,
    pub error_message: &'a Option<String>,
}

/// Main draw function — called on every frame.
pub fn draw(frame: &mut Frame, ctx: &mut DrawContext) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header + status bar
            Constraint::Min(0),    // main content
            Constraint::Length(1), // footer
        ])
        .split(frame.size());

    header::draw_header(frame, chunks[0], ctx);

    match ctx.tab {
        Tab::Packets => packets::draw_packets(frame, chunks[1], ctx),
        Tab::Aprs => aprs::draw_aprs(frame, chunks[1], ctx),
        Tab::Settings => settings::draw_settings(frame, chunks[1], ctx),
    }

    header::draw_footer(frame, chunks[2], ctx);

    if ctx.show_quit_dialog {
        use super::widgets::DialogBuilder;
        DialogBuilder::new("Quit")
            .message("Are you sure you want to quit?")
            .empty_line()
            .button("Yes")
            .button("No")
            .selected(ctx.quit_selected)
            .render(frame, frame.size());
    }

    if ctx.show_error_dialog {
        use super::widgets::DialogBuilder;
        let msg = ctx.error_message.as_deref().unwrap_or("Unknown error");
        DialogBuilder::new("Audio Error")
            .border_color(Color::Red)
            .width(60)
            .empty_line()
            .message(msg)
            .empty_line()
            .button("OK")
            .selected(0)
            .render(frame, frame.size());
    }
}
