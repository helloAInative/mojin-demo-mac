//! mojinprince-server 业务代码。
//! 二进制入口在 `bin/mojinprince-server.rs`（自动发现同名 crate）。
//! 抽 lib 是为了让 `tests/` 与未来的 CLI 子命令能复用同一份代码。

pub mod api;
pub mod config;
pub mod error;
pub mod model;
pub mod service;
pub mod state;

pub use config::Config;
pub use error::{AppError, AppResult};
pub use state::AppState;
