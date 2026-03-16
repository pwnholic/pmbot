use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use pmbot_core::messages::MarketSnapshot;

/// Widget that renders market information.
///
/// Shows market slug, mid price, spread, imbalance, and time to expiry.
pub struct MarketWidget<'a> {
    snapshot: Option<&'a MarketSnapshot>,
}

impl<'a> MarketWidget<'a> {
    /// Create a new market widget, optionally with a snapshot.
    pub fn new(snapshot: Option<&'a MarketSnapshot>) -> Self {
        Self { snapshot }
    }
}

impl Widget for MarketWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Market ")
            .borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(snap) = self.snapshot else {
            let p = Paragraph::new("No market data");
            p.render(inner, buf);
            return;
        };

        let mut lines: Vec<Line<'_>> = Vec::new();

        // Market slug / question
        lines.push(Line::from(Span::styled(
            snap.info.slug.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

        // Mid price
        let mid_str = snap
            .mid_price
            .map(|p| p.round_dp(4).to_string())
            .unwrap_or_else(|| "---".into());
        lines.push(Line::from(vec![
            Span::styled("Mid:       ", Style::default().fg(Color::DarkGray)),
            Span::raw(mid_str),
        ]));

        // Spread
        let spread_str = snap
            .spread
            .map(|s| s.round_dp(4).to_string())
            .unwrap_or_else(|| "---".into());
        lines.push(Line::from(vec![
            Span::styled("Spread:    ", Style::default().fg(Color::DarkGray)),
            Span::raw(spread_str),
        ]));

        // Imbalance
        lines.push(Line::from(vec![
            Span::styled("Imbalance: ", Style::default().fg(Color::DarkGray)),
            Span::raw(snap.imbalance.round_dp(4).to_string()),
        ]));

        // Time to expiry
        let expiry_str = match snap.info.end_date {
            Some(end) => {
                let now = chrono::Utc::now();
                if end > now {
                    let dur = end - now;
                    let hours = dur.num_hours();
                    let days = hours / 24;
                    if days > 0 {
                        format!("{}d {}h", days, hours % 24)
                    } else {
                        format!("{}h {}m", hours, dur.num_minutes() % 60)
                    }
                } else {
                    "EXPIRED".into()
                }
            }
            None => "N/A".into(),
        };
        lines.push(Line::from(vec![
            Span::styled("Expiry:    ", Style::default().fg(Color::DarkGray)),
            Span::raw(expiry_str),
        ]));

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}
