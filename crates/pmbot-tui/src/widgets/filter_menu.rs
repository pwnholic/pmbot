use pmbot_market::discovery::DiscoveryFilters;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Widget},
};
use rust_decimal::Decimal;
use std::collections::HashMap;

/// Available market categories.
pub const CATEGORIES: &[&str] = &[
    "All",
    "Politics",
    "Sports",
    "Crypto",
    "Finance",
    "Geopolitics",
];

/// Filter menu tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterTab {
    Category,
    Liquidity,
    Tags,
    Strategy,
}

impl FilterTab {
    fn all() -> &'static [FilterTab] {
        &[
            FilterTab::Category,
            FilterTab::Liquidity,
            FilterTab::Tags,
            FilterTab::Strategy,
        ]
    }

    fn name(&self) -> &'static str {
        match self {
            FilterTab::Category => "Category",
            FilterTab::Liquidity => "Liquidity",
            FilterTab::Tags => "Tags",
            FilterTab::Strategy => "Strategy",
        }
    }

    fn next(&self) -> FilterTab {
        let all = Self::all();
        let idx = all.iter().position(|t| t == self).unwrap_or(0);
        all[(idx + 1) % all.len()].clone()
    }

    fn prev(&self) -> FilterTab {
        let all = Self::all();
        let idx = all.iter().position(|t| t == self).unwrap_or(0);
        all[(idx + all.len() - 1) % all.len()].clone()
    }
}

#[derive(Debug, Clone, Default)]
pub struct FilterState {
    pub selected_category: usize,
    pub min_liquidity: Option<String>,
    pub max_liquidity: Option<String>,
    pub exclude_tags: Vec<String>,
    pub selected_strategy: Option<String>,
    pub cursor: usize,
    pub editing_field: Option<FilterField>,
    pub input_buffer: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterField {
    MinLiquidity,
    MaxLiquidity,
    TagInput,
}

impl Default for FilterMenu {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FilterMenu {
    pub state: FilterState,
    available_strategies: Vec<String>,
    current_tab: FilterTab,
}

impl FilterMenu {
    pub fn new() -> Self {
        Self {
            state: FilterState::default(),
            available_strategies: vec![
                "fair_value".to_string(),
                "lead_lag".to_string(),
                "flash_crash".to_string(),
                "book_imbalance".to_string(),
                "negrisk_arb".to_string(),
                "convergence".to_string(),
                "market_maker".to_string(),
            ],
            current_tab: FilterTab::Category,
        }
    }

    pub fn current_tab(&self) -> FilterTab {
        self.current_tab
    }

    pub fn next_tab(&mut self) {
        self.current_tab = self.current_tab.next();
        self.state.cursor = 0;
    }

    pub fn prev_tab(&mut self) {
        self.current_tab = self.current_tab.prev();
        self.state.cursor = 0;
    }

    pub fn current_category(&self) -> &str {
        CATEGORIES
            .get(self.state.selected_category)
            .unwrap_or(&"All")
    }

    pub fn cursor_up(&mut self) {
        match self.current_tab {
            FilterTab::Category => {
                if self.state.cursor > 0 {
                    self.state.cursor -= 1;
                }
            }
            FilterTab::Liquidity | FilterTab::Tags => {
                if self.state.cursor > 0 {
                    self.state.cursor -= 1;
                }
            }
            FilterTab::Strategy => {
                if self.state.cursor > 0 {
                    self.state.cursor -= 1;
                }
            }
        }
    }

    pub fn cursor_down(&mut self) {
        match self.current_tab {
            FilterTab::Category => {
                if self.state.cursor < CATEGORIES.len() - 1 {
                    self.state.cursor += 1;
                }
            }
            FilterTab::Liquidity => {
                if self.state.cursor < 1 {
                    self.state.cursor += 1;
                }
            }
            FilterTab::Tags => {
                // Can go down to add new tag (field index1)
                if self.state.cursor < 1 {
                    self.state.cursor += 1;
                }
            }
            FilterTab::Strategy => {
                if self.state.cursor < self.available_strategies.len() - 1 {
                    self.state.cursor += 1;
                }
            }
        }
    }

    pub fn select_current(&mut self) {
        match self.current_tab {
            FilterTab::Category => {
                self.state.selected_category = self.state.cursor;
            }
            FilterTab::Liquidity | FilterTab::Tags => {
                self.toggle_editing();
            }
            FilterTab::Strategy => {
                self.toggle_strategy();
            }
        }
    }

