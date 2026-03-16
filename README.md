# Polymarket Trading Bot

A production-ready trading bot for Polymarket prediction markets, written in Rust.

## Features

- **Actor-based Architecture** — 5 independent actors (Market, Feed, Strategy, Risk, Executor) communicating via typed channels
- **Multiple Strategies** — Lead-Lag, Fair Value, Flash Crash, Book Imbalance, NegRisk Arb, Convergence, Market Maker
- **Risk Management** — Kelly criterion sizing, circuit breakers, portfolio exposure limits, kill switch
- **Paper Trading** — Full simulation mode for testing without real money
- **Live Trading** — Real order execution via Polymarket CLOB API
- **Config Hot-Reload** — Watch config files for changes without restart
- **Rate Limiting** — Built-in rate limiting to prevent API quota exhaustion
- **TUI Dashboard** — Terminal UI for monitoring positions, orders, and P&L

## Quick Start

```bash
# Build
cargo build --release

# Run in paper mode (default)
./target/release/pmbot run

# Run in live mode (REAL MONEY)
./target/release/pmbot run --paper=false

# Show config
./target/release/pmbot config show
```

## Configuration

Copy `config/default.toml` and customize:

```toml
[general]
mode = "paper"
log_level = "info"
strategies = ["lead_lag", "fair_value"]

[risk]
bankroll = 1000
kelly_fraction = 0.15
max_position_pct = 0.05
max_positions = 5
daily_loss_limit_pct = 0.05
stop_loss_pct = 0.30
min_edge = 0.08
```

## Environment Variables

```bash
# Required for live trading
export PMBOT_PRIVATE_KEY="0x..."

# Optional
export RUST_LOG=pmbot=debug
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     main.rs (orchestrator)                   │
└──────┬──────────┬───────────┬───────────┬───────────────┘
       │          │           │           │
       ▼          ▼           ▼           ▼
┌──────────┐ ┌─────────┐ ┌────────┐ ┌─────────┐ ┌────────┐
│ Market   │ │ Feed    │ │Strategy│ │  Risk   │ │Executor│
│ Actor    │ │ Actor   │ │ Actor  │ │  Actor  │ │ Actor  │
│          │ │         │ │        │ │         │ │        │
│ Gamma    │ │ Binance │ │ N      │ │ Kelly   │ │ CLOB   │
│ discovery│ │ WS feed │ │ strats │ │ limits  │ │ sign   │
│ PM WS    │ │ vol     │ │ signal │ │ TP/SL   │ │ submit │
│ rotate   │ │ compute │ │ gen    │ │ sizing  │ │ track  │
└────┬─────┘ └───┬─────┘ └───┬────┘ └───┬─────┘ └───┬────┘
     │           │           │           │           │
     └───────────┴───────────┴───────────┴───────────┘
                    tokio::sync::mpsc channels
```

## Crates

| Crate | Description |
|-------|-------------|
| `pmbot-core` | Types, config, messages, error handling |
| `pmbot-market` | Market discovery, order book, rotation |
| `pmbot-feed` | Binance WebSocket, volatility calculation |
| `pmbot-strategy` | Strategy trait + 7 implementations |
| `pmbot-risk` | Risk checks, Kelly sizing, circuit breakers |
| `pmbot-executor` | Order execution (live + paper) |
| `pmbot-tui` | Terminal dashboard |

## License

MIT
