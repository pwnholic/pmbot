//! Trade execution view widget.
//!
//! Displays trade configuration UI with risk summary.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::theme;

/// Order type for trade execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Gtc,
    Gtd,
    Fok,
    Fak,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::Gtc => write!(f, "GTC"),
            OrderType::Gtd => write!(f, "GTD"),
            OrderType::Fok => write!(f, "FOK"),
            OrderType::Fak => write!(f, "FAK"),
        }
    }
}

/// State for the trade execution widget.
#[derive(Debug, Clone)]
pub struct TradeExecutionState {
    /// Selected market question.
    pub market_question: Option<String>,
    /// Available outcomes.
    pub outcomes: Vec<String>,
    /// Selected outcome index.
    pub selected_outcome: Option<usize>,
    /// Current prices for each outcome.
    pub outcome_prices: Vec<Decimal>,
    /// Size input string.
    pub size_input: String,
    /// Price input string.
    pub price_input: String,
    /// Selected order type.
    pub order_type: OrderType,
    /// Current field being edited.
    pub editing_field: Option<EditField>,
    /// Whether confirmation dialog is shown.
    pub confirmation_shown: bool,
}

/// Field being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditField {
    Size,
    Price,
}

impl Default for TradeExecutionState {
    fn default() -> Self {
        Self {
            market_question: None,
            outcomes: Vec::new(),
            selected_outcome: None,
            outcome_prices: Vec::new(),
            size_input: String::new(),
            price_input: String::new(),
            order_type: OrderType::Gtc,
            editing_field: None,
            confirmation_shown: false,
        }
    }
}

impl TradeExecutionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_market(&mut self, question: String, outcomes: Vec<String>, prices: Vec<Decimal>) {
        self.market_question = Some(question);
        self.outcomes = outcomes;
        self.outcome_prices = prices;
        self.selected_outcome = None;
        self.size_input.clear();
        self.price_input.clear();
    }

    pub fn select_outcome(&mut self, index: usize) {
        if index < self.outcomes.len() {
            self.selected_outcome = Some(index);
            if let Some(&price) = self.outcome_prices.get(index) {
                self.price_input = format!("{:.4}", price);
            }
        }
    }

    pub fn get_size(&self) -> Option<Decimal> {
        self.size_input.parse().ok()
    }

    pub fn get_price(&self) -> Option<Decimal> {
        self.price_input.parse().ok()
    }

    /// Calculate max loss (100% of position).
    pub fn max_loss(&self) -> Option<Decimal> {
        let size = self.get_size()?;
        Some(size)
    }

    /// Calculate max gain (if position goes to $1).
    pub fn max_gain(&self) -> Option<Decimal> {
        let size = self.get_size()?;
        let price = self.get_price()?;
        if price <= Decimal::ZERO || price >= Decimal::ONE {
            return None;
        }
        // Max payout = size / price, Gain = payout - cost = size / price - size = size * (1/price - 1)
        Some(size * (Decimal::ONE / price - Decimal::ONE))
    }

    /// Calculate expected return (simplified: assume50% edge).
    pub fn expected_return(&self) -> Option<Decimal> {
        let max_gain = self.max_gain()?;
        let max_loss = self.max_loss()?;
        // Assume 50% win probability for expected value calculation
        Some(max_gain * dec!(0.5) - max_loss * dec!(0.5))
    }

    pub fn handle_char(&mut self, c: char) {
        if let Some(field) = self.editing_field {
            match field {
                EditField::Size => {
                    if c.is_ascii_digit() || c == '.' {
                        self.size_input.push(c);
                    }
                }
                EditField::Price => {
                    if c.is_ascii_digit() || c == '.' {
                        self.price_input.push(c);
                    }
                }
            }
        }
    }

    pub fn handle_backspace(&mut self) {
        if let Some(field) = self.editing_field {
            match field {
                EditField::Size => {
                    self.size_input.pop();
                }
                EditField::Price => {
                    self.price_input.pop();
                }
            }
        }
    }

    pub fn cycle_order_type(&mut self) {
        self.order_type = match self.order_type {
            OrderType::Gtc => OrderType::Gtd,
            OrderType::Gtd => OrderType::Fok,
            OrderType::Fok => OrderType::Fak,
            OrderType::Fak => OrderType::Gtc,
        };
    }
}

/// Widget that renders trade execution view.
pub struct TradeExecutionWidget<'a> {
    state: &'a TradeExecutionState,
}

impl<'a> TradeExecutionWidget<'a> {
    pub fn new(state: &'a TradeExecutionState) -> Self {
        Self { state }
    }
}

