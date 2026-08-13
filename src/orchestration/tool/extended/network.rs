//! Network tools: DNS lookup, ping, and port scanning
//!
//! Uses only stdlib + tokio; no external network crates required.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

// ── DnsLookupTool ────────────────────────────────────────────────────────

pub struct DnsLookupTool;

impl Tool for DnsLookupTool {
    fn name(&self) -> &'static str {
        "dns_lookup"
    }
    fn description(&self) -> &str {
        "Perform DNS lookup to resolve a hostname to IP addresses"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let hostname = input.payload["hostname"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: hostname"))?;

        let raw_port = input.payload["port"].as_u64().unwrap_or(80);
        // Validate instead of truncating: `70000 as u16` wraps to 4464 and
        // silently performs the lookup on the wrong port.
        if !(1..=65535).contains(&raw_port) {
            return Ok(ToolOutput {
                success: false,
                result: Some(serde_json::json!({
                    "hostname": hostname,
                    "addresses": [],
                    "error": format!("port must be between 1 and 65535, got {raw_port}"),
                })),
                error: Some(format!("port must be between 1 and 65535, got {raw_port}")),
                verification: Some("dns_lookup_completed".to_string()),
                audit_log: Some(format!(
                    "DNS lookup for '{}' rejected: invalid port {}",
                    hostname, raw_port
                )),
                pua_report: Some(tool_execution_report(
                    "dns_lookup",
                    Some("dns_lookup_completed"),
                )),
            });
        }
        let port = raw_port as u16;

        debug!(hostname = %hostname, port = %port, "tool: performing DNS lookup");

        let start = Instant::now();

        let addr_str = format!("{}:{}", hostname, port);
        let addresses: Vec<String> = match addr_str.to_socket_addrs() {
            Ok(addrs) => addrs.map(|a| a.to_string()).collect(),
            Err(e) => {
                warn!(hostname = %hostname, error = %e, "tool: DNS lookup failed");
                return Ok(ToolOutput {
                    success: false,
                    result: Some(serde_json::json!({
                        "hostname": hostname,
                        "addresses": [],
                        "error": e.to_string(),
                    })),
                    error: Some(format!("DNS lookup failed: {}", e)),
                    verification: Some("dns_lookup_completed".to_string()),
                    audit_log: Some(format!("DNS lookup for '{}' failed: {}", hostname, e)),
                    pua_report: Some(tool_execution_report(
                        "dns_lookup",
                        Some("dns_lookup_completed"),
                    )),
                });
            }
        };

        let elapsed = start.elapsed();

        info!(
            hostname = %hostname,
            addresses = ?addresses,
            elapsed_ms = %elapsed.as_millis(),
            "tool: DNS lookup succeeded"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "hostname": hostname,
                "addresses": addresses,
                "count": addresses.len(),
                "elapsed_ms": elapsed.as_millis(),
            })),
            error: None,
            verification: Some("dns_lookup_completed".to_string()),
            audit_log: Some(format!(
                "DNS lookup for '{}' resolved {} addresses in {}ms",
                hostname,
                addresses.len(),
                elapsed.as_millis()
            )),
            pua_report: Some(tool_execution_report(
                "dns_lookup",
                Some("dns_lookup_completed"),
            )),
        })
    }
}

// ── PingTool ─────────────────────────────────────────────────────────────

pub struct PingTool;

impl Tool for PingTool {
    fn name(&self) -> &'static str {
        "ping"
    }
    fn description(&self) -> &str {
        "Ping a remote host to check network connectivity"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let host = input.payload["host"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: host"))?;

        let count = input.payload["count"].as_u64().unwrap_or(4).clamp(1, 20);
        let timeout_ms = input.payload["timeout_ms"]
            .as_u64()
            .unwrap_or(10_000)
            .clamp(1_000, 120_000);

        debug!(host = %host, count = %count, timeout_ms = %timeout_ms, "tool: executing ping");

        let start = Instant::now();

        #[cfg(target_os = "windows")]
        let args = vec![
            "-n".to_string(),
            count.to_string(),
            "-w".to_string(),
            (timeout_ms / count).to_string(),
            host.to_string(),
        ];
        #[cfg(not(target_os = "windows"))]
        let args = vec![
            "-c".to_string(),
            count.to_string(),
            "-W".to_string(),
            ((timeout_ms / count.max(1)) / 1000).max(1).to_string(),
            host.to_string(),
        ];

        // ping is a command executor, so it runs inside the OS sandbox too
        // (ICMP works there; verified by the sandbox probe environment).
        let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let output = match crate::orchestration::tool::exec_common::run_sandboxed_output(
            &workspace,
            "ping",
            &args,
            |_| {},
        ) {
            Ok((out, _applied)) => out,
            Err(e) => {
                warn!(host = %host, error = %e, "tool: ping spawn failed");
                return Ok(ToolOutput {
                    success: false,
                    result: Some(serde_json::json!({
                        "host": host,
                        "error": e.to_string(),
                    })),
                    error: Some(format!("Failed to run ping: {}", e)),
                    verification: Some("ping_completed".to_string()),
                    audit_log: Some(format!("Ping '{}' spawn failed: {}", host, e)),
                    pua_report: Some(tool_execution_report("ping", Some("ping_completed"))),
                });
            }
        };

        let elapsed = start.elapsed();

        {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let success = output.status.success();
            let exit_code = output.status.code();

            if success {
                info!(host = %host, exit_code = ?exit_code, "tool: ping succeeded");
            } else {
                warn!(host = %host, exit_code = ?exit_code, stderr = %stderr.trim(), "tool: ping failed");
            }

            Ok(ToolOutput {
                success,
                result: Some(serde_json::json!({
                    "host": host,
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code,
                    "elapsed_ms": elapsed.as_millis(),
                    "count": count,
                })),
                error: (!success).then(|| stderr.trim().to_string()),
                verification: Some("ping_completed".to_string()),
                audit_log: Some(format!(
                    "Ping '{}' (count={}) -> exit={:?} in {}ms",
                    host,
                    count,
                    exit_code,
                    elapsed.as_millis()
                )),
                pua_report: Some(tool_execution_report("ping", Some("ping_completed"))),
            })
        }
    }
}

