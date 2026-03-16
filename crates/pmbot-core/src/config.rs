use std::path::Path;

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::types::SignatureType;

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BotConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub wallet: WalletConfig,
    #[serde(default)]
    pub clob: ClobConfig,
    #[serde(default)]
    pub builder: BuilderConfig,
    #[serde(default)]
    pub market: MarketConfig,
    #[serde(default)]
    pub risk: RiskConfig,
    #[serde(default)]
    pub feed: FeedConfig,
    #[serde(default)]
    pub strategy: StrategyConfig,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub backtest: BacktestConfig,
}

impl BotConfig {
    /// Load config from a TOML file with environment variable overlay.
    ///
    /// Precedence: env vars (PMBOT_ prefix) > TOML file > defaults.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(ConfigError::NotFound {
                path: path.display().to_string(),
            });
        }

        let config: Self = Figment::new()
            .merge(Toml::file(path))
            .merge(Env::prefixed("PMBOT_").split("_").lowercase(true))
            .extract()
            .map_err(|e| ConfigError::Parse(e.to_string()))?;

        config.validate()?;
        Ok(config)
    }

    /// Load from figment with only defaults + env overlay (no TOML file).
    pub fn from_env() -> Result<Self, ConfigError> {
        let config: Self = Figment::new()
            .merge(Env::prefixed("PMBOT_").split("_").lowercase(true))
            .extract()
            .map_err(|e| ConfigError::Parse(e.to_string()))?;

        config.validate()?;
        Ok(config)
    }

    /// Validate the loaded config for logical consistency.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.risk.bankroll <= Decimal::ZERO {
            return Err(ConfigError::Invalid {
                field: "risk.bankroll".into(),
                reason: "must be positive".into(),
            });
        }
        if self.risk.kelly_fraction <= Decimal::ZERO || self.risk.kelly_fraction > Decimal::ONE {
            return Err(ConfigError::Invalid {
                field: "risk.kelly_fraction".into(),
                reason: "must be between 0 (exclusive) and 1 (inclusive)".into(),
            });
        }
        if self.risk.max_position_pct <= Decimal::ZERO || self.risk.max_position_pct > Decimal::ONE
        {
            return Err(ConfigError::Invalid {
                field: "risk.max_position_pct".into(),
                reason: "must be between 0 (exclusive) and 1 (inclusive)".into(),
            });
        }
        Ok(())
    }
}


// ---------------------------------------------------------------------------
// Section configs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub strategies: Vec<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            log_level: default_log_level(),
            strategies: vec!["lead_lag".into(), "fair_value".into()],
        }
    }
}

