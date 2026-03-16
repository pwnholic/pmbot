use rust_decimal::Decimal;

/// Fractional Kelly criterion position sizing for binary prediction markets.
///
/// Returns the dollar amount to allocate to a position, capped at `max_pct * bankroll`.
///
/// # Arguments
/// * `win_prob` - Estimated probability of winning (0..=1)
/// * `market_price` - Current market price / implied probability (0..1, exclusive of 0 and 1)
/// * `bankroll` - Total available capital
/// * `fraction` - Kelly fraction (e.g., 0.15 for quarter-Kelly)
/// * `max_pct` - Maximum position as a fraction of bankroll
pub fn fractional_kelly(
    win_prob: Decimal,
    market_price: Decimal,
    bankroll: Decimal,
    fraction: Decimal,
    max_pct: Decimal,
) -> Decimal {
    // Edge cases: if market_price is 0 or 1, no meaningful bet can be sized.
    if market_price <= Decimal::ZERO || market_price >= Decimal::ONE {
        return Decimal::ZERO;
    }

    // Edge case: win_prob out of range
    if win_prob <= Decimal::ZERO {
        return Decimal::ZERO;
    }

    // Clamp win_prob to 1 at most
    let win_prob = win_prob.min(Decimal::ONE);

    // odds = (1 - market_price) / market_price
    let odds = (Decimal::ONE - market_price) / market_price;

    // f* = (win_prob * odds - (1 - win_prob)) / odds
    let f_star = (win_prob * odds - (Decimal::ONE - win_prob)) / odds;

    // Never negative
    let f_star = f_star.max(Decimal::ZERO);

    // Position = f* * fraction * bankroll
    let position = f_star * fraction * bankroll;

    // Cap at max_pct * bankroll
    let cap = max_pct * bankroll;
    position.min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_normal_case() {
        // win_prob=0.60, market_price=0.50 => odds=1.0
        // f* = (0.60*1.0 - 0.40)/1.0 = 0.20
        // position = 0.20 * 0.25 * 1000 = 50
        let size = fractional_kelly(dec!(0.60), dec!(0.50), dec!(1000), dec!(0.25), dec!(0.10));
        assert_eq!(size, dec!(50));
    }

    #[test]
    fn test_capped_at_max_pct() {
        // Large edge should cap at max_pct * bankroll
        // win_prob=0.90, market_price=0.50 => odds=1.0
        // f* = (0.90*1.0 - 0.10)/1.0 = 0.80
        // position = 0.80 * 1.0 * 1000 = 800, but cap = 0.05 * 1000 = 50
        let size = fractional_kelly(dec!(0.90), dec!(0.50), dec!(1000), dec!(1.0), dec!(0.05));
        assert_eq!(size, dec!(50));
    }

    #[test]
    fn test_market_price_zero_returns_zero() {
        let size = fractional_kelly(dec!(0.60), dec!(0), dec!(1000), dec!(0.25), dec!(0.10));
        assert_eq!(size, Decimal::ZERO);
    }

    #[test]
    fn test_market_price_one_returns_zero() {
        let size = fractional_kelly(dec!(0.60), dec!(1), dec!(1000), dec!(0.25), dec!(0.10));
        assert_eq!(size, Decimal::ZERO);
    }

    #[test]
    fn test_negative_edge_returns_zero() {
        // win_prob=0.30, market_price=0.50 => odds=1.0
        // f* = (0.30*1.0 - 0.70)/1.0 = -0.40, clamped to 0
        let size = fractional_kelly(dec!(0.30), dec!(0.50), dec!(1000), dec!(0.25), dec!(0.10));
        assert_eq!(size, Decimal::ZERO);
    }

    #[test]
    fn test_win_prob_zero_returns_zero() {
        let size = fractional_kelly(dec!(0), dec!(0.50), dec!(1000), dec!(0.25), dec!(0.10));
        assert_eq!(size, Decimal::ZERO);
    }

    #[test]
    fn test_win_prob_one() {
        // win_prob=1.0, market_price=0.50 => odds=1.0
        // f* = (1.0*1.0 - 0.0)/1.0 = 1.0
        // position = 1.0 * 0.25 * 1000 = 250, cap = 0.10 * 1000 = 100
        let size = fractional_kelly(dec!(1.0), dec!(0.50), dec!(1000), dec!(0.25), dec!(0.10));
        assert_eq!(size, dec!(100));
    }

    #[test]
    fn test_asymmetric_odds() {
        // win_prob=0.70, market_price=0.60 => odds=(0.40/0.60)=0.6667
        // f* = (0.70*0.6667 - 0.30)/0.6667 = (0.4667 - 0.30)/0.6667 = 0.1667/0.6667 = 0.25
        // position = 0.25 * 0.15 * 1000 = 37.5
        let size = fractional_kelly(dec!(0.70), dec!(0.60), dec!(1000), dec!(0.15), dec!(0.10));
        assert_eq!(size, dec!(37.5));
    }

    #[test]
    fn test_no_edge_returns_zero() {
        // win_prob equals market_price => no edge
        // win_prob=0.50, market_price=0.50 => odds=1.0
        // f* = (0.50*1.0 - 0.50)/1.0 = 0.0
        let size = fractional_kelly(dec!(0.50), dec!(0.50), dec!(1000), dec!(0.25), dec!(0.10));
        assert_eq!(size, Decimal::ZERO);
    }
}
