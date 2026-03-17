use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Computed layout areas for the TUI dashboard.
///
/// ```text
/// +-- Market -----------+-- PnL ------------+
/// |  (compact)           |                   |
/// +---------------------+--------------------+
/// +-- Orderbook --------+-- Positions -------+
/// |                     |                    |
/// |                     |                    |
/// +---------------------+--------------------+
/// +-- Log --------------+-- Strategy --------+
/// |                     |                    |
/// +---------------------+--------------------+
/// ```
pub struct AppLayout {
    /// Market info panel (top-left).
    pub market_info: Rect,
    /// Strategy metrics panel (bottom-right).
    pub strategy_panel: Rect,
    /// Risk / PnL panel (top-right).
    pub risk_panel: Rect,
    /// Orderbook depth panel (middle-left).
    pub orderbook: Rect,
    /// Positions table (middle-right).
    pub positions: Rect,
    /// Scrolling log panel (bottom-left).
    pub log_panel: Rect,
}

impl AppLayout {
    /// Compute layout areas for the given terminal size.
    pub fn new(area: Rect) -> Self {
        // Vertical split: top row (18%), middle row (47%), bottom row (35%)
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(18),
                Constraint::Percentage(47),
                Constraint::Percentage(35),
            ])
            .split(area);

        // Top row: market (50%) | pnl (50%)
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(vertical[0]);

        // Middle row: orderbook (45%) | positions (55%)
        let middle = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(vertical[1]);

        // Bottom row: log (65%) | strategy (35%)
        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(vertical[2]);

        Self {
            market_info: top[0],
            strategy_panel: bottom[1],
            risk_panel: top[1],
            orderbook: middle[0],
            positions: middle[1],
            log_panel: bottom[0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_areas_do_not_overlap() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = AppLayout::new(area);

        let panels = [
            layout.market_info,
            layout.strategy_panel,
            layout.risk_panel,
            layout.orderbook,
            layout.positions,
            layout.log_panel,
        ];

        // Check no two panels overlap
        for i in 0..panels.len() {
            for j in (i + 1)..panels.len() {
                let a = panels[i];
                let b = panels[j];
                let overlap_x = a.x < b.x + b.width && b.x < a.x + a.width;
                let overlap_y = a.y < b.y + b.height && b.y < a.y + a.height;
                assert!(
                    !(overlap_x && overlap_y),
                    "Panels {i} and {j} overlap: {a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn layout_panels_fit_within_area() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = AppLayout::new(area);

        let panels = [
            layout.market_info,
            layout.strategy_panel,
            layout.risk_panel,
            layout.orderbook,
            layout.positions,
            layout.log_panel,
        ];

        for (i, panel) in panels.iter().enumerate() {
            assert!(
                panel.x + panel.width <= area.width && panel.y + panel.height <= area.height,
                "Panel {i} ({panel:?}) exceeds area bounds ({area:?})"
            );
        }
    }
}
