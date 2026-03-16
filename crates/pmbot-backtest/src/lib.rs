//! pmbot-backtest: Historical backtesting engine.
//!
//! Provides event recording/replay, simulated execution,
//! performance reporting, and the main backtest engine.

pub mod engine;
pub mod recorder;
pub mod report;
pub mod sim_executor;

pub use engine::{BacktestConfig, BacktestEngine};
pub use recorder::{EventReader, EventRecorder, RecordedEvent};
pub use report::{BacktestReport, ReportBuilder};
pub use sim_executor::SimExecutor;
