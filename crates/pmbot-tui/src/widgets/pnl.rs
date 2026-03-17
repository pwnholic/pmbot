use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline, Widget};
use rust_decimal::Decimal;
use std::collections::VecDeque;

use crate::theme;

/// Widget that renders PnL summary and sparkline.
pub struct PnlWidget<'a> {
    data: &'a VecDeque<Decimal>,
    balance: Decimal,
    daily_pnl: Decimal,
}

impl<'a> PnlWidget<'a> {
    pub fn new(data: &'a VecDeque<Decimal>, balance: Decimal, daily_pnl: Decimal) -> Self {
        Self {
            data,
            balance,
            daily_pnl,
        }
    }
}

impl Widget for PnlWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" PnL ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 2 || inner.width < 10 {
            return;
        }

        // Split: text summary (2 lines) + sparkline
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        // PnL text
        let pnl_style = if self.daily_pnl > Decimal::ZERO {
            theme::profit()
        } else if self.daily_pnl < Decimal::ZERO {
            theme::loss()
        } else {
            theme::value()
        };

        let pnl_str = if self.daily_pnl >= Decimal::ZERO {
            format!("+${}", self.daily_pnl.round_dp(2))
        } else {
            format!("-${}", self.daily_pnl.abs().round_dp(2))
        };

        let summary = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Balance: ", theme::label()),
                Span::styled(
                    format!("${}", self.balance.round_dp(2)),
                    Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Day PnL: ", theme::label()),
                Span::styled(pnl_str, pnl_style),
            ]),
        ]);
        summary.render(chunks[0], buf);

        // Sparkline
        if !self.data.is_empty() {
            let min_val = self.data.iter().copied().min().unwrap_or(Decimal::ZERO);
            let sparkline_data: Vec<u64> = self
                .data
                .iter()
                .map(|d| {
                    let shifted = *d - min_val;
                    let scaled = shifted * Decimal::from(100);
                    scaled.to_string().parse::<f64>().unwrap_or(0.0).round() as u64
                })
                .collect();

            let spark_color = if self.daily_pnl >= Decimal::ZERO {
                theme::GREEN
            } else {
                theme::RED
            };
            let sparkline = Sparkline::default()
                .data(&sparkline_data)
                .style(Style::default().fg(spark_color));
            sparkline.render(chunks[1], buf);
        }
    }
}
