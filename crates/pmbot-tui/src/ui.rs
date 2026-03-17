use ratatui::Frame;

use crate::widgets::{
    log::LogWidget, market::MarketWidget, orderbook::OrderbookWidget, pnl::PnlWidget,
    positions::PositionsWidget, strategy::StrategyWidget,
};
use crate::{App, AppLayout};

/// Draw the entire TUI application dashboard to the frame.
pub fn draw(f: &mut Frame, app: &mut App) {
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
