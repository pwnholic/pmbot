# Polymarket Bot - Known Issues & Remediation Plan

> Generated: 2026-03-17
> Total: 42 issues (5 Critical, 8 High, 15 Medium, 14 Low)

---

## CRITICAL (5 issues)

These will cause data loss, crashes, or incorrect trading in production.

### C1. Position state machine panics on invalid transitions
- **File**: `crates/pmbot-risk/src/position.rs:76-122`
- **Problem**: `TrackedPosition::open()`, `start_closing()`, and `close()` use `assert!()` to enforce state transitions. A duplicate fill event or race condition between executor and risk actors will **panic and crash the bot**.
- **Fix**: Replace `assert!()` with `Result<(), PositionError>` returns. Callers should log the error and skip the transition instead of crashing.
- [ ] Fixed

### C2. Exit signals are completely broken
- **File**: `crates/pmbot-risk/src/actor.rs:206-215`
- **Problem**: The exit signal emits `ExecutableOrder::Market` with `size: Decimal::ZERO` and hardcodes `side: Side::Buy` regardless of position direction. Zero-size orders do nothing; wrong side doesn't close a position.
- **Fix**: 
  - Set `size` to the position's remaining open size (`position.size - position.filled`).
  - Set `side` to the **opposite** of the position's entry side (`Side::Buy` -> exit with `Side::Sell`, vice versa).
- [ ] Fixed

### C3. Config watcher leaks OS file handle
- **File**: `crates/pmbot-core/src/config_watcher.rs:91`
- **Problem**: `std::mem::forget(watcher)` permanently leaks the `notify::RecommendedWatcher` handle. On long-running bots, this wastes OS resources.
- **Fix**: Store the watcher in an `Arc` or return it from the function so its lifetime is tied to the application. Alternatively, store it in a struct field on the actor that owns the config watch.
- [ ] Fixed

### C4. Shutdown aborts tasks, risking orphaned orders
- **File**: `src/main.rs:421-426` (paper), `src/main.rs:565-570` (live)
- **Problem**: Both modes shut down actors via `tokio::JoinHandle::abort()`, forcibly killing tasks mid-execution. In live mode, this can abort a task in the middle of order submission, leaving **orphaned orders** on the exchange with no local record.
- **Fix**:
  - Send a shutdown signal via the existing `shutdown_tx` broadcast channel.
  - Each actor's `run()` loop should `tokio::select!` on the shutdown receiver and break gracefully.
  - In live mode, call `executor.cancel_all()` before shutting down the executor actor.
  - Only `.abort()` as a last resort after a timeout (e.g., 5s).
- [ ] Fixed

### C5. Fair value strategy uses wrong units for Black-Scholes
- **File**: `crates/pmbot-strategy/src/fair_value.rs:161-174`
- **Problem**: Uses BTC spot price (~$87,000) as the spot and Polymarket mid-price (0.01-0.99) as the strike in `binary_call_fv()`. These are in completely different units. The binary call value is always ~1.0 (deep in-the-money), so the strategy **always buys**.
- **Fix**: The fair value for a BTC up/down market should be computed differently:
  - The anchor price (price at market open) is the strike.
  - Current BTC price is the spot.
  - Time-to-expiry in years = remaining_seconds / (365.25 * 86400).
  - Realized vol from the feed.
  - `binary_call_fv(btc_spot, btc_anchor, vol, time_to_expiry)` → probability ∈ [0,1].
  - Compare this probability to the Polymarket mid-price to determine edge.
- [ ] Fixed

---

## HIGH (8 issues)

These cause incorrect behavior or silent failures in core trading logic.

### H1. Strategies use non-deterministic HashMap iteration
- **File**: All 7 strategy files (`lead_lag.rs`, `fair_value.rs`, `flash_crash.rs`, `book_imbalance.rs`, `negrisk_arb.rs`, `convergence.rs`, `market_maker.rs`)
- **Problem**: `world.markets.iter().next()` on a `HashMap` returns an arbitrary entry. If multiple markets exist (e.g., during rotation), strategies may evaluate against the wrong market.
- **Fix**: Either:
  - (A) Pass the active `MarketId` explicitly to `Strategy::evaluate()`, or
  - (B) Use `IndexMap` or `BTreeMap` for deterministic iteration, or
  - (C) Add an `active_market_id` field to `WorldState` and have strategies look up by ID.
  - Recommended: **(C)** — least disruptive, most explicit.
