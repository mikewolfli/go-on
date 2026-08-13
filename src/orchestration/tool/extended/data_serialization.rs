//! Data serialization tools
//!
//! Provides tools for reading and writing CSV, TOML, and YAML files.
//! Feature-gated behind `data-export`.

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::t;
use crate::orchestration::tool::{
    sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput,
};
use anyhow::{Context, Result};
use tracing::debug;
use tracing::info;

// ── CsvReadTool ─────────────────────────────────────────────────────────────

pub struct CsvReadTool;

impl Tool for CsvReadTool {
    fn name(&self) -> &'static str {
        "csv_read"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let has_headers = input.payload["has_headers"].as_bool().unwrap_or(true);
        let delimiter_str = input.payload["delimiter"].as_str().unwrap_or(",");
        let delimiter = delimiter_str.as_bytes().first().copied().unwrap_or(b',');

        let validated_path = sanitize_path(input, path)?;

        if !validated_path.exists() {
            anyhow::bail!("file not found: {}", validated_path.display());
        }

        debug!(
            path = %validated_path.display(),
            has_headers = has_headers,
            delimiter = %delimiter_str,
            "tool: reading CSV file"
        );

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(has_headers)
            .delimiter(delimiter)
            .from_path(&validated_path)
            .with_context(|| format!("failed to open CSV file: {}", validated_path.display()))?;

        let headers: Vec<String> = reader
            .headers()
            .map(|h| h.iter().map(|f| f.to_string()).collect())
            .unwrap_or_default();

        let mut records: Vec<serde_json::Value> = Vec::new();
        let mut row_count: usize = 0;

        for result in reader.records() {
            let record = result
                .with_context(|| format!("failed to read record {} from CSV", row_count + 1))?;

            let entry: serde_json::Value = if has_headers && !headers.is_empty() {
                let mut map = serde_json::Map::new();
                for (i, field) in record.iter().enumerate() {
                    let default_key = i.to_string();
                    let key = headers.get(i).map(|s| s.as_str()).unwrap_or(&default_key);
                    map.insert(
                        key.to_string(),
                        serde_json::Value::String(field.to_string()),
                    );
                }
                serde_json::Value::Object(map)
            } else {
                let fields: Vec<serde_json::Value> = record
                    .iter()
                    .map(|f| serde_json::Value::String(f.to_string()))
                    .collect();
                serde_json::Value::Array(fields)
            };

            records.push(entry);
            row_count += 1;
        }

        info!(
            path = %validated_path.display(),
            rows = row_count,
            columns = headers.len(),
            "tool: CSV file read successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated_path.to_string_lossy(),
                "headers": headers,
                "records": records,
                "row_count": row_count,
                "column_count": headers.len(),
            })),
            error: None,
            verification: Some("csv_read".to_string()),
            audit_log: Some(format!(
                "Read CSV '{}': {} rows, {} columns",
                validated_path.display(),
                row_count,
                headers.len()
            )),
            pua_report: Some(tool_execution_report("csv_read", Some("csv_read"))),
        })
    }
}

// ── CsvWriteTool ────────────────────────────────────────────────────────────

pub struct CsvWriteTool;

impl Tool for CsvWriteTool {
    fn name(&self) -> &'static str {
        "csv_write"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let headers = input.payload["headers"].as_array();
        let records = input.payload["records"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'records' array"))?;
        let delimiter_str = input.payload["delimiter"].as_str().unwrap_or(",");
        let delimiter = delimiter_str.as_bytes().first().copied().unwrap_or(b',');

        let validated_path = sanitize_path_for_write(input, path)?;

        // Ensure parent directory exists
        if let Some(parent) = validated_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).context("failed to create parent directories")?;
            }
        }

        debug!(
            path = %validated_path.display(),
            record_count = records.len(),
            "tool: writing CSV file"
        );

        let mut writer = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_path(&validated_path)
            .with_context(|| format!("failed to create CSV file: {}", validated_path.display()))?;

        // Write headers if provided
        if let Some(h) = headers {
            let header_strs: Vec<&str> = h.iter().filter_map(|v| v.as_str()).collect();
            writer
                .write_record(&header_strs)
                .context("failed to write CSV headers")?;
        }

        // Write records
        for record in records {
            match record {
                serde_json::Value::Object(map) => {
                    let values: Vec<String> = if let Some(h) = headers {
                        h.iter()
                            .filter_map(|key| {
                                key.as_str().and_then(|k| {
                                    map.get(k).map(|v| match v {
                                        serde_json::Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    })
                                })
                            })
                            .collect()
                    } else {
                        map.values()
                            .map(|v| match v {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .collect()
                    };
                    writer
                        .write_record(&values)
                        .context("failed to write CSV record")?;
                }
                serde_json::Value::Array(arr) => {
                    let values: Vec<String> = arr
                        .iter()
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect();
                    writer
                        .write_record(&values)
                        .context("failed to write CSV record")?;
                }
                other => {
                    writer
                        .write_record(&[other.to_string()])
                        .context("failed to write CSV record")?;
                }
            }
        }

        writer.flush().context("failed to flush CSV writer")?;
        let output_len = validated_path.metadata().ok().map(|m| m.len()).unwrap_or(0);

        info!(
            path = %validated_path.display(),
            records = records.len(),
            "tool: CSV file written successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated_path.to_string_lossy(),
                "row_count": records.len(),
                "file_size_bytes": output_len,
            })),
            error: None,
            verification: Some("csv_written".to_string()),
            audit_log: Some(format!(
                "Wrote CSV '{}': {} rows",
                validated_path.display(),
                records.len()
            )),
            pua_report: Some(tool_execution_report("csv_write", Some("csv_written"))),
        })
    }
}

