use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::future::Future;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use pmbot_core::BotConfig;
use pmbot_core::messages::{
    ExecutableOrder, ExecutionEvent, FeedEvent, MarketEvent, PositionSnapshot, Signal,
};
use pmbot_core::types::{MarketId, Symbol};
use pmbot_db::Database;
use pmbot_executor::{ExecutorActor, LiveExecutor, PaperExecutor};
use pmbot_feed::{FeedActor, RawFeedMessage, VolMethod, run_binance_ws};
use pmbot_market::actor::RawBookMessage;
use pmbot_market::{GammaDiscovery, MarketActor};
use pmbot_risk::RiskActor;
use pmbot_strategy::{
    BookImbalance, Convergence, FairValue, FlashCrash, LeadLag, MarketMaker, NegRiskArb, Strategy,
    StrategyActor, StrategyRegistry,
};

// ---------------------------------------------------------------------------
// ActorSupervisor - monitors actors for crashes and handles restarts
// ---------------------------------------------------------------------------

/// Actor supervisor using JoinSet for crash notification and restart logic.
struct ActorSupervisor {
    actors: JoinSet<Result<()>>,
}

impl ActorSupervisor {
    fn new() -> Self {
        Self {
            actors: JoinSet::new(),
        }
    }

    /// Spawn an actor with crash logging.
    fn spawn<F>(&mut self, name: &'static str, fut: F)
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        self.actors.spawn(async move {
            match fut.await {
                Ok(()) => {
                    tracing::info!(actor = name, "actor completed successfully");
                    Ok(())
                }
                Err(e) => {
                    tracing::error!(actor = name, error = %e, "actor failed");
                    Err(e)
                }
            }
        });
    }

    /// Spawn an actor with automatic restart on failure.
    fn spawn_with_restart<F, Factory>(
        &mut self,
        name: &'static str,
        factory: Factory,
        max_restarts: u32,
    )
    where
        F: Future<Output = Result<()>> + Send + 'static,
        Factory: Fn() -> F + Send + Sync + 'static,
    {
        self.actors.spawn(async move {
            let mut restarts = 0u32;
            loop {
                match factory().await {
                    Ok(()) => {
                        tracing::info!(actor = name, "actor completed successfully");
                        return Ok(());
                    }
                    Err(e) => {
                        restarts += 1;
                        if restarts > max_restarts {
                            tracing::error!(actor = name, restarts, "max restarts reached, giving up");
                            return Err(e);
                        }
                        
                        let backoff = Duration::from_millis(100 *2u64.pow(restarts.min(8)));
                        tracing::warn!(
                            actor = name,
                            error = %e,
                            restarts,
                            backoff_ms = backoff.as_millis() as u64,
                            "actor failed, restarting"
                        );
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        });
    }

    /// Wait for all actors to complete, logging any failures.
    /// Returns Ok if all actors completed successfully, Err if any failed.
    async fn wait_all(mut self) -> Result<()> {
        let mut failed = Vec::new();
        while let Some(result) = self.actors.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    failed.push(e);
                }
                Err(e) => {
                    failed.push(anyhow::anyhow!("actor panicked: {}", e));
                }
            }
        }
        
        if failed.is_empty() {
            Ok(())
        } else {
            for e in &failed {
                tracing::error!(error = %e, "actor failed during shutdown");
            }
            Err(anyhow::anyhow!("{} actors failed", failed.len()))
        }
    }

