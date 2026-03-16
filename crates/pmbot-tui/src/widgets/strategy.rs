use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use pmbot_core::messages::StrategyMetrics;

/// Widget that renders strategy status information.
///
/// Displays each strategy's name, state, edge, and signal count.
pub struct StrategyWidget<'a> {
    metrics: &'a [StrategyMetrics],
}

impl<'a> StrategyWidget<'a> {
    /// Create a new strategy widget from a slice of metrics.
    pub fn new(metrics: &'a [StrategyMetrics]) -> Self {
        Self { metrics }
    }
}

impl Widget for StrategyWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Strategies ")
            .borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);

        if self.metrics.is_empty() {
            let p = Paragraph::new("No strategies");
            p.render(inner, buf);
            return;
        }

        let mut lines: Vec<Line<'_>> = Vec::new();

        for m in self.metrics {
            // Strategy name + state
            let state_color = match m.state {
                "active" => Color::Green,
                "paused" => Color::Yellow,
                _ => Color::DarkGray,
            };
            lines.push(Line::from(vec![
                Span::styled(
                    m.name.to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(format!("[{}]", m.state), Style::default().fg(state_color)),
            ]));

            // Edge
            let edge_str = m
                .edge
                .map(|e| format!("{:.4}", e))
                .unwrap_or_else(|| "---".into());
            lines.push(Line::from(vec![
                Span::styled("  Edge: ", Style::default().fg(Color::DarkGray)),
                Span::raw(edge_str),
                Span::styled("  Signals: ", Style::default().fg(Color::DarkGray)),
                Span::raw(m.signals_generated.to_string()),
            ]));

            // Custom fields
            for (key, val) in &m.custom {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key}: "), Style::default().fg(Color::DarkGray)),
                    Span::raw(val.clone()),
                ]));
            }
        }

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}