// ── TomlReadTool ────────────────────────────────────────────────────────────

pub struct TomlReadTool;

impl Tool for TomlReadTool {
    fn name(&self) -> &'static str {
        "toml_read"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated_path = sanitize_path(input, path)?;

        if !validated_path.exists() {
            anyhow::bail!("file not found: {}", validated_path.display());
        }

        debug!(path = %validated_path.display(), "tool: reading TOML file");

        let content = crate::orchestration::tool::exec_common::read_text_capped(
            &validated_path,
            crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
        )
        .with_context(|| format!("failed to read TOML file: {}", validated_path.display()))?;

        let value: toml::Value = content
            .parse()
            .with_context(|| format!("failed to parse TOML in {}", validated_path.display()))?;

        let json_value = toml_to_json(&value);

        info!(
            path = %validated_path.display(),
            "tool: TOML file read successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated_path.to_string_lossy(),
                "data": json_value,
                "file_size_bytes": validated_path.metadata().ok().map(|m| m.len()).unwrap_or(0),
            })),
            error: None,
            verification: Some("toml_read".to_string()),
            audit_log: Some(format!("Read TOML '{}'", validated_path.display())),
            pua_report: Some(tool_execution_report("toml_read", Some("toml_read"))),
        })
    }
}

// ── TomlWriteTool ───────────────────────────────────────────────────────────

pub struct TomlWriteTool;

impl Tool for TomlWriteTool {
    fn name(&self) -> &'static str {
        "toml_write"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let data = &input.payload["data"];

        let validated_path = sanitize_path_for_write(input, path)?;

        // Ensure parent directory exists
        if let Some(parent) = validated_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).context("failed to create parent directories")?;
            }
        }

        debug!(path = %validated_path.display(), "tool: writing TOML file");

        let toml_value = json_to_toml(data);
        let toml_string =
            toml::to_string_pretty(&toml_value).context("failed to serialize TOML")?;

        std::fs::write(&validated_path, &toml_string)
            .with_context(|| format!("failed to write TOML file: {}", validated_path.display()))?;

        let output_len = validated_path.metadata().ok().map(|m| m.len()).unwrap_or(0);

        info!(
            path = %validated_path.display(),
            "tool: TOML file written successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated_path.to_string_lossy(),
                "file_size_bytes": output_len,
            })),
            error: None,
            verification: Some("toml_written".to_string()),
            audit_log: Some(format!("Wrote TOML '{}'", validated_path.display())),
            pua_report: Some(tool_execution_report("toml_write", Some("toml_written"))),
        })
    }
}

// ── YamlReadTool ────────────────────────────────────────────────────────────

pub struct YamlReadTool;

impl Tool for YamlReadTool {
    fn name(&self) -> &'static str {
        "yaml_read"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated_path = sanitize_path(input, path)?;

        if !validated_path.exists() {
            anyhow::bail!("file not found: {}", validated_path.display());
        }

        debug!(path = %validated_path.display(), "tool: reading YAML file");

        let content = crate::orchestration::tool::exec_common::read_text_capped(
            &validated_path,
            crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
        )
        .with_context(|| format!("failed to read YAML file: {}", validated_path.display()))?;

        let value: serde_yaml::Value = serde_yaml::from_str(&content)
            .with_context(|| format!("failed to parse YAML in {}", validated_path.display()))?;

        let json_value = yaml_to_json(&value);