    /// Wait for actors with timeout.
    async fn wait_all_with_timeout(self, timeout: Duration) -> Result<()> {
        match tokio::time::timeout(timeout, self.wait_all()).await {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(timeout_secs = timeout.as_secs(), "timeout waiting for actors to stop");
                Err(anyhow::anyhow!("timeout waiting for actors"))
            }
        }
    }
}

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
        /// Path to config YAML file.
        #[arg(short, long, default_value = "config/default.yaml")]
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

        /// Enable the TUI dashboard.
        #[arg(long)]
        tui: bool,
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
        /// Path to config YAML file.
        #[arg(short, long, default_value = "config/default.yaml")]
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
                if !config.strategy.lead_lag.enabled {
                    continue;
                }
                Box::new(LeadLag::new(
                    config.strategy.lead_lag.lag_threshold,
                    config.strategy.lead_lag.entry_delay_ms,
                    config.strategy.lead_lag.exit_convergence_pct,
                ))
            }
            "fair_value" => {
                if !config.strategy.fair_value.enabled {
                    continue;
                }
                Box::new(FairValue::new(
                    config.strategy.fair_value.vol_multiplier,
                    config.strategy.fair_value.min_time_to_expiry_secs,
                    config.strategy.fair_value.min_activation_edge,
                ))
            }
            "flash_crash" => {
                if !config.strategy.flash_crash.enabled {
                    continue;
                }
                Box::new(FlashCrash::new(
                    config.strategy.flash_crash.drop_threshold,
                    config.strategy.flash_crash.lookback_secs,
                    config.strategy.flash_crash.reversion_target,
                    config.strategy.flash_crash.min_recovery_imbalance,
                ))
            }
            "book_imbalance" => {
                if !config.strategy.book_imbalance.enabled {
                    continue;
                }
                Box::new(BookImbalance::new(
                    config.strategy.book_imbalance.imbalance_threshold,
                    config.strategy.book_imbalance.levels,
                    config.strategy.book_imbalance.momentum_window,
                    config.strategy.book_imbalance.min_activation_edge,
                ))
            }
            "negrisk_arb" => {
                if !config.strategy.negrisk_arb.enabled {
                    continue;
                }
                Box::new(NegRiskArb::new(
                    config.strategy.negrisk_arb.sum_deviation_threshold,
                ))
            }
            "convergence" => {
                if !config.strategy.convergence.enabled {
                    continue;
                }
                Box::new(Convergence::new(
                    config.strategy.convergence.min_probability,
                    config.strategy.convergence.max_time_to_expiry_secs as i64,
                    config.strategy.convergence.min_activation_edge,
                ))
            }
            "market_maker" => {
                if !config.strategy.market_maker.enabled {
                    continue;
                }
                Box::new(MarketMaker::new(
                    config.strategy.market_maker.spread_bps as u32,
                    Decimal::from(config.strategy.market_maker.max_inventory),
                    Decimal::from(config.strategy.market_maker.quote_size),
                    config.strategy.market_maker.refresh_interval_ms,
                ))
            }
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
    raw_trade_tx: mpsc::Sender<RawFeedMessage>,
    raw_trade_rx: mpsc::Receiver<RawFeedMessage>,
    shutdown_tx: broadcast::Sender<()>,
    position_tx: tokio::sync::watch::Sender<PositionSnapshot>,
    position_rx: tokio::sync::watch::Receiver<PositionSnapshot>,
    world_tx: tokio::sync::watch::Sender<pmbot_core::messages::WorldState>,
    world_rx: tokio::sync::watch::Receiver<pmbot_core::messages::WorldState>,
    tui_tx: Option<
        tokio::sync::watch::Sender<
            Option<(
                pmbot_core::messages::WorldState,
                Vec<pmbot_core::messages::StrategyMetrics>,
            )>,
        >,
    >,
    tui_rx: Option<
        tokio::sync::watch::Receiver<
            Option<(
                pmbot_core::messages::WorldState,
                Vec<pmbot_core::messages::StrategyMetrics>,
            )>,
        >,
    >,
}

