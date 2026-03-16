use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::VecDeque;

use crate::app::{LogEntry, LogLevel};

/// Widget that renders a scrolling log panel.
///
/// Each entry is formatted as `HH:MM:SS [LEVEL] message`
/// and color-coded by severity level.
pub struct LogWidget<'a> {
    entries: &'a VecDeque<LogEntry>,
    max_lines: usize,
}

impl<'a> LogWidget<'a> {
    /// Create a new log widget from a deque of entries.
    pub fn new(entries: &'a VecDeque<LogEntry>) -> Self {
        Self {
            entries,
            max_lines: 50,
        }
    }

    /// Set the maximum number of lines to display.
    pub fn max_lines(mut self, n: usize) -> Self {
        self.max_lines = n;
        self
    }

    /// Map a log level to its display color.
    fn level_color(level: LogLevel) -> Color {
        match level {
            LogLevel::Info => Color::White,
            LogLevel::Warn => Color::Yellow,
            LogLevel::Error => Color::Red,
            LogLevel::Trade => Color::Cyan,
        }
    }

    /// Map a log level to its display tag.
    fn level_tag(level: LogLevel) -> &'static str {
        match level {
            LogLevel::Info => "INFO ",
            LogLevel::Warn => "WARN ",
            LogLevel::Error => "ERROR",
            LogLevel::Trade => "TRADE",
        }
    }
}

impl Widget for LogWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title(" Log ").borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);

        let skip = self.entries.len().saturating_sub(self.max_lines);
        let lines: Vec<Line<'_>> = self
            .entries
            .iter()
            .skip(skip)
            .map(|entry| {
                let ts = entry.timestamp.format("%H:%M:%S").to_string();
                let color = Self::level_color(entry.level);
                let tag = Self::level_tag(entry.level);
                Line::from(vec![
                    Span::styled(ts, Style::default().fg(Color::DarkGray)),
                    Span::raw(" ["),
                    Span::styled(tag, Style::default().fg(color)),
                    Span::raw("] "),
                    Span::raw(entry.message.clone()),
                ])
            })
            .collect();

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}