fn default_mode() -> String {
    "paper".into()
}
fn default_log_level() -> String {
    "info".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletConfig {
    #[serde(default)]
    pub encrypted_key_path: String,
    #[serde(default = "default_signature_type")]
    pub signature_type: SignatureType,
}

impl Default for WalletConfig {
    fn default() -> Self {
        Self {
            encrypted_key_path: String::new(),
            signature_type: default_signature_type(),
        }
    }
}

fn default_signature_type() -> SignatureType {
    SignatureType::GnosisSafe
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobConfig {
    #[serde(default = "default_clob_host")]
    pub host: String,
    #[serde(default = "default_ws_host")]
    pub ws_host: String,
    #[serde(default = "default_chain_id")]
    pub chain_id: u64,
}

impl Default for ClobConfig {
    fn default() -> Self {
        Self {
            host: default_clob_host(),
            ws_host: default_ws_host(),
            chain_id: default_chain_id(),
        }
    }
}

fn default_clob_host() -> String {
    "https://clob.polymarket.com".into()
}
fn default_ws_host() -> String {
    "wss://ws-subscriptions-clob.polymarket.com".into()
}
fn default_chain_id() -> u64 {
    137
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuilderConfig {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub api_passphrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketConfig {
    #[serde(default = "default_discovery_interval")]
    pub discovery_interval_secs: u64,
    #[serde(default = "default_min_liquidity")]
    pub min_liquidity: u64,
    #[serde(default = "default_min_volume")]
    pub min_volume: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_market_type")]
    pub market_type: String,
    #[serde(default = "default_no_trade_zone")]
    pub no_trade_zone_secs: u64,
    #[serde(default = "default_rotation_lookahead")]
    pub rotation_lookahead_secs: u64,
}

impl Default for MarketConfig {
    fn default() -> Self {
        Self {
            discovery_interval_secs: default_discovery_interval(),
            min_liquidity: default_min_liquidity(),
            min_volume: default_min_volume(),
            tags: vec!["crypto".into()],
            market_type: default_market_type(),
            no_trade_zone_secs: default_no_trade_zone(),
            rotation_lookahead_secs: default_rotation_lookahead(),
        }
    }
}

fn default_discovery_interval() -> u64 {
    60
}
fn default_min_liquidity() -> u64 {
    5000
}
fn default_min_volume() -> u64 {
    10000
}
fn default_market_type() -> String {
    "15min".into()
}
fn default_no_trade_zone() -> u64 {
    60
}
fn default_rotation_lookahead() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    #[serde(default = "default_bankroll")]
    pub bankroll: Decimal,
    #[serde(default = "default_kelly_fraction")]
    pub kelly_fraction: Decimal,
    #[serde(default = "default_max_position_pct")]
    pub max_position_pct: Decimal,
    #[serde(default = "default_max_positions")]
    pub max_positions: usize,
    #[serde(default = "default_daily_loss_limit_pct")]
    pub daily_loss_limit_pct: Decimal,
    #[serde(default = "default_stop_loss_pct")]
    pub stop_loss_pct: Decimal,
    #[serde(default = "default_take_profit_multiplier")]
    pub take_profit_multiplier: Decimal,
    #[serde(default = "default_min_edge")]
    pub min_edge: Decimal,
    #[serde(default = "default_kill_switch_path")]
    pub kill_switch_path: String,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            bankroll: default_bankroll(),
            kelly_fraction: default_kelly_fraction(),
            max_position_pct: default_max_position_pct(),
            max_positions: default_max_positions(),
            daily_loss_limit_pct: default_daily_loss_limit_pct(),
            stop_loss_pct: default_stop_loss_pct(),
            take_profit_multiplier: default_take_profit_multiplier(),
            min_edge: default_min_edge(),
            kill_switch_path: default_kill_switch_path(),
        }
    }
}

fn default_bankroll() -> Decimal {
    Decimal::new(1000, 0)
}
fn default_kelly_fraction() -> Decimal {
    Decimal::new(15, 2) // 0.15
}
fn default_max_position_pct() -> Decimal {
    Decimal::new(5, 2) // 0.05
}
fn default_max_positions() -> usize {
    5
}
fn default_daily_loss_limit_pct() -> Decimal {
    Decimal::new(5, 2) // 0.05
}
fn default_stop_loss_pct() -> Decimal {
    Decimal::new(30, 2) // 0.30
}
fn default_take_profit_multiplier() -> Decimal {
    Decimal::new(2, 0) // 2.0
}
fn default_min_edge() -> Decimal {
    Decimal::new(8, 2) // 0.08
}
fn default_kill_switch_path() -> String {
    "/tmp/pmbot-kill".into()
}

// ---------------------------------------------------------------------------
// Feed config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedConfig {
    #[serde(default)]
    pub binance: BinanceFeedConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinanceFeedConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_symbols")]
    pub symbols: Vec<String>,
    #[serde(default = "default_vol_window")]
    pub vol_window_secs: u64,
    #[serde(default = "default_vol_method")]
    pub vol_method: String,
}

