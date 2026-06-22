//! SQLite database query tools
//!
//! Provides `SqliteQueryTool` for executing read-only SQL queries against
//! SQLite database files.
//! Only compiled when `feature = "backend-sqlite"` is enabled.

#[cfg(feature = "backend-sqlite")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "backend-sqlite")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "backend-sqlite")]
use anyhow::{Context, Result};
#[cfg(feature = "backend-sqlite")]
use std::fs;
#[cfg(feature = "backend-sqlite")]
use tracing::info;

#[cfg(feature = "backend-sqlite")]
pub struct SqliteQueryTool;

#[cfg(feature = "backend-sqlite")]
impl Tool for SqliteQueryTool {
    fn name(&self) -> &'static str {
        "sqlite_query"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let sql = input.payload["sql"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'sql'"))?;
        let max_rows = input.payload["max_rows"].as_u64().unwrap_or(100) as usize;

        let validated = sanitize_path(input, path)?;

        let conn = rusqlite::Connection::open(&validated)
            .with_context(|| format!("failed to open SQLite DB: {validated}"))?;

        let mut stmt = conn.prepare(sql)
            .with_context(|| format!("failed to prepare SQL: {sql}"))?;

        let column_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let mut rows = Vec::new();
        let mut row_count = 0u64;

        let result_iter = stmt.query_map([], |row| {
            let mut map = serde_json::Map::new();
            for (i, name) in column_names.iter().enumerate() {
                let value: rusqlite::types::Value = row.get_unwrap(i);
                let json_val = match value {
                    rusqlite::types::Value::Null => serde_json::Value::Null,
                    rusqlite::types::Value::Integer(i) => serde_json::Value::Number(i.into()),
                    rusqlite::types::Value::Real(f) => serde_json::Value::Number(
                        serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0))
                    ),
                    rusqlite::types::Value::Text(s) => serde_json::Value::String(s),
                    rusqlite::types::Value::Blob(b) => serde_json::Value::String(
                        format!("<blob: {} bytes>", b.len())
                    ),
                };
                map.insert(name.clone(), json_val);
            }
            Ok(serde_json::Value::Object(map))
        })?;

        for result in result_iter {
            if row_count >= max_rows as u64 {
                break;
            }
            match result {
                Ok(row) => {
                    rows.push(row);
                    row_count += 1;
                }
                Err(e) => {
                    anyhow::bail!("SQL row fetch error: {e}");
                }
            }
        }

        info!(path = %validated, rows = row_count, "SQLite query executed");

        let report = tool_execution_report("sqlite_query", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "columns": column_names,
                "rows": rows,
                "row_count": row_count,
                "sql": sql,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!("sqlite_query: {} rows from {}", row_count, validated.display())),
            pua_report: Some(report),
        })
    }
}
