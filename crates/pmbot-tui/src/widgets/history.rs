//! Trade history panel widget.
//!
//! Displays recent trades with time, strategy, side, price, and PnL.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use pmbot_db::trades::Trade;

use crate::theme;

/// Widget that renders trade history.
pub struct TradeHistoryWidget<'a> {
    trades: &'a [Trade],
    scroll_offset: usize,
}

impl<'a> TradeHistoryWidget<'a> {
    pub fn new(trades: &'a [Trade]) -> Self {
        Self {
            trades,
            scroll_offset: 0,
        }
    }

    pub fn scroll_up(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset < self.trades.len().saturating_sub(1) {
            self.scroll_offset += 1;
        }
    }
}

impl Widget for TradeHistoryWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Trade History ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style())
            .style(Style::default().bg(theme::BG_DARK));
        let inner = block.inner(area);
        block.render(area, buf);

        if self.trades.is_empty() {
            let p = Paragraph::new("No trades yet").style(theme::label());
            p.render(inner, buf);
            return;
        }

        // Header
        let header = Line::from(vec![
            Span::styled(
                "Time",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "Strategy",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "Side",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "Price",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "PnL",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        let mut lines: Vec<Line<'_>> = vec![header];

        // Trades (scrollable)
        let visible_count = (inner.height as usize).saturating_sub(1); // Leave room for header
        let start = self.scroll_offset;
        let end = (start + visible_count).min(self.trades.len());

        for trade in self.trades.iter().skip(start).take(end - start) {
            // Format time
            let time_str = trade.opened_at.format("%H:%M:%S").to_string();

            // Side color
            let side_color = match trade.side {
                pmbot_core::types::Side::Buy => theme::GREEN,
                pmbot_core::types::Side::Sell => theme::RED,
            };

            // PnL color
            let pnl = trade.pnl.unwrap_or(rust_decimal::Decimal::ZERO);
            let pnl_color = if pnl >= rust_decimal::Decimal::ZERO {
                theme::GREEN
            } else {
                theme::RED
            };

            // Price
            let price_str = if let Some(exit_price) = trade.exit_price {
                format!("{:.4}→{:.4}", trade.entry_price, exit_price)
            } else {
                format!("{:.4}", trade.entry_price)
            };

            // PnL string
            let pnl_str = trade.pnl.map_or("—".into(), |p| {
                if p >= rust_decimal::Decimal::ZERO {
                    format!("+${:.2}", p)
                } else {
                    format!("-${:.2}", p.abs())
                }
            });

            lines.push(Line::from(vec![
                Span::styled(time_str, theme::value()),
                Span::raw("  "),
                Span::styled(&trade.strategy, Style::default().fg(theme::CYAN)),
                Span::raw("  "),
                Span::styled(format!("{:?}", trade.side), Style::default().fg(side_color)),
                Span::raw("  "),
                Span::styled(price_str, theme::value()),
                Span::raw("  "),
                Span::styled(pnl_str, Style::default().fg(pnl_color)),
            ]));
        }

        // Scroll indicator
        if self.trades.len() > visible_count {
            let scroll_hint = format!(" [{}/{}]", start + 1, self.trades.len());
            lines.push(Line::from(vec![Span::styled(
                scroll_hint,
                Style::default().fg(theme::FG_DIM),
            )]));
        }

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pmbot_core::types::{MarketId, OrderId, Side, SignalId, TokenId};
    use rust_decimal::Decimal;
    use uuid::Uuid;

    fn make_trade(id: i64, strategy: &str, side: Side, pnl: Option<Decimal>) -> Trade {
        Trade {
            id,
            order_id: OrderId(format!("ord-{}", id)),
            signal_id: SignalId(Uuid::new_v4()),
            market_id: MarketId("mkt-1".into()),
            token_id: TokenId("tok-1".into()),
            outcome: "Yes".into(),
            strategy: strategy.into(),
            side,
            entry_price: Decimal::from(50),
            exit_price: pnl.map(|_| Decimal::from(55)),
            size: Decimal::from(100),
            filled_size: Decimal::from(100),
            pnl,
            opened_at: Utc::now(),
            closed_at: pnl.map(|_| Utc::now()),
        }
    }

    #[test]
    fn test_scroll_up_down() {
        let trades: Vec<Trade> = (0..10)
            .map(|i| make_trade(i, "test", Side::Buy, None))
            .collect();

        let mut widget = TradeHistoryWidget::new(&trades);
        assert_eq!(widget.scroll_offset, 0);

        widget.scroll_down();
        assert_eq!(widget.scroll_offset, 1);

        widget.scroll_up();
        assert_eq!(widget.scroll_offset, 0);
    }

    #[test]
    fn test_empty_trades() {
        let trades: Vec<Trade> = vec![];
        let widget = TradeHistoryWidget::new(&trades);
        assert_eq!(widget.scroll_offset, 0);
    }
}
