use rust_decimal::Decimal;

use crate::types::{Level, Side};

/// Calculate Polymarket taker fee.
///
/// Formula: `size * min(price, 1 - price) * fee_rate_bps / 10_000`
pub fn taker_fee(price: Decimal, size: Decimal, fee_rate_bps: u32) -> Decimal {
    let complement = Decimal::ONE - price;
    let effective_price = price.min(complement);
    size * effective_price * Decimal::new(fee_rate_bps.into(), 0) / Decimal::new(10_000, 0)
}

/// Calculate Polymarket maker rebate.
///
/// Same formula as taker fee but returned as a positive rebate amount.
pub fn maker_rebate(price: Decimal, size: Decimal, rebate_bps: u32) -> Decimal {
    taker_fee(price, size, rebate_bps)
}

/// Calculate net edge after fees.
///
/// `net_edge = |model_prob - market_price| - fee_per_unit`
/// where fee_per_unit = `min(market_price, 1 - market_price) * fee_rate_bps / 10_000`
pub fn net_edge(model_prob: Decimal, market_price: Decimal, fee_rate_bps: u32) -> Decimal {
    let gross_edge = (model_prob - market_price).abs();
    let complement = Decimal::ONE - market_price;
    let fee_per_unit = market_price.min(complement) * Decimal::new(fee_rate_bps.into(), 0)
        / Decimal::new(10_000, 0);
    gross_edge - fee_per_unit
}

/// Calculate book imbalance from bid/ask levels.
///
/// `(sum_bids - sum_asks) / (sum_bids + sum_asks)`
///
/// Returns a value in [-1.0, +1.0]. Positive means bid-heavy (buying pressure).
/// Returns `Decimal::ZERO` if both sides are empty.
pub fn book_imbalance(bids: &[Level], asks: &[Level], levels: usize) -> Decimal {
    let sum_bids: Decimal = bids.iter().take(levels).map(|l| l.size).sum();
    let sum_asks: Decimal = asks.iter().take(levels).map(|l| l.size).sum();
    let total = sum_bids + sum_asks;

    if total.is_zero() {
        return Decimal::ZERO;
    }

    (sum_bids - sum_asks) / total
}

