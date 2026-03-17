use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use rust_decimal::Decimal;

use crate::theme;

/// Widget that renders PnL summary (balance + daily PnL).
pub struct PnlWidget {
    balance: Decimal,
    daily_pnl: Decimal,
}

impl PnlWidget {
    pub fn new(balance: Decimal, daily_pnl: Decimal) -> Self {
        Self { balance, daily_pnl }
    }
}

impl Widget for PnlWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" PnL ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style())
            .style(Style::default().bg(theme::BG_DARK));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 2 || inner.width < 10 {
            return;
        }

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
        summary.render(inner, buf);
    }
}