- [ ] Fixed

### H2. neg_risk hardcoded to false in Gamma mapper
- **File**: `crates/pmbot-market/src/gamma.rs:205`
- **Problem**: When mapping Gamma API responses to our `MarketInfo`, `neg_risk` is always set to `false`. The `NegRiskArb` strategy filters for `neg_risk == true` and will **never find any markets**.
- **Fix**: Read the `neg_risk` field from the Gamma API response (`market.neg_risk`) and map it through.
- [ ] Fixed

### H3. Market rotation not implemented
- **File**: `crates/pmbot-market/src/actor.rs:122-126`
- **Problem**: `should_rotate()` is called and logs when rotation is needed, but **no action is taken**. For 5-minute markets, the bot continues trading an expired market indefinitely after the first window closes.
- **Fix**:
  - When `should_rotate()` returns true, call `discovery.discover()` again with the same filters.
  - Select the next upcoming market (highest liquidity, not yet expired).
  - Emit a `MarketEvent::MarketRotation { old, new }` event.
  - Strategies should reset their state via `on_market_change()`.
  - Cancel any open orders on the old market.
- [ ] Fixed

### H4. Daily PnL reset never called
- **File**: `crates/pmbot-risk/src/limits.rs`
- **Problem**: `CircuitBreaker::reset_daily()` exists but is never called. Daily PnL accumulates forever across calendar days, eventually triggering the daily loss limit even if each individual day was profitable.
- **Fix**: In `RiskActor::run()`, check `Utc::now().date_naive()` on each tick and call `reset_daily()` when the date changes.
- [ ] Fixed

### H5. Strategy signal size is hardcoded
- **File**: All strategy files
- **Problem**: All strategies hardcode `size: dec!(10)` in their `Signal` output. The risk actor's Kelly sizing adjusts this, but the circuit breaker's pre-check (`limits.rs:71`) uses the **raw** `dec!(10)` for balance validation, which may incorrectly reject or accept signals.
- **Fix**:
  - Strategies should emit `size: Decimal::ONE` (a "unit" signal).
  - The risk actor's Kelly fraction multiplies this by the computed optimal size.
  - The circuit breaker should check the Kelly-sized amount, not the raw signal size.
- [ ] Fixed

### H6. Strategy on_fill() callback never invoked
- **File**: `crates/pmbot-strategy/src/actor.rs`
- **Problem**: `Strategy::on_fill()` is never called by the strategy actor. The `MarketMaker` strategy relies on `on_fill()` to track inventory. Without it, the MM always thinks inventory is zero and will keep quoting as if flat.
- **Fix**: Subscribe the strategy actor to `ExecutionEvent`s. When `OrderFilled` or `OrderPartialFill` events arrive, call `strategy.on_fill(fill_info)` for each registered strategy.
- [ ] Fixed

### H7. Strategy on_market_change() callback never invoked
- **File**: `crates/pmbot-strategy/src/actor.rs`
- **Problem**: `Strategy::on_market_change()` is defined but never called. After a market rotation, strategies carry stale state (BTC anchor prices, position IDs, cached probabilities), generating invalid signals for the new market.
- **Fix**: When a `MarketEvent::MarketRotation` is received by the strategy actor, call `strategy.on_market_change(old_market, new_market)` for each registered strategy.
- [ ] Fixed

### H8. RiskActor world state never updated
- **File**: `crates/pmbot-risk/src/actor.rs:308-343`
- **Problem**: `RiskActor::run()` never calls `update_world()`. The world state used by the circuit breaker for balance checking stays at the default (balance = 1,000,000), so `InsufficientBalance` is **never triggered** in production.
- **Fix**: Update the risk actor's world state from `ExecutionEvent`s:
  - On `OrderFilled`: adjust balance and positions.
  - On `OrderPartialFill`: adjust partial fill tracking.
  - Initialize balance from config `risk.bankroll` at startup.
- [ ] Fixed

---

## MEDIUM (15 issues)

### M1. Binance feed emits zero-duration vol window
- **File**: `crates/pmbot-feed/src/binance.rs:81`
- **Problem**: `FeedEvent::VolUpdate` emitted with `window: Duration::from_secs(0)`.
- **Fix**: Use the configured `vol_window_secs` from `BinanceFeedConfig`.
- [ ] Fixed

