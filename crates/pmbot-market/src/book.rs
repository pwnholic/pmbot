use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::collections::HashMap;

use pmbot_core::math::book_imbalance;
use pmbot_core::types::{Level, MarketId, OrderbookSnapshot, Side, TokenId};

/// Order book side for a single outcome token.
#[derive(Debug, Clone, Default)]
pub struct OrderBookSide {
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

/// Local orderbook maintaining sorted bids (descending) and asks (ascending)
/// for a single outcome token.
#[derive(Debug, Clone)]
pub struct SingleBook {
    market_id: MarketId,
    token_id: TokenId,
    bids: Vec<Level>,
    asks: Vec<Level>,
    last_update: DateTime<Utc>,
}

impl SingleBook {
    pub fn new(market_id: MarketId, token_id: TokenId) -> Self {
        Self {
            market_id,
            token_id,
            bids: Vec::new(),
            asks: Vec::new(),
            last_update: Utc::now(),
        }
    }

    pub fn apply_snapshot(&mut self, bids: Vec<Level>, asks: Vec<Level>, ts: DateTime<Utc>) {
        self.bids = bids;
        self.asks = asks;
        self.bids.sort_by(|a, b| b.price.cmp(&a.price));
        self.asks.sort_by(|a, b| a.price.cmp(&b.price));
        self.last_update = ts;
    }

    pub fn apply_delta(&mut self, bid_updates: &[Level], ask_updates: &[Level], ts: DateTime<Utc>) {
        for update in bid_updates {
            Self::apply_side_delta(&mut self.bids, update, true);
        }
        for update in ask_updates {
            Self::apply_side_delta(&mut self.asks, update, false);
        }
        self.last_update = ts;
    }

    fn apply_side_delta(levels: &mut Vec<Level>, update: &Level, descending: bool) {
        if let Some(pos) = levels.iter().position(|l| l.price == update.price) {
            if update.size.is_zero() {
                levels.remove(pos);
            } else {
                levels[pos].size = update.size;
            }
        } else if !update.size.is_zero() {
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

    pub fn snapshot(&self) -> OrderbookSnapshot {
        OrderbookSnapshot {
            market_id: self.market_id.clone(),
            token_id: self.token_id.clone(),
            bids: self.bids.clone(),
            asks: self.asks.clone(),
            timestamp: self.last_update,
        }
    }

    pub fn imbalance(&self, levels: usize) -> Decimal {
        book_imbalance(&self.bids, &self.asks, levels)
    }

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

    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.first().map(|l| l.price)
    }

    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.first().map(|l| l.price)
    }

    pub fn market_id(&self) -> &MarketId {
        &self.market_id
    }

    pub fn token_id(&self) -> &TokenId {
        &self.token_id
    }

    pub fn last_update(&self) -> DateTime<Utc> {
        self.last_update
    }
}

/// Multi-outcome order book for a market.
///
/// For binary markets: 2 books (Yes/No)
/// For multi-option markets: N books (one per outcome)
#[derive(Debug, Clone)]
pub struct LocalBook {
    market_id: MarketId,
    books: HashMap<TokenId, OrderBookSide>,
    primary_token_id: Option<TokenId>,
    last_update: DateTime<Utc>,
}

impl LocalBook {
    pub fn new(market_id: MarketId, primary_token_id: TokenId) -> Self {
        let mut books = HashMap::new();
        books.insert(primary_token_id.clone(), OrderBookSide::default());
        Self {
            market_id,
            books,
            primary_token_id: Some(primary_token_id),
            last_update: Utc::now(),
        }
    }

    pub fn from_tokens(market_id: MarketId, token_ids: &[TokenId]) -> Self {
        let mut books = HashMap::new();
        for token_id in token_ids {
            books.insert(token_id.clone(), OrderBookSide::default());
        }
        Self {
            market_id,
            books,
            primary_token_id: token_ids.first().cloned(),
            last_update: Utc::now(),
        }
    }

