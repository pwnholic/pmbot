use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use pmbot_core::BotConfig;
use pmbot_core::messages::{ExecutableOrder, ExecutionEvent, FeedEvent, MarketEvent, Signal};
use pmbot_core::types::{MarketId, Symbol};
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
        #[arg(long, conflicts_with = "live")]
        paper: bool,

        /// Force live trading mode (REAL MONEY).
        #[arg(long, conflicts_with = "paper")]
        live: bool,

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
            "lead_lag" => {
                if !config.strategy.lead_lag.enabled { continue; }
                Box::new(LeadLag::new(
                    config.strategy.lead_lag.lag_threshold,
                    config.strategy.lead_lag.entry_delay_ms,
                    config.strategy.lead_lag.exit_convergence_pct,
                ))
            },
            "fair_value" => {
                if !config.strategy.fair_value.enabled { continue; }
                Box::new(FairValue::new(
                    config.strategy.fair_value.vol_multiplier,
                    config.strategy.fair_value.min_time_to_expiry_secs,
                    config.risk.min_edge,
                ))
            },
            "flash_crash" => {
                if !config.strategy.flash_crash.enabled { continue; }
                Box::new(FlashCrash::new(
                    config.strategy.flash_crash.drop_threshold,
                    config.strategy.flash_crash.lookback_secs,
                    config.strategy.flash_crash.reversion_target,
                    dec!(0.1),
                ))
            },
            "book_imbalance" => {
                if !config.strategy.book_imbalance.enabled { continue; }
                Box::new(BookImbalance::new(
                    config.strategy.book_imbalance.imbalance_threshold,
                    config.strategy.book_imbalance.levels,
                    5,
                    config.risk.min_edge,
                ))
            },
            "negrisk_arb" => {
                if !config.strategy.negrisk_arb.enabled { continue; }
                Box::new(NegRiskArb::new(
                    config.strategy.negrisk_arb.sum_deviation_threshold,
                ))
            },
            "convergence" => {
                if !config.strategy.convergence.enabled { continue; }
                Box::new(Convergence::new(
                    config.strategy.convergence.min_probability,
                    config.strategy.convergence.max_time_to_expiry_secs as i64,
                    config.risk.min_edge,
                ))
            },
            "market_maker" => {
                if !config.strategy.market_maker.enabled { continue; }
                Box::new(MarketMaker::new(
                    config.strategy.market_maker.spread_bps as u32,
                    Decimal::from(config.strategy.market_maker.max_inventory),
                    Decimal::from(config.strategy.market_maker.quote_size),
                    config.strategy.market_maker.refresh_interval_ms,
                ))
            },
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
    #[allow(dead_code)]
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

