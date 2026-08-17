pub mod chat;
pub mod config; // M1.2: `go-on config` — layered config dump + source tracking
#[cfg(feature = "backend-sqlite")]
pub mod cron; // M3.3: `go-on cron` — user-level cron job management
pub mod exec;
pub mod markdown_renderer;