    pub fn set_primary_token(&mut self, token_id: TokenId) {
        self.primary_token_id = Some(token_id);
    }

    pub fn primary_token_id(&self) -> Option<&TokenId> {
        self.primary_token_id.as_ref()
    }

    pub fn update(
        &mut self,
        token_id: &TokenId,
        bids: Vec<Level>,
        asks: Vec<Level>,
        ts: DateTime<Utc>,
    ) {
        let side = self
            .books
            .entry(token_id.clone())
            .or_insert_with(OrderBookSide::default);
        side.bids = bids;
        side.asks = asks;
        side.bids.sort_by(|a, b| b.price.cmp(&a.price));
        side.asks.sort_by(|a, b| a.price.cmp(&b.price));
        self.last_update = ts;
    }

    pub fn apply_delta(
        &mut self,
        token_id: &TokenId,
        bid_updates: &[Level],
        ask_updates: &[Level],
        ts: DateTime<Utc>,
    ) {
        let side = self
            .books
            .entry(token_id.clone())
            .or_insert_with(OrderBookSide::default);
        for update in bid_updates {
            SingleBook::apply_side_delta(&mut side.bids, update, true);
        }
        for update in ask_updates {
            SingleBook::apply_side_delta(&mut side.asks, update, false);
        }
        self.last_update = ts;
    }

    pub fn price_for_outcome(&self, token_id: &TokenId) -> Option<Decimal> {
        self.books
            .get(token_id)
            .and_then(|s| s.bids.first().map(|l| l.price))
    }

    pub fn best_bid_for(&self, token_id: &TokenId) -> Option<Decimal> {
        self.books
            .get(token_id)
            .and_then(|s| s.bids.first().map(|l| l.price))
    }

    pub fn best_ask_for(&self, token_id: &TokenId) -> Option<Decimal> {
        self.books
            .get(token_id)
            .and_then(|s| s.asks.first().map(|l| l.price))
    }

    pub fn spread_for(&self, token_id: &TokenId) -> Option<Decimal> {
        let side = self.books.get(token_id)?;
        let bid = side.bids.first()?;
        let ask = side.asks.first()?;
        Some(ask.price - bid.price)
    }

    pub fn imbalance_for(&self, token_id: &TokenId, levels: usize) -> Option<Decimal> {
        let side = self.books.get(token_id)?;
        Some(book_imbalance(&side.bids, &side.asks, levels))
    }

    pub fn books(&self) -> &HashMap<TokenId, OrderBookSide> {
        &self.books
    }

    pub fn market_id(&self) -> &MarketId {
        &self.market_id
    }

    pub fn last_update(&self) -> DateTime<Utc> {
        self.last_update
    }

    pub fn token_ids(&self) -> impl Iterator<Item = &TokenId> {
        self.books.keys()
    }

    pub fn num_outcomes(&self) -> usize {
        self.books.len()
    }

    pub fn is_binary(&self) -> bool {
        self.books.len() == 2
    }

    pub fn is_multi_option(&self) -> bool {
        self.books.len() > 2
    }

    pub fn snapshot_for(&self, token_id: &TokenId) -> Option<OrderbookSnapshot> {
        let side = self.books.get(token_id)?;
        Some(OrderbookSnapshot {
            market_id: self.market_id.clone(),
            token_id: token_id.clone(),
            bids: side.bids.clone(),
            asks: side.asks.clone(),
            timestamp: self.last_update,
        })
    }

    pub fn snapshot(&self) -> OrderbookSnapshot {
        match &self.primary_token_id {
            Some(token_id) => self
                .snapshot_for(token_id)
                .unwrap_or_else(|| OrderbookSnapshot {
                    market_id: self.market_id.clone(),
                    token_id: token_id.clone(),
                    bids: vec![],
                    asks: vec![],
                    timestamp: self.last_update,
                }),
            None => OrderbookSnapshot {
                market_id: self.market_id.clone(),
                token_id: TokenId("unknown".into()),
                bids: vec![],
                asks: vec![],
                timestamp: self.last_update,
            },
        }
    }

