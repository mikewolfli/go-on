//! Date/Time tools
//!
//! Provides current time in various formats, timestamp formatting,
//! duration calculations, and date string parsing.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use std::time::{Duration, UNIX_EPOCH};
use tracing::debug;

/// Format a `Duration` into a human-friendly string.
fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

pub struct DateTimeTool;

impl Tool for DateTimeTool {
    fn name(&self) -> &'static str {
        "date_time"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let operation = input.payload["operation"].as_str().unwrap_or("now");

        match operation {
            "now" => self.op_now(input),
            "format" => self.op_format(input),
            "diff" => self.op_diff(input),
            "parse" => self.op_parse(input),
            other => anyhow::bail!(
                "unsupported date_time operation '{}'. Supported: now, format, diff, parse",
                other
            ),
        }
    }
}

impl DateTimeTool {
    fn op_now(&self, _input: &ToolInput) -> Result<ToolOutput> {
        // Single-source timestamps (shared::timestamps) instead of a local copy.
        let unix_secs = crate::shared::timestamps::now_ts().max(0) as u64;
        let unix_millis = crate::shared::timestamps::now_ts_ms().max(0) as u64;

        // Build ISO 8601 string manually (no chrono dependency)
        let iso_8601 = iso_from_unix(unix_secs);

        debug!(
            unix_secs = %unix_secs,
            iso = %iso_8601,
            "tool: date_time now"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "operation": "now",
                "iso_8601": iso_8601,
                "unix_seconds": unix_secs,
                "unix_milliseconds": unix_millis,
            })),
            error: None,
            verification: Some("date_time_now".to_string()),
            audit_log: Some(format!(
                "date_time now -> {} (unix: {})",
                iso_8601, unix_secs
            )),
            pua_report: Some(tool_execution_report("date_time", Some("date_time_now"))),
        })
    }

    fn op_format(&self, input: &ToolInput) -> Result<ToolOutput> {
        let timestamp = input.payload["timestamp"].as_u64().ok_or_else(|| {
            anyhow::anyhow!("missing required field: timestamp (u64 unix seconds)")
        })?;

        let iso_8601 = iso_from_unix(timestamp);

        debug!(timestamp = %timestamp, iso = %iso_8601, "tool: date_time format");

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "operation": "format",
                "input_timestamp": timestamp,
                "iso_8601": iso_8601,
            })),
            error: None,
            verification: Some("date_time_formatted".to_string()),
            audit_log: Some(format!("date_time format {} -> {}", timestamp, iso_8601)),
            pua_report: Some(tool_execution_report(
                "date_time",
                Some("date_time_formatted"),
            )),
        })
    }

    fn op_diff(&self, input: &ToolInput) -> Result<ToolOutput> {
        let from = input.payload["from"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing required field: from (u64 unix seconds)"))?;
        let to = input.payload["to"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing required field: to (u64 unix seconds)"))?;

        let from_time = UNIX_EPOCH + Duration::from_secs(from);
        let to_time = UNIX_EPOCH + Duration::from_secs(to);

        let diff = if to >= from {
            to_time.duration_since(from_time).unwrap_or_default()
        } else {
            from_time.duration_since(to_time).unwrap_or_default()
        };

        let abs_seconds = diff.as_secs();
        let human = format_duration(diff);

        debug!(from = %from, to = %to, diff_secs = %abs_seconds, "tool: date_time diff");

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "operation": "diff",
                "from": from,
                "to": to,
                "diff_seconds": abs_seconds,
                "diff_human": human,
                "from_iso": iso_from_unix(from),
                "to_iso": iso_from_unix(to),
            })),
            error: None,
            verification: Some("date_time_diff".to_string()),
            audit_log: Some(format!(
                "date_time diff {} -> {} = {}s ({})",
                from, to, abs_seconds, human
            )),
            pua_report: Some(tool_execution_report("date_time", Some("date_time_diff"))),
        })
    }

    fn op_parse(&self, input: &ToolInput) -> Result<ToolOutput> {
        let date_str = input.payload["date"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: date (ISO 8601 string)"))?;

        // Parse simple ISO 8601 formats like "2024-01-15T10:30:00Z"
        // or "2024-01-15T10:30:00+00:00"
        use std::num::ParseIntError;

        fn parse_two_digit(s: &str) -> Result<u64, ParseIntError> {
            // Char-safe: the field may carry non-ASCII digits (CJK/full-width),
            // so a naive `&s[..2]` byte slice could land mid-code-point.
            let head: String = s.chars().take(2).collect();
            if head.len() >= 2 || head.len() == s.len() {
                head.parse()
            } else {
                s.parse()
            }
        }

        // Normalize: strip trailing Z, replace T with space for uniform parsing
        let cleaned = date_str
            .trim_end_matches('Z')
            .trim_end_matches("+00:00")
            .trim_end_matches("-00:00");
        let cleaned = cleaned.replace('T', " ");

        // Try "YYYY-MM-DD HH:MM:SS" or "YYYY-MM-DD HH:MM"
        let parts: Vec<&str> = cleaned.split([' ', 'T']).collect();
        let date_part = parts.first().copied().unwrap_or("");
        let time_part = parts.get(1).copied().unwrap_or("00:00:00");

        let date_fields: Vec<&str> = date_part.split('-').collect();
        let time_fields: Vec<&str> = time_part.split(':').collect();

        if date_fields.len() < 3 {
            anyhow::bail!(
                "unable to parse date string '{}': expected YYYY-MM-DD format",
                date_str
            );
        }

        let year: u64 = date_fields[0].parse().context("failed to parse year")?;
        let month: u64 = parse_two_digit(date_fields[1])
            .map_err(|_| anyhow::anyhow!("failed to parse month"))?;
        let day: u64 =
            parse_two_digit(date_fields[2]).map_err(|_| anyhow::anyhow!("failed to parse day"))?;

        // Validate ranges before any arithmetic: a hostile date like
        // "2024-01-00" or "9999999999-01-01" must not underflow `day - 1`,
        // hang the year loop, or overflow `total_days * 86400`.
        if !(1970..=2100).contains(&year) {
            anyhow::bail!("year out of supported range 1970-2100: {year}");
        }
        if !(1..=12).contains(&month) {
            anyhow::bail!("month out of range 1-12: {month}");
        }
        if day == 0 || day > days_in_month(year, month) {
            anyhow::bail!("day out of range for {year}-{month:02}: {day}");
        }
        let hour: u64 = if !time_fields.is_empty() {
            parse_two_digit(time_fields[0]).unwrap_or(0)
        } else {
            0
        };
        let minute: u64 = if time_fields.len() > 1 {
            parse_two_digit(time_fields[1]).unwrap_or(0)
        } else {
            0
        };
        let second: u64 = if time_fields.len() > 2 {
            parse_two_digit(time_fields[2]).unwrap_or(0)
        } else {
            0
        };
        if hour > 23 || minute > 59 || second > 59 {
            anyhow::bail!("time out of range: {hour}:{minute}:{second}");
        }

        // Approximate Unix timestamp using a basic days-since-epoch calculation.
        // This handles dates from 1970 to 2100 reasonably well.
        fn days_in_year(y: u64) -> u64 {
            if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
                366
            } else {
                365
            }
        }

        fn days_in_month(y: u64, m: u64) -> u64 {
            match m {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                2 => {
                    if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
                        29
                    } else {
                        28
                    }
                }
                _ => 0,
            }
        }

        // Days from 1970-01-01 to the given date
        let mut total_days = 0u64;
        for y in 1970..year {
            total_days += days_in_year(y);
        }
        for m in 1..month {
            total_days += days_in_month(year, m);
        }
        total_days += day - 1;

        let total_seconds = total_days * 86400 + hour * 3600 + minute * 60 + second;

        debug!(
            input = %date_str,
            year = %year,
            month = %month,
            day = %day,
            hour = %hour,
            minute = %minute,
            second = %second,
            unix = %total_seconds,
            "tool: date_time parse"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "operation": "parse",
                "input": date_str,
                "parsed": {
                    "year": year,
                    "month": month,
                    "day": day,
                    "hour": hour,
                    "minute": minute,
                    "second": second,
                },
                "unix_seconds": total_seconds,
                "iso_8601": iso_from_unix(total_seconds),
            })),
            error: None,
            verification: Some("date_time_parsed".to_string()),
            audit_log: Some(format!(
                "date_time parse '{}' -> unix {}",
                date_str, total_seconds
            )),
            pua_report: Some(tool_execution_report("date_time", Some("date_time_parsed"))),
        })
    }
}