fn create_actor_channels(use_tui: bool) -> ActorChannels {
    let (market_event_tx, _) = broadcast::channel(2048);
    let (feed_event_tx, _) = broadcast::channel(2048);
    let (execution_event_tx, _) = broadcast::channel(2048);
    let (signal_tx, signal_rx) = mpsc::channel(64);
    let (order_tx, order_rx) = mpsc::channel(64);
    let (book_tx, book_rx) = mpsc::channel(256);
    let (raw_trade_tx, raw_trade_rx) = mpsc::channel(256);
    let (shutdown_tx, _) = broadcast::channel(1);
    let (position_tx, position_rx) = tokio::sync::watch::channel(PositionSnapshot {
        positions: vec![],
        daily_pnl: Decimal::ZERO,
    });
    let (world_tx, world_rx) =
        tokio::sync::watch::channel(pmbot_core::messages::WorldState::default());

    let (tui_tx, tui_rx) = if use_tui {
        let (tx, rx) = tokio::sync::watch::channel(None);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

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
        position_tx,
        position_rx,
        world_tx,
        world_rx,
        tui_tx,
        tui_rx,
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
    raw_trade_rx: mpsc::Receiver<RawFeedMessage>,
    position_tx: tokio::sync::watch::Sender<PositionSnapshot>,
    position_rx: tokio::sync::watch::Receiver<PositionSnapshot>,
    world_tx: tokio::sync::watch::Sender<pmbot_core::messages::WorldState>,
    world_rx: tokio::sync::watch::Receiver<pmbot_core::messages::WorldState>,
    tui_tx: Option<
        tokio::sync::watch::Sender<
            Option<(
                pmbot_core::messages::WorldState,
                Vec<pmbot_core::messages::StrategyMetrics>,
            )>,
        >,
    >,
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
        position_rx,
        signal_tx,
        world_tx,
        tui_tx,
        100,
    );

    let risk_actor = RiskActor::new(
        &config.risk,
        signal_rx,
        order_tx,
        execution_event_tx.subscribe(),
        market_event_tx.subscribe(),
        position_tx,
        world_rx,
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
    info!(
        mode = "paper",
        "starting actor orchestration with real market data"
    );

    let (_watcher, mut config_rx) = pmbot_core::config_watcher::start_config_watcher(
        config_path.clone(),
        std::time::Duration::from_secs(5),
    )?;

    // Create a watch channel for propagating config updates
    let (config_tx, _) = tokio::sync::watch::channel(config.clone());

    tokio::spawn(async move {
        while let Ok(event) = config_rx.recv().await {
            match event {
                pmbot_core::ConfigEvent::Reloaded(new_config) => {
                    if new_config.validate().is_ok() {
                        info!(
                            mode = %new_config.general.mode,
                            bankroll = %new_config.risk.bankroll,
                            "config hot-reloaded successfully"
                        );
                        let _ = config_tx.send(new_config);
                    } else {
                        warn!("invalid config reloaded, keeping previous configuration");
                    }
                }
                pmbot_core::ConfigEvent::Invalid(err) => {
                    warn!(error = %err, "config reload failed validation");
                }
                pmbot_core::ConfigEvent::ReloadFailed(err) => {
                    warn!(error = %err, "config reload failed");
                }
            }
        }
    });

    let use_tui = config.tui.enabled;
    let mut channels = create_actor_channels(use_tui);
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
        channels.position_tx,
        channels.position_rx,
        channels.world_tx,
        channels.world_rx,
        channels.tui_tx.take(),
        registry,
    );

    // Use PaperExecutor for simulated order execution (no real orders)
    let paper_executor = PaperExecutor::new(
        config.risk.bankroll,
        dec!(0.50),
        MarketId("paper-mode".into()),
    );

    // Initialize database for persistence
    let db_path = config.general.data_dir.join("pmbot.db");
    let db_path_str = db_path.to_string_lossy().into_owned();
    let db = match Database::connect(&db_path_str).await {
        Ok(db) => {
            info!(path = %db_path.display(), "database connected");
            let db = Arc::new(db);
            // Record startup time
            if let Err(e) = db.state().record_startup().await {
                warn!(error = %e, "failed to record startup time");
            }
            Some(db)
        }
        Err(e) => {
            warn!(error = %e, "failed to connect database, persistence disabled");
            None
        }
    };

    // Spawn a task that updates paper executor's mid price from live market data
    let paper_exec_clone = paper_executor.clone();
    let mut paper_market_rx = channels.market_event_tx.subscribe();
    tokio::spawn(async move {
        loop {
            match paper_market_rx.recv().await {
                Ok(MarketEvent::PriceChange { price, .. }) => {
                    paper_exec_clone.update_mid_price(price).await;
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    });

    let mut executor_actor = ExecutorActor::new(
        paper_executor,
        channels.order_rx,
        channels.execution_event_tx.clone(),
    );
    
    // Wire up database for persistence
    if let Some(ref db) = db {
        executor_actor = executor_actor.with_db(Arc::clone(db));
    }

    // Run reconciliation before starting actors
    info!("running startup reconciliation...");
    match executor_actor.reconcile().await {
        Ok(events) => {
            for event in events {
                let _ = channels.execution_event_tx.send(event);
            }
            info!(tracked = executor_actor.tracked_orders().len(), "reconciliation complete");
        }
        Err(e) => {
            error!(error = %e, "reconciliation failed, continuing anyway");
        }
    }

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

    // Use ActorSupervisor for crash notification
    let mut supervisor = ActorSupervisor::new();
    
    supervisor.spawn("market", async move {
        if let Err(e) = actors.market_actor.run(&discovery, channels.book_rx).await {
            error!(error = %e, "market actor error");
            Err(e)
        } else {
            Ok(())
        }
    });
    
    supervisor.spawn("feed", async move {
        actors.feed_actor.run().await;
        Ok(())
    });
    
    supervisor.spawn("strategy", async move {
        actors.strategy_actor.run().await;
        Ok(())
    });
    
    supervisor.spawn("risk", async move {
        actors.risk_actor.run().await;
        Ok(())
    });
    
    supervisor.spawn("executor", async move {
        executor_actor.run().await;
        Ok(())
    });

    let tui_rx_opt = channels.tui_rx.take();
    let tui_shutdown_tx = channels.shutdown_tx.clone();
    let _tui = tokio::spawn(async move {
        if let Some(tui_rx) = tui_rx_opt {
            if let Err(e) = pmbot_tui::run::run_tui(tui_rx, tui_shutdown_tx).await {
                error!("TUI error: {}", e);
            }
        }
    });

    if !use_tui {
        info!("all actors running — press Ctrl+C to stop");
        println!("\n  [PAPER] Bot is running in paper mode. Press Ctrl+C to stop.\n");
    }

    tokio::signal::ctrl_c()
        .await
        .context("Ctrl+C listener failed")?;

    if !use_tui {
        info!("shutting down...");
    }
    
    // Send shutdown signal to all actors
    let _ = channels.shutdown_tx.send(());
    
    // Wait for actors to stop with timeout
    match tokio::time::timeout(Duration::from_secs(10), supervisor.wait_all()).await {
        Ok(Ok(())) => {
            info!("all actors stopped gracefully");
        }
        Ok(Err(e)) => {
            warn!(error = %e, "some actors failed during shutdown");
        }
        Err(_) => {
            warn!("timeout waiting for actors to stop, forcing shutdown");
        }
    }

    // Wait for websocket tasks
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = binance_ws.await;
        let _ = polymarket_ws.await;
    })
    .await;

    // Record graceful shutdown in database
    if let Some(ref db) = db {
        if let Err(e) = db.state().record_shutdown().await {
            warn!(error = %e, "failed to record shutdown time");
        } else {
            info!("shutdown time recorded");
        }
    }

    println!("\n  Bot stopped. Goodbye!\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Run live mode — real SDK + real feeds
// ---------------------------------------------------------------------------

async fn run_live(mut config: BotConfig, config_path: PathBuf) -> Result<()> {
    info!(mode = "live", "starting live actor orchestration");

    let (_watcher, mut config_rx) = pmbot_core::config_watcher::start_config_watcher(
        config_path.clone(),
        std::time::Duration::from_secs(5),
    )?;

    // Create a watch channel for propagating config updates
    let (config_tx, _) = tokio::sync::watch::channel(config.clone());

    tokio::spawn(async move {
        while let Ok(event) = config_rx.recv().await {
            match event {
                pmbot_core::ConfigEvent::Reloaded(new_config) => {
                    if new_config.validate().is_ok() {
                        info!(
                            mode = %new_config.general.mode,
                            bankroll = %new_config.risk.bankroll,
                            "config hot-reloaded successfully"
                        );
                        let _ = config_tx.send(new_config);
                    } else {
                        warn!("invalid config reloaded, keeping previous configuration");
                    }
                }
                pmbot_core::ConfigEvent::Invalid(err) => {
                    warn!(error = %err, "config reload failed validation");
                }
                pmbot_core::ConfigEvent::ReloadFailed(err) => {
                    warn!(error = %err, "config reload failed");
                }
            }
        }
    });

    // --- Safety confirmation ---
    eprintln!();
    eprintln!("  ============================================");
    eprintln!("  WARNING: You are about to start LIVE trading");
    eprintln!("  Real orders will be placed with real money.");
    eprintln!("  Bankroll: ${}", config.risk.bankroll);
    eprintln!(
        "  Max position: {}% of bankroll",
        config.risk.max_position_pct * dec!(100)
    );
    eprintln!(
        "  Daily loss limit: {}%",
        config.risk.daily_loss_limit_pct * dec!(100)
    );
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
    use alloy::primitives::Address;
    use alloy::signers::Signer as _;
    use alloy::signers::local::LocalSigner;
    use polymarket_client_sdk::POLYGON;
    use polymarket_client_sdk::clob::types::SignatureType as SdkSignatureType;
    use polymarket_client_sdk::clob::{Client as ClobClient, Config as ClobConfig};
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
        let funder = Address::from_str(&funder_hex).context("invalid POLY_SAFE_ADDRESS")?;
        info!(%funder, "using explicit funder/safe address");
        auth_builder = auth_builder.funder(funder);
    }

    let clob_client = auth_builder
        .authenticate()
        .await
        .context("CLOB authentication failed")?;
    
    info!(sig_type = ?config.wallet.signature_type, "authenticated with Polymarket CLOB");

    // Fetch real balance from Polymarket for live mode
    let live_executor_temp = LiveExecutor::new(clob_client.clone(), signer.clone());
    let live_balance = live_executor_temp.get_balance().await?;
    info!(%live_balance, "fetched live account balance from Polymarket");
    
    // Update bankroll with actual balance
    config.risk.bankroll = live_balance;
    info!(bankroll = %config.risk.bankroll, "updated bankroll from live account");

    let use_tui = config.tui.enabled;
    let mut channels = create_actor_channels(use_tui);
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
        channels.position_tx,
        channels.position_rx,
        channels.world_tx,
        channels.world_rx,
        channels.tui_tx.take(),
        registry,
    );

    let live_executor = LiveExecutor::new(clob_client, signer);

    // Initialize database for persistence
    let db_path = config.general.data_dir.join("pmbot.db");
    let db_path_str = db_path.to_string_lossy().into_owned();
    let db = match Database::connect(&db_path_str).await {
        Ok(db) => {
            info!(path = %db_path.display(), "database connected");
            let db = Arc::new(db);
            // Record startup time
            if let Err(e) = db.state().record_startup().await {
                warn!(error = %e, "failed to record startup time");
            }
            Some(db)
        }
        Err(e) => {
            warn!(error = %e, "failed to connect database, persistence disabled");
            None
        }
    };

    let mut executor_actor = ExecutorActor::new(
        live_executor,
        channels.order_rx,
        channels.execution_event_tx.clone(),
    );
    
    // Wire up database for persistence
    if let Some(ref db) = db {
        executor_actor = executor_actor.with_db(Arc::clone(db));
    }

    // Run reconciliation before starting actors
    info!("running startup reconciliation...");
    match executor_actor.reconcile().await {
        Ok(events) => {
            for event in events {
                let _ = channels.execution_event_tx.send(event);
            }
            info!(tracked = executor_actor.tracked_orders().len(), "reconciliation complete");
        }
        Err(e) => {
            error!(error = %e, "reconciliation failed, continuing anyway");
        }
    }

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

    // Use ActorSupervisor for crash notification
    let mut supervisor = ActorSupervisor::new();
    
    supervisor.spawn("market", async move {
        if let Err(e) = actors.market_actor.run(&discovery, channels.book_rx).await {
            error!(error = %e, "market actor error");
            Err(e)
        } else {
            Ok(())
        }
    });
    
    supervisor.spawn("feed", async move {
        actors.feed_actor.run().await;
        Ok(())
    });
    
    supervisor.spawn("strategy", async move {
        actors.strategy_actor.run().await;
        Ok(())
    });
    
    supervisor.spawn("risk", async move {
        actors.risk_actor.run().await;
        Ok(())
    });
    
    supervisor.spawn("executor", async move {
        executor_actor.run().await;
        Ok(())
    });

    let tui_rx_opt = channels.tui_rx.take();
    let tui_shutdown_tx = channels.shutdown_tx.clone();
    let _tui = tokio::spawn(async move {
        if let Some(tui_rx) = tui_rx_opt {
            if let Err(e) = pmbot_tui::run::run_tui(tui_rx, tui_shutdown_tx).await {
                error!("TUI error: {}", e);
            }
        }
    });

    if !use_tui {
        info!("all actors running — press Ctrl+C to stop");
        println!("\n  [LIVE] Bot is running in LIVE mode. Press Ctrl+C to stop.\n");
        println!("  WARNING: REAL MONEY — orders will be submitted to Polymarket.\n");
    }

    tokio::signal::ctrl_c()
        .await
        .context("Ctrl+C listener failed")?;

    if !use_tui {
        info!("shutting down...");
    }
    
    // Send shutdown signal to all actors
    let _ = channels.shutdown_tx.send(());
    
    // Wait for actors to stop with timeout
    match tokio::time::timeout(Duration::from_secs(10), supervisor.wait_all()).await {
        Ok(Ok(())) => {
            info!("all actors stopped gracefully");
        }
        Ok(Err(e)) => {
            warn!(error = %e, "some actors failed during shutdown");
        }
        Err(_) => {
            warn!("timeout waiting for actors to stop, forcing shutdown");
        }
    }

    // Wait for websocket tasks
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = binance_ws.await;
        let _ = polymarket_ws.await;
    })
    .await;

    // Record graceful shutdown in database
    if let Some(ref db) = db {
        if let Err(e) = db.state().record_shutdown().await {
            warn!(error = %e, "failed to record shutdown time");
        } else {
            info!("shutdown time recorded");
        }
    }

    if !use_tui {
        println!("\n  Bot stopped. Goodbye!\n");
    }
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
            tui,
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

            if tui {
                config.tui.enabled = true;
            }

            let _log_guard = init_tracing(&config.general.log_level, config.tui.enabled)?;

            if !config.tui.enabled {
                print_banner();
            }

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

fn init_tracing(
    level: &str,
    use_tui: bool,
) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let default_filter = format!("{level},polymarket_client_sdk::serde_helpers=error");
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    if use_tui {
        let file_appender = tracing_appender::rolling::daily("logs", "pmbot.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(non_blocking)
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false)
            .init();
        Ok(Some(guard))
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false)
            .init();
        Ok(None)
    }
}
