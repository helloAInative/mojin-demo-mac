//! 数据访问层。
//! 阶段 3 起引入：signal_event / position / watchlist / settings 等落库。
pub mod settings;
pub mod signal;

pub use settings::{SettingsRepo, SettingsRow};
pub use signal::{SignalEventRow, SignalRepo};