/// Format a Unix timestamp as ISO 8601 (e.g. "2024-01-15T10:30:00Z").
/// Uses only stdlib date/time calculations.
///
/// Input is clamped to a sane range (≈ 1970-01-01 .. 2239-01-01): a hostile
/// u64::MAX timestamp would otherwise loop ~5.8e11 years (minute-scale hang).
fn iso_from_unix(unix_secs: u64) -> String {
    const MAX_UNIX_SECS: u64 = 8_500_000_000; // ≈ 2239-01-01
    let unix_secs = unix_secs.min(MAX_UNIX_SECS);
    let days_since_epoch = unix_secs / 86400;
    let remaining_secs = unix_secs % 86400;

    let hours = remaining_secs / 3600;
    let minutes = (remaining_secs % 3600) / 60;
    let seconds = remaining_secs % 60;

    // Compute year/month/day from days since 1970-01-01
    let mut y = 1970i64;
    let mut remaining_days = days_since_epoch as i64;

    loop {
        let days_this_year = if is_leap(y) { 366 } else { 365 };
        if remaining_days < days_this_year {
            break;
        }
        remaining_days -= days_this_year;
        y += 1;
    }

    let year = y as u64;
    let month_days = [
        31,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];

    let mut m = 1u64;
    let mut day_of_month = remaining_days as u64 + 1;
    for &md in &month_days {
        if day_of_month > md {
            day_of_month -= md;
            m += 1;
        } else {
            break;
        }
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, day_of_month, hours, minutes, seconds
    )
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-time".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn date_time_now_returns_valid() {
        let tool = DateTimeTool;
        let input = tool_input(serde_json::json!({ "operation": "now" }));
        let output = tool.run(&input).expect("date_time now should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert!(result["unix_seconds"].as_u64().unwrap() > 1_600_000_000);
        assert!(result["iso_8601"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn date_time_format_known_timestamp() {
        let tool = DateTimeTool;
        let input = tool_input(serde_json::json!({
            "operation": "format",
            "timestamp": 1_704_060_000,
        }));
        let output = tool.run(&input).expect("date_time format should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        // ~2024-01-01
        let iso = result["iso_8601"].as_str().unwrap();
        assert!(iso.contains("2024") || iso.contains("2023"));
    }

    #[test]
    fn date_time_diff_positive() {
        let tool = DateTimeTool;
        let input = tool_input(serde_json::json!({
            "operation": "diff",
            "from": 1_700_000_000,
            "to": 1_700_008_000,
        }));
        let output = tool.run(&input).expect("date_time diff should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["diff_seconds"].as_u64().unwrap(), 8000);
    }

    #[test]
    fn date_time_parse_iso() {
        let tool = DateTimeTool;
        let input = tool_input(serde_json::json!({
            "operation": "parse",
            "date": "2024-01-01T00:00:00Z",
        }));
        let output = tool.run(&input).expect("date_time parse should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        // 2024-01-01 00:00:00 UTC = 1704067200
        assert_eq!(result["unix_seconds"].as_u64().unwrap(), 1_704_067_200);
    }

    #[test]
    fn date_time_invalid_operation() {
        let tool = DateTimeTool;
        let input = tool_input(serde_json::json!({ "operation": "nonexistent" }));
        let result = tool.run(&input);
        assert!(result.is_err());
    }

    #[test]
    fn iso_roundtrip() {
        // Verify that iso_from_unix produces correct output for known values
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(86400), "1970-01-02T00:00:00Z");
        // 2024-01-01 00:00:00 UTC
        assert_eq!(iso_from_unix(1_704_067_200), "2024-01-01T00:00:00Z");
    }
}