async fn run_paper(config: BotConfig, config_path: PathBuf) -> Result<()> {
    info!(mode = "paper", "starting actor orchestration with real market data");

    let (_watcher, mut config_rx) = pmbot_core::config_watcher::start_config_watcher(
        config_path,
        std::time::Duration::from_secs(5),
    )?;
    tokio::spawn(async move {
        while let Ok(event) = config_rx.recv().await {
            info!(?event, "config reloaded");
        }
    });

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

    let shutdown_rx_pm = channels.shutdown_tx.subscribe();
    let pm_event_rx = channels.market_event_tx.subscribe();
    let pm_book_tx = channels.book_tx.clone();
    let polymarket_ws = tokio::spawn(async move {
        pmbot_market::run_polymarket_ws(pm_event_rx, pm_book_tx, shutdown_rx_pm).await;
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
    println!(
        "\n  [PAPER] Bot is running in paper mode. Press Ctrl+C to stop.\n"
    );

    tokio::signal::ctrl_c()
        .await
        .context("Ctrl+C listener failed")?;

    info!("shutting down...");
    let _ = channels.shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = tokio::join!(binance_ws, polymarket_ws, market, feed, strat, risk, exec);
    }).await;

    println!("\n  Bot stopped. Goodbye!\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Run live mode — real SDK + real feeds
// ---------------------------------------------------------------------------

async fn run_live(config: BotConfig, config_path: PathBuf) -> Result<()> {
    info!(mode = "live", "starting live actor orchestration");

    let (_watcher, mut config_rx) = pmbot_core::config_watcher::start_config_watcher(
        config_path,
        std::time::Duration::from_secs(5),
    )?;
    tokio::spawn(async move {
        while let Ok(event) = config_rx.recv().await {
            info!(?event, "config reloaded");
        }
    });

    // --- Safety confirmation ---
    eprintln!();
    eprintln!("  ============================================");
    eprintln!("  WARNING: You are about to start LIVE trading");
    eprintln!("  Real orders will be placed with real money.");
    eprintln!("  Bankroll: ${}", config.risk.bankroll);
    eprintln!("  Max position: {}% of bankroll", config.risk.max_position_pct * dec!(100));
    eprintln!("  Daily loss limit: {}%", config.risk.daily_loss_limit_pct * dec!(100));
    eprintln!("  Kill switch: {}", config.risk.kill_switch_path);
    eprintln!("  ============================================");
    eprintln!();
    eprint!("  Type 'yes' to confirm: ");
    use std::io::BufRead;
    let mut input = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut input)
        .context("failed to read confirmation")?;
    if input.trim().to_lowercase() != "yes" {
        eprintln!("  Aborted. No orders will be placed.");
        return Ok(());
    }
    eprintln!();

    // --- Authenticate with Polymarket SDK ---
    use alloy::signers::Signer as _;
    use alloy::signers::local::LocalSigner;
    use alloy::primitives::Address;
    use polymarket_client_sdk::POLYGON;
    use polymarket_client_sdk::clob::{Client as ClobClient, Config as ClobConfig};
    use polymarket_client_sdk::clob::types::SignatureType as SdkSignatureType;
    use std::str::FromStr as _;

    let private_key = std::env::var("PMBOT_PRIVATE_KEY")
        .context("PMBOT_PRIVATE_KEY env var required for live mode")?;

    let signer = LocalSigner::from_str(&private_key)?.with_chain_id(Some(POLYGON));
    info!(address = %signer.address(), "signer loaded");

    // Determine SDK signature type from config
    let sdk_sig_type = match config.wallet.signature_type {
        pmbot_core::types::SignatureType::Eoa => SdkSignatureType::Eoa,
        pmbot_core::types::SignatureType::Proxy => SdkSignatureType::Proxy,
        pmbot_core::types::SignatureType::GnosisSafe => SdkSignatureType::GnosisSafe,
    };

    let clob_config = ClobConfig::builder().use_server_time(true).build();

    let mut auth_builder = ClobClient::new(&config.clob.host, clob_config)?
        .authentication_builder(&signer)
        .signature_type(sdk_sig_type);

    // If user provides an explicit funder/safe address, use it
    if let Ok(funder_hex) = std::env::var("POLY_SAFE_ADDRESS") {
        let funder = Address::from_str(&funder_hex)
            .context("invalid POLY_SAFE_ADDRESS")?;
        info!(%funder, "using explicit funder/safe address");
        auth_builder = auth_builder.funder(funder);
    }

    let clob_client = auth_builder
        .authenticate()
        .await
        .context("CLOB authentication failed")?;

    info!(sig_type = ?config.wallet.signature_type, "authenticated with Polymarket CLOB");

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

    let shutdown_rx_pm = channels.shutdown_tx.subscribe();
    let pm_event_rx = channels.market_event_tx.subscribe();
    let pm_book_tx = channels.book_tx.clone();
    let polymarket_ws = tokio::spawn(async move {
        pmbot_market::run_polymarket_ws(pm_event_rx, pm_book_tx, shutdown_rx_pm).await;
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
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = tokio::join!(binance_ws, polymarket_ws, market, feed, strat, risk, exec);
    }).await;

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
            live,
            strategies,
        } => {
            let mut config = BotConfig::load(&config_path)
                .with_context(|| format!("failed to load config from {}", config_path.display()))?;

            if paper {
                config.general.mode = "paper".into();
            } else if live {
                config.general.mode = "live".into();
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
                "paper" => run_paper(config, config_path).await,
                "live" => run_live(config, config_path).await,
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
    // Suppress noisy "unknown field" warnings from the SDK's serde deserializer.
    // The Gamma API returns fields (feeType, eventMetadata) the SDK doesn't model yet.
    let default_filter = format!("{level},polymarket_client_sdk::serde_helpers=error");
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
    Ok(())
}
