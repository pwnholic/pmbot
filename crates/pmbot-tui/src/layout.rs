use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Computed layout areas for the TUI dashboard.
///
/// ```text
/// +-- Market ----+-- Strategy ----+-- PnL --------+
/// |  (compact)   |                |               |
/// +--------------+----------------+---------------+
/// +-- Orderbook --------+-- Positions ------------+
/// |                     |                         |
/// |                     |                         |
/// +---------------------+-------------------------+
/// +-- Log -----------------------------------------+
/// |                                                |
/// +------------------------------------------------+
/// ```
pub struct AppLayout {
    /// Market info panel (top-left).
    pub market_info: Rect,
    /// Strategy metrics panel (top-center).
    pub strategy_panel: Rect,
    /// Risk / PnL sparkline panel (top-right).
    pub risk_panel: Rect,
    /// Orderbook depth panel (middle-left).
    pub orderbook: Rect,
    /// Positions table (middle-right).
    pub positions: Rect,
    /// Scrolling log panel (bottom, full width).
    pub log_panel: Rect,
}

impl AppLayout {
    /// Compute layout areas for the given terminal size.
    pub fn new(area: Rect) -> Self {
        // Vertical split: top row (20%), middle row (55%), log (25%)
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(55),
                Constraint::Percentage(25),
            ])
            .split(area);

        // Top row: market (30%) | strategy (40%) | pnl (30%)
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(40),
                Constraint::Percentage(30),
            ])
            .split(vertical[0]);

        // Middle row: orderbook (45%) | positions (55%)
        let middle = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(vertical[1]);

        Self {
            market_info: top[0],
            strategy_panel: top[1],
            risk_panel: top[2],
            orderbook: middle[0],
            positions: middle[1],
            log_panel: vertical[2],
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
