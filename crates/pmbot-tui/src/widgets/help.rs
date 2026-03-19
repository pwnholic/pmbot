//! Help view widget.
//!
//! Displays keyboard shortcuts and explanations for the TUI.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::theme;

/// Help section for organized shortcuts.
#[derive(Debug, Clone)]
pub struct HelpSection {
    pub title: &'static str,
    pub items: Vec<(&'static str, &'static str)>,
}

/// Widget that renders help view with keyboard shortcuts.
pub struct HelpWidget {
    sections: Vec<HelpSection>,
}

impl HelpWidget {
    pub fn new() -> Self {
        Self {
            sections: vec![
                HelpSection {
                    title: "Navigation",
                    items: vec![
                        ("↑/↓", "Navigate markets/outcomes"),
                        ("←/→", "Switch categories"),
                        ("Tab", "Cycle through tabs"),
                        ("Esc", "Return/cancel"),
                    ],
                },
                HelpSection {
                    title: "Main Dashboard",
                    items: vec![
                        ("q", "Quit application"),
                        ("/", "Open market search"),
                        ("f", "Open filter menu"),
                        ("h", "Show this help"),
                    ],
                },
                HelpSection {
                    title: "Market Search",
                    items: vec![
                        ("/", "Focus search box"),
                        ("↑/↓", "Navigate results"),
                        ("←/→", "Switch categories"),
                        ("Enter", "Select market"),
                        ("Esc", "Close search"),
                    ],
                },
                HelpSection {
                    title: "Filter Menu",
                    items: vec![
                        ("↑/↓", "Navigate options"),
                        ("Enter", "Toggle/edit field"),
                        ("Space", "Toggle strategy"),
                        ("d", "Remove exclude tag"),
                        ("a", "Apply filters"),
                        ("Esc/q", "Close menu"),
                    ],
                },
                HelpSection {
                    title: "Trade Execution",
                    items: vec![
                        ("↑/↓", "Select outcome"),
                        ("s", "Edit size"),
                        ("p", "Edit price"),
                        ("t", "Cycle order type"),
                        ("Enter", "Confirm trade"),
                        ("Esc", "Cancel"),
                    ],
                },
                HelpSection {
                    title: "Multi-Option Markets",
                    items: vec![
                        ("↑/↓", "Select outcome"),
                        ("Enter", "Start trade for outcome"),
                    ],
                },
            ],
        }
    }
}

impl Default for HelpWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for HelpWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Help - Keyboard Shortcuts ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style())
            .style(Style::default().bg(theme::BG_DARK));
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines: Vec<Line<'_>> = Vec::new();

        for section in &self.sections {
            // Section title
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                section.title,
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            )]));

            // Section items
            for (key, description) in &section.items {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:12}", key), Style::default().fg(theme::CYAN)),
                    Span::styled(*description, Style::default().fg(theme::FG)),
                ]));
            }
        }

        // Footer
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Press Esc or q to close",
            Style::default().fg(theme::FG_DIM),
        )]));

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_widget_new() {
        let widget = HelpWidget::new();
        assert!(!widget.sections.is_empty());
        assert!(widget.sections.iter().any(|s| s.title == "Navigation"));
        assert!(widget.sections.iter().any(|s| s.title == "Main Dashboard"));
    }

    #[test]
    fn test_help_widget_default() {
        let widget = HelpWidget::default();
        assert!(!widget.sections.is_empty());
    }
}
