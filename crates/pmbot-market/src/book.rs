use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use pmbot_core::math::book_imbalance;
use pmbot_core::types::{Level, MarketId, OrderbookSnapshot, Side, TokenId};

/// Local orderbook maintaining sorted bids (descending) and asks (ascending).
#[derive(Debug, Clone)]
pub struct LocalBook {
    market_id: MarketId,
    token_id: TokenId,
    bids: Vec<Level>,
    asks: Vec<Level>,
    last_update: DateTime<Utc>,
}

impl LocalBook {
    /// Create an empty book for the given market and token.
    pub fn new(market_id: MarketId, token_id: TokenId) -> Self {
        Self {
            market_id,
            token_id,
            bids: Vec::new(),
            asks: Vec::new(),
            last_update: Utc::now(),
        }
    }

    /// Replace the entire book with a new snapshot.
    pub fn apply_snapshot(&mut self, bids: Vec<Level>, asks: Vec<Level>, ts: DateTime<Utc>) {
        self.bids = bids;
        self.asks = asks;
        // Ensure sorting invariants
        self.bids
            .sort_by(|a, b| b.price.cmp(&a.price)); // descending
        self.asks
            .sort_by(|a, b| a.price.cmp(&b.price)); // ascending
        self.last_update = ts;
    }

    /// Apply incremental delta updates. A size of zero means remove that level.
    pub fn apply_delta(
        &mut self,
        bid_updates: &[Level],
        ask_updates: &[Level],
        ts: DateTime<Utc>,
    ) {
        for update in bid_updates {
            Self::apply_side_delta(&mut self.bids, update, true);
        }
        for update in ask_updates {
            Self::apply_side_delta(&mut self.asks, update, false);
        }
        self.last_update = ts;
    }

    /// Apply a single level update to one side of the book.
    /// `descending` = true for bids, false for asks.
    fn apply_side_delta(levels: &mut Vec<Level>, update: &Level, descending: bool) {
        // Find existing level at this price
        if let Some(pos) = levels.iter().position(|l| l.price == update.price) {
            if update.size.is_zero() {
                levels.remove(pos);
            } else {
                levels[pos].size = update.size;
            }
        } else if !update.size.is_zero() {
            // Insert in sorted position
            let insert_pos = if descending {
                levels
                    .iter()
                    .position(|l| l.price < update.price)
                    .unwrap_or(levels.len())
            } else {
                levels
                    .iter()
                    .position(|l| l.price > update.price)
                    .unwrap_or(levels.len())
            };
            levels.insert(insert_pos, *update);
        }
    }

    /// Return the current state as an immutable snapshot.
    pub fn snapshot(&self) -> OrderbookSnapshot {
        OrderbookSnapshot {
            market_id: self.market_id.clone(),
            token_id: self.token_id.clone(),
            bids: self.bids.clone(),
            asks: self.asks.clone(),
            timestamp: self.last_update,
        }
    }

    /// Compute book imbalance using the top N levels.
    pub fn imbalance(&self, levels: usize) -> Decimal {
        book_imbalance(&self.bids, &self.asks, levels)
    }

    /// Cumulative volume up to (and including) the given price on the specified side.
    ///
    /// For bids: sum sizes where level price >= target price.
    /// For asks: sum sizes where level price <= target price.
    pub fn depth_at_price(&self, price: Decimal, side: Side) -> Decimal {
        match side {
            Side::Buy => self
                .bids
                .iter()
                .take_while(|l| l.price >= price)
                .map(|l| l.size)
                .sum(),
            Side::Sell => self
                .asks
                .iter()
                .take_while(|l| l.price <= price)
                .map(|l| l.size)
                .sum(),
        }
    }

    /// Access to the market ID.
    pub fn market_id(&self) -> &MarketId {
        &self.market_id
    }

    /// Access to the token ID.
    pub fn token_id(&self) -> &TokenId {
        &self.token_id
    }

