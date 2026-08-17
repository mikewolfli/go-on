//! M3.3: user-level cron scheduling — durable SQLite job store and schedule
//! evaluation.
//!
//! The server-side tick loop that fires due jobs lives in
//! `acp::background`; the `go-on cron` CLI lives in `cli::cron`. This module
//! owns the shared store (`.goon/cron/cron.db`) and the scheduling semantics.

pub mod cron;

pub use cron::{CronJob, CronStore};
