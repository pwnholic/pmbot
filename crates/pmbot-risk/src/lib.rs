//! pmbot-risk: Risk management, position sizing, and circuit breakers.

pub mod actor;
pub mod kelly;
pub mod limits;
pub mod portfolio;
pub mod position;

pub use actor::{RiskActor, ShutdownState};
pub use kelly::fractional_kelly;
pub use limits::CircuitBreaker;
pub use portfolio::PortfolioRisk;
pub use position::{PositionState, TrackedPosition};
