use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::VecDeque;

use crate::app::{LogEntry, LogLevel};
use crate::theme;

/// Widget that renders a scrolling log panel.
pub struct LogWidget<'a> {
    entries: &'a VecDeque<LogEntry>,
    max_lines: usize,
}

impl<'a> LogWidget<'a> {
    pub fn new(entries: &'a VecDeque<LogEntry>) -> Self {
        Self {
            entries,
            max_lines: 50,
        }
    }

    pub fn max_lines(mut self, n: usize) -> Self {
        self.max_lines = n;
        self
    }

    fn level_color(level: LogLevel) -> ratatui::style::Color {
        match level {
            LogLevel::Info => theme::FG_DIM,
            LogLevel::Warn => theme::YELLOW,
            LogLevel::Error => theme::RED,
            LogLevel::Trade => theme::GREEN,
        }
    }

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
        let block = Block::default()
            .title(" Log ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style())
            .style(Style::default().bg(theme::BG_DARK));
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
                    Span::styled(ts, theme::label()),
                    Span::styled(" [", Style::default().fg(theme::FG_DIM)),
                    Span::styled(tag, Style::default().fg(color)),
                    Span::styled("] ", Style::default().fg(theme::FG_DIM)),
                    Span::styled(entry.message.clone(), theme::value()),
                ])
            })
            .collect();

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}
