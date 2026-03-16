use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline, Widget};
use rust_decimal::Decimal;
use std::collections::VecDeque;

/// Widget that renders PnL summary and sparkline.
///
/// Shows current bankroll, daily PnL, and a sparkline of recent PnL history.
pub struct PnlWidget<'a> {
    data: &'a VecDeque<Decimal>,
    balance: Decimal,
    daily_pnl: Decimal,
}

impl<'a> PnlWidget<'a> {
    /// Create a new PnL widget.
    pub fn new(data: &'a VecDeque<Decimal>, balance: Decimal, daily_pnl: Decimal) -> Self {
        Self {
            data,
            balance,
            daily_pnl,
        }
    }

    /// Choose color for PnL display.
    fn pnl_color(pnl: Decimal) -> Color {
        if pnl > Decimal::ZERO {
            Color::Green
        } else if pnl < Decimal::ZERO {
            Color::Red
        } else {
            Color::White
        }
    }
}

impl Widget for PnlWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" PnL ")
            .borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 2 || inner.width < 10 {
            return;
        }

        // Split inner into text summary (2 lines) and sparkline (rest).
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        // Summary text
        let pnl_color = Self::pnl_color(self.daily_pnl);
        let summary = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Bankroll: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("${}", self.balance.round_dp(2)),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Day PnL:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("${}", self.daily_pnl.round_dp(2)),
                    Style::default().fg(pnl_color),
                ),
            ]),
        ]);
        summary.render(chunks[0], buf);

        // Convert Decimal PnL history to u64 for sparkline.
        // Shift values so minimum maps to 0.
        if !self.data.is_empty() {
            let min_val = self.data.iter().copied().min().unwrap_or(Decimal::ZERO);
            let sparkline_data: Vec<u64> = self
                .data
                .iter()
                .map(|d| {
                    let shifted = *d - min_val;
                    // Scale to reasonable u64 range
                    let scaled = shifted * Decimal::from(100);
                    scaled
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0)
                        .round() as u64
                })
                .collect();

            let spark_color = Self::pnl_color(self.daily_pnl);
            let sparkline = Sparkline::default()
                .data(&sparkline_data)
                .style(Style::default().fg(spark_color));
            sparkline.render(chunks[1], buf);
        }
    }
}
