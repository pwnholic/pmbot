use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Clear,
    Frame,
};

use crate::widgets::{
    log::LogWidget, market::MarketWidget, market_search::MarketSearchWidget,
    orderbook::OrderbookWidget, pnl::PnlWidget, positions::PositionsWidget,
    strategy::StrategyWidget,
};
use crate::{App, AppLayout, AppMode};

/// Draw the entire TUI application dashboard to the frame.
pub fn draw(f: &mut Frame, app: &mut App) {
    match app.mode {
        AppMode::Dashboard => draw_dashboard(f, app),
        AppMode::Search => draw_search(f, app),
        AppMode::Filter => draw_filter(f, app),
    }
}

/// Draw the main dashboard view.
fn draw_dashboard(f: &mut Frame, app: &App) {
    let layout = AppLayout::new(f.area());

    let mut active_snap = None;
    let mut bids = &[][..];
    let mut asks = &[][..];

    if let Some(world) = &app.world {
        if let Some(market_id) = &world.active_market_id {
            if let Some(snap) = world.markets.get(market_id) {
                active_snap = Some(snap);
                bids = &snap.book.bids;
                asks = &snap.book.asks;
            }
        }
    }

    // Market info (top-left)
    let empty_latency = std::collections::HashMap::new();
    let latency = if let Some(world) = &app.world {
        &world.network_latency
    } else {
        &empty_latency
    };
    f.render_widget(MarketWidget::new(active_snap, latency), layout.market_info);

    // Strategy metrics (top-center)
    f.render_widget(
        StrategyWidget::new(&app.strategy_metrics),
        layout.strategy_panel,
    );

    // PnL (top-right)
    let (balance, daily_pnl) = if let Some(world) = &app.world {
        (world.balance, world.daily_pnl)
    } else {
        (rust_decimal::Decimal::ZERO, rust_decimal::Decimal::ZERO)
    };
    f.render_widget(PnlWidget::new(balance, daily_pnl), layout.risk_panel);

    // Orderbook (middle-left)
    let mut orderbook = OrderbookWidget::new(bids, asks);
    if let Some(snap) = active_snap {
        orderbook = orderbook.mid_price(snap.mid_price);
    }
    f.render_widget(orderbook.max_levels(12), layout.orderbook);

    // Positions (middle-right)
    let positions = if let Some(world) = &app.world {
        &world.positions[..]
    } else {
        &[][..]
    };
    f.render_widget(PositionsWidget::new(positions), layout.positions);

    // Logs (bottom)
    f.render_widget(LogWidget::new(&app.logs), layout.log_panel);
}

/// Draw the market search view.
fn draw_search(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(MarketSearchWidget::new(&app.search_state), area);
}

/// Draw the filter menu view as a floating overlay.
fn draw_filter(f: &mut Frame, app: &App) {
    // Draw the dashboard behind the filter
    draw_dashboard(f, app);

    // Clear the area for the filter modal
    let area = f.area();

    // Center the filter modal (70% width, 80% height)
    let modal_width = (area.width as f32 * 0.7) as u16;
    let modal_height = (area.height as f32 * 0.8) as u16;
    let modal_x = (area.width.saturating_sub(modal_width)) / 2;
    let modal_y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    // Clear the modal area
    f.render_widget(Clear, modal_area);

    // Render the filter menu
    app.filter_menu.render(modal_area, f.buffer_mut());
}
