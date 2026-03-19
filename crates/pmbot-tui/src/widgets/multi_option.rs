//! Multi-option market view widget.
//!
//! Displays markets with N>2 outcomes, showing prices, positions, and arbitrage opportunities.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::theme;

/// State for multi-option market view.
#[derive(Debug, Clone)]
pub struct MultiOptionState {
    /// Market question.
    pub question: String,
    /// Available outcomes.
    pub outcomes: Vec<String>,
    /// Current prices for each outcome.
    pub prices: Vec<Decimal>,
    /// Current positions for each outcome (positive=long, negative=short).
    pub positions: Vec<Decimal>,
    /// Selected outcome index (for trading).
    pub selected: Option<usize>,
    /// Whetherarbitrage opportunity exists.
    pub has_arbitrage: bool,
}

impl Default for MultiOptionState {
    fn default() -> Self {
        Self {
            question: String::new(),
            outcomes: Vec::new(),
            prices: Vec::new(),
            positions: Vec::new(),
            selected: None,
            has_arbitrage: false,
        }
    }
}

impl MultiOptionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_market(&mut self, question: String, outcomes: Vec<String>, prices: Vec<Decimal>) {
        self.question = question;
        self.outcomes = outcomes;
        self.prices = prices;
        self.positions = vec![];
        self.selected = None;
        self.check_arbitrage();
    }

    pub fn set_positions(&mut self, positions: Vec<Decimal>) {
        self.positions = positions;
    }

    /// Check if sum of prices ≠ 100% (arbitrage opportunity).
    fn check_arbitrage(&mut self) {
        if self.prices.is_empty() {
            self.has_arbitrage = false;
            return;
        }
        let total: Decimal = self.prices.iter().sum();
        // If prices don't sum to ~1.0, there's an arb opportunity
        // Typical arb: all prices < 100% (buy all outcomes)
        self.has_arbitrage = total < dec!(0.98) || total > dec!(1.02);
    }

    /// Calculate edge for each outcome (deviation from fair value).
    pub fn edges(&self) -> Vec<Decimal> {
        if self.outcomes.is_empty() {
            return Vec::new();
        }
        let n = Decimal::from(self.outcomes.len() as u32);
        // Fair value for equal probability = 1/n
        let fair_value = Decimal::ONE / n;
        self.prices
            .iter()
            .map(|&price| fair_value - price)
            .collect()
    }

    pub fn select_next(&mut self) {
        if self.outcomes.is_empty() {
            return;
        }
        let next = self
            .selected
            .map(|s| (s + 1) % self.outcomes.len())
            .unwrap_or(0);
        self.selected = Some(next);
    }

    pub fn select_prev(&mut self) {
        if self.outcomes.is_empty() {
            return;
        }
        let prev = self
            .selected
            .map(|s| {
                if s == 0 {
                    self.outcomes.len() - 1
                } else {
                    s - 1
                }
            })
            .unwrap_or(0);
        self.selected = Some(prev);
    }
}

/// Widget that renders multi-option market view.
pub struct MultiOptionWidget<'a> {
    state: &'a MultiOptionState,
}

impl<'a> MultiOptionWidget<'a> {
    pub fn new(state: &'a MultiOptionState) -> Self {
        Self { state }
    }
}