    pub fn apply_snapshot(&mut self, bids: Vec<Level>, asks: Vec<Level>, ts: DateTime<Utc>) {
        if let Some(token_id) = self.primary_token_id.clone() {
            self.update(&token_id, bids, asks, ts);
        }
    }

    pub fn apply_delta_no_token(
        &mut self,
        bid_updates: &[Level],
        ask_updates: &[Level],
        ts: DateTime<Utc>,
    ) {
        if let Some(token_id) = self.primary_token_id.clone() {
            self.apply_delta(&token_id, bid_updates, ask_updates, ts);
        }
    }

    pub fn imbalance(&self, levels: usize) -> Decimal {
        match &self.primary_token_id {
            Some(token_id) => self
                .imbalance_for(token_id, levels)
                .unwrap_or(Decimal::ZERO),
            None => Decimal::ZERO,
        }
    }

    pub fn best_bid(&self) -> Option<Decimal> {
        self.primary_token_id
            .as_ref()
            .and_then(|t| self.best_bid_for(t))
    }

    pub fn best_ask(&self) -> Option<Decimal> {
        self.primary_token_id
            .as_ref()
            .and_then(|t| self.best_ask_for(t))
    }

    pub fn mid_price(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        Some((bid + ask) / Decimal::from(2))
    }

