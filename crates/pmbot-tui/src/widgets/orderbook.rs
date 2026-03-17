use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use rust_decimal::Decimal;

use pmbot_core::types::Level;

use crate::theme;

/// Widget that renders an orderbook as depth bars.
///
/// Asks are displayed on top in reverse order, a mid-price separator
/// in the center, and bids on the bottom. Bar width is proportional
/// to size relative to the largest level.
pub struct OrderbookWidget<'a> {
    bids: &'a [Level],
    asks: &'a [Level],
    mid_price: Option<Decimal>,
    max_levels: usize,
}

impl<'a> OrderbookWidget<'a> {
    pub fn new(bids: &'a [Level], asks: &'a [Level]) -> Self {
        Self {
            bids,
            asks,
            mid_price: None,
            max_levels: 5,
        }
    }

    pub fn mid_price(mut self, mid: Option<Decimal>) -> Self {
        self.mid_price = mid;
        self
    }

    pub fn max_levels(mut self, n: usize) -> Self {
        self.max_levels = n;
        self
    }

    fn bar(width: u16, fill_ratio: f64) -> String {
        let filled = ((width as f64) * fill_ratio).round() as usize;
        "\u{2588}".repeat(filled)
    }
}

impl Widget for OrderbookWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Orderbook ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 3 || inner.width < 10 {
            return;
        }

        let asks_display: Vec<&Level> = self.asks.iter().take(self.max_levels).collect();
        let bids_display: Vec<&Level> = self.bids.iter().take(self.max_levels).collect();

        let max_size = asks_display
            .iter()
            .chain(bids_display.iter())
            .map(|l| l.size)
            .max()
            .unwrap_or(Decimal::ONE);

        let max_size_f64 = decimal_to_f64(max_size);
        let bar_width = inner.width.saturating_sub(20).max(4);

        let mut lines: Vec<Line<'static>> = Vec::new();

        // Asks (highest first)
        for level in asks_display.iter().rev() {
            let ratio = if max_size_f64 > 0.0 {
                decimal_to_f64(level.size) / max_size_f64
            } else {
                0.0
            };
            let bar_str = Self::bar(bar_width, ratio);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>7}  ", level.price.round_dp(4)),
                    Style::default().fg(theme::ASK),
                ),
                Span::styled(bar_str, Style::default().fg(theme::ASK)),
                Span::styled(format!("  {}", level.size.round_dp(0)), theme::label()),
            ]));
        }

        // Mid separator
        let mid_str = match self.mid_price {
            Some(p) => format!("mid {}", p.round_dp(4)),
            None => "mid ---".to_string(),
        };
        let pad = (inner.width as usize).saturating_sub(mid_str.len() + 6) / 2;
        let sep = format!(
            " {}\u{2500}{}\u{2500}{}",
            "\u{2500}".repeat(pad),
            mid_str,
            "\u{2500}".repeat(pad)
        );
        lines.push(Line::from(Span::styled(
            sep,
            Style::default().fg(theme::FG_DIM),
        )));

        // Bids
        for level in &bids_display {
            let ratio = if max_size_f64 > 0.0 {
                decimal_to_f64(level.size) / max_size_f64
            } else {
                0.0
            };
            let bar_str = Self::bar(bar_width, ratio);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>7}  ", level.price.round_dp(4)),
                    Style::default().fg(theme::BID),
                ),
                Span::styled(bar_str, Style::default().fg(theme::BID)),
                Span::styled(format!("  {}", level.size.round_dp(0)), theme::label()),
            ]));
        }

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}

fn decimal_to_f64(d: Decimal) -> f64 {
    use std::str::FromStr;
    f64::from_str(&d.to_string()).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn bar_full_width() {
        let bar = OrderbookWidget::bar(10, 1.0);
        assert_eq!(bar.chars().count(), 10);
    }

    #[test]
    fn bar_zero_width() {
        let bar = OrderbookWidget::bar(10, 0.0);
        assert!(bar.is_empty());
    }

    #[test]
    fn widget_renders_without_panic() {
        let bids = vec![
            Level {
                price: dec!(0.50),
                size: dec!(200),
            },
            Level {
                price: dec!(0.49),
                size: dec!(150),
            },
        ];
        let asks = vec![
            Level {
                price: dec!(0.51),
                size: dec!(100),
            },
            Level {
                price: dec!(0.52),
                size: dec!(80),
            },
        ];
        let widget = OrderbookWidget::new(&bids, &asks).mid_price(Some(dec!(0.505)));
        let area = Rect::new(0, 0, 60, 15);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }
}
