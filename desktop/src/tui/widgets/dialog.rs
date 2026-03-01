//! Reusable dialog builder widget
//!
//! Provides a builder pattern for creating modal dialogs with consistent styling.

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Builder for creating modal dialogs
pub struct DialogBuilder<'a> {
    title: &'a str,
    lines: Vec<Line<'a>>,
    buttons: Vec<&'a str>,
    selected_button: usize,
    width: u16,
    border_color: Color,
}

impl<'a> DialogBuilder<'a> {
    /// Create a new dialog builder with a title
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            lines: Vec::new(),
            buttons: Vec::new(),
            selected_button: 0,
            width: 50,
            border_color: Color::Yellow,
        }
    }

    /// Set the dialog width
    pub fn width(mut self, w: u16) -> Self {
        self.width = w;
        self
    }

    /// Set the border color
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    /// Add a message line
    pub fn message(mut self, text: &'a str) -> Self {
        self.lines.push(Line::from(text.to_string()));
        self
    }

    /// Add an empty line for spacing
    pub fn empty_line(mut self) -> Self {
        self.lines.push(Line::from(""));
        self
    }

    /// Add a button label
    pub fn button(mut self, label: &'a str) -> Self {
        self.buttons.push(label);
        self
    }

    /// Set which button is currently selected (0-indexed)
    pub fn selected(mut self, idx: usize) -> Self {
        self.selected_button = idx;
        self
    }

    /// Render the dialog centered in the given area
    pub fn render(mut self, frame: &mut Frame, area: Rect) {
        // Build button line if we have buttons
        if !self.buttons.is_empty() {
            let mut spans = Vec::new();
            for (i, label) in self.buttons.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("    "));
                }
                let style = if i == self.selected_button {
                    Style::default().bg(Color::White).fg(Color::Black).bold()
                } else {
                    Style::default().fg(Color::Gray)
                };
                spans.push(Span::styled(format!("  {}  ", label), style));
            }
            self.lines.push(Line::from(spans));
        }

        let height = (self.lines.len() as u16) + 2; // +2 for borders
        let dialog_area = centered_rect(self.width, height, area);

        frame.render_widget(Clear, dialog_area);

        let dialog = Paragraph::new(self.lines)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(format!(" {} ", self.title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.border_color)),
            );

        frame.render_widget(dialog, dialog_area);
    }
}

/// Calculate a centered rectangle within an area
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: (area.width.saturating_sub(width)) / 2,
        y: (area.height.saturating_sub(height)) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let centered = centered_rect(40, 20, area);

        assert_eq!(centered.x, 30);
        assert_eq!(centered.y, 15);
        assert_eq!(centered.width, 40);
        assert_eq!(centered.height, 20);
    }

    #[test]
    fn test_centered_rect_overflow() {
        let area = Rect::new(0, 0, 30, 20);
        let centered = centered_rect(50, 30, area);

        // Should be clamped to area size
        assert_eq!(centered.width, 30);
        assert_eq!(centered.height, 20);
    }

    #[test]
    fn test_dialog_builder_chain() {
        let builder = DialogBuilder::new("Test")
            .width(60)
            .border_color(Color::Cyan)
            .empty_line()
            .message("Hello, World!")
            .empty_line();

        assert_eq!(builder.width, 60);
        assert_eq!(builder.border_color, Color::Cyan);
        assert_eq!(builder.lines.len(), 3);
    }

    #[test]
    fn test_dialog_builder_buttons() {
        let builder = DialogBuilder::new("Quit")
            .message("Are you sure?")
            .button("Yes")
            .button("No")
            .selected(1);

        assert_eq!(builder.buttons.len(), 2);
        assert_eq!(builder.selected_button, 1);
        assert_eq!(builder.buttons[0], "Yes");
        assert_eq!(builder.buttons[1], "No");
    }

    #[test]
    fn test_dialog_builder_default_selection() {
        let builder = DialogBuilder::new("Confirm")
            .button("OK")
            .button("Cancel");

        assert_eq!(builder.selected_button, 0);
    }

    #[test]
    fn test_centered_rect_exact_fit() {
        let area = Rect::new(0, 0, 40, 20);
        let centered = centered_rect(40, 20, area);

        assert_eq!(centered.x, 0);
        assert_eq!(centered.y, 0);
        assert_eq!(centered.width, 40);
        assert_eq!(centered.height, 20);
    }
}