impl Default for BinanceFeedConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            symbols: default_symbols(),
            vol_window_secs: default_vol_window(),
            vol_method: default_vol_method(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_symbols() -> Vec<String> {
    vec!["BTCUSDT".into(), "ETHUSDT".into(), "SOLUSDT".into()]
}
fn default_vol_window() -> u64 {
    900
}
fn default_vol_method() -> String {
    "ewma".into()
}

// ---------------------------------------------------------------------------
// Strategy configs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StrategyConfig {
    #[serde(default)]
    pub lead_lag: LeadLagConfig,
    #[serde(default)]
    pub fair_value: FairValueConfig,
    #[serde(default)]
    pub flash_crash: FlashCrashConfig,
    #[serde(default)]
    pub book_imbalance: BookImbalanceConfig,
    #[serde(default)]
    pub negrisk_arb: NegriskArbConfig,
    #[serde(default)]
    pub convergence: ConvergenceConfig,
    #[serde(default)]
    pub market_maker: MarketMakerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeadLagConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_lag_threshold")]
    pub lag_threshold: Decimal,
    #[serde(default = "default_entry_delay_ms")]
    pub entry_delay_ms: u64,
    #[serde(default = "default_exit_convergence_pct")]
    pub exit_convergence_pct: Decimal,
}

impl Default for LeadLagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            lag_threshold: default_lag_threshold(),
            entry_delay_ms: default_entry_delay_ms(),
            exit_convergence_pct: default_exit_convergence_pct(),
        }
    }
}

fn default_lag_threshold() -> Decimal {
    Decimal::new(2, 2) // 0.02
}
fn default_entry_delay_ms() -> u64 {
    500
}
fn default_exit_convergence_pct() -> Decimal {
    Decimal::new(5, 3) // 0.005
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FairValueConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_min_time_to_expiry_secs")]
    pub min_time_to_expiry_secs: u64,
    #[serde(default = "default_vol_multiplier")]
    pub vol_multiplier: Decimal,
}

impl Default for FairValueConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_time_to_expiry_secs: default_min_time_to_expiry_secs(),
            vol_multiplier: default_vol_multiplier(),
        }
    }
}

fn default_min_time_to_expiry_secs() -> u64 {
    120
}
fn default_vol_multiplier() -> Decimal {
    Decimal::ONE
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashCrashConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_drop_threshold")]
    pub drop_threshold: Decimal,
    #[serde(default = "default_lookback_secs")]
    pub lookback_secs: u64,
    #[serde(default = "default_reversion_target")]
    pub reversion_target: Decimal,
}

impl Default for FlashCrashConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            drop_threshold: default_drop_threshold(),
            lookback_secs: default_lookback_secs(),
            reversion_target: default_reversion_target(),
        }
    }
}

fn default_drop_threshold() -> Decimal {
    Decimal::new(15, 2) // 0.15
}
fn default_lookback_secs() -> u64 {
    30
}
fn default_reversion_target() -> Decimal {
    Decimal::new(5, 1) // 0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookImbalanceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_imbalance_threshold")]
    pub imbalance_threshold: Decimal,
    #[serde(default = "default_imbalance_levels")]
    pub levels: usize,
}

impl Default for BookImbalanceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            imbalance_threshold: default_imbalance_threshold(),
            levels: default_imbalance_levels(),
        }
    }
}

fn default_imbalance_threshold() -> Decimal {
    Decimal::new(6, 1) // 0.6
}
fn default_imbalance_levels() -> usize {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegriskArbConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_sum_deviation_threshold")]
    pub sum_deviation_threshold: Decimal,
}

impl Default for NegriskArbConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sum_deviation_threshold: default_sum_deviation_threshold(),
        }
    }
}

fn default_sum_deviation_threshold() -> Decimal {
    Decimal::new(2, 2) // 0.02
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvergenceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_min_probability")]
    pub min_probability: Decimal,
    #[serde(default = "default_max_time_to_expiry_secs")]
    pub max_time_to_expiry_secs: u64,
}

impl Default for ConvergenceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_probability: default_min_probability(),
            max_time_to_expiry_secs: default_max_time_to_expiry_secs(),
        }
    }
}

