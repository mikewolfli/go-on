//! `go-on config` subcommand (M1.2): inspect the effective layered
//! configuration.
//!
//! # Usage
//!
//! - `go-on config dump` — print the effective config (builtin defaults →
//!   project file → user config → CLI patch) as TOML.
//! - `go-on config dump --sources` — also print each top-level key's source
//!   layer and config path.
//! - `go-on config dump --patch '<inline toml or json>'` — apply an inline
//!   patch as the top (cli) layer for this invocation only. The patch can
//!   override any scalar or table without code changes.
//!
//! The layered merge is opt-in: the user layer participates only when the
//! project config sets `layered_merge = true`. An explicit `--patch` always
//! participates (it is scoped to this invocation).

use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Subcommand;

use crate::config::patch::LayeredLoad;
use crate::config::AppConfig;

/// Subcommands for `go-on config`.
#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    /// Print the layer-resolved configuration (builtin → project → user →
    /// cli). Shows the resolved layer view: keys no layer sets (and that have
    /// no builtin value) are omitted — they fall back to builtin serde
    /// defaults at load time.
    Dump {
        /// Inline TOML or JSON object applied as the top (cli) layer for this
        /// invocation only
        #[arg(long, value_name = "TOML_OR_JSON")]
        patch: Option<String>,
        /// Print each top-level key's source layer and config path
        #[arg(long, default_value_t = false)]
        sources: bool,
    },
}

/// Handle the `config` CLI subcommand.
pub async fn handle_config_command(cmd: ConfigCommand, config_path: &Path) -> Result<()> {
    match cmd {
        ConfigCommand::Dump { patch, sources } => {
            let path = config_path.to_path_buf();
            let output = tokio::task::spawn_blocking(move || {
                run_dump_sync(&path, patch.as_deref(), sources)
            })
            .await??;
            print!("{output}");
            Ok(())
        }
    }
}

fn run_dump_sync(config_path: &Path, patch: Option<&str>, show_sources: bool) -> Result<String> {
    // Write the bootstrap defaults when the config is missing/blank, matching
    // the startup path (`main/server.rs`), so `config dump` works on a fresh
    // checkout.
    crate::config::defaults::ensure_bootstrap_config(config_path)?;

    let cli_patch = match patch {
        Some(raw) => Some(patch_to_toml(raw)?),
        None => None,
    };
    let loaded = AppConfig::load_layered(config_path, cli_patch.as_deref())?;
    Ok(render_dump(&loaded, show_sources))
}

/// Render the dump output: the effective config as TOML, then (with
/// `--sources`) the per-key provenance table.
fn render_dump(loaded: &LayeredLoad, show_sources: bool) -> String {
    let mut out = String::new();
    for warning in &loaded.warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }

    match crate::config::patch::value_to_toml(&loaded.merged) {
        Some(toml_text) => out.push_str(&toml_text),
        None => match serde_json::to_string_pretty(&loaded.merged) {
            Ok(json) => {
                out.push_str(&json);
                out.push('\n');
            }
            Err(_) => out.push_str("# (merged config could not be rendered)\n"),
        },
    }

    if show_sources {
        out.push_str("\n# effective config provenance (top-level keys)\n");
        if loaded.sources.is_empty() {
            out.push_str(
                "# layered merge is off: set `layered_merge = true` in the config \
                 (or pass --patch) to track per-key sources\n",
            );
        }
        for source in &loaded.sources {
            let path = source.path.as_deref().unwrap_or("(inline / defaults)");
            out.push_str(&format!(
                "{:<26} {:<9} {}\n",
                source.key, source.layer, path
            ));
        }
    }
    out
}

/// Normalize a `--patch` argument into a TOML document: TOML passes through
/// unchanged, JSON objects are converted so the merge layer always sees TOML.
fn patch_to_toml(input: &str) -> Result<String> {
    if input.trim().parse::<toml::Table>().is_ok() {
        return Ok(input.to_string());
    }
    let json: serde_json::Value = serde_json::from_str(input)
        .with_context(|| "invalid --patch: expected an inline TOML table or a JSON object")?;
    let value = json_to_toml(&json)?;
    match value {
        toml::Value::Table(_) => {
            toml::to_string(&value).context("failed to render --patch as TOML")
        }
        _ => bail!("--patch must be a TOML/JSON object"),
    }
}

/// Convert a JSON value into a `toml::Value` (JSON has no datetime; TOML has
/// no null, so `null` is rejected).
fn json_to_toml(value: &serde_json::Value) -> Result<toml::Value> {
    match value {
        serde_json::Value::Null => bail!("--patch JSON must not contain null"),
        serde_json::Value::Bool(b) => Ok(toml::Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(toml::Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(toml::Value::Float(f))
            } else {
                bail!("--patch JSON number out of TOML range")
            }
        }
        serde_json::Value::String(s) => Ok(toml::Value::String(s.clone())),
        serde_json::Value::Array(items) => items
            .iter()
            .map(json_to_toml)
            .collect::<Result<Vec<_>>>()
            .map(toml::Value::Array),
        serde_json::Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (key, value) in map {
                table.insert(key.clone(), json_to_toml(value)?);
            }
            Ok(toml::Value::Table(table))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    fn write_temp(path: &Path, content: &str) {
        let mut file = std::fs::File::create(path).expect("temp file should be created");
        file.write_all(content.as_bytes())
            .expect("temp file should be written");
    }

    #[test]
    fn dump_sources_shows_layer_markers_for_overridden_key() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let project_path = dir.path().join("config.toml");
        write_temp(
            &project_path,
            r#"
                layered_merge = true
                default_phase = "planning"
                [cache]
                enabled = false
            "#,
        );

        // User config resolved through the GO_ON_CONFIG_DIR override; it
        // overrides `cache` from the project layer. The lock serializes with
        // the parser-level test that also mutates this env var.
        let user_dir = tempfile::tempdir().expect("tempdir should be created");
        let user_cfg = user_dir.path().join("config.toml");
        write_temp(
            &user_cfg,
            r#"
                [cache]
                enabled = true
            "#,
        );
        let _guard = crate::config::patch::USER_CONFIG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("GO_ON_CONFIG_DIR", user_dir.path());

        let output = run_dump_sync(
            &project_path,
            Some(r#"{"default_phase": "delivery"}"#),
            true,
        )
        .expect("dump should render");

        std::env::remove_var("GO_ON_CONFIG_DIR");

        // The cli patch layer wins `default_phase`; the user layer wins
        // `cache`; both markers plus the user config path must appear.
        assert!(
            output.contains("default_phase"),
            "dump should contain the key"
        );
        assert!(output.contains("cli"), "cli patch marker should appear");
        assert!(output.contains("user"), "user layer marker should appear");
        assert!(
            output.contains("project"),
            "project layer marker should appear"
        );
        assert!(
            output.contains(&user_cfg.display().to_string()),
            "user config path should appear in the provenance table"
        );
    }

    #[test]
    fn dump_reports_when_layered_merge_is_off() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let project_path = dir.path().join("config.toml");
        write_temp(&project_path, "default_phase = \"planning\"");

        // No `layered_merge` knob and no --patch: single-file behavior, no
        // sources to report.
        let output = run_dump_sync(&project_path, None, true).expect("dump should render");
        assert!(
            output.contains("layered merge is off"),
            "off state should be reported"
        );
    }
}
