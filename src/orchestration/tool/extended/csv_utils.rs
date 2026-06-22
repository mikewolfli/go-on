//! Advanced CSV utility tools
//!
//! Provides `CsvAnalyzeTool` for column-level statistics and null detection,
//! and `CsvTransformTool` for filtering, selecting, and renaming columns.
//! Extends the basic `CsvReadTool`/`CsvWriteTool` from `data_serialization.rs`.
//! Feature-gated behind `data-export`.

#[cfg(feature = "data-export")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "data-export")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "data-export")]
use anyhow::{Context, Result};
#[cfg(feature = "data-export")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "data-export")]
use tracing::{debug, info};

// ── CsvAnalyzeTool ──────────────────────────────────────────────────────────

#[cfg(feature = "data-export")]
pub struct CsvAnalyzeTool;

#[cfg(feature = "data-export")]
impl Tool for CsvAnalyzeTool {
    fn name(&self) -> &'static str {
        "csv_analyze"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let has_headers = input.payload["has_headers"].as_bool().unwrap_or(true);
        let delimiter_str = input.payload["delimiter"].as_str().unwrap_or(",");
        let delimiter = delimiter_str.as_bytes().first().copied().unwrap_or(b',');

        let validated = sanitize_path(input, path)?;

        if !validated.exists() {
            anyhow::bail!("file not found: {}", validated.display());
        }

        debug!(path = %validated.display(), "csv_analyze: analyzing CSV");

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(has_headers)
            .delimiter(delimiter)
            .from_path(&validated)
            .with_context(|| format!("failed to open CSV: {}", validated.display()))?;

        let headers: Vec<String> = reader
            .headers()
            .map(|h| h.iter().map(|f| f.to_string()).collect())
            .unwrap_or_default();

        let column_count = headers.len();

        // Column-level statistics
        // Using column index as key (0..column_count), with name if available
        struct ColumnStats {
            count: usize,
            null_count: usize,
            unique_values: HashSet<String>,
            non_null_values: HashSet<String>,
            // Numeric tracking
            numeric_sum: f64,
            numeric_count: usize,
            min_val: Option<f64>,
            max_val: Option<f64>,
        }

        let mut col_stats: Vec<ColumnStats> = (0..column_count)
            .map(|_| ColumnStats {
                count: 0,
                null_count: 0,
                unique_values: HashSet::new(),
                non_null_values: HashSet::new(),
                numeric_sum: 0.0,
                numeric_count: 0,
                min_val: None,
                max_val: None,
            })
            .collect();

        let mut total_rows = 0usize;

        for result in reader.records() {
            let record = result
                .with_context(|| format!("failed to read record {total_rows}"))?;
            total_rows += 1;

            for (i, field) in record.iter().enumerate() {
                if i >= column_count {
                    break;
                }
                let val = field.trim();
                col_stats[i].count += 1;

                if val.is_empty() {
                    col_stats[i].null_count += 1;
                } else {
                    col_stats[i]
                        .non_null_values
                        .insert(val.to_string());
                }
                col_stats[i].unique_values.insert(val.to_string());

                // Attempt numeric parsing
                if let Ok(num) = val.parse::<f64>() {
                    col_stats[i].numeric_sum += num;
                    col_stats[i].numeric_count += 1;
                    match col_stats[i].min_val {
                        None => col_stats[i].min_val = Some(num),
                        Some(m) if num < m => col_stats[i].min_val = Some(num),
                        _ => {}
                    }
                    match col_stats[i].max_val {
                        None => col_stats[i].max_val = Some(num),
                        Some(m) if num > m => col_stats[i].max_val = Some(num),
                        _ => {}
                    }
                }
            }
        }

        // Build column analysis JSON
        let columns_json: Vec<serde_json::Value> = col_stats
            .into_iter()
            .enumerate()
            .map(|(i, stats)| {
                let avg = if stats.numeric_count > 0 {
                    Some(stats.numeric_sum / stats.numeric_count as f64)
                } else {
                    None
                };

                let col_name = headers.get(i).cloned().unwrap_or_else(|| format!("col_{i}"));

                // Sample up to 10 unique non-null values
                let sample_values: Vec<&str> = stats
                    .non_null_values
                    .iter()
                    .take(10)
                    .map(|s| s.as_str())
                    .collect();

                serde_json::json!({
                    "index": i,
                    "name": col_name,
                    "row_count": stats.count,
                    "null_count": stats.null_count,
                    "null_percent": if stats.count > 0 {
                        format!("{:.1}%", (stats.null_count as f64 / stats.count as f64) * 100.0)
                    } else {
                        "0.0%".to_string()
                    },
                    "unique_count": stats.unique_values.len(),
                    "is_numeric": stats.numeric_count > 0,
                    "numeric_count": stats.numeric_count,
                    "numeric_mean": avg,
                    "numeric_min": stats.min_val,
                    "numeric_max": stats.max_val,
                    "sample_values": sample_values,
                })
            })
            .collect();

        info!(
            path = %validated.display(),
            rows = total_rows,
            columns = column_count,
            "csv_analyze: analysis complete"
        );