fn default_min_probability() -> Decimal {
    Decimal::new(90, 2) // 0.90
}
fn default_max_time_to_expiry_secs() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketMakerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_spread_bps")]
    pub spread_bps: u64,
    #[serde(default = "default_max_inventory")]
    pub max_inventory: u64,
    #[serde(default = "default_quote_size")]
    pub quote_size: u64,
    #[serde(default = "default_refresh_interval_ms")]
    pub refresh_interval_ms: u64,
}

impl Default for MarketMakerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            spread_bps: default_spread_bps(),
            max_inventory: default_max_inventory(),
            quote_size: default_quote_size(),
            refresh_interval_ms: default_refresh_interval_ms(),
        }
    }
}

fn default_spread_bps() -> u64 {
    200
}
fn default_max_inventory() -> u64 {
    100
}
fn default_quote_size() -> u64 {
    10
}
fn default_refresh_interval_ms() -> u64 {
    1000
}

// ---------------------------------------------------------------------------
// TUI config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_refresh_rate_ms")]
    pub refresh_rate_ms: u64,
    #[serde(default = "default_log_buffer_size")]
    pub log_buffer_size: usize,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            refresh_rate_ms: default_refresh_rate_ms(),
            log_buffer_size: default_log_buffer_size(),
        }
    }
}

fn default_refresh_rate_ms() -> u64 {
    100
}
fn default_log_buffer_size() -> usize {
    500
}

// ---------------------------------------------------------------------------
// Backtest config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestConfig {
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
    #[serde(default = "default_slippage_bps")]
    pub slippage_bps: u64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            output_dir: default_output_dir(),
            slippage_bps: default_slippage_bps(),
        }
    }
}

fn default_data_dir() -> String {
    "data/".into()
}
fn default_output_dir() -> String {
    "reports/".into()
}
fn default_slippage_bps() -> u64 {
    5
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_default_config() {
        let config = BotConfig::default();
        assert_eq!(config.general.mode, "paper");
        assert_eq!(config.clob.chain_id, 137);
        assert_eq!(config.risk.bankroll, Decimal::new(1000, 0));
        assert_eq!(config.risk.max_positions, 5);
        assert!(config.strategy.lead_lag.enabled);
        assert!(!config.strategy.flash_crash.enabled);
        assert!(config.tui.enabled);
    }

    #[test]
    fn test_load_from_toml() {
        let toml_content = r#"
[general]
mode = "live"
log_level = "debug"
strategies = ["fair_value"]

[risk]
bankroll = 500.0
kelly_fraction = 0.10
max_position_pct = 0.03
max_positions = 3
"#;
        let dir = std::env::temp_dir().join("pmbot-test-config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(toml_content.as_bytes()).unwrap();

        let config = BotConfig::load(&path).unwrap();
        assert_eq!(config.general.mode, "live");
        assert_eq!(config.general.log_level, "debug");
        assert_eq!(config.general.strategies, vec!["fair_value".to_string()]);
        assert_eq!(config.risk.bankroll, Decimal::new(500, 0));
        assert_eq!(config.risk.kelly_fraction, Decimal::new(10, 2));
        assert_eq!(config.risk.max_positions, 3);
        // defaults should still apply for unset fields
        assert_eq!(config.clob.chain_id, 137);
        assert!(config.strategy.lead_lag.enabled);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_config_not_found() {
        let result = BotConfig::load("/nonexistent/path.toml");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::NotFound { path } => {
                assert!(path.contains("nonexistent"));
            }
            other => panic!("expected NotFound, got: {other:?}"),
        }
    }

    #[test]
    fn test_config_validation_rejects_zero_bankroll() {
        let mut config = BotConfig::default();
        config.risk.bankroll = Decimal::ZERO;
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validation_rejects_bad_kelly() {
        let mut config = BotConfig::default();
        config.risk.kelly_fraction = Decimal::new(15, 1); // 1.5 > 1.0
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_full_toml_roundtrip() {
        let config = BotConfig::default();
        let toml_str =
            toml::to_string_pretty(&config).expect("default config should serialize to TOML");
        assert!(toml_str.contains("paper"));
        assert!(toml_str.contains("137"));
    }
}