/// Calculate cumulative depth up to a given price level.
///
/// For `Side::Buy` (bids): sums sizes of all bids with `price >= target_price`.
/// For `Side::Sell` (asks): sums sizes of all asks with `price <= target_price`.
pub fn depth_at_price(book: &[Level], target_price: Decimal, side: Side) -> Decimal {
    match side {
        Side::Buy => book
            .iter()
            .filter(|l| l.price >= target_price)
            .map(|l| l.size)
            .sum(),
        Side::Sell => book
            .iter()
            .filter(|l| l.price <= target_price)
            .map(|l| l.size)
            .sum(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_taker_fee_symmetric() {
        // price=0.30, size=100, 200bps
        // fee = 100 * min(0.30, 0.70) * 0.02 = 100 * 0.30 * 0.02 = 0.60
        let fee = taker_fee(dec!(0.30), dec!(100), 200);
        assert_eq!(fee, dec!(0.60));
    }

    #[test]
    fn test_taker_fee_uses_complement_when_cheaper() {
        // price=0.80, size=100, 200bps
        // fee = 100 * min(0.80, 0.20) * 0.02 = 100 * 0.20 * 0.02 = 0.40
        let fee = taker_fee(dec!(0.80), dec!(100), 200);
        assert_eq!(fee, dec!(0.40));
    }

    #[test]
    fn test_taker_fee_at_midpoint() {
        // price=0.50, size=100, 200bps
        // fee = 100 * 0.50 * 0.02 = 1.00
        let fee = taker_fee(dec!(0.50), dec!(100), 200);
        assert_eq!(fee, dec!(1.00));
    }

    #[test]
    fn test_maker_rebate_equals_fee_formula() {
        let fee = taker_fee(dec!(0.40), dec!(50), 150);
        let rebate = maker_rebate(dec!(0.40), dec!(50), 150);
        assert_eq!(fee, rebate);
    }

    #[test]
    fn test_net_edge_positive() {
        // model_prob=0.60, market_price=0.50, 200bps
        // gross_edge = |0.60 - 0.50| = 0.10
        // fee_per_unit = min(0.50, 0.50) * 0.02 = 0.01
        // net = 0.10 - 0.01 = 0.09
        let edge = net_edge(dec!(0.60), dec!(0.50), 200);
        assert_eq!(edge, dec!(0.09));
    }

    #[test]
    fn test_net_edge_negative_when_fee_exceeds_edge() {
        // model_prob=0.505, market_price=0.50, 200bps
        // gross_edge = 0.005
        // fee_per_unit = 0.01
        // net = -0.005
        let edge = net_edge(dec!(0.505), dec!(0.50), 200);
        assert_eq!(edge, dec!(-0.005));
    }

    #[test]
    fn test_book_imbalance_balanced() {
        let bids = vec![
            Level {
                price: dec!(0.50),
                size: dec!(100),
            },
            Level {
                price: dec!(0.49),
                size: dec!(100),
            },
        ];
        let asks = vec![
            Level {
                price: dec!(0.51),
                size: dec!(100),
            },
            Level {
                price: dec!(0.52),
                size: dec!(100),
            },
        ];
        let imb = book_imbalance(&bids, &asks, 5);
        assert_eq!(imb, dec!(0));
    }

    #[test]
    fn test_book_imbalance_bid_heavy() {
        let bids = vec![Level {
            price: dec!(0.50),
            size: dec!(300),
        }];
        let asks = vec![Level {
            price: dec!(0.51),
            size: dec!(100),
        }];
        // (300 - 100) / (300 + 100) = 200/400 = 0.5
        let imb = book_imbalance(&bids, &asks, 5);
        assert_eq!(imb, dec!(0.5));
    }

    #[test]
    fn test_book_imbalance_ask_heavy() {
        let bids = vec![Level {
            price: dec!(0.50),
            size: dec!(100),
        }];
        let asks = vec![Level {
            price: dec!(0.51),
            size: dec!(300),
        }];
        let imb = book_imbalance(&bids, &asks, 5);
        assert_eq!(imb, dec!(-0.5));
    }

    #[test]
    fn test_book_imbalance_empty() {
        let imb = book_imbalance(&[], &[], 5);
        assert_eq!(imb, dec!(0));
    }

    #[test]
    fn test_book_imbalance_respects_levels() {
        let bids = vec![
            Level {
                price: dec!(0.50),
                size: dec!(100),
            },
            Level {
                price: dec!(0.49),
                size: dec!(900),
            }, // should be ignored with levels=1
        ];
        let asks = vec![Level {
            price: dec!(0.51),
            size: dec!(100),
        }];
        let imb = book_imbalance(&bids, &asks, 1);
        assert_eq!(imb, dec!(0)); // 100 bids, 100 asks at level 1
    }

    #[test]
    fn test_depth_at_price_bids() {
        let bids = vec![
            Level {
                price: dec!(0.50),
                size: dec!(100),
            },
            Level {
                price: dec!(0.49),
                size: dec!(200),
            },
            Level {
                price: dec!(0.48),
                size: dec!(300),
            },
        ];
        // Depth at 0.49: bids >= 0.49 => 0.50 (100) + 0.49 (200) = 300
        let depth = depth_at_price(&bids, dec!(0.49), Side::Buy);
        assert_eq!(depth, dec!(300));
    }

    #[test]
    fn test_depth_at_price_asks() {
        let asks = vec![
            Level {
                price: dec!(0.51),
                size: dec!(100),
            },
            Level {
                price: dec!(0.52),
                size: dec!(200),
            },
            Level {
                price: dec!(0.53),
                size: dec!(300),
            },
        ];
        // Depth at 0.52: asks <= 0.52 => 0.51 (100) + 0.52 (200) = 300
        let depth = depth_at_price(&asks, dec!(0.52), Side::Sell);
        assert_eq!(depth, dec!(300));
    }
}
