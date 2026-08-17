//! `go-on cron` subcommand (M3.3): manage user-level cron jobs.
//!
//! Jobs live in the same SQLite store the server's tick loop reads
//! (`.goon/cron/cron.db`), so CLI additions/enables/removes take effect on the
//! next server tick without a restart. When a job fires, the server runs its
//! payload through the `workflow.execute` executor contract.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use clap::Subcommand;
use serde_json::Value;

use crate::orchestration::schedule::cron::{cron_db_path, parse_schedule};
use crate::orchestration::schedule::{CronJob, CronStore};

/// Subcommands for `go-on cron`.
#[derive(Debug, Clone, Subcommand)]
pub enum CronCommand {
    /// Add a cron job: `go-on cron add "<expression>" --payload '<json>' [--disabled]`
    Add {
        /// Cron expression (Vixie-style, e.g. "*/5 * * * *")
        expression: String,
        /// JSON object passed as the workflow.execute params when the job fires
        #[arg(long)]
        payload: String,
        /// Register the job disabled (it will not fire until enabled)
        #[arg(long, default_value_t = false)]
        disabled: bool,
    },
    /// List cron jobs (id, expression, enabled, next/last run)
    List,
    /// Remove a cron job by id
    Remove {
        /// Job id (from `go-on cron list`)
        id: String,
    },
    /// Enable a cron job (reschedules from the first future match)
    Enable {
        /// Job id (from `go-on cron list`)
        id: String,
    },
    /// Disable a cron job (schedule is preserved; no catch-up runs on re-enable)
    Disable {
        /// Job id (from `go-on cron list`)
        id: String,
    },
}

/// Handle the `cron` CLI subcommand.
pub async fn handle_cron_command(cmd: CronCommand, _config_path: &Path) -> Result<()> {
    let store = CronStore::new(&cron_db_path())?;
    match cmd {
        CronCommand::Add {
            expression,
            payload,
            disabled,
        } => {
            // Validate the expression up front so an invalid schedule is never
            // stored (the store re-validates on insert anyway).
            parse_schedule(&expression)?;
            let payload = parse_payload(&payload)?;
            let job = CronJob::new(
                uuid::Uuid::new_v4().to_string(),
                expression,
                payload,
                !disabled,
            );
            let stored = store.add(job)?;
            println!("Added cron job {}", stored.id);
            println!("  expression:  {}", stored.expression);
            println!("  enabled:     {}", stored.enabled);
            println!("  next_run_at: {}", format_ts(stored.next_run_at));
            println!("  payload:     {}", stored.payload);
        }
        CronCommand::List => {
            let jobs = store.list()?;
            if jobs.is_empty() {
                println!("No cron jobs.");
                return Ok(());
            }
            println!(
                "{:<38} {:<24} {:<8} {:<24} {:<24} payload",
                "id", "expression", "enabled", "next_run_at", "last_run_at"
            );
            for job in &jobs {
                println!(
                    "{:<38} {:<24} {:<8} {:<24} {:<24} {}",
                    job.id,
                    job.expression,
                    job.enabled,
                    format_ts(job.next_run_at),
                    format_ts(job.last_run_at),
                    job.payload
                );
            }
        }
        CronCommand::Remove { id } => {
            if store.remove(&id) {
                println!("Removed cron job {id}");
            } else {
                bail!("cron job '{id}' not found");
            }
        }
        CronCommand::Enable { id } => {
            if store.set_enabled(&id, true)? {
                println!("Enabled cron job {id}");
            } else {
                bail!("cron job '{id}' not found");
            }
        }
        CronCommand::Disable { id } => {
            if store.set_enabled(&id, false)? {
                println!("Disabled cron job {id}");
            } else {
                bail!("cron job '{id}' not found");
            }
        }
    }
    Ok(())
}

/// Parse the `--payload` argument into the JSON object the job runs with.
fn parse_payload(raw: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| anyhow!("payload is not valid JSON: {e}"))?;
    if !value.is_object() {
        bail!("payload must be a JSON object (the workflow.execute params), got: {value}");
    }
    Ok(value)
}

/// Format an epoch-seconds timestamp for the CLI table (`-` when absent).
fn format_ts(ts: Option<i64>) -> String {
    match ts {
        Some(ts) => chrono::DateTime::from_timestamp(ts, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| ts.to_string()),
        None => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_parse_requires_valid_json_object() {
        // Valid JSON object parses.
        let parsed = parse_payload(r#"{"task": "hello", "governance": {"mode": "auto"}}"#)
            .expect("valid object");
        assert_eq!(parsed["task"], "hello");

        // Invalid JSON is rejected.
        assert!(parse_payload("{not json").is_err());
        // JSON that is not an object is rejected (workflow.execute params are
        // an object).
        assert!(parse_payload(r#"[1, 2, 3]"#).is_err());
        assert!(parse_payload(r#""a string""#).is_err());
        assert!(parse_payload("42").is_err());
    }

    #[test]
    fn expression_validation_error_is_surfaceable() {
        // `go-on cron add` validates the expression before storing; the store
        // rejects the same input.
        assert!(parse_schedule("*/5 * * * *").is_ok());
        assert!(parse_schedule("not a cron").is_err());
        let err = parse_schedule("not a cron").expect_err("invalid expression");
        assert!(err.to_string().contains("invalid cron expression"));
    }
}
