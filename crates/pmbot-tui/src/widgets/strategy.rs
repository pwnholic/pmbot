use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use pmbot_core::messages::StrategyMetrics;

use crate::theme;

/// Render a sparkline from PnL history.
/// Returns a string with Unicode block characters.
fn render_sparkline(history: &[rust_decimal::Decimal], width: usize) -> String {
    if history.is_empty() || width == 0 {
        return String::new();
    }

    // Get the range for normalization
    let min = history
        .iter()
        .fold(rust_decimal::Decimal::MAX, |a, &b| a.min(b));
    let max = history
        .iter()
        .fold(rust_decimal::Decimal::MIN, |a, &b| a.max(b));

    // If all values are the same, return flat line
    if min == max {
        return "─".repeat(width.min(history.len()));
    }

    // Sparkline characters (from lowest to highest)
    const SPARK_CHARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    // Sample history to fit width
    let sampled: Vec<_> = if history.len() <= width {
        history.to_vec()
    } else {
        // Take the last `width` values
        history[history.len() - width..].to_vec()
    };

    let range = max - min;
    sampled
        .iter()
        .map(|&v| {
            let normalized = ((v - min) / range).try_into().unwrap_or(0.5f64);
            let idx = ((normalized * (SPARK_CHARS.len() - 1) as f64).round() as usize)
                .min(SPARK_CHARS.len() - 1);
            SPARK_CHARS[idx]
        })
        .collect::<String>()
}

/// Widget that renders strategy status information.
pub struct StrategyWidget<'a> {
    metrics: &'a [StrategyMetrics],
}

impl<'a> StrategyWidget<'a> {
    pub fn new(metrics: &'a [StrategyMetrics]) -> Self {
        Self { metrics }
    }
}

impl Widget for StrategyWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Strategies ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style())
            .style(Style::default().bg(theme::BG_DARK));
        let inner = block.inner(area);
        block.render(area, buf);

        if self.metrics.is_empty() {
            let p = Paragraph::new("No strategies loaded").style(theme::label());
            p.render(inner, buf);
            return;
        }

        let mut lines: Vec<Line<'_>> = Vec::new();

        for m in self.metrics {
            let state_color = match m.state {
                "active" => theme::GREEN,
                "paused" => theme::YELLOW,
                "in_position" => theme::MAGENTA,
                _ => theme::FG_DIM,
            };

            // Name + state on one line
            lines.push(Line::from(vec![
                Span::styled(
                    m.name.to_string(),
                    Style::default()
                        .fg(theme::CYAN)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(format!("[{}]", m.state), Style::default().fg(state_color)),
            ]));

            // Edge + signals on one line
            let edge_str = m
                .edge
                .map(|e| format!("{:.4}", e))
                .unwrap_or_else(|| "---".into());
            lines.push(Line::from(vec![
                Span::styled("  edge:", theme::label()),
                Span::styled(format!(" {}", edge_str), theme::value()),
                Span::styled("  sigs:", theme::label()),
                Span::styled(
                    format!(" {}", m.signals_generated),
                    Style::default().fg(theme::ORANGE),
                ),
            ]));

            // Win rate and PnL line
            let win_rate = if m.trades > 0 {
                format!("{:.1}%", (m.wins as f64 / m.trades as f64) * 100.0)
            } else {
                "---".into()
            };
            let pnl_color = if m.total_pnl >= rust_decimal::Decimal::ZERO {
                theme::GREEN
            } else {
                theme::RED
            };
            lines.push(Line::from(vec![
                Span::styled("  trades:", theme::label()),
                Span::styled(format!(" {}", m.trades), theme::value()),
                Span::styled("  win:", theme::label()),
                Span::styled(format!(" {}", win_rate), theme::value()),
                Span::styled("  pnl:", theme::label()),
                Span::styled(
                    format!(" ${:.2}", m.total_pnl),
                    Style::default().fg(pnl_color),
                ),
            ]));

            // Sparkline (PnL history) - only if we have history
            if !m.pnl_history.is_empty() {
                let sparkline = render_sparkline(&m.pnl_history, 20);
                let spark_color = if m.total_pnl >= rust_decimal::Decimal::ZERO {
                    theme::GREEN
                } else {
                    theme::RED
                };
                lines.push(Line::from(vec![
                    Span::styled("  trend:", theme::label()),
                    Span::styled(format!(" {}", sparkline), Style::default().fg(spark_color)),
                ]));
            }

            // Custom fields (compact)
            for (key, val) in &m.custom {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key}: "), theme::label()),
                    Span::styled(val.clone(), theme::value()),
                ]));
            }
        }

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}
