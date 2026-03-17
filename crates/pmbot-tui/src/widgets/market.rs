use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use std::collections::HashMap;
use std::time::Duration;

use pmbot_core::messages::MarketSnapshot;

use crate::theme;

/// Widget that renders market information.
///
/// Shows market slug, mid price, spread, imbalance, and time to expiry.
pub struct MarketWidget<'a> {
    snapshot: Option<&'a MarketSnapshot>,
    latency: &'a HashMap<&'static str, Duration>,
}

impl<'a> MarketWidget<'a> {
    pub fn new(
        snapshot: Option<&'a MarketSnapshot>,
        latency: &'a HashMap<&'static str, Duration>,
    ) -> Self {
        Self { snapshot, latency }
    }
}

impl Widget for MarketWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Market ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style());
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(snap) = self.snapshot else {
            let p = Paragraph::new("Waiting for market data...").style(theme::label());
            p.render(inner, buf);
            return;
        };

        let mut lines: Vec<Line<'_>> = Vec::new();

        // Market slug
        lines.push(Line::from(Span::styled(
            snap.info.slug.clone(),
            Style::default()
                .fg(theme::CYAN)
                .add_modifier(Modifier::BOLD),
        )));

        // Mid price
        let mid_str = snap
            .mid_price
            .map(|p| p.round_dp(4).to_string())
            .unwrap_or_else(|| "---".into());
        lines.push(Line::from(vec![
            Span::styled("Mid:    ", theme::label()),
            Span::styled(mid_str, theme::value()),
        ]));

        // Spread
        let spread_str = snap
            .spread
            .map(|s| format!("{} ({:.1}%)", s.round_dp(4), decimal_to_f64(s) * 100.0))
            .unwrap_or_else(|| "---".into());
        lines.push(Line::from(vec![
            Span::styled("Spread: ", theme::label()),
            Span::styled(spread_str, theme::value()),
        ]));

        // Time to expiry (compact)
        let expiry_str = match snap.info.end_date {
            Some(end) => {
                let now = chrono::Utc::now();
                if end > now {
                    let dur = end - now;
                    let total_secs = dur.num_seconds();
                    if total_secs < 600 {
                        format!("{}m {}s", dur.num_minutes(), total_secs % 60)
                    } else {
                        let hours = dur.num_hours();
                        format!("{}h {}m", hours, dur.num_minutes() % 60)
                    }
                } else {
                    "EXPIRED".into()
                }
            }
            None => "N/A".into(),
        };
        let expiry_style = if expiry_str == "EXPIRED" {
            Style::default().fg(theme::RED).add_modifier(Modifier::BOLD)
        } else {
            theme::value()
        };
        lines.push(Line::from(vec![
            Span::styled("Expiry: ", theme::label()),
            Span::styled(expiry_str, expiry_style),
        ]));

        // Ping latency (compact, single line)
        let binance_ping = self
            .latency
            .get("binance")
            .map(|dur| format!("{}ms", dur.as_millis()))
            .unwrap_or_else(|| "---".into());
        let poly_ping = self
            .latency
            .get("polymarket")
            .map(|dur| format!("{}ms", dur.as_millis()))
            .unwrap_or_else(|| "---".into());
        lines.push(Line::from(vec![
            Span::styled("Ping:   ", theme::label()),
            Span::styled(binance_ping, Style::default().fg(theme::YELLOW)),
            Span::styled(" | ", theme::label()),
            Span::styled(poly_ping, Style::default().fg(theme::BLUE)),
        ]));

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}

fn decimal_to_f64(d: rust_decimal::Decimal) -> f64 {
    use std::str::FromStr;
    f64::from_str(&d.to_string()).unwrap_or(0.0)
}