impl Widget for MultiOptionWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Multi-Option Market ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style())
            .style(Style::default().bg(theme::BG_DARK));
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines: Vec<Line<'_>> = Vec::new();

        // Market question
        if self.state.question.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "No market loaded",
                Style::default().fg(theme::FG_DIM),
            )]));
            let paragraph = Paragraph::new(lines);
            paragraph.render(inner, buf);
            return;
        }

        // Truncate question if too long
        let question_display = if self.state.question.len() > inner.width as usize - 10 {
            format!("{}...", &self.state.question[..inner.width as usize - 13])
        } else {
            self.state.question.clone()
        };
        lines.push(Line::from(vec![Span::styled(
            &question_display,
            Style::default()
                .fg(theme::CYAN)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(""));

        // Arbitrage warning
        if self.state.has_arbitrage {
            lines.push(Line::from(vec![Span::styled(
                "⚠ ARBITRAGE OPPORTUNITY: Prices don't sum to 100%",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));
        }

        // Header
        lines.push(Line::from(vec![
            Span::styled(
                "Outcome",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                "Price",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                "Position",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                "Edge",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Calculate edges
        let edges = self.state.edges();

        // Outcome rows
        for (i, (outcome, price)) in self
            .state
            .outcomes
            .iter()
            .zip(self.state.prices.iter())
            .enumerate()
        {
            let edge = edges.get(i).copied().unwrap_or(Decimal::ZERO);
            let position = self
                .state
                .positions
                .get(i)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let is_selected = self.state.selected == Some(i);

            let row_style = if is_selected {
                Style::default().fg(theme::BLACK).bg(theme::CYAN)
            } else {
                Style::default()
            };

            let edge_color = if edge > Decimal::ZERO {
                theme::GREEN
            } else if edge < Decimal::ZERO {
                theme::RED
            } else {
                theme::FG_DIM
            };

            let pos_color = if position > Decimal::ZERO {
                theme::GREEN
            } else if position < Decimal::ZERO {
                theme::RED
            } else {
                theme::FG_DIM
            };

            lines.push(Line::from(vec![
                Span::styled(
                    if is_selected { "► " } else { "  " },
                    Style::default().fg(theme::YELLOW),
                ),
                Span::styled(
                    format!(
                        "{:12}",
                        if outcome.len() > 12 {
                            &outcome[..12]
                        } else {
                            outcome
                        }
                    ),
                    row_style,
                ),
                Span::raw(" "),
                Span::styled(format!("${:.4}", price), row_style),
                Span::raw(" "),
                Span::styled(format!("{:+.2}", position), Style::default().fg(pos_color)),
                Span::raw(" "),
                Span::styled(format!("{:+.4}", edge), Style::default().fg(edge_color)),
            ]));
        }

        // Price sum
        let total: Decimal = self.state.prices.iter().sum();
        let sum_color = if total > dec!(1.02) || total < dec!(0.98) {
            theme::RED
        } else {
            theme::GREEN
        };
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Price Sum:", theme::label()),
            Span::styled(format!(" {:.4}", total), Style::default().fg(sum_color)),
        ]));

        // Net position
        let net_position: Decimal = self.state.positions.iter().sum();
        let net_color = if net_position > Decimal::ZERO {
            theme::GREEN
        } else if net_position < Decimal::ZERO {
            theme::RED
        } else {
            theme::FG_DIM
        };
        lines.push(Line::from(vec![
            Span::styled("Net Position:", theme::label()),
            Span::styled(
                format!(" {:+.2}", net_position),
                Style::default().fg(net_color),
            ),
        ]));

        // Instructions
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("[↑/↓] Navigate", Style::default().fg(theme::CYAN)),
            Span::raw("  "),
            Span::styled("[Enter] Trade", Style::default().fg(theme::FG_DIM)),
        ]));

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_option_state_defaults() {
        let state = MultiOptionState::new();
        assert!(state.question.is_empty());
        assert!(state.outcomes.is_empty());
    }

    #[test]
    fn test_set_market() {
        let mut state = MultiOptionState::new();
        // Prices sum to 0.95 - this IS an arbitrage (buy all for 95c, guarantee $1)
        state.set_market(
            "Who wins the election?".into(),
            vec!["Candidate A".into(), "Candidate B".into(), "Other".into()],
            vec![dec!(0.45), dec!(0.40), dec!(0.10)],
        );
        assert_eq!(state.outcomes.len(), 3);
        assert_eq!(state.prices.len(), 3);
        assert!(state.has_arbitrage); // 0.95 sum < 0.98 = arbitrage
    }

    #[test]
    fn test_arbitrage_detection() {
        let mut state = MultiOptionState::new();
        // Prices sum to 95% - arbitrage opportunity
        state.set_market(
            "Test?".into(),
            vec!["A".into(), "B".into()],
            vec![dec!(0.48), dec!(0.47)],
        );
        assert!(state.has_arbitrage);

        // Prices sum to 105% - also arb
        state.set_market(
            "Test?".into(),
            vec!["A".into(), "B".into()],
            vec![dec!(0.55), dec!(0.50)],
        );
        assert!(state.has_arbitrage);
    }

    #[test]
    fn test_edges_calculation() {
        let mut state = MultiOptionState::new();
        state.set_market(
            "Test?".into(),
            vec!["A".into(), "B".into(), "C".into()],
            vec![dec!(0.50), dec!(0.30), dec!(0.20)],
        );
        let edges = state.edges();
        // Fair value = 1/3 ≈ 0.333
        // Edge for A = 0.333 - 0.50 = -0.167 (overpriced)
        // Edge for B = 0.333 - 0.30 = 0.033 (underpriced)
        // Edge for C = 0.333 - 0.20 = 0.133 (underpriced)
        assert!(edges[0] < Decimal::ZERO);
        assert!(edges[1] > Decimal::ZERO);
        assert!(edges[2] > Decimal::ZERO);
    }

    #[test]
    fn test_navigation() {
        let mut state = MultiOptionState::new();
        state.set_market(
            "Test?".into(),
            vec!["A".into(), "B".into(), "C".into()],
            vec![dec!(0.4), dec!(0.35), dec!(0.25)],
        );

        state.select_next();
        assert_eq!(state.selected, Some(0));

        state.select_next();
        assert_eq!(state.selected, Some(1));

        state.select_next();
        assert_eq!(state.selected, Some(2));

        state.select_next(); // Wrap to 0
        assert_eq!(state.selected, Some(0));

        state.select_prev(); // Wrap to 2
        assert_eq!(state.selected, Some(2));
    }
}