### M2. BinanceFeed connect/disconnect are no-ops
- **File**: `crates/pmbot-feed/src/binance.rs:102-119`
- **Problem**: `BinanceFeed::connect()` and `disconnect()` just toggle a boolean. Real WS logic is in `ws.rs`.
- **Fix**: Remove or wire up properly. If keeping the `PriceFeed` trait, delegate to the real WS.
- [ ] Fixed

### M3. Compiler warnings — unused imports in config_watcher
- **File**: `crates/pmbot-core/src/config_watcher.rs:7,11,14`
- **Problem**: Unused imports: `Arc`, `mpsc`, `ConfigError`.
- **Fix**: Remove the unused imports.
- [ ] Fixed

### M4. Compiler warnings — unused imports in main.rs
- **File**: `src/main.rs:15`
- **Problem**: Unused imports: `MarketInfo`, `TokenId`.
- **Fix**: Remove.
- [ ] Fixed

### M5. Dead code — run_synthetic_feed
- **File**: `src/main.rs:158`
- **Problem**: `run_synthetic_feed()` is defined but never called.
- **Fix**: Remove it entirely (paper mode now uses real data).
- [ ] Fixed

### M6. Dead code — book_tx field never read
- **File**: `src/main.rs:246`
- **Problem**: `ActorChannels::book_tx` is created but never used.
- **Fix**: Remove the field, or wire it into the order book feed when implemented.
- [ ] Fixed

### M7. Price history grows unbounded
- **File**: `crates/pmbot-strategy/src/context.rs:51-55`
- **Problem**: `price_history` in `WorldStateBuilder` grows without bound. At 100ms ticks over a 5-minute market, this reaches ~3000 entries.
- **Fix**: Cap the history with a `VecDeque` and evict entries older than `max_lookback_secs`.
- [ ] Fixed

### M8. BuilderConfig loaded but never used
- **File**: `crates/pmbot-core/src/config.rs:177-185`
- **Problem**: `api_key`, `api_secret`, `api_passphrase` are deserialized but never referenced anywhere.
- **Fix**: Either wire them into the auth flow (for Builder/market-maker authentication) or remove the config section.
- [ ] Fixed

### M9. ClobConfig fields ws_host/chain_id unused
- **File**: `crates/pmbot-core/src/config.rs:151-153`
- **Problem**: `ws_host` and `chain_id` are loaded but the CLOB client uses only `host` and the SDK's `POLYGON` constant.
- **Fix**: Wire `chain_id` into signer creation, or remove if always Polygon.
- [ ] Fixed

### M10. WalletConfig encrypted_key_path unused
- **File**: `crates/pmbot-core/src/config.rs:129`
- **Problem**: `encrypted_key_path` is loaded but the private key comes from an env var.
- **Fix**: Either implement encrypted key loading or remove the field.
- [ ] Fixed

### M11. TUI never launched
- **File**: `crates/pmbot-tui/` + `src/main.rs`
- **Problem**: A full TUI implementation exists (dashboard, widgets for positions, PnL, strategies, log, market) but is never spawned in `run_paper()` or `run_live()`.
- **Fix**: Add a `--tui` flag or check `config.tui.enabled` and spawn the TUI actor.
- [ ] Fixed

### M12. ConfigWatcher never used
- **File**: `crates/pmbot-core/src/config_watcher.rs`
- **Problem**: `ConfigWatcher` is fully implemented but never instantiated.
- **Fix**: Wire it into the main run loop, or remove until needed.
- [ ] Fixed

### M13. Reconciler never called on startup
- **File**: `crates/pmbot-executor/src/reconciler.rs`
- **Problem**: `Reconciler::reconcile()` is implemented but never called. After a crash, the bot has no knowledge of existing exchange orders.
- **Fix**: Call `reconciler.reconcile()` during `run_live()` startup, before spawning the executor actor.
- [ ] Fixed

### M14. No-trade-zone never checked
- **File**: `crates/pmbot-risk/src/limits.rs`
- **Problem**: `RejectReason::NoTradeZone` exists and `no_trade_zone_secs` is configured (30s), but the circuit breaker never checks proximity to market expiry.
- **Fix**: In `CircuitBreaker::check()`, compare `time_to_expiry` against `no_trade_zone_secs` and reject if too close to expiry.
- [ ] Fixed

