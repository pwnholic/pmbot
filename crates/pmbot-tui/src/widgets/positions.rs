use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, Widget};
use rust_decimal::Decimal;

use pmbot_core::messages::Position;

use crate::theme;

/// Widget that renders open positions as a table.
///
/// Columns: Strategy | Side | Entry | Size | TP | SL | PnL | Age
pub struct PositionsWidget<'a> {
    positions: &'a [Position],
}

impl<'a> PositionsWidget<'a> {
    pub fn new(positions: &'a [Position]) -> Self {
        Self { positions }
    }

    fn pnl_style(pnl: Decimal) -> Style {
        if pnl > Decimal::ZERO {
            theme::profit()
        } else if pnl < Decimal::ZERO {
            theme::loss()
        } else {
            theme::value()
        }
    }

    fn format_age(opened_at: chrono::DateTime<chrono::Utc>) -> String {
        let dur = chrono::Utc::now() - opened_at;
        let secs = dur.num_seconds();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

impl Widget for PositionsWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let header = Row::new(vec![
            Cell::from("Strategy"),
            Cell::from("Side"),
            Cell::from("Entry"),
            Cell::from("Size"),
            Cell::from("TP"),
            Cell::from("SL"),
            Cell::from("PnL"),
            Cell::from("Age"),
        ])
        .style(theme::table_header());

        let rows: Vec<Row> = self
            .positions
            .iter()
            .map(|p| {
                let side_color = match p.side {
                    pmbot_core::types::Side::Buy => theme::BUY_SIDE,
                    pmbot_core::types::Side::Sell => theme::SELL_SIDE,
                };
                let pnl_str = if p.unrealized_pnl >= Decimal::ZERO {
                    format!("+{}", p.unrealized_pnl.round_dp(2))
                } else {
                    format!("{}", p.unrealized_pnl.round_dp(2))
                };
                Row::new(vec![
                    Cell::from(p.strategy.to_string()),
                    Cell::from(p.side.to_string())
                        .style(Style::default().fg(side_color).add_modifier(Modifier::BOLD)),
                    Cell::from(p.entry_price.round_dp(4).to_string()),
                    Cell::from(format!("${}", p.size.round_dp(2))),
                    Cell::from(p.tp_price.round_dp(4).to_string())
                        .style(Style::default().fg(theme::GREEN)),
                    Cell::from(p.sl_price.round_dp(4).to_string())
                        .style(Style::default().fg(theme::RED)),
                    Cell::from(pnl_str).style(Self::pnl_style(p.unrealized_pnl)),
                    Cell::from(Self::format_age(p.opened_at)).style(theme::label()),
                ])
                .style(theme::value())
            })
            .collect();

        let widths = [
            ratatui::layout::Constraint::Percentage(14),
            ratatui::layout::Constraint::Percentage(8),
            ratatui::layout::Constraint::Percentage(12),
            ratatui::layout::Constraint::Percentage(12),
            ratatui::layout::Constraint::Percentage(12),
            ratatui::layout::Constraint::Percentage(12),
            ratatui::layout::Constraint::Percentage(14),
            ratatui::layout::Constraint::Percentage(12),
        ];

        let table = Table::new(rows, widths).header(header).block(
            Block::default()
                .title(format!(" Positions ({}) ", self.positions.len()))
                .title_style(theme::title_style())
                .borders(Borders::ALL)
                .border_style(theme::border_style()),
        );

        Widget::render(table, area, buf);
    }
}
