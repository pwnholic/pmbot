use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use pmbot_core::BotConfig;
use pmbot_core::messages::{ExecutableOrder, ExecutionEvent, FeedEvent, MarketEvent, Signal};
use pmbot_core::types::{Level, MarketId, MarketInfo, Symbol, TokenId};
use pmbot_executor::{ExecutorActor, LiveExecutor, PaperExecutor};
use pmbot_feed::{FeedActor, RawTradeMessage, VolMethod, run_binance_ws};
use pmbot_market::actor::RawBookMessage;
use pmbot_market::{GammaDiscovery, MarketActor};
use pmbot_risk::RiskActor;
use pmbot_strategy::{
    BookImbalance, Convergence, FairValue, FlashCrash, LeadLag, MarketMaker, NegRiskArb, Strategy,
    StrategyActor, StrategyRegistry,
};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "pmbot",
    about = "Polymarket trading bot",
    version,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the trading bot.
    Run {
        /// Path to config TOML file.
        #[arg(short, long, default_value = "config/default.toml")]
        config: PathBuf,

        /// Force paper trading mode regardless of config.
        #[arg(long)]
        paper: bool,

        /// Override enabled strategies (comma-separated).
        #[arg(long, value_delimiter = ',')]
        strategies: Option<Vec<String>>,
    },

    /// Configuration management.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the fully resolved configuration.
    Show {
        /// Path to config TOML file.
        #[arg(short, long, default_value = "config/default.toml")]
        config: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// Banner
// ---------------------------------------------------------------------------

fn print_banner() {
    println!(
        r#"
  ____  __  __ ____        _
 |  _ \|  \/  | __ )  ___ | |_
 | |_) | |\/| |  _ \ / _ \| __|
 |  __/| |  | | |_) | (_) | |_
 |_|   |_|  |_|____/ \___/ \__|
  Polymarket Trading Bot v{}
"#,
        env!("CARGO_PKG_VERSION")
    );
}

// ---------------------------------------------------------------------------
// Strategy factory
// ---------------------------------------------------------------------------

fn build_strategies(config: &BotConfig) -> StrategyRegistry {
    let mut registry = StrategyRegistry::new();
    for name in &config.general.strategies {
        let strategy: Box<dyn Strategy> = match name.as_str() {
            "lead_lag" => Box::new(LeadLag::new(
                config.strategy.lead_lag.lag_threshold,
                config.strategy.lead_lag.entry_delay_ms,
                config.strategy.lead_lag.exit_convergence_pct,
            )),
            "fair_value" => Box::new(FairValue::new(
                config.strategy.fair_value.vol_multiplier,
                config.strategy.fair_value.min_time_to_expiry_secs,
                config.risk.min_edge,
            )),
            "flash_crash" => Box::new(FlashCrash::new(
                config.strategy.flash_crash.drop_threshold,
                config.strategy.flash_crash.lookback_secs,
                config.strategy.flash_crash.reversion_target,
                dec!(0.1),
            )),
            "book_imbalance" => Box::new(BookImbalance::new(
                config.strategy.book_imbalance.imbalance_threshold,
                config.strategy.book_imbalance.levels,
                5,
                config.risk.min_edge,
            )),
            "negrisk_arb" => Box::new(NegRiskArb::new(
                config.strategy.negrisk_arb.sum_deviation_threshold,
            )),
            "convergence" => Box::new(Convergence::new(
                config.strategy.convergence.min_probability,
                config.strategy.convergence.max_time_to_expiry_secs as i64,
                config.risk.min_edge,
            )),
            "market_maker" => Box::new(MarketMaker::new(
                config.strategy.market_maker.spread_bps as u32,
                Decimal::from(config.strategy.market_maker.max_inventory),
                Decimal::from(config.strategy.market_maker.quote_size),
                config.strategy.market_maker.refresh_interval_ms,
            )),
            other => {
                warn!(name = other, "unknown strategy, skipping");
                continue;
            }
        };
        info!(name = name.as_str(), "registered strategy");
        registry.register(strategy);
    }

    registry
}

// ---------------------------------------------------------------------------
// Paper mode synthetic data generator
// ---------------------------------------------------------------------------