impl Widget for TradeExecutionWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Execute Trade ")
            .title_style(theme::title_style())
            .borders(Borders::ALL)
            .border_style(theme::border_style())
            .style(Style::default().bg(theme::BG_DARK));
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines: Vec<Line<'_>> = Vec::new();

        // Market question
        if let Some(ref question) = self.state.market_question {
            lines.push(Line::from(vec![
                Span::styled("Market: ", theme::label()),
                Span::styled(
                    if question.len() > 50 {
                        &question[..50]
                    } else {
                        question.as_str()
                    },
                    Style::default().fg(theme::CYAN),
                ),
            ]));
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(vec![Span::styled(
                "No market selected",
                Style::default().fg(theme::FG_DIM),
            )]));
            let paragraph = Paragraph::new(lines);
            paragraph.render(inner, buf);
            return;
        }

        // Outcomes
        lines.push(Line::from(vec![Span::styled("Outcomes:", theme::label())]));
        for (i, outcome) in self.state.outcomes.iter().enumerate() {
            let price = self
                .state
                .outcome_prices
                .get(i)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let is_selected = self.state.selected_outcome == Some(i);
            let style = if is_selected {
                Style::default()
                    .fg(theme::BLACK)
                    .bg(theme::CYAN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::FG_DIM)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    if is_selected { "► " } else { "  " },
                    Style::default().fg(theme::YELLOW),
                ),
                Span::styled(format!("{}: ${:.4}", outcome, price), style),
            ]));
        }
        lines.push(Line::from(""));

        // Trade configuration
        let size_style = if self.state.editing_field == Some(EditField::Size) {
            Style::default().fg(theme::BLACK).bg(theme::YELLOW)
        } else {
            Style::default().fg(theme::WHITE)
        };
        let price_style = if self.state.editing_field == Some(EditField::Price) {
            Style::default().fg(theme::BLACK).bg(theme::YELLOW)
        } else {
            Style::default().fg(theme::WHITE)
        };

        lines.push(Line::from(vec![
            Span::styled("Size: ", theme::label()),
            Span::styled(
                format!(
                    "{}{}_",
                    self.state.size_input,
                    if self.state.editing_field == Some(EditField::Size) {
                        ""
                    } else {
                        ""
                    }
                ),
                size_style,
            ),
            Span::styled(" [s] to edit", Style::default().fg(theme::FG_DIM)),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Price:", theme::label()),
            Span::styled(
                format!(
                    " {}{}",
                    self.state.price_input,
                    if self.state.editing_field == Some(EditField::Price) {
                        ""
                    } else {
                        ""
                    }
                ),
                price_style,
            ),
            Span::styled(" [p] to edit", Style::default().fg(theme::FG_DIM)),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Type: ", theme::label()),
            Span::styled(
                format!(" {} ", self.state.order_type),
                Style::default().fg(theme::ORANGE),
            ),
            Span::styled("[t] to cycle", Style::default().fg(theme::FG_DIM)),
        ]));
        lines.push(Line::from(""));

        // Risk summary
        if let (Some(size), Some(price)) = (self.state.get_size(), self.state.get_price()) {
            if size > Decimal::ZERO && price > Decimal::ZERO && price < Decimal::ONE {
                let max_loss = self.state.max_loss().unwrap_or(Decimal::ZERO);
                let max_gain = self.state.max_gain().unwrap_or(Decimal::ZERO);

                lines.push(Line::from(vec![Span::styled(
                    "─── Risk Summary ───",
                    Style::default().fg(theme::YELLOW),
                )]));

                lines.push(Line::from(vec![
                    Span::styled("Max Loss:  ", theme::label()),
                    Span::styled(format!("${:.2}", max_loss), Style::default().fg(theme::RED)),
                ]));

                lines.push(Line::from(vec![
                    Span::styled("Max Gain:  ", theme::label()),
                    Span::styled(
                        format!("${:.2}", max_gain),
                        Style::default().fg(theme::GREEN),
                    ),
                ]));

                // Edge calculation
                let edge = max_gain * dec!(0.5) - max_loss * dec!(0.5);
                let edge_color = if edge >= Decimal::ZERO {
                    theme::GREEN
                } else {
                    theme::RED
                };
                lines.push(Line::from(vec![
                    Span::styled("EV:        ", theme::label()),
                    Span::styled(format!("${:.2}", edge), Style::default().fg(edge_color)),
                ]));
            }
        }

        // Instructions
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("[Enter] Confirm", Style::default().fg(theme::CYAN)),
            Span::styled("  [Esc] Cancel", Style::default().fg(theme::FG_DIM)),
        ]));

        // Confirmation dialog
        if self.state.confirmation_shown {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Confirm trade? [y/N]",
                Style::default()
                    .fg(theme::YELLOW)
                    .add_modifier(Modifier::BOLD),
            )]));
        }

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trade_state_defaults() {
        let state = TradeExecutionState::new();
        assert!(state.market_question.is_none());
        assert!(state.outcomes.is_empty());
        assert!(state.selected_outcome.is_none());
    }

    #[test]
    fn test_set_market() {
        let mut state = TradeExecutionState::new();
        state.set_market(
            "Test question?".into(),
            vec!["Yes".into(), "No".into()],
            vec![dec!(0.65), dec!(0.35)],
        );
        assert_eq!(state.market_question, Some("Test question?".into()));
        assert_eq!(state.outcomes.len(), 2);
    }

    #[test]
    fn test_select_outcome() {
        let mut state = TradeExecutionState::new();
        state.set_market(
            "Test?".into(),
            vec!["Yes".into(), "No".into()],
            vec![dec!(0.65), dec!(0.35)],
        );
        state.select_outcome(0);
        assert_eq!(state.selected_outcome, Some(0));
        assert_eq!(state.price_input, "0.6500");
    }

    #[test]
    fn test_risk_calculations() {
        let mut state = TradeExecutionState::new();
        state.set_market(
            "Test?".into(),
            vec!["Yes".into(), "No".into()],
            vec![dec!(0.65), dec!(0.35)],
        );
        state.select_outcome(0);
        state.size_input = "100".into();
        state.price_input = "0.50".into();

        // Max loss = 100 (100% of position)
        assert_eq!(state.max_loss(), Some(dec!(100)));
        // Max gain = 100 * (1/0.5 - 1) = 100 * 1= 100
        assert_eq!(state.max_gain(), Some(dec!(100)));
    }

    #[test]
    fn test_cycle_order_type() {
        let mut state = TradeExecutionState::new();
        assert_eq!(state.order_type, OrderType::Gtc);
        state.cycle_order_type();
        assert_eq!(state.order_type, OrderType::Gtd);
        state.cycle_order_type();
        assert_eq!(state.order_type, OrderType::Fok);
        state.cycle_order_type();
        assert_eq!(state.order_type, OrderType::Fak);
        state.cycle_order_type();
        assert_eq!(state.order_type, OrderType::Gtc);
    }
}