        info!(
            path = %validated_path.display(),
            "tool: YAML file read successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated_path.to_string_lossy(),
                "data": json_value,
                "file_size_bytes": validated_path.metadata().ok().map(|m| m.len()).unwrap_or(0),
            })),
            error: None,
            verification: Some("yaml_read".to_string()),
            audit_log: Some(format!("Read YAML '{}'", validated_path.display())),
            pua_report: Some(tool_execution_report("yaml_read", Some("yaml_read"))),
        })
    }
}

// ── YamlWriteTool ───────────────────────────────────────────────────────────

pub struct YamlWriteTool;

impl Tool for YamlWriteTool {
    fn name(&self) -> &'static str {
        "yaml_write"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let data = &input.payload["data"];

        let validated_path = sanitize_path_for_write(input, path)?;

        // Ensure parent directory exists
        if let Some(parent) = validated_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).context("failed to create parent directories")?;
            }
        }

        debug!(path = %validated_path.display(), "tool: writing YAML file");

        let yaml_value = json_to_yaml(data);
        let yaml_string = serde_yaml::to_string(&yaml_value).context("failed to serialize YAML")?;

        std::fs::write(&validated_path, &yaml_string)
            .with_context(|| format!("failed to write YAML file: {}", validated_path.display()))?;

        let output_len = validated_path.metadata().ok().map(|m| m.len()).unwrap_or(0);

        info!(
            path = %validated_path.display(),
            "tool: YAML file written successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated_path.to_string_lossy(),
                "file_size_bytes": output_len,
            })),
            error: None,
            verification: Some("yaml_written".to_string()),
            audit_log: Some(format!("Wrote YAML '{}'", validated_path.display())),
            pua_report: Some(tool_execution_report("yaml_write", Some("yaml_written"))),
        })
    }
}

// ── Conversion helpers ──────────────────────────────────────────────────────

/// Recursively convert a `toml::Value` into a `serde_json::Value`.
fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => {
            let mut map = serde_json::Map::new();
            for (k, v) in table {
                map.insert(k.clone(), toml_to_json(v));
            }
            serde_json::Value::Object(map)
        }
    }
}

/// Recursively convert a `serde_json::Value` into a `toml::Value`.
fn json_to_toml(value: &serde_json::Value) -> toml::Value {
    match value {
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Array(arr) => toml::Value::Array(arr.iter().map(json_to_toml).collect()),
        serde_json::Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (k, v) in map {
                table.insert(k.clone(), json_to_toml(v));
            }
            toml::Value::Table(table)
        }
        serde_json::Value::Null => toml::Value::String("null".to_string()),
    }
}

/// Recursively convert a `serde_yaml::Value` into a `serde_json::Value`.
fn yaml_to_json(value: &serde_yaml::Value) -> serde_json::Value {
    match value {
        serde_yaml::Value::String(s) => serde_json::Value::String(s.clone()),
        serde_yaml::Value::Number(n) => {
            // serde_yaml's Number can be integer or float
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::String(n.to_string())
            }
        }
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_yaml::Value::Sequence(seq) => {
            serde_json::Value::Array(seq.iter().map(yaml_to_json).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut json_map = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => format!("{:?}", other),
                };
                json_map.insert(key, yaml_to_json(v));
            }
            serde_json::Value::Object(json_map)
        }
        serde_yaml::Value::Null => serde_json::Value::Null,
        // serde_yaml has a Tagged variant that we convert to string
        _ => serde_json::Value::String(format!("{:?}", value)),
    }
}

/// Recursively convert a `serde_json::Value` into a `serde_yaml::Value`.
fn json_to_yaml(value: &serde_json::Value) -> serde_yaml::Value {
    match value {
        serde_json::Value::String(s) => serde_yaml::Value::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_yaml::Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_yaml::Value::Number(serde_yaml::Number::from(f))
            } else {
                serde_yaml::Value::String(n.to_string())
            }
        }
        serde_json::Value::Bool(b) => serde_yaml::Value::Bool(*b),
        serde_json::Value::Array(arr) => {
            serde_yaml::Value::Sequence(arr.iter().map(json_to_yaml).collect())
        }
        serde_json::Value::Object(map) => {
            let mut yaml_map = serde_yaml::Mapping::new();
            for (k, v) in map {
                yaml_map.insert(serde_yaml::Value::String(k.clone()), json_to_yaml(v));
            }
            serde_yaml::Value::Mapping(yaml_map)
        }
        serde_json::Value::Null => serde_yaml::Value::Null,
    }
}