    /// Timestamp of the last update.
    pub fn last_update(&self) -> DateTime<Utc> {
        self.last_update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_book() -> LocalBook {
        LocalBook::new(
            MarketId("test-market".into()),
            TokenId("test-token".into()),
        )
    }

    fn level(price: Decimal, size: Decimal) -> Level {
        Level { price, size }
    }

    #[test]
    fn test_new_book_is_empty() {
        let book = make_book();
        let snap = book.snapshot();
        assert!(snap.bids.is_empty());
        assert!(snap.asks.is_empty());
    }

    #[test]
    fn test_apply_snapshot_sorts_correctly() {
        let mut book = make_book();
        let bids = vec![
            level(dec!(0.40), dec!(10)),
            level(dec!(0.50), dec!(20)),
            level(dec!(0.45), dec!(15)),
        ];
        let asks = vec![
            level(dec!(0.60), dec!(10)),
            level(dec!(0.55), dec!(20)),
            level(dec!(0.58), dec!(5)),
        ];
        let ts = Utc::now();
        book.apply_snapshot(bids, asks, ts);

        let snap = book.snapshot();
        // Bids sorted descending
        assert_eq!(snap.bids[0].price, dec!(0.50));
        assert_eq!(snap.bids[1].price, dec!(0.45));
        assert_eq!(snap.bids[2].price, dec!(0.40));
        // Asks sorted ascending
        assert_eq!(snap.asks[0].price, dec!(0.55));
        assert_eq!(snap.asks[1].price, dec!(0.58));
        assert_eq!(snap.asks[2].price, dec!(0.60));
    }

    #[test]
    fn test_apply_delta_add_and_update() {
        let mut book = make_book();
        let ts = Utc::now();
        book.apply_snapshot(
            vec![level(dec!(0.50), dec!(10))],
            vec![level(dec!(0.55), dec!(10))],
            ts,
        );

        // Add a new bid level, update existing ask size
        book.apply_delta(
            &[level(dec!(0.48), dec!(5))],
            &[level(dec!(0.55), dec!(20))],
            ts,
        );

        let snap = book.snapshot();
        assert_eq!(snap.bids.len(), 2);
        assert_eq!(snap.bids[0].price, dec!(0.50));
        assert_eq!(snap.bids[1].price, dec!(0.48));
        assert_eq!(snap.bids[1].size, dec!(5));
        assert_eq!(snap.asks[0].size, dec!(20));
    }

    #[test]
    fn test_apply_delta_remove_level() {
        let mut book = make_book();
        let ts = Utc::now();
        book.apply_snapshot(
            vec![level(dec!(0.50), dec!(10)), level(dec!(0.48), dec!(5))],
            vec![level(dec!(0.55), dec!(10))],
            ts,
        );

        // Remove bid at 0.50 by setting size to 0
        book.apply_delta(&[level(dec!(0.50), dec!(0))], &[], ts);

        let snap = book.snapshot();
        assert_eq!(snap.bids.len(), 1);
        assert_eq!(snap.bids[0].price, dec!(0.48));
    }

    #[test]
    fn test_imbalance() {
        let mut book = make_book();
        let ts = Utc::now();
        book.apply_snapshot(
            vec![level(dec!(0.50), dec!(100)), level(dec!(0.49), dec!(50))],
            vec![level(dec!(0.51), dec!(50)), level(dec!(0.52), dec!(25))],
            ts,
        );

        // Top 1 level: bids=100, asks=50 => (100-50)/(100+50) = 50/150
        let imb = book.imbalance(1);
        assert_eq!(imb, dec!(50) / dec!(150));

        // Top 2 levels: bids=150, asks=75 => 75/225 = 1/3
        let imb2 = book.imbalance(2);
        assert_eq!(imb2, dec!(75) / dec!(225));
    }

    #[test]
    fn test_depth_at_price_bids() {
        let mut book = make_book();
        let ts = Utc::now();
        book.apply_snapshot(
            vec![
                level(dec!(0.50), dec!(100)),
                level(dec!(0.49), dec!(50)),
                level(dec!(0.48), dec!(25)),
            ],
            vec![],
            ts,
        );

        // Depth at 0.49 on bid side: prices >= 0.49 => 0.50 (100) + 0.49 (50) = 150
        assert_eq!(book.depth_at_price(dec!(0.49), Side::Buy), dec!(150));
        // Depth at 0.50: only top level
        assert_eq!(book.depth_at_price(dec!(0.50), Side::Buy), dec!(100));
    }

    #[test]
    fn test_depth_at_price_asks() {
        let mut book = make_book();
        let ts = Utc::now();
        book.apply_snapshot(
            vec![],
            vec![
                level(dec!(0.55), dec!(100)),
                level(dec!(0.56), dec!(50)),
                level(dec!(0.57), dec!(25)),
            ],
            ts,
        );

        // Depth at 0.56 on ask side: prices <= 0.56 => 0.55 (100) + 0.56 (50) = 150
        assert_eq!(book.depth_at_price(dec!(0.56), Side::Sell), dec!(150));
    }
}
