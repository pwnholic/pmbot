use thiserror::Error;

/// Top-level bot error.
#[derive(Error, Debug)]
pub enum BotError {
    #[error("config error: {0}")]
    Config(#[from] ConfigError),

    #[error("market error: {0}")]
    Market(#[from] MarketError),

    #[error("executor error: {0}")]
    Executor(#[from] ExecutorError),

    #[error("risk error: {0}")]
    Risk(#[from] RiskError),

    #[error("feed error: {0}")]
    Feed(#[from] FeedError),

    #[error("strategy error: {0}")]
    Strategy(#[from] StrategyError),

    #[error("channel closed: {0}")]
    ChannelClosed(String),

    #[error("shutdown requested")]
    Shutdown,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("config file not found: {path}")]
    NotFound { path: String },

    #[error("invalid config: {field} — {reason}")]
    Invalid { field: String, reason: String },

    #[error("missing required field: {0}")]
    MissingField(String),

    #[error("parse error: {0}")]
    Parse(String),
}

#[derive(Error, Debug)]
pub enum MarketError {
    #[error("market not found: {0}")]
    NotFound(String),

    #[error("websocket disconnected")]
    WsDisconnected,

    #[error("gamma api error: {0}")]
    GammaApi(String),

    #[error("no active markets matching filters")]
    NoMarketsAvailable,

    #[error("market expired: {0}")]
    Expired(String),
}

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("order rejected: {reason}")]
    OrderRejected { order_id: String, reason: String },

    #[error("signing failed: {0}")]
    SigningFailed(String),

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("api error: {status} — {body}")]
    ApiError { status: u16, body: String },

    #[error("order timeout: {0}")]
    Timeout(String),
}

#[derive(Error, Debug)]
pub enum RiskError {
    #[error("signal rejected: {0}")]
    Rejected(RejectReason),

    #[error("position not found: {0}")]
    PositionNotFound(String),
}

/// Why a signal was rejected by the risk engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    KillSwitchActive,
    DailyLossExceeded,
    MaxPositionsReached,
    InsufficientEdge { edge: String, min: String },
    NoTradeZone { seconds_remaining: u64 },
    InsufficientLiquidity { available: String, required: String },
    ExcessiveCorrelation,
    InsufficientBalance,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KillSwitchActive => write!(f, "kill switch active"),
            Self::DailyLossExceeded => write!(f, "daily loss limit exceeded"),
            Self::MaxPositionsReached => write!(f, "max positions reached"),
            Self::InsufficientEdge { edge, min } => {
                write!(f, "edge {edge} < minimum {min}")
            }
            Self::NoTradeZone { seconds_remaining } => {
                write!(f, "no-trade zone ({seconds_remaining}s to expiry)")
            }
            Self::InsufficientLiquidity { available, required } => {
                write!(f, "liquidity {available} < required {required}")
            }
            Self::ExcessiveCorrelation => write!(f, "excessive correlation with portfolio"),
            Self::InsufficientBalance => write!(f, "insufficient balance"),
        }
    }
}

#[derive(Error, Debug)]
pub enum FeedError {
    #[error("feed disconnected: {0}")]
    Disconnected(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("connection failed: {0}")]
    ConnectionFailed(String),
}

#[derive(Error, Debug)]
pub enum StrategyError {
    #[error("strategy not found: {0}")]
    NotFound(String),

    #[error("strategy error in {strategy}: {reason}")]
    Runtime { strategy: String, reason: String },
}