    pub fn toggle_editing(&mut self) {
        if self.current_tab != FilterTab::Liquidity && self.current_tab != FilterTab::Tags {
            return;
        }

        let field = match self.current_tab {
            FilterTab::Liquidity => {
                if self.state.cursor == 0 {
                    Some(FilterField::MinLiquidity)
                } else {
                    Some(FilterField::MaxLiquidity)
                }
            }
            FilterTab::Tags => Some(FilterField::TagInput),
            _ => None,
        };

        if let Some(f) = field {
            if self.state.editing_field == Some(f) {
                self.state.editing_field = None;
                match f {
                    FilterField::MinLiquidity => {
                        self.state.min_liquidity = Some(self.state.input_buffer.clone());
                        self.state.input_buffer.clear();
                    }
                    FilterField::MaxLiquidity => {
                        self.state.max_liquidity = Some(self.state.input_buffer.clone());
                        self.state.input_buffer.clear();
                    }
                    FilterField::TagInput => {
                        if !self.state.input_buffer.is_empty() {
                            self.state
                                .exclude_tags
                                .push(self.state.input_buffer.clone());
                            self.state.input_buffer.clear();
                        }
                    }
                }
            } else {
                self.state.editing_field = Some(f);
                self.state.input_buffer = match f {
                    FilterField::MinLiquidity => {
                        self.state.min_liquidity.clone().unwrap_or_default()
                    }
                    FilterField::MaxLiquidity => {
                        self.state.max_liquidity.clone().unwrap_or_default()
                    }
                    FilterField::TagInput => String::new(),
                };
            }
        }
    }

    pub fn handle_char(&mut self, c: char) {
        if self.state.editing_field.is_some() {
            self.state.input_buffer.push(c);
        }
    }

    pub fn handle_backspace(&mut self) {
        if self.state.editing_field.is_some() {
            self.state.input_buffer.pop();
        }
    }

    pub fn remove_tag(&mut self) {
        if self.current_tab == FilterTab::Tags && !self.state.exclude_tags.is_empty() {
            self.state.exclude_tags.pop();
        }
    }

    pub fn toggle_strategy(&mut self) {
        if self.current_tab != FilterTab::Strategy {
            return;
        }
        let strategy_idx = self.state.cursor;
        if strategy_idx < self.available_strategies.len() {
            let strategy = self.available_strategies[strategy_idx].clone();
            self.state.selected_strategy =
                if self.state.selected_strategy.as_ref() == Some(&strategy) {
                    None
                } else {
                    Some(strategy)
                };
        }
    }

    pub fn to_filters(&self) -> DiscoveryFilters {
        let categories = if self.state.selected_category == 0 {
            vec![]
        } else {
            vec![self.current_category().to_lowercase()]
        };

        DiscoveryFilters {
            categories,
            tags: HashMap::new(),
            search_queries: vec![],
            exclude_tags: self.state.exclude_tags.clone(),
            min_liquidity: self
                .state
                .min_liquidity
                .as_ref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::ZERO),
            min_volume: Decimal::ZERO,
            active_only: true,
        }
    }

    pub fn render(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let items: Vec<ListItem> = self.build_menu_items(area.width as usize);

        let block = Block::default()
            .title(" Filter Menu ")
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        Widget::render(List::new(items).block(block), area, buf);
    }

    fn build_menu_items(&self, width: usize) -> Vec<ListItem<'_>> {
        let mut items = Vec::new();
        let sep_width = width.saturating_sub(2).max(1);

        // Tab bar
        let tab_line = Line::from(vec![
            Span::styled(" ", Style::default()),
            self.tab_span(FilterTab::Category),
            Span::styled(" ", Style::default()),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(" ", Style::default()),
            self.tab_span(FilterTab::Liquidity),
            Span::styled(" ", Style::default()),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(" ", Style::default()),
            self.tab_span(FilterTab::Tags),
            Span::styled(" ", Style::default()),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(" ", Style::default()),
            self.tab_span(FilterTab::Strategy),
        ]);
        items.push(ListItem::new(tab_line));
        items.push(ListItem::new(Line::styled(
            "─".repeat(sep_width),
            Style::default().fg(Color::DarkGray),
        )));

        // Tab content
        match self.current_tab {
            FilterTab::Category => self.render_category_tab(&mut items),
            FilterTab::Liquidity => self.render_liquidity_tab(&mut items),
            FilterTab::Tags => self.render_tags_tab(&mut items),
            FilterTab::Strategy => self.render_strategy_tab(&mut items),
        }

        // Navigation help - all on one line
        items.push(ListItem::new(Line::styled(
            "─".repeat(sep_width),
            Style::default().fg(Color::DarkGray),
        )));
        items.push(ListItem::new(Line::from(vec![
            Span::styled("←/→", Style::default().fg(Color::Yellow)),
            Span::styled(" Tab", Style::default().fg(Color::Gray)),
            Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
            Span::styled("↑/↓", Style::default().fg(Color::Yellow)),
            Span::styled(" Nav", Style::default().fg(Color::Gray)),
            Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::styled(" Select", Style::default().fg(Color::Gray)),
            Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::styled(" Back", Style::default().fg(Color::Gray)),
            Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
            Span::styled("a", Style::default().fg(Color::Yellow)),
            Span::styled(" Apply", Style::default().fg(Color::Gray)),
        ])));