async fn run_synthetic_feed(
    book_tx: mpsc::Sender<RawBookMessage>,
    raw_trade_tx: mpsc::Sender<RawTradeMessage>,
    mut shutdown: broadcast::Receiver<()>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut btc_price: f64 = 87500.0;
    let mut pm_mid: f64 = 0.52;
    let mut tick_count: u64 = 0;

    info!(
        btc = format!("{btc_price:.2}"),
        pm_mid = format!("{pm_mid:.4}"),
        "synthetic data generator started"
    );

    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                info!("synthetic data generator shutting down");
                break;
            }
            _ = tick.tick() => {
                tick_count += 1;

                // BTC random walk with sine/cosine oscillation
                let btc_change = ((tick_count as f64 * 0.7).sin() * 0.002)
                    + ((tick_count as f64 * 1.3).cos() * 0.001);
                btc_price *= 1.0 + btc_change;

                // PM mid follows BTC with lag + noise
                let pm_target = 0.50 + (btc_price - 87500.0) / 87500.0 * 5.0;
                pm_mid += (pm_target - pm_mid) * 0.1
                    + ((tick_count as f64 * 2.1).sin() * 0.005);
                pm_mid = pm_mid.clamp(0.01, 0.99);

                let spread = 0.02;
                let bid = pm_mid - spread / 2.0;
                let ask = pm_mid + spread / 2.0;

                let to_dec = |v: f64| Decimal::from_f64_retain(v).unwrap_or(dec!(0.50));

                let book = RawBookMessage::Snapshot {
                    bids: vec![
                        Level { price: to_dec(bid), size: dec!(150) },
                        Level { price: to_dec(bid - 0.01), size: dec!(200) },
                        Level { price: to_dec(bid - 0.02), size: dec!(300) },
                    ],
                    asks: vec![
                        Level { price: to_dec(ask), size: dec!(120) },
                        Level { price: to_dec(ask + 0.01), size: dec!(180) },
                        Level { price: to_dec(ask + 0.02), size: dec!(250) },
                    ],
                };
                if book_tx.send(book).await.is_err() { break; }

                let trade = RawTradeMessage {
                    symbol: Symbol("BTCUSDT".into()),
                    price: to_dec(btc_price),
                    timestamp: Utc::now(),
                };
                if raw_trade_tx.send(trade).await.is_err() { break; }

                if tick_count.is_multiple_of(20) {
                    info!(
                        tick = tick_count,
                        btc = format!("{btc_price:.2}"),
                        pm_mid = format!("{pm_mid:.4}"),
                        "synthetic tick"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Common actor orchestration helpers
// ---------------------------------------------------------------------------

struct ActorChannels {
    market_event_tx: broadcast::Sender<MarketEvent>,
    feed_event_tx: broadcast::Sender<FeedEvent>,
    execution_event_tx: broadcast::Sender<ExecutionEvent>,
    signal_tx: mpsc::Sender<Signal>,
    signal_rx: mpsc::Receiver<Signal>,
    order_tx: mpsc::Sender<ExecutableOrder>,
    order_rx: mpsc::Receiver<ExecutableOrder>,
    book_tx: mpsc::Sender<RawBookMessage>,
    book_rx: mpsc::Receiver<RawBookMessage>,
    raw_trade_tx: mpsc::Sender<RawTradeMessage>,
    raw_trade_rx: mpsc::Receiver<RawTradeMessage>,
    shutdown_tx: broadcast::Sender<()>,
}

fn create_actor_channels() -> ActorChannels {
    let (market_event_tx, _) = broadcast::channel(256);
    let (feed_event_tx, _) = broadcast::channel(256);
    let (execution_event_tx, _) = broadcast::channel(256);
    let (signal_tx, signal_rx) = mpsc::channel(64);
    let (order_tx, order_rx) = mpsc::channel(64);
    let (book_tx, book_rx) = mpsc::channel(256);
    let (raw_trade_tx, raw_trade_rx) = mpsc::channel(256);
    let (shutdown_tx, _) = broadcast::channel(1);

    ActorChannels {
        market_event_tx,
        feed_event_tx,
        execution_event_tx,
        signal_tx,
        signal_rx,
        order_tx,
        order_rx,
        book_tx,
        book_rx,
        raw_trade_tx,
        raw_trade_rx,
        shutdown_tx,
    }
}

struct CommonActors {
    market_actor: MarketActor,
    feed_actor: FeedActor,
    strategy_actor: StrategyActor,
    risk_actor: RiskActor,
    symbols: Vec<Symbol>,
}

fn build_common_actors(
    config: &BotConfig,
    market_event_tx: broadcast::Sender<MarketEvent>,
    feed_event_tx: broadcast::Sender<FeedEvent>,
    execution_event_tx: broadcast::Sender<ExecutionEvent>,
    signal_tx: mpsc::Sender<Signal>,
    signal_rx: mpsc::Receiver<Signal>,
    order_tx: mpsc::Sender<ExecutableOrder>,
    raw_trade_rx: mpsc::Receiver<RawTradeMessage>,
    registry: StrategyRegistry,
) -> CommonActors {
    let symbols: Vec<Symbol> = config
        .feed
        .binance
        .symbols
        .iter()
        .map(|s| Symbol(s.clone()))
        .collect();

    let vol_method = match config.feed.binance.vol_method.as_str() {
        "rolling" => VolMethod::Rolling,
        "parkinson" => VolMethod::Parkinson,
        _ => VolMethod::Ewma,
    };

    let feed_actor = FeedActor::new(
        feed_event_tx.clone(),
        raw_trade_rx,
        &symbols,
        vol_method,
        Duration::from_secs(config.feed.binance.vol_window_secs),
    );

    let market_actor = MarketActor::new(config.market.clone(), market_event_tx.clone());

    let strategy_actor = StrategyActor::new(
        registry,
        config.risk.bankroll,
        market_event_tx.subscribe(),
        feed_event_tx.subscribe(),
        execution_event_tx.subscribe(),
        signal_tx,
        100,
    );

    let risk_actor = RiskActor::new(
        &config.risk,
        signal_rx,
        order_tx,
        execution_event_tx.subscribe(),
    );

    CommonActors {
        market_actor,
        feed_actor,
        strategy_actor,
        risk_actor,
        symbols,
    }
}

// ---------------------------------------------------------------------------
// Run paper mode — full actor orchestration with real market data
// ---------------------------------------------------------------------------

async fn run_paper(config: BotConfig) -> Result<()> {
    info!(mode = "paper", "starting actor orchestration with real market data");

    let channels = create_actor_channels();
    let registry = build_strategies(&config);
    info!(count = registry.len(), "strategies loaded");

    // Use real Polymarket market data (same as live mode)
    let discovery = GammaDiscovery::new();

    let mut actors = build_common_actors(
        &config,
        channels.market_event_tx.clone(),
        channels.feed_event_tx.clone(),
        channels.execution_event_tx.clone(),
        channels.signal_tx.clone(),
        channels.signal_rx,
        channels.order_tx.clone(),
        channels.raw_trade_rx,
        registry,
    );

    // Use PaperExecutor for simulated order execution (no real orders)
    let paper_executor = PaperExecutor::new(
        config.risk.bankroll,
        dec!(0.50), // Default mid price for simulation
        MarketId("paper-mode".into()),
    );
    let executor_actor = ExecutorActor::new(paper_executor, channels.order_rx, channels.execution_event_tx.clone());

    info!("spawning all actors...");

    // Use real Binance WebSocket for price feed (same as live mode)
    let ws_symbols = actors.symbols.clone();
    let shutdown_rx_ws = channels.shutdown_tx.subscribe();
    let binance_ws = tokio::spawn(async move {
        run_binance_ws(&ws_symbols, channels.raw_trade_tx, shutdown_rx_ws).await;
    });

    let market = tokio::spawn(async move {
        if let Err(e) = actors.market_actor.run(&discovery, channels.book_rx).await {
            error!(error = %e, "market actor error");
        }
    });

    let feed = tokio::spawn(async move {
        actors.feed_actor.run().await;
    });
    let strat = tokio::spawn(async move {
        actors.strategy_actor.run().await;
    });
    let risk = tokio::spawn(async move {
        actors.risk_actor.run().await;
    });
    let exec = tokio::spawn(async move {
        executor_actor.run().await;
    });

    info!("all actors running — press Ctrl+C to stop");
    println!("\n  [PAPER] Bot is running in paper mode. Press Ctrl+C to stop.\n");

    tokio::signal::ctrl_c()
        .await
        .context("Ctrl+C listener failed")?;

    info!("shutting down...");
    let _ = channels.shutdown_tx.send(());
    tokio::time::sleep(Duration::from_millis(300)).await;

    binance_ws.abort();
    market.abort();
    feed.abort();
    strat.abort();
    risk.abort();
    exec.abort();

    println!("\n  Bot stopped. Goodbye!\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Run live mode — real SDK + real feeds
// ---------------------------------------------------------------------------

async fn run_live(config: BotConfig) -> Result<()> {
    info!(mode = "live", "starting live actor orchestration");

    // --- Authenticate with Polymarket SDK ---
    use alloy::signers::Signer as _;
    use alloy::signers::local::LocalSigner;
    use polymarket_client_sdk::POLYGON;
    use polymarket_client_sdk::clob::{Client as ClobClient, Config as ClobConfig};
    use std::str::FromStr as _;

    let private_key = std::env::var("PMBOT_PRIVATE_KEY")
        .context("PMBOT_PRIVATE_KEY env var required for live mode")?;

    let signer = LocalSigner::from_str(&private_key)?.with_chain_id(Some(POLYGON));

    let clob_config = ClobConfig::builder().use_server_time(true).build();
    let clob_client = ClobClient::new(&config.clob.host, clob_config)?
        .authentication_builder(&signer)
        .authenticate()
        .await
        .context("CLOB authentication failed")?;

    info!("authenticated with Polymarket CLOB");

    let channels = create_actor_channels();
    let registry = build_strategies(&config);
    info!(count = registry.len(), "strategies loaded");

    let discovery = GammaDiscovery::new();
    let mut actors = build_common_actors(
        &config,
        channels.market_event_tx.clone(),
        channels.feed_event_tx.clone(),
        channels.execution_event_tx.clone(),
        channels.signal_tx.clone(),
        channels.signal_rx,
        channels.order_tx.clone(),
        channels.raw_trade_rx,
        registry,
    );

    let live_executor = LiveExecutor::new(clob_client, signer);
    let executor_actor = ExecutorActor::new(live_executor, channels.order_rx, channels.execution_event_tx.clone());

    info!("spawning all actors...");

    let ws_symbols = actors.symbols.clone();
    let shutdown_rx_ws = channels.shutdown_tx.subscribe();
    let binance_ws = tokio::spawn(async move {
        run_binance_ws(&ws_symbols, channels.raw_trade_tx, shutdown_rx_ws).await;
    });

    let market = tokio::spawn(async move {
        if let Err(e) = actors.market_actor.run(&discovery, channels.book_rx).await {
            error!(error = %e, "market actor error");
        }
    });

    let feed = tokio::spawn(async move {
        actors.feed_actor.run().await;
    });
    let strat = tokio::spawn(async move {
        actors.strategy_actor.run().await;
    });
    let risk = tokio::spawn(async move {
        actors.risk_actor.run().await;
    });
    let exec = tokio::spawn(async move {
        executor_actor.run().await;
    });

    info!("all actors running — press Ctrl+C to stop");
    println!("\n  [LIVE] Bot is running in LIVE mode. Press Ctrl+C to stop.\n");
    println!("  WARNING: REAL MONEY — orders will be submitted to Polymarket.\n");

    tokio::signal::ctrl_c()
        .await
        .context("Ctrl+C listener failed")?;

    info!("shutting down...");
    let _ = channels.shutdown_tx.send(());
    tokio::time::sleep(Duration::from_millis(500)).await;

    binance_ws.abort();
    market.abort();
    feed.abort();
    strat.abort();
    risk.abort();
    exec.abort();

    println!("\n  Bot stopped. Goodbye!\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            config: config_path,
            paper,
            strategies,
        } => {
            let mut config = BotConfig::load(&config_path)
                .with_context(|| format!("failed to load config from {}", config_path.display()))?;

            if paper {
                config.general.mode = "paper".into();
            }
            if let Some(strats) = strategies {
                config.general.strategies = strats;
            }

            init_tracing(&config.general.log_level)?;
            print_banner();

            println!(
                "  Mode:       {}\n  Strategies: [{}]\n  Bankroll:   ${}\n",
                config.general.mode,
                config.general.strategies.join(", "),
                config.risk.bankroll,
            );

            match config.general.mode.as_str() {
                "paper" => run_paper(config).await,
                "live" => run_live(config).await,
                other => anyhow::bail!("unknown mode: {other}"),
            }
        }

        Command::Config {
            action: ConfigAction::Show {
                config: config_path,
            },
        } => {
            let config = BotConfig::load(&config_path)
                .with_context(|| format!("failed to load config from {}", config_path.display()))?;
            let toml_str = toml::to_string_pretty(&config).context("failed to serialize config")?;
            println!("{toml_str}");
            Ok(())
        }
    }
}

fn init_tracing(level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
    Ok(())
}