    pub fn token_id(&self) -> Option<&TokenId> {
        self.primary_token_id.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_book() -> LocalBook {
        LocalBook::new(MarketId("test-market".into()), TokenId("token-yes".into()))
    }

    fn make_book_with_tokens(token_ids: &[&str]) -> LocalBook {
        LocalBook::from_tokens(
            MarketId("test-market".into()),
            &token_ids
                .iter()
                .map(|s| TokenId(s.to_string()))
                .collect::<Vec<_>>(),
        )
    }

    fn level(price: Decimal, size: Decimal) -> Level {
        Level { price, size }
    }

    #[test]
    fn test_new_book_has_primary_token() {
        let book = make_book();
        assert_eq!(book.num_outcomes(), 1);
        assert!(book.primary_token_id().is_some());
    }

    #[test]
    fn test_from_tokens_creates_books() {
        let book = make_book_with_tokens(&["token-yes", "token-no"]);
        assert_eq!(book.num_outcomes(), 2);
        assert!(book.is_binary());
    }

    #[test]
    fn test_update_adds_book() {
        let mut book = make_book();
        let token_id = TokenId("token-yes".into());

        book.update(
            &token_id,
            vec![level(dec!(0.50), dec!(100))],
            vec![level(dec!(0.55), dec!(50))],
            Utc::now(),
        );

        assert_eq!(book.num_outcomes(), 1);
        assert_eq!(book.best_bid_for(&token_id), Some(dec!(0.50)));
        assert_eq!(book.best_ask_for(&token_id), Some(dec!(0.55)));
    }

    #[test]
    fn test_price_for_outcome() {
        let mut book = make_book_with_tokens(&["token-yes", "token-no"]);

        book.update(
            &TokenId("token-yes".into()),
            vec![level(dec!(0.60), dec!(100))],
            vec![level(dec!(0.65), dec!(50))],
            Utc::now(),
        );

        assert_eq!(
            book.price_for_outcome(&TokenId("token-yes".into())),
            Some(dec!(0.60))
        );
        assert_eq!(book.price_for_outcome(&TokenId("token-no".into())), None);
    }

    #[test]
    fn test_spread_for() {
        let mut book = make_book();
        let token_id = TokenId("token-yes".into());

        book.update(
            &token_id,
            vec![level(dec!(0.50), dec!(100))],
            vec![level(dec!(0.55), dec!(50))],
            Utc::now(),
        );

        assert_eq!(book.spread_for(&token_id), Some(dec!(0.05)));
    }

    #[test]
    fn test_imbalance_for() {
        let mut book = make_book();
        let token_id = TokenId("token-yes".into());

        book.update(
            &token_id,
            vec![level(dec!(0.50), dec!(100)), level(dec!(0.49), dec!(50))],
            vec![level(dec!(0.51), dec!(50)), level(dec!(0.52), dec!(25))],
            Utc::now(),
        );

        let imb = book.imbalance_for(&token_id, 1);
        assert!(imb.is_some());
        // Top 1: bids=100, asks=50 => (100-50)/(100+50) = 50/150
        assert_eq!(imb.unwrap(), dec!(50) / dec!(150));
    }

    #[test]
    fn test_apply_delta() {
        let mut book = make_book();
        let token_id = TokenId("token-yes".into());

        book.update(
            &token_id,
            vec![level(dec!(0.50), dec!(100))],
            vec![level(dec!(0.55), dec!(50))],
            Utc::now(),
        );

        book.apply_delta(
            &token_id,
            &[level(dec!(0.48), dec!(25))],
            &[level(dec!(0.55), dec!(75))],
            Utc::now(),
        );

        let bids = &book.books().get(&token_id).unwrap().bids;
        let asks = &book.books().get(&token_id).unwrap().asks;

        assert_eq!(bids.len(), 2);
        assert_eq!(asks[0].size, dec!(75));
    }

    #[test]
    fn test_is_binary_and_multi_option() {
        let mut book = make_book_with_tokens(&["t1", "t2"]);
        assert!(book.is_binary());
        assert!(!book.is_multi_option());

        book.update(&TokenId("t3".into()), vec![], vec![], Utc::now());
        assert!(!book.is_binary());
        assert!(book.is_multi_option());
    }

    #[test]
    fn test_snapshot_for_primary_token() {
        let mut book = make_book();
        book.apply_snapshot(
            vec![level(dec!(0.50), dec!(100))],
            vec![level(dec!(0.55), dec!(50))],
            Utc::now(),
        );

        let snap = book.snapshot();
        assert_eq!(snap.market_id.0, "test-market");
        assert_eq!(snap.token_id.0, "token-yes");
        assert_eq!(snap.bids.len(), 1);
        assert_eq!(snap.asks.len(), 1);
    }

    #[test]
    fn test_backward_compat_apply_snapshot() {
        let mut book = make_book();
        let ts = Utc::now();
        book.apply_snapshot(
            vec![level(dec!(0.50), dec!(10)), level(dec!(0.45), dec!(5))],
            vec![level(dec!(0.55), dec!(10))],
            ts,
        );

        let snap = book.snapshot();
        assert_eq!(snap.bids[0].price, dec!(0.50));
        assert_eq!(snap.bids[1].price, dec!(0.45));
    }

    #[test]
    fn test_backward_compat_apply_delta() {
        let mut book = make_book();
        let ts = Utc::now();
        book.apply_snapshot(
            vec![level(dec!(0.50), dec!(10))],
            vec![level(dec!(0.55), dec!(10))],
            ts,
        );

        book.apply_delta_no_token(
            &[level(dec!(0.48), dec!(5))],
            &[level(dec!(0.55), dec!(20))],
            ts,
        );

        let snap = book.snapshot();
        assert_eq!(snap.bids.len(), 2);
        assert_eq!(snap.asks[0].size, dec!(20));
    }

    #[test]
    fn test_mid_price() {
        let mut book = make_book();
        book.apply_snapshot(
            vec![level(dec!(0.50), dec!(100))],
            vec![level(dec!(0.52), dec!(100))],
            Utc::now(),
        );

        assert_eq!(book.mid_price(), Some(dec!(0.51)));
    }
}