### M15. Strategy `enabled` fields never checked
- **File**: `crates/pmbot-core/src/config.rs` (7 strategy sections)
- **Problem**: Each strategy section has `enabled: bool` but activation is controlled only by `general.strategies` list.
- **Fix**: Either check `enabled` in `build_strategies()` or remove the field to avoid confusion.
- [ ] Fixed

---

## LOW (14 issues)

### L1. Duplicate decimal_sqrt implementations
- **File**: `crates/pmbot-market/src/tracker.rs:192` + `crates/pmbot-feed/src/vol.rs:202`
- **Fix**: Consolidate into `pmbot-core::math::decimal_sqrt()`.
- [ ] Fixed

### L2. BinanceFeed struct and PriceFeed trait unused
- **File**: `crates/pmbot-feed/src/binance.rs` + `crates/pmbot-feed/src/traits.rs`
- **Fix**: Remove or use in production code.
- [ ] Fixed

### L3. discovery_interval_secs loaded but unused
- **File**: `crates/pmbot-core/src/config.rs:190`
- **Fix**: Wire into MarketActor's periodic re-discovery loop when market rotation is implemented.
- [ ] Fixed

### L4. TuiConfig and BinanceFeedConfig::enabled loaded but unused
- **File**: `crates/pmbot-core/src/config.rs`
- **Fix**: Wire into TUI launch logic and feed startup respectively.
- [ ] Fixed

### L5. Convergence metrics() hardcodes edge
- **File**: `crates/pmbot-strategy/src/convergence.rs:215`
- **Fix**: Return actual computed edge from `self.last_edge`.
- [ ] Fixed

### L6. NegRiskArb metrics() hardcodes edge
- **File**: `crates/pmbot-strategy/src/negrisk_arb.rs:284-285`
- **Fix**: Return actual entry deviation from `self.last_deviation`.
- [ ] Fixed

### L7. DiscoveryFilters::tags field never used
- **File**: `crates/pmbot-market/src/discovery.rs:13`
- **Fix**: Either use in filtering or remove.
- [ ] Fixed

### L8. MarketActor public methods never called
- **File**: `crates/pmbot-market/src/actor.rs:168-180`
- **Fix**: Keep for testability but mark with `#[cfg(test)]` or remove.
- [ ] Fixed

### L9. Broadcast channel lag drops events silently
- **File**: `crates/pmbot-strategy/src/actor.rs:72-97`
- **Fix**: Add a mechanism to request a full state refresh after lag is detected.
- [ ] Fixed

### L10. Vol calculation uses linear approximation
- **File**: `crates/pmbot-feed/src/vol.rs:183`
- **Problem**: Uses `(curr - prev) / prev` instead of `ln(curr/prev)` for log-returns.
- **Fix**: Use proper log-return: `(curr / prev).ln()`. Requires `Decimal` to `f64` conversion.
- [ ] Fixed

### L11. Rate limiter uses f64 token counts
- **File**: `crates/pmbot-executor/src/rate_limit.rs`
- **Fix**: Use integer-based or `Decimal`-based token counting to avoid drift.
- [ ] Fixed

### L12. Strategy config casts u64 → i64/u32
- **File**: `src/main.rs:133,137-139`
- **Fix**: Use `TryFrom` with proper error handling, or change config types.
- [ ] Fixed

### L13. tempfile is a regular dependency, not dev-dependency
- **File**: `crates/pmbot-core/Cargo.toml`
- **Fix**: Move `tempfile` to `[dev-dependencies]`.
- [ ] Fixed

### L14. Missing test coverage
- **Files without tests**: `src/main.rs`, `executor/src/actor.rs`, `executor/src/live.rs`, all TUI widgets.
- **Fix**: Add integration tests for actor lifecycle and unit tests for critical paths.
- [ ] Fixed

---

## Recommended Fix Order

**Phase 1 — Safety (before any live trading):**
1. C2 (broken exits) + C5 (broken fair value) — trading logic is wrong
2. C1 (panic on state transition) — bot will crash
3. C4 (graceful shutdown) — orphaned orders risk
4. H3 (market rotation) — 5-min markets expire without rotation
5. H2 (neg_risk mapping) — if using NegRiskArb

**Phase 2 — Correctness:**
6. H1 (deterministic market selection)
7. H8 (risk world state update)
8. H6 + H7 (strategy callbacks)
9. H4 (daily PnL reset)
10. H5 (signal sizing)

**Phase 3 — Polish:**
11. All Medium issues (dead code, unused config, TUI, reconciler)
12. All Low issues (code quality, test coverage)
