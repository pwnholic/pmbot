use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, Widget};
use rust_decimal::Decimal;

use pmbot_core::messages::Position;

/// Widget that renders open positions as a table.
///
/// Columns: Strategy | Side | Entry | Size | PnL
/// PnL is green when positive, red when negative.
pub struct PositionsWidget<'a> {
    positions: &'a [Position],
}

impl<'a> PositionsWidget<'a> {
    /// Create a new positions table widget.
    pub fn new(positions: &'a [Position]) -> Self {
        Self { positions }
    }

    /// Choose a color for PnL display.
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

impl Widget for PositionsWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let header = Row::new(vec![
            Cell::from("Strategy"),
            Cell::from("Side"),
            Cell::from("Entry"),
            Cell::from("Size"),
            Cell::from("PnL"),
        ])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

        let rows: Vec<Row> = self
            .positions
            .iter()
            .map(|p| {
                let side_color = match p.side {
                    pmbot_core::types::Side::Buy => Color::Green,
                    pmbot_core::types::Side::Sell => Color::Red,
                };
                Row::new(vec![
                    Cell::from(p.strategy.to_string()),
                    Cell::from(p.side.to_string()).style(Style::default().fg(side_color)),
                    Cell::from(p.entry_price.round_dp(4).to_string()),
                    Cell::from(p.size.round_dp(2).to_string()),
                    Cell::from(p.unrealized_pnl.round_dp(2).to_string())
                        .style(Style::default().fg(Self::pnl_color(p.unrealized_pnl))),
                ])
            })
            .collect();

        let widths = [
            ratatui::layout::Constraint::Percentage(25),
            ratatui::layout::Constraint::Percentage(15),
            ratatui::layout::Constraint::Percentage(20),
            ratatui::layout::Constraint::Percentage(20),
            ratatui::layout::Constraint::Percentage(20),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(
                Block::default()
                    .title(" Positions ")
                    .borders(Borders::ALL),
            );

        Widget::render(table, area, buf);
    }
}