        let report = tool_execution_report("csv_analyze", Some("csv_analyzed"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated.to_string_lossy(),
                "row_count": total_rows,
                "column_count": column_count,
                "has_headers": has_headers,
                "columns": columns_json,
            })),
            error: None,
            verification: Some("csv_analyzed".to_string()),
            audit_log: Some(format!(
                "Analyzed CSV '{}': {} rows, {} columns",
                validated.display(),
                total_rows,
                column_count,
            )),
            pua_report: Some(report),
        })
    }
}

// ── CsvTransformTool ────────────────────────────────────────────────────────

#[cfg(feature = "data-export")]
pub struct CsvTransformTool;

#[cfg(feature = "data-export")]
impl Tool for CsvTransformTool {
    fn name(&self) -> &'static str {
        "csv_transform"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'output_path'"))?;
        let has_headers = input.payload["has_headers"].as_bool().unwrap_or(true);
        let delimiter_str = input.payload["delimiter"].as_str().unwrap_or(",");
        let delimiter = delimiter_str.as_bytes().first().copied().unwrap_or(b',');

        // Transformation parameters
        let select_columns: Option<Vec<String>> = input.payload["select"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
        let rename_map: Option<HashMap<String, String>> = input.payload["rename"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or(k).to_string()))
                    .collect()
            });
        let filter_column = input.payload["filter_column"].as_str();
        let filter_value = input.payload["filter_value"].as_str();
        let filter_invert = input.payload["filter_invert"].as_bool().unwrap_or(false);

        let validated = sanitize_path(input, path)?;
        let validated_output = crate::orchestration::tool::sanitize_path_for_write(input, output_path)?;

        if !validated.exists() {
            anyhow::bail!("file not found: {}", validated.display());
        }

        // Ensure output parent directory exists
        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .context("failed to create output parent directories")?;
            }
        }

        debug!(
            path = %validated.display(),
            output = %validated_output.display(),
            "csv_transform: transforming CSV"
        );

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(has_headers)
            .delimiter(delimiter)
            .from_path(&validated)
            .with_context(|| format!("failed to open CSV: {}", validated.display()))?;

        let original_headers: Vec<String> = reader
            .headers()
            .map(|h| h.iter().map(|f| f.to_string()).collect())
            .unwrap_or_default();

        // Determine output column order and mapping
        let output_columns: Vec<(usize, String)> = if let Some(ref select) = select_columns {
            select
                .iter()
                .filter_map(|name| {
                    original_headers
                        .iter()
                        .position(|h| h == name)
                        .map(|idx| (idx, name.clone()))
                })
                .collect()
        } else {
            original_headers
                .iter()
                .cloned()
                .enumerate()
                .collect()
        };

        // Apply renames
        let output_headers: Vec<String> = output_columns
            .iter()
            .map(|(_, name)| {
                rename_map
                    .as_ref()
                    .and_then(|rm| rm.get(name))
                    .cloned()
                    .unwrap_or_else(|| name.clone())
            })
            .collect();

        let mut writer = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_path(&validated_output)
            .with_context(|| format!("failed to create output CSV: {}", validated_output.display()))?;

        // Write header row if source had headers
        if has_headers && !output_headers.is_empty() {
            writer
                .write_record(&output_headers)
                .context("failed to write CSV headers")?;
        }

        // Determine filter column index
        let filter_idx = filter_column.and_then(|fc| {
            original_headers.iter().position(|h| h == fc)
        });

        let mut output_row_count = 0usize;
        let mut filtered_row_count = 0usize;

        for result in reader.records() {
            let record = result.context("failed to read CSV record")?;

            // Apply row filter
            if let Some(idx) = filter_idx {
                let field_val = record.get(idx).unwrap_or("");
                let matches = if let Some(fv) = filter_value {
                    field_val == fv || field_val.trim() == fv.trim()
                } else {
                    !field_val.trim().is_empty()
                };

                let include = if filter_invert { !matches } else { matches };
                if !include {
                    filtered_row_count += 1;
                    continue;
                }
            }

            // Write selected/ordered columns
            let out_row: Vec<String> = output_columns
                .iter()
                .map(|(idx, _)| record.get(*idx).unwrap_or("").to_string())
                .collect();

            writer
                .write_record(&out_row)
                .context("failed to write CSV record")?;
            output_row_count += 1;
        }

        writer.flush().context("failed to flush CSV writer")?;
        let output_len = validated_output
            .metadata()
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        info!(
            path = %validated.display(),
            output = %validated_output.display(),
            rows_written = output_row_count,
            rows_filtered = filtered_row_count,
            "csv_transform: transformation complete"
        );

        let report = tool_execution_report("csv_transform", Some("csv_transformed"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "input_path": validated.to_string_lossy(),
                "output_path": validated_output.to_string_lossy(),
                "output_row_count": output_row_count,
                "filtered_row_count": filtered_row_count,
                "output_columns": output_headers,
                "file_size_bytes": output_len,
            })),
            error: None,
            verification: Some("csv_transformed".to_string()),
            audit_log: Some(format!(
                "Transformed CSV '{}' -> '{}': {} rows written, {} filtered",
                validated.display(),
                validated_output.display(),
                output_row_count,
                filtered_row_count,
            )),
            pua_report: Some(report),
        })
    }
}
