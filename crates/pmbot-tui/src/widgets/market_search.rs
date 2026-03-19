//! Market search widget for discovering markets.
//!
//! Provides interactive search with category filtering.

use pmbot_core::types::MarketInfo;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget},
};

/// Available market categories.
pub const CATEGORIES: &[&str] = &[
    "All",
    "Politics",
    "Sports",
    "Crypto",
    "Finance",
    "Geopolitics",
];

/// Market search widget state.
#[derive(Debug, Clone)]
pub struct MarketSearchState {
    /// Search query string.
    pub query: String,
    /// Selected category index.
    pub selected_category: usize,
    /// Markets matching the search.
    pub markets: Vec<MarketInfo>,
    /// Cursor position in the market list.
    pub cursor: usize,
    /// Scroll offset for the market list.
    pub scroll_offset: usize,
    /// Whether the search box is focused.
    pub focused: bool,
}

impl Default for MarketSearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            selected_category: 0,
            markets: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            focused: false,
        }
    }
}

impl MarketSearchState {
    /// Create new search state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current category.
    pub fn current_category(&self) -> &str {
        CATEGORIES.get(self.selected_category).unwrap_or(&"All")
    }

    /// Get the currently selected market.
    pub fn selected_market(&self) -> Option<&MarketInfo> {
        self.markets.get(self.cursor)
    }

    /// Move cursor up.
    pub fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    /// Move cursor down.
    pub fn cursor_down(&mut self) {
        if self.cursor < self.markets.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    /// Move to next category.
    pub fn next_category(&mut self) {
        self.selected_category = (self.selected_category + 1) % CATEGORIES.len();
    }

    /// Move to previous category.
    pub fn prev_category(&mut self) {
        self.selected_category = if self.selected_category == 0 {
            CATEGORIES.len() - 1
        } else {
            self.selected_category - 1
        };
    }

    /// Add character to search query.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
    }

    /// Remove last character from search query.
    pub fn pop_char(&mut self) {
        self.query.pop();
    }

    /// Clear search query.
    pub fn clear_query(&mut self) {
        self.query.clear();
    }

    /// Update markets list.
    pub fn set_markets(&mut self, markets: Vec<MarketInfo>) {
        self.markets = markets;
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    fn adjust_scroll(&mut self) {
        let visible_height = 10;
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor - visible_height + 1;
        }
    }
}

/// Market search widget.
pub struct MarketSearchWidget<'a> {
    state: &'a MarketSearchState,
}

impl<'a> MarketSearchWidget<'a> {
    /// Create new widget with the given state.
    pub fn new(state: &'a MarketSearchState) -> Self {
        Self { state }
    }

    /// Render the search box.
    fn render_search_box(&self, area: Rect, buf: &mut Buffer) {
        let style = if self.state.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };

        let title = if self.state.focused {
            " Search Markets (press / to unfocus) "
        } else {
            " Search Markets (press / to focus) "
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(style);

        let inner = block.inner(area);
        block.render(area, buf);

        let query_display = if self.state.query.is_empty() {
            "Type to search..."
        } else {
            &self.state.query
        };

        Paragraph::new(query_display)
            .style(if self.state.focused {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .render(inner, buf);
    }

    /// Render category tabs.
    fn render_categories(&self, area: Rect, buf: &mut Buffer) {
        let tab_width = area.width as usize / CATEGORIES.len().max(1);

        for (i, category) in CATEGORIES.iter().enumerate() {
            let is_selected = i == self.state.selected_category;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            let x = area.x + (i * tab_width) as u16;
            if x < area.x + area.width {
                let text = if category.len() > tab_width.saturating_sub(2) {
                    &category[..tab_width.saturating_sub(2)]
                } else {
                    category
                };
                buf.set_string(x, area.y, &format!(" {} ", text), style);
            }
        }
    }

    /// Render market list.
    fn render_market_list(&self, area: Rect, buf: &mut Buffer) {
        let rows: Vec<Row> = self
            .state
            .markets
            .iter()
            .skip(self.state.scroll_offset)
            .enumerate()
            .map(|(i, market)| {
                let is_selected = i + self.state.scroll_offset == self.state.cursor;
                let style = if is_selected {
                    Style::default().fg(Color::Black).bg(Color::Yellow)
                } else {
                    Style::default()
                };

                let question = if market.question.len() > 40 {
                    format!("{}...", &market.question[..37])
                } else {
                    market.question.clone()
                };

                let category = market
                    .slug
                    .split('-')
                    .next()
                    .unwrap_or("unknown")
                    .to_string();

                Row::new(vec![
                    Cell::from(question),
                    Cell::from(category),
                    Cell::from(format!("${:.0}", market.liquidity)),
                    Cell::from(format!("${:.0}", market.volume)),
                ])
                .style(style)
            })
            .collect();

        Table::new(
            rows,
            &[
                Constraint::Percentage(50),
                Constraint::Percentage(20),
                Constraint::Percentage(15),
                Constraint::Percentage(15),
            ],
        )
        .header(
            Row::new(vec!["Question", "Category", "Liquidity", "Volume"]).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::default()
                .title(format!(" {} markets ", self.state.markets.len()))
                .borders(Borders::ALL),
        )
        .render(area, buf);
    }

    /// Render selected market details.
    fn render_details(&self, area: Rect, buf: &mut Buffer) {
        let details = if let Some(market) = self.state.selected_market() {
            let outcomes: String = market
                .outcomes
                .iter()
                .map(|o| format!(" [{}]", o))
                .collect();
            format!("Selected: {}\nOutcomes: {}", market.question, outcomes)
        } else {
            "No market selected".into()
        };

        Paragraph::new(details)
            .style(Style::default().fg(Color::Gray))
            .render(area, buf);
    }
}

impl<'a> Widget for MarketSearchWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let search_height = 3;
        let categories_height = 1;
        let details_height = 2;

        let search_area = Rect::new(area.x, area.y, area.width, search_height);
        let categories_area = Rect::new(
            area.x,
            area.y + search_height,
            area.width,
            categories_height,
        );
        let list_area = Rect::new(
            area.x,
            area.y + search_height + categories_height,
            area.width,
            area.height
                .saturating_sub(search_height + categories_height + details_height),
        );
        let details_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(details_height),
            area.width,
            details_height,
        );

        self.render_search_box(search_area, buf);
        self.render_categories(categories_area, buf);
        self.render_market_list(list_area, buf);
        self.render_details(details_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pmbot_core::types::{MarketId, TokenId};
    use rust_decimal::Decimal;
    use std::collections::HashMap;

    fn make_market(id: &str, question: &str) -> MarketInfo {
        let mut outcome_prices = HashMap::new();
        outcome_prices.insert("Yes".to_string(), Decimal::from(50));
        outcome_prices.insert("No".to_string(), Decimal::from(50));

        MarketInfo {
            id: MarketId(id.into()),
            question: question.into(),
            slug: format!("{}-test", id),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![
                TokenId(format!("{}-yes", id)),
                TokenId(format!("{}-no", id)),
            ],
            outcome_prices,
            condition_id: format!("cond-{}", id),
            neg_risk: false,
            active: true,
            end_date: Some(Utc::now() + chrono::Duration::hours(1)),
            liquidity: Decimal::from(10000),
            volume: Decimal::from(50000),
            category: "Test".into(),
            tags: vec!["test".into()],
        }
    }

    #[test]
    fn test_state_navigation() {
        let mut state = MarketSearchState::new();
        state.set_markets(vec![
            make_market("m1", "Market 1"),
            make_market("m2", "Market 2"),
            make_market("m3", "Market 3"),
        ]);

        assert_eq!(state.cursor, 0);

        state.cursor_down();
        assert_eq!(state.cursor, 1);

        state.cursor_down();
        assert_eq!(state.cursor, 2);

        state.cursor_down(); // Should stay at 2
        assert_eq!(state.cursor, 2);

        state.cursor_up();
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn test_category_navigation() {
        let mut state = MarketSearchState::new();

        assert_eq!(state.current_category(), "All");

        state.next_category();
        assert_eq!(state.current_category(), "Politics");

        state.prev_category();
        assert_eq!(state.current_category(), "All");
    }

    #[test]
    fn test_query_editing() {
        let mut state = MarketSearchState::new();

        state.push_char('B');
        state.push_char('T');
        state.push_char('C');
        assert_eq!(state.query, "BTC");

        state.pop_char();
        assert_eq!(state.query, "BT");

        state.clear_query();
        assert_eq!(state.query, "");
    }

    #[test]
    fn test_selected_market() {
        let mut state = MarketSearchState::new();
        state.set_markets(vec![
            make_market("m1", "Market 1"),
            make_market("m2", "Market 2"),
        ]);

        assert_eq!(state.selected_market().unwrap().id.0, "m1");

        state.cursor_down();
        assert_eq!(state.selected_market().unwrap().id.0, "m2");
    }
}