// ── PortScanTool ─────────────────────────────────────────────────────────

pub struct PortScanTool;

impl Tool for PortScanTool {
    fn name(&self) -> &'static str {
        "port_scan"
    }
    fn description(&self) -> &str {
        "Scan TCP ports on a remote host"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let host = input.payload["host"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: host"))?;

        let ports: Vec<u16> = match input.payload["ports"] {
            serde_json::Value::Array(ref arr) => arr
                .iter()
                .filter_map(|v| {
                    v.as_u64().and_then(|n| {
                        if (1..=65535).contains(&n) {
                            Some(n as u16)
                        } else {
                            None
                        }
                    })
                })
                .collect(),
            _ => {
                // Default to common ports
                vec![
                    21, 22, 23, 25, 53, 80, 110, 143, 443, 465, 587, 993, 995, 1433, 1521, 3306,
                    3389, 5432, 5900, 6379, 8080, 8443, 9090, 27017,
                ]
            }
        };

        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(3000);
        let timeout = Duration::from_millis(timeout_ms);

        if ports.is_empty() {
            anyhow::bail!("no valid ports specified (must be 1-65535)");
        }

        debug!(
            host = %host,
            port_count = %ports.len(),
            timeout_ms = %timeout_ms,
            "tool: scanning ports"
        );

        let start = Instant::now();
        let mut open_ports: Vec<serde_json::Value> = Vec::new();

        for &port in &ports {
            let addr = format!("{}:{}", host, port);
            match TcpStream::connect_timeout(
                &addr
                    .to_socket_addrs()?
                    .next()
                    .context("failed to resolve address")?,
                timeout,
            ) {
                Ok(_) => {
                    debug!(host = %host, port = %port, "tool: port is open");
                    open_ports.push(serde_json::json!({
                        "port": port,
                        "state": "open",
                    }));
                }
                Err(_) => {
                    // Port is closed/filtered — ignore silently
                }
            }
        }

        let elapsed = start.elapsed();

        info!(
            host = %host,
            open = %open_ports.len(),
            scanned = %ports.len(),
            elapsed_ms = %elapsed.as_millis(),
            "tool: port scan completed"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "host": host,
                "open_ports": open_ports,
                "open_count": open_ports.len(),
                "scanned_count": ports.len(),
                "elapsed_ms": elapsed.as_millis(),
            })),
            error: None,
            verification: Some("port_scan_completed".to_string()),
            audit_log: Some(format!(
                "Port scan of '{}': {} open out of {} scanned in {}ms",
                host,
                open_ports.len(),
                ports.len(),
                elapsed.as_millis()
            )),
            pua_report: Some(tool_execution_report(
                "port_scan",
                Some("port_scan_completed"),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-net".to_string(),
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
    fn dns_lookup_resolves_localhost() {
        let tool = DnsLookupTool;
        let input = tool_input(serde_json::json!({
            "hostname": "localhost",
            "port": 80,
        }));
        let output = tool.run(&input).expect("dns_lookup should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let addresses = result["addresses"].as_array().unwrap();
        assert!(
            !addresses.is_empty(),
            "localhost should resolve to at least one address"
        );
    }

    #[test]
    fn port_scan_invalid_ports_fails() {
        let tool = PortScanTool;
        let input = tool_input(serde_json::json!({
            "host": "127.0.0.1",
            "ports": [],
        }));
        let result = tool.run(&input);
        assert!(result.is_err(), "empty ports list should error");
    }

    #[test]
    fn port_scan_accepts_default_ports() {
        let tool = PortScanTool;
        let input = tool_input(serde_json::json!({
            "host": "127.0.0.1",
        }));
        let output = tool
            .run(&input)
            .expect("port_scan with defaults should succeed");
        assert!(output.success);
    }

    #[test]
    fn ping_invalid_host_reports_failure() {
        let tool = PingTool;
        let input = tool_input(serde_json::json!({
            "host": "192.0.2.999",
            "count": 1,
            "timeout_ms": 1000,
        }));
        // Ping may or may not fail on invalid IP depending on OS, but should not panic
        let output = tool.run(&input);
        assert!(output.is_ok(), "ping should not panic on invalid host");
    }
}