        items
    }

    fn tab_span(&self, tab: FilterTab) -> Span<'_> {
        let is_active = self.current_tab == tab;
        let style = if is_active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        Span::styled(tab.name(), style)
    }

    fn render_category_tab(&self, items: &mut Vec<ListItem<'_>>) {
        items.push(ListItem::new(Line::styled(
            "Select market category:",
            Style::default().fg(Color::White),
        )));
        items.push(ListItem::new(Line::from("")));

        for (i, category) in CATEGORIES.iter().enumerate() {
            let is_selected = i == self.state.selected_category;
            let is_cursor = i == self.state.cursor;

            let style = if is_selected && is_cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else if is_cursor {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };

            let marker = if is_selected { "●" } else { "○" };
            items.push(ListItem::new(Line::styled(
                format!("  {} {}", marker, category),
                style,
            )));
        }
    }

    fn render_liquidity_tab(&self, items: &mut Vec<ListItem<'_>>) {
        items.push(ListItem::new(Line::styled(
            "Filter by liquidity (USD):",
            Style::default().fg(Color::White),
        )));
        items.push(ListItem::new(Line::from("")));

        // Min Liquidity
        let min_style = if self.state.editing_field == Some(FilterField::MinLiquidity) {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if self.state.cursor == 0 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Gray)
        };
        let min_val = if self.state.editing_field == Some(FilterField::MinLiquidity) {
            format!("{}_", self.state.input_buffer)
        } else {
            self.state
                .min_liquidity
                .clone()
                .unwrap_or_else(|| "─".repeat(10))
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled("  Min: $", Style::default().fg(Color::White)),
            Span::styled(min_val, min_style),
        ])));

        // Max Liquidity
        let max_style = if self.state.editing_field == Some(FilterField::MaxLiquidity) {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if self.state.cursor == 1 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Gray)
        };
        let max_val = if self.state.editing_field == Some(FilterField::MaxLiquidity) {
            format!("{}_", self.state.input_buffer)
        } else {
            self.state
                .max_liquidity
                .clone()
                .unwrap_or_else(|| "─".repeat(10))
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled("  Max: $", Style::default().fg(Color::White)),
            Span::styled(max_val, max_style),
        ])));

        if self.state.editing_field.is_some() {
            items.push(ListItem::new(Line::from("")));
            items.push(ListItem::new(Line::styled(
                "  Type value and press Enter",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    fn render_tags_tab(&self, items: &mut Vec<ListItem<'_>>) {
        items.push(ListItem::new(Line::styled(
            "Exclude tags from discovery:",
            Style::default().fg(Color::White),
        )));
        items.push(ListItem::new(Line::from("")));

        // Input field
        let input_style = if self.state.editing_field == Some(FilterField::TagInput) {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if self.state.cursor == 0 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Gray)
        };

        let input_val = if self.state.editing_field == Some(FilterField::TagInput) {
            format!("{}_", self.state.input_buffer)
        } else {
            "Type tag and press Enter".to_string()
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled("  Add: ", Style::default().fg(Color::White)),
            Span::styled(input_val, input_style),
        ])));

        // Existing tags
        if !self.state.exclude_tags.is_empty() {
            items.push(ListItem::new(Line::from("")));
            items.push(ListItem::new(Line::styled(
                "  Excluded tags:",
                Style::default().fg(Color::White),
            )));
            for tag in &self.state.exclude_tags {
                items.push(ListItem::new(Line::styled(
                    format!("    ✗ {}", tag),
                    Style::default().fg(Color::Red),
                )));
            }
            items.push(ListItem::new(Line::from("")));
            items.push(ListItem::new(Line::styled(
                "  Press 'd' to delete last tag",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    fn render_strategy_tab(&self, items: &mut Vec<ListItem<'_>>) {
        items.push(ListItem::new(Line::styled(
            "Select trading strategy:",
            Style::default().fg(Color::White),
        )));
        items.push(ListItem::new(Line::from("")));

        for (i, strategy) in self.available_strategies.iter().enumerate() {
            let is_selected = self.state.selected_strategy.as_ref() == Some(strategy);
            let is_cursor = i == self.state.cursor;

            let style = if is_selected && is_cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else if is_cursor {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };

            let marker = if is_selected { "●" } else { "○" };
            items.push(ListItem::new(Line::styled(
                format!("  {} {}", marker, strategy),
                style,
            )));
        }

        items.push(ListItem::new(Line::from("")));
        items.push(ListItem::new(Line::styled(
            "  Press Space to toggle selection",
            Style::default().fg(Color::DarkGray),
        )));
    }
}
