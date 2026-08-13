//! LSP-like code intelligence tools.
//!
//! Provides symbol navigation and code action capabilities without
//! requiring a running LSP server. Uses direct code analysis:
//! - `go_to_definition`: searches for struct/impl/fn/trait/enum definitions
//! - `find_references`: searches for usages across the codebase
//! - `apply_code_action`: provides common code action patterns (add import, fix diagnostics)
//!
//! When `lsp_address` is provided, connects to an LSP server via TCP
//! for accurate results using the LSP protocol.

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::debug;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};

// ── LSP TCP Client ──────────────────────────────────────────────────

/// Minimal async LSP TCP client. Connects to an LSP server, performs
/// the LSP handshake (initialize + initialized), and provides methods
/// for definition, references, and code action queries.
struct LspClient {
    stream: Arc<Mutex<TcpStream>>,
    next_id: AtomicU64,
}

impl LspClient {
    /// Connect to an LSP server at `address` (e.g. "127.0.0.1:9258").
    /// Sends `initialize` + `initialized` automatically.
    async fn connect(address: &str) -> Result<Self> {
        let stream = TcpStream::connect(address)
            .await
            .with_context(|| format!("failed to connect to LSP server at {address}"))?;
        let client = Self {
            stream: Arc::new(Mutex::new(stream)),
            next_id: AtomicU64::new(1),
        };
        // Send initialize request
        client
            .send_request(
                "initialize",
                json!({
                    "processId": null,
                    "capabilities": {},
                    "rootUri": null,
                }),
            )
            .await?;
        // Then initialized notification
        client.send_notification("initialized", json!({})).await?;
        Ok(client)
    }

    /// Send a JSON-RPC request and wait for the response.
    async fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&request)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stream = self.stream.lock().await;
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        stream.flush().await?;
        // Read response
        Self::read_response(&mut stream).await
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&notification)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stream = self.stream.lock().await;
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Read an LSP response from the TCP stream. Parses the
    /// Content-Length header and reads exactly that many body bytes.
    async fn read_response(stream: &mut TcpStream) -> Result<Value> {
        let mut buf = vec![0u8; 8192];
        let mut pos = 0;
        loop {
            // Grow the buffer when full so responses larger than the initial
            // 8 KiB (large hover/diagnostic payloads) keep reading instead of
            // hitting a zero-length read and being misread as a closed
            // connection.
            if pos >= buf.len() {
                let new_len = buf.len().saturating_mul(2).max(16 * 1024);
                buf.resize(new_len, 0u8);
            }
            let n = stream.read(&mut buf[pos..]).await?;
            if n == 0 {
                anyhow::bail!("LSP connection closed");
            }
            pos += n;
            let header_end = buf[..pos].windows(4).position(|w| w == b"\r\n\r\n");
            if let Some(end) = header_end {
                let header_str = std::str::from_utf8(&buf[..end])?;
                let content_length = header_str
                    .split("\r\n")
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .ok_or_else(|| anyhow::anyhow!("missing Content-Length header"))?;
                let body_start = end + 4;
                // Guard against absurd Content-Length values from a hostile or
                // malformed LSP server: checked_add prevents overflow panic and
                // the cap avoids giant allocations (OOM abort).
                const MAX_LSP_RESPONSE_BODY: usize = 64 * 1024 * 1024;
                if content_length > MAX_LSP_RESPONSE_BODY {
                    anyhow::bail!("LSP response body too large ({content_length} bytes)");
                }
                let total_needed = body_start
                    .checked_add(content_length)
                    .ok_or_else(|| anyhow::anyhow!("LSP Content-Length overflow"))?;
                // The body may exceed the initial buffer — resize up front so
                // the body-read loop never runs out of room.
                if total_needed > buf.len() {
                    buf.resize(total_needed, 0u8);
                }
                while pos < total_needed {
                    let n = stream.read(&mut buf[pos..]).await?;
                    if n == 0 {
                        anyhow::bail!("LSP connection closed");
                    }
                    pos += n;
                }
                let body = std::str::from_utf8(&buf[body_start..body_start + content_length])?;
                let response: Value = serde_json::from_str(body)?;
                return Ok(response);
            }
        }
    }

    /// Open a document via `textDocument/didOpen` notification.
    async fn open_document(&self, file_path: &str) -> Result<()> {
        let uri = format!("file://{}", file_path);
        self.send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "rust",
                    "version": 1,
                    "text": "",
                },
            }),
        )
        .await?;
        Ok(())
    }

    /// Query `textDocument/definition` at the given position (1-based).
    /// Returns a list of normalized location objects.
    async fn query_definition(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> Result<Vec<Value>> {
        let uri = format!("file://{}", file_path);
        self.open_document(file_path).await?;
        let resp = self
            .send_request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": line.saturating_sub(1),
                        "character": column.saturating_sub(1),
                    },
                }),
            )
            .await?;

        let result = resp
            .get("result")
            .ok_or_else(|| anyhow::anyhow!("LSP definition response missing 'result'"))?;

        // LSP can return a single Location or a Location[]
        let locations = match result {
            Value::Array(arr) => arr.clone(),
            Value::Object(_) => vec![result.clone()],
            Value::Null => vec![],
            _ => vec![result.clone()],
        };

        // Normalize to our output format
        let definitions: Vec<Value> = locations
            .iter()
            .filter_map(|loc| {
                let uri = loc.get("uri")?.as_str()?;
                let range = loc.get("range")?;
                let start = range.get("start")?;
                let def_line = start.get("line")?.as_u64()?;
                let file = uri.strip_prefix("file://").unwrap_or(uri);
                Some(json!({
                    "file": file,
                    // LSP lines are 0-based; saturate so a hostile server's
                    // u64::MAX line cannot overflow in debug builds.
                    "line": def_line.saturating_add(1),
                    "content": "",
                }))
            })
            .collect();

        Ok(definitions)
    }

    /// Query `textDocument/references` at the given position (1-based).
    async fn query_references(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> Result<Vec<Value>> {
        let uri = format!("file://{}", file_path);
        self.open_document(file_path).await?;
        let resp = self
            .send_request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": line.saturating_sub(1),
                        "character": column.saturating_sub(1),
                    },
                    "context": {
                        "includeDeclaration": false,
                    },
                }),
            )
            .await?;

        let result = resp
            .get("result")
            .ok_or_else(|| anyhow::anyhow!("LSP references response missing 'result'"))?;

        let locations = match result {
            Value::Array(arr) => arr.clone(),
            Value::Null => vec![],
            _ => vec![result.clone()],
        };

        let references: Vec<Value> = locations
            .iter()
            .filter_map(|loc| {
                let uri = loc.get("uri")?.as_str()?;
                let range = loc.get("range")?;
                let start = range.get("start")?;
                let ref_line = start.get("line")?.as_u64()?;
                let file = uri.strip_prefix("file://").unwrap_or(uri);
                Some(json!({
                    "file": file,
                    // LSP lines are 0-based; saturate (see query_definitions).
                    "line": ref_line.saturating_add(1),
                    "content": "",
                }))
            })
            .collect();

        Ok(references)
    }

    /// Query `textDocument/codeAction` at the given position and apply
    /// the first matching action. Returns the result of applying the edit.
    async fn query_code_action(
        &self,
        file_path: &str,
        action: &str,
        detail: &str,
        line: u32,
        column: u32,
    ) -> Result<Value> {
        let uri = format!("file://{}", file_path);
        self.open_document(file_path).await?;

        let resp = self
            .send_request(
                "textDocument/codeAction",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": line.saturating_sub(1), "character": column.saturating_sub(1) },
                        "end": { "line": line.saturating_sub(1) + 1, "character": column.saturating_sub(1) + 1 },
                    },
                    "context": {
                        "diagnostics": [],
                    },
                }),
            )
            .await?;

        let result = resp
            .get("result")
            .ok_or_else(|| anyhow::anyhow!("LSP codeAction response missing 'result'"))?;

        let actions = match result {
            Value::Array(arr) => arr.clone(),
            Value::Null => vec![],
            _ => vec![result.clone()],
        };

        if actions.is_empty() {
            anyhow::bail!("LSP server returned no code actions");
        }

        // Try to find an action matching the requested action + detail
        let matched = actions.iter().find(|a| {
            let title = a
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_lowercase();
            title.contains(&action.to_lowercase())
                || (!detail.is_empty() && title.contains(&detail.to_lowercase()))
        });

        let chosen = match matched {
            Some(a) => a,
            None => actions
                .first()
                .ok_or_else(|| anyhow::anyhow!("no code actions available"))?,
        };

        // Check if the action has an edit to apply
        if let Some(edit) = chosen.get("edit") {
            let apply_resp = self
                .send_request("workspace/applyEdit", json!({ "edit": edit }))
                .await?;
            let applied = apply_resp
                .get("result")
                .and_then(|r| r.get("applied"))
                .and_then(|a| a.as_bool())
                .unwrap_or(false);
            return Ok(json!({
                "applied": applied,
                "title": chosen.get("title"),
                "kind": chosen.get("kind"),
            }));
        }

        // If no edit but there's a command, resolve and execute
        if let Some(command) = chosen.get("command") {
            let cmd_resp = self
                .send_request(
                    "workspace/executeCommand",
                    json!({
                        "command": command.get("command"),
                        "arguments": command.get("arguments"),
                    }),
                )
                .await?;
            return Ok(json!({
                "applied": true,
                "command_result": cmd_resp.get("result"),
                "title": chosen.get("title"),
            }));
        }

        Ok(json!({
            "applied": false,
            "title": chosen.get("title"),
            "message": "code action has no edit or command to apply",
        }))
    }
}

/// Maximum number of distinct LSP clients kept in the cache. When the cache
/// exceeds this, the oldest (FIFO) client is evicted so an unbounded set of
/// LSP addresses cannot leak open connections.
const MAX_CACHED_LSP_CLIENTS: usize = 32;

/// LSP client cache: address → client, plus a FIFO queue of insertion order
/// for oldest-first eviction.
type LspClientCache = (
    std::collections::HashMap<String, Arc<LspClient>>,
    std::collections::VecDeque<String>,
);

/// Connect to an LSP server, reusing a cached client when one already exists
/// for the same address (avoids a fresh TCP connection + initialize handshake
/// on every tool call).
async fn cached_lsp_client(addr: &str) -> Result<Arc<LspClient>> {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    static CLIENTS: OnceLock<Mutex<LspClientCache>> = OnceLock::new();
    {
        // Fast path: reuse an existing client for this address. The guard is
        // dropped before the (potentially slow) connect below.
        let map = CLIENTS
            .get_or_init(|| Mutex::new((HashMap::new(), VecDeque::new())))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(client) = map.0.get(addr) {
            return Ok(client.clone());
        }
    }
    let client = Arc::new(LspClient::connect(addr).await?);
    let mut map = CLIENTS
        .get_or_init(|| Mutex::new((HashMap::new(), VecDeque::new())))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // The key may have been inserted by a concurrent caller while we were
    // connecting — only track FIFO order for genuinely new addresses.
    if !map.0.contains_key(addr) {
        map.1.push_back(addr.to_string());
        if map.0.len() >= MAX_CACHED_LSP_CLIENTS {
            if let Some(oldest) = map.1.pop_front() {
                map.0.remove(&oldest);
            }
        }
    }
    map.0.insert(addr.to_string(), client.clone());
    Ok(client)
}

/// Helper: run an async LSP `query_definition` from a sync context.
/// Uses the shared LSP runtime to avoid creating a new Runtime each call.
fn run_lsp_definition(
    address: &str,
    file_path: &str,
    line: u32,
    column: u32,
) -> Result<Vec<Value>> {
    let addr = address.to_string();
    let fp = file_path.to_string();
    crate::orchestration::tool::exec_common::with_blocking_runtime(|rt| {
        rt.block_on(async {
            let client = cached_lsp_client(&addr).await?;
            client.query_definition(&fp, line, column).await
        })
    })
}

/// Helper: run an async LSP `query_references` from a sync context.
/// Uses the shared LSP runtime to avoid creating a new Runtime each call.
fn run_lsp_references(
    address: &str,
    file_path: &str,
    line: u32,
    column: u32,
) -> Result<Vec<Value>> {
    let addr = address.to_string();
    let fp = file_path.to_string();
    crate::orchestration::tool::exec_common::with_blocking_runtime(|rt| {
        rt.block_on(async {
            let client = cached_lsp_client(&addr).await?;
            client.query_references(&fp, line, column).await
        })
    })
}

/// Helper: run an async LSP `query_code_action` from a sync context.
/// Uses the shared LSP runtime to avoid creating a new Runtime each call.
fn run_lsp_code_action(
    address: &str,
    file_path: &str,
    action: &str,
    detail: &str,
    line: u32,
    column: u32,
) -> Result<Value> {
    let addr = address.to_string();
    let fp = file_path.to_string();
    let act = action.to_string();
    let det = detail.to_string();
    crate::orchestration::tool::exec_common::with_blocking_runtime(|rt| {
        rt.block_on(async {
            let client = cached_lsp_client(&addr).await?;
            client
                .query_code_action(&fp, &act, &det, line, column)
                .await
        })
    })
}

// ── GoToDefinitionTool ─────────────────────────────────────────────

/// Find the definition of a symbol (function, struct, enum, trait, etc.)
/// by searching the codebase for declaration patterns.
///
/// When `lsp_address` is provided, connects to an LSP server via TCP
/// and uses `textDocument/definition` for accurate results.
pub struct GoToDefinitionTool;

impl Tool for GoToDefinitionTool {
    fn name(&self) -> &'static str {
        "go_to_definition"
    }

    fn description(&self) -> &str {
        "Find definition of a symbol (fn, struct, enum, trait, impl, type, const) in the codebase"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "The symbol name to find the definition of"},
                "directory": {"type": "string", "description": "Optional directory scope (default: project root)"},
                "language": {
                    "type": "string",
                    "enum": ["auto", "rust", "python", "typescript", "javascript", "go", "java"],
                    "description": "Language hint for definition patterns (default: auto-detect from file extension)"
                },
                "lsp_address": {"type": "string", "description": "Optional LSP TCP address (e.g. '127.0.0.1:9258'). Uses LSP protocol instead of regex-based search. Requires 'path', 'line', and 'column'."},
                "path": {"type": "string", "description": "File path to query for definition (required when lsp_address is set)"},
                "line": {"type": "integer", "description": "Line number (1-based) of the symbol usage (required when lsp_address is set)"},
                "column": {"type": "integer", "description": "Column number (1-based) of the symbol usage (required when lsp_address is set)"}
            },
            "required": ["symbol"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let symbol = input.payload["symbol"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("go_to_definition requires arguments.symbol"))?;

        // ── LSP mode ─────────────────────────────────────────────────
        if let Some(lsp_address) = input.payload.get("lsp_address").and_then(|v| v.as_str()) {
            let file_path = input.payload["path"].as_str().ok_or_else(|| {
                anyhow::anyhow!("go_to_definition with lsp_address requires arguments.path")
            })?;
            let line = input.payload["line"].as_u64().ok_or_else(|| {
                anyhow::anyhow!(
                    "go_to_definition with lsp_address requires arguments.line (1-based)"
                )
            })? as u32;
            let column = input.payload["column"].as_u64().ok_or_else(|| {
                anyhow::anyhow!(
                    "go_to_definition with lsp_address requires arguments.column (1-based)"
                )
            })? as u32;

            let definitions = run_lsp_definition(lsp_address, file_path, line, column)?;

            debug!(
                symbol = %symbol,
                count = definitions.len(),
                "go_to_definition (LSP): found definitions"
            );

            return Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "symbol": symbol,
                    "definitions": definitions,
                    "found": !definitions.is_empty(),
                    "mode": "lsp",
                })),
                error: None,
                verification: Some("definition_search_completed".to_string()),
                audit_log: Some(format!(
                    "go_to_definition (LSP) '{}' at {}:{}:{}: {} definition(s) found",
                    symbol,
                    file_path,
                    line,
                    column,
                    definitions.len()
                )),
                pua_report: Some(tool_execution_report(
                    "go_to_definition",
                    Some("definition_search_completed"),
                )),
            });
        }

        // ── Regex mode (fallback) ────────────────────────────────────
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let language = input
            .payload
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");

        let root = sanitize_path(input, directory)?;
        let escaped = regex::escape(symbol);
        let patterns = build_definition_patterns(&escaped, language);

        let mut definitions: Vec<Value> = Vec::new();

        for pattern in &patterns {
            if let Ok(re) = Regex::new(pattern) {
                collect_definition_matches(&root, &root, &re, &mut definitions, 50)?;
                if !definitions.is_empty() {
                    break;
                }
            }
        }

        debug!(
            symbol = %symbol,
            count = definitions.len(),
            "go_to_definition: found definitions"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "symbol": symbol,
                "definitions": definitions,
                "found": !definitions.is_empty(),
            })),
            error: None,
            verification: Some("definition_search_completed".to_string()),
            audit_log: Some(format!(
                "go_to_definition '{}': {} definition(s) found",
                symbol,
                definitions.len()
            )),
            pua_report: Some(tool_execution_report(
                "go_to_definition",
                Some("definition_search_completed"),
            )),
        })
    }
}

// ── FindReferencesTool ─────────────────────────────────────────────

/// Find all references to a symbol by searching the codebase for its usage.
/// Excludes the definition site so results are pure references.
///
/// When `lsp_address` is provided, connects to an LSP server via TCP
/// and uses `textDocument/references` for accurate results.
pub struct FindReferencesTool;

impl Tool for FindReferencesTool {
    fn name(&self) -> &'static str {
        "find_references"
    }

    fn description(&self) -> &str {
        "Find all references to a symbol across the codebase (source files only)"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "The symbol name to find references for"},
                "directory": {"type": "string", "description": "Optional directory scope (default: project root)"},
                "include": {"type": "string", "description": "Optional glob pattern to filter files (e.g. '**/*.rs')"},
                "lsp_address": {"type": "string", "description": "Optional LSP TCP address (e.g. '127.0.0.1:9258'). Uses LSP protocol instead of regex-based search. Requires 'path', 'line', and 'column'."},
                "path": {"type": "string", "description": "File path to query for references (required when lsp_address is set)"},
                "line": {"type": "integer", "description": "Line number (1-based) of the symbol usage (required when lsp_address is set)"},
                "column": {"type": "integer", "description": "Column number (1-based) of the symbol usage (required when lsp_address is set)"}
            },
            "required": ["symbol"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let symbol = input.payload["symbol"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("find_references requires arguments.symbol"))?;

        // ── LSP mode ─────────────────────────────────────────────────
        if let Some(lsp_address) = input.payload.get("lsp_address").and_then(|v| v.as_str()) {
            let file_path = input.payload["path"].as_str().ok_or_else(|| {
                anyhow::anyhow!("find_references with lsp_address requires arguments.path")
            })?;
            let line = input.payload["line"].as_u64().ok_or_else(|| {
                anyhow::anyhow!(
                    "find_references with lsp_address requires arguments.line (1-based)"
                )
            })? as u32;
            let column = input.payload["column"].as_u64().ok_or_else(|| {
                anyhow::anyhow!(
                    "find_references with lsp_address requires arguments.column (1-based)"
                )
            })? as u32;

            let references = run_lsp_references(lsp_address, file_path, line, column)?;
            let total = references.len();

            debug!(
                symbol = %symbol,
                count = total,
                "find_references (LSP): search complete"
            );

            return Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "symbol": symbol,
                    "references": references,
                    "files_scanned": 0,
                    "total": total,
                    "truncated": false,
                    "mode": "lsp",
                })),
                error: None,
                verification: Some("reference_search_completed".to_string()),
                audit_log: Some(format!(
                    "find_references (LSP) '{}' at {}:{}:{}: {} reference(s)",
                    symbol, file_path, line, column, total
                )),
                pua_report: Some(tool_execution_report(
                    "find_references",
                    Some("reference_search_completed"),
                )),
            });
        }

        // ── Regex mode (fallback) ────────────────────────────────────
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let include_pattern = input.payload["include"].as_str();

        let root = sanitize_path(input, directory)?;

        let escaped = regex::escape(symbol);
        let word_re =
            Regex::new(&format!(r"\b{}\b", escaped)).context("failed to build reference regex")?;

        let glob_matcher = include_pattern.and_then(|p| glob::Pattern::new(p).ok());

        let mut references: Vec<Value> = Vec::new();
        let mut files_scanned = 0u64;
        let max_references = 500u64;

        collect_reference_matches(
            &root,
            &root,
            &word_re,
            &glob_matcher,
            &mut references,
            &mut files_scanned,
            max_references,
        )?;

        let truncated = references.len() as u64 >= max_references;

        debug!(
            symbol = %symbol,
            count = references.len(),
            files = files_scanned,
            truncated = truncated,
            "find_references: search complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "symbol": symbol,
                "references": references,
                "files_scanned": files_scanned,
                "total": references.len(),
                "truncated": truncated,
            })),
            error: None,
            verification: Some("reference_search_completed".to_string()),
            audit_log: Some(format!(
                "find_references '{}': {} reference(s) in {} files",
                symbol,
                references.len(),
                files_scanned
            )),
            pua_report: Some(tool_execution_report(
                "find_references",
                Some("reference_search_completed"),
            )),
        })
    }
}

// ── ApplyCodeActionTool ────────────────────────────────────────────

/// Apply common code actions (add import, fix lint warnings, etc.).
///
/// This tool does not require a running LSP server. It handles a curated
/// set of frequently needed code transformations:
/// - `add_import`: insert a `use` statement (Rust) or `import` (Python/JS/TS)
/// - `fix_lint`: suppress a lint warning with an allow attribute
/// - `auto_fix_diagnostic`: run `cargo clippy --fix` (Rust only)
///
/// When `lsp_address` is provided, delegates to the LSP server's
/// `textDocument/codeAction` for language-server-driven code actions.
pub struct ApplyCodeActionTool;

impl Tool for ApplyCodeActionTool {
    fn name(&self) -> &'static str {
        "apply_code_action"
    }

    fn description(&self) -> &str {
        "Apply code actions (add_import, fix_lint, auto_fix_diagnostic) at a location"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path to apply the action at"},
                "action": {
                    "type": "string",
                    "enum": ["add_import", "fix_lint", "auto_fix_diagnostic"],
                    "description": "Type of code action to apply"
                },
                "detail": {"type": "string", "description": "Action-specific detail (e.g. 'HashMap' for add_import, or the lint rule name)"},
                "line": {"type": "integer", "description": "Line number for the action (1-based, default: 1)"},
                "lsp_address": {"type": "string", "description": "Optional LSP TCP address (e.g. '127.0.0.1:9258'). Delegates to the LSP server's textDocument/codeAction."},
                "column": {"type": "integer", "description": "Column number for the action (1-based, used with lsp_address, default: 1)"}
            },
            "required": ["path", "action"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("apply_code_action requires arguments.path"))?;
        let action = input.payload["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("apply_code_action requires arguments.action"))?;
        let detail = input.payload["detail"].as_str().unwrap_or_default();
        let line = input
            .payload
            .get("line")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as u32;

        let file_path = sanitize_path(input, path)?;

        // ── LSP mode ─────────────────────────────────────────────────
        if let Some(lsp_address) = input.payload.get("lsp_address").and_then(|v| v.as_str()) {
            let column = input
                .payload
                .get("column")
                .and_then(|v| v.as_i64())
                .unwrap_or(1) as u32;
            let fp_str = file_path.to_string_lossy().to_string();
            let result = run_lsp_code_action(lsp_address, &fp_str, action, detail, line, column)?;
            let applied = result
                .get("applied")
                .and_then(|a| a.as_bool())
                .unwrap_or(false);

            return Ok(ToolOutput {
                success: applied,
                result: Some(json!({
                    "action": action,
                    "detail": detail,
                    "path": file_path.to_string_lossy(),
                    "mode": "lsp",
                    "lsp_result": result,
                })),
                error: if applied {
                    None
                } else {
                    Some("LSP code action was not applied".to_string())
                },
                verification: Some(
                    if applied {
                        "code_action_applied"
                    } else {
                        "code_action_failed"
                    }
                    .to_string(),
                ),
                audit_log: Some(format!(
                    "apply_code_action (LSP) '{}' '{}' at {}:{}",
                    action,
                    detail,
                    file_path.display(),
                    line
                )),
                pua_report: Some(tool_execution_report(
                    "apply_code_action",
                    Some(if applied {
                        "code_action_applied"
                    } else {
                        "code_action_failed"
                    }),
                )),
            });
        }

        // ── Regex mode (fallback) ────────────────────────────────────
        let line_usize = line as usize;

        match action {
            "add_import" => {
                let content = String::from_utf8_lossy(
                    &crate::orchestration::tool::exec_common::read_file_capped(
                        &file_path,
                        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
                    )?,
                )
                .into_owned();
                let new_content = add_import_to_file(&content, detail, &file_path)?;
                fs::write(&file_path, &new_content)
                    .context("failed to write file after import addition")?;
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "action": "add_import",
                        "detail": detail,
                        "path": file_path.to_string_lossy(),
                        "message": format!("Added import '{}'", detail),
                    })),
                    error: None,
                    verification: Some("code_action_applied".to_string()),
                    audit_log: Some(format!(
                        "apply_code_action add_import '{}' in {}",
                        detail,
                        file_path.display()
                    )),
                    pua_report: Some(tool_execution_report(
                        "apply_code_action",
                        Some("code_action_applied"),
                    )),
                })
            }
            "fix_lint" => {
                if detail.is_empty() {
                    return Err(anyhow::anyhow!(
                        "fix_lint requires a non-empty 'detail' (lint name, e.g. 'dead_code')"
                    ));
                }
                let content = String::from_utf8_lossy(
                    &crate::orchestration::tool::exec_common::read_file_capped(
                        &file_path,
                        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
                    )?,
                )
                .into_owned();
                let new_content = add_lint_allow(&content, detail, line_usize, &file_path)?;
                fs::write(&file_path, &new_content)
                    .context("failed to write file after lint fix")?;
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "action": "fix_lint",
                        "detail": detail,
                        "line": line_usize,
                        "path": file_path.to_string_lossy(),
                        "message": format!("Added #[allow({})] at line {}", detail, line_usize),
                    })),
                    error: None,
                    verification: Some("code_action_applied".to_string()),
                    audit_log: Some(format!(
                        "apply_code_action fix_lint '{}' at {}:{}",
                        detail,
                        file_path.display(),
                        line_usize
                    )),
                    pua_report: Some(tool_execution_report(
                        "apply_code_action",
                        Some("code_action_applied"),
                    )),
                })
            }
            "auto_fix_diagnostic" => {
                if file_path.extension().map(|e| e != "rs").unwrap_or(true) {
                    return Err(anyhow::anyhow!(
                        "auto_fix_diagnostic is only supported for Rust (.rs) files"
                    ));
                }
                let dir = file_path.parent().unwrap_or_else(|| Path::new("."));
                let output = std::process::Command::new("cargo")
                    .args(["clippy", "--fix", "--allow-dirty", "--allow-staged"])
                    .current_dir(dir)
                    .output()
                    .context("failed to run cargo clippy --fix")?;
                Ok(ToolOutput {
                    success: output.status.success(),
                    result: Some(json!({
                        "action": "auto_fix_diagnostic",
                        "path": file_path.to_string_lossy(),
                        "exit_code": output.status.code(),
                        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                    })),
                    error: None,
                    verification: Some("code_action_applied".to_string()),
                    audit_log: Some("auto_fix_diagnostic: cargo clippy --fix executed".to_string()),
                    pua_report: Some(tool_execution_report(
                        "apply_code_action",
                        Some("code_action_applied"),
                    )),
                })
            }
            _ => Err(anyhow::anyhow!(
                "Unknown action '{}'. Supported actions: add_import, fix_lint, auto_fix_diagnostic",
                action
            )),
        }
    }
}

// ── Helper functions ───────────────────────────────────────────────

/// Build definition regex patterns for the given symbol, optionally scoped
/// by language.
fn build_definition_patterns(symbol: &str, language: &str) -> Vec<String> {
    // Universal patterns across languages
    let mut patterns = vec![
        format!(r#"(?m)^\s*(pub\s+)?fn\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?struct\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?enum\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?trait\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?(unsafe\s+)?trait\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?type\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?(const|static)\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?macro_rules!\s*{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?mod\s+{}\b"#, symbol),
    ];

    if language == "auto" || language == "rust" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*impl\s+.*{}\b"#, symbol),
            format!(r#"(?m)^\s*impl\s+.*{}\s*<"#, symbol),
            format!(r#"(?m)^\s*impl\s+.*{}\s+for"#, symbol),
        ]);
    }

    if language == "auto" || language == "python" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*(async\s+)?def\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*class\s+{}\b"#, symbol),
        ]);
    }

    if language == "auto" || language == "typescript" || language == "javascript" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*(export\s+)?(async\s+)?function\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?class\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?(const|let|var)\s+{}\s*[:=]"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?interface\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?type\s+{}\b"#, symbol),
        ]);
    }

    if language == "auto" || language == "go" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*func\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*type\s+{}\s"#, symbol),
            format!(r#"(?m)^\s*type\s+{}\s+struct"#, symbol),
            format!(r#"(?m)^\s*type\s+{}\s+interface"#, symbol),
            format!(r#"(?m)^\s*func\s+\(.*\)\s+{}\b"#, symbol),
        ]);
    }

    if language == "auto" || language == "java" {
        patterns.extend_from_slice(&[
            format!(
                r#"(?m)^\s*(public|private|protected)\s+.*\s+{}\s*\("#,
                symbol
            ),
            format!(
                r#"(?m)^\s*(public|private|protected)?\s*class\s+{}\b"#,
                symbol
            ),
            format!(
                r#"(?m)^\s*(public|private|protected)?\s*interface\s+{}\b"#,
                symbol
            ),
            format!(
                r#"(?m)^\s*(public|private|protected)?\s*enum\s+{}\b"#,
                symbol
            ),
        ]);
    }

    patterns
}

/// Walk directories and collect lines matching definition patterns.
fn collect_definition_matches(
    root: &Path,
    current: &Path,
    regex: &Regex,
    results: &mut Vec<Value>,
    max_results: usize,
) -> Result<()> {
    if results.len() >= max_results {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name == ".git"
                || dir_name == "target"
                || dir_name == "node_modules"
                || dir_name == "__pycache__"
                || dir_name == ".venv"
            {
                continue;
            }
            collect_definition_matches(root, &path, regex, results, max_results)?;
            continue;
        }

        // Only scan known source extensions
        let is_source = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e,
                    "rs" | "py"
                        | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "go"
                        | "java"
                        | "rb"
                        | "c"
                        | "cpp"
                        | "h"
                        | "hpp"
                        | "cs"
                        | "swift"
                        | "kt"
                        | "scala"
                        | "php"
                )
            })
            .unwrap_or(false);

        if !is_source {
            continue;
        }

        if let Ok(content) = crate::orchestration::tool::exec_common::read_text_capped(
            &path,
            crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
        ) {
            for (line_num, line) in content.lines().enumerate() {
                if results.len() >= max_results {
                    break;
                }
                if regex.is_match(line) {
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    results.push(json!({
                        "file": relative.to_string_lossy(),
                        "line": line_num + 1,
                        "content": line.trim_end(),
                    }));
                }
            }
        }
    }
    Ok(())
}

/// Walk directories and collect all references (occurrences) of a symbol,
/// excluding lines that look like definitions.
fn collect_reference_matches(
    root: &Path,
    current: &Path,
    regex: &Regex,
    glob_matcher: &Option<glob::Pattern>,
    results: &mut Vec<Value>,
    files_scanned: &mut u64,
    max_results: u64,
) -> Result<()> {
    if results.len() as u64 >= max_results {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name == ".git"
                || dir_name == "target"
                || dir_name == "node_modules"
                || dir_name == "__pycache__"
                || dir_name == ".venv"
            {
                continue;
            }
            collect_reference_matches(
                root,
                &path,
                regex,
                glob_matcher,
                results,
                files_scanned,
                max_results,
            )?;
            continue;
        }

        // Only scan known source extensions
        let is_source = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e,
                    "rs" | "py"
                        | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "go"
                        | "java"
                        | "rb"
                        | "c"
                        | "cpp"
                        | "h"
                        | "hpp"
                        | "cs"
                        | "swift"
                        | "kt"
                        | "scala"
                        | "php"
                        | "toml"
                        | "json"
                        | "yaml"
                        | "yml"
                        | "md"
                )
            })
            .unwrap_or(false);

        if !is_source {
            continue;
        }

        // Apply glob filter if provided
        if let Some(ref matcher) = glob_matcher {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if !matcher.matches_path(relative) {
                continue;
            }
        }

        *files_scanned += 1;

        if let Ok(content) = crate::orchestration::tool::exec_common::read_text_capped(
            &path,
            crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
        ) {
            for (line_num, line) in content.lines().enumerate() {
                if results.len() as u64 >= max_results {
                    break;
                }
                if regex.is_match(line) {
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    results.push(json!({
                        "file": relative.to_string_lossy(),
                        "line": line_num + 1,
                        "content": line.trim_end(),
                    }));
                }
            }
        }
    }
    Ok(())
}

/// Add an import statement to a source file. Handles Rust (`use`), Python/JS/TS (`import`).
fn add_import_to_file(content: &str, import_path: &str, file_path: &Path) -> Result<String> {
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "rs" {
        // If the import already exists, no-op
        if content.contains(&format!("use {}", import_path))
            || content.contains(&format!("use {};", import_path))
        {
            return Ok(content.to_string());
        }

        let import_line = format!("use {};", import_path);

        // Find the best insertion point: after existing use statements, or at top of file
        let lines: Vec<&str> = content.lines().collect();
        let mut last_use_index = None;
        let mut has_shebang_or_attr = false;

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("#!") || trimmed.starts_with("//") || trimmed.starts_with("/*") {
                has_shebang_or_attr = true;
                continue;
            }
            if trimmed.starts_with("use ") && trimmed.ends_with(';') {
                last_use_index = Some(i);
                has_shebang_or_attr = true;
            }
            if !trimmed.is_empty() && !trimmed.starts_with("use ") {
                // We've passed the use section
                break;
            }
        }

        let insert_at = if let Some(idx) = last_use_index {
            // After the last existing use statement
            if idx + 1 < lines.len() && lines[idx + 1].trim().is_empty() {
                idx + 2 // skip the existing blank line after uses
            } else {
                idx + 1
            }
        } else if has_shebang_or_attr {
            // After any shebang/attribute lines, find first blank line or start
            let mut pos = 0;
            for (i, line) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    pos = i;
                    break;
                }
                pos = i + 1;
            }
            pos
        } else {
            0
        };

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i == insert_at {
                result.push_str(&import_line);
                result.push('\n');
            }
            result.push_str(line);
            result.push('\n');
        }
        // If insert_at is past the last line, append
        if insert_at >= lines.len() {
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&import_line);
            result.push('\n');
        }

        Ok(result)
    } else if ext == "py" {
        let import_stmt = format!("import {}", import_path);
        if content.contains(&import_stmt) {
            return Ok(content.to_string());
        }
        // Insert after existing imports or at top
        let mut result = String::new();
        let mut inserted = false;
        for line in content.lines() {
            if !inserted
                && !line.trim().starts_with("import ")
                && !line.trim().starts_with("from ")
                && !line.trim().starts_with('#')
                && !line.trim().is_empty()
                && !line.trim().starts_with("\"\"\"")
                && !line.trim().starts_with("'''")
            {
                result.push_str(&import_stmt);
                result.push('\n');
                result.push('\n');
                inserted = true;
            }
            result.push_str(line);
            result.push('\n');
        }
        if !inserted {
            result.push_str(&import_stmt);
            result.push('\n');
        }
        Ok(result)
    } else {
        // JS/TS: import { ... } from '...'
        let import_stmt = format!("import '{}';", import_path);
        if content.contains(&import_stmt) {
            return Ok(content.to_string());
        }
        let mut result = String::new();
        let mut inserted = false;
        for line in content.lines() {
            if !inserted
                && !line.trim().starts_with("import ")
                && !line.trim().starts_with("//")
                && !line.trim().starts_with("/*")
                && !line.trim().is_empty()
            {
                result.push_str(&import_stmt);
                result.push('\n');
                inserted = true;
            }
            result.push_str(line);
            result.push('\n');
        }
        if !inserted {
            result.push_str(&import_stmt);
            result.push('\n');
        }
        Ok(result)
    }
}

/// Add an `#[allow(lint_name)]` attribute before a specific line in a Rust file.
fn add_lint_allow(
    content: &str,
    lint_name: &str,
    line: usize,
    _file_path: &Path,
) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    if line == 0 || line > lines.len() {
        return Err(anyhow::anyhow!(
            "Line {} is out of range (file has {} lines)",
            line,
            lines.len()
        ));
    }

    let insert_idx = line - 1; // 0-based
    let allow_attr = format!("#[allow({})]", lint_name);

    // Don't add if there's already an allow for this lint on the preceding line
    if insert_idx > 0 {
        let prev = lines[insert_idx - 1].trim();
        if prev.starts_with("#[allow(") && prev.contains(lint_name) {
            return Ok(content.to_string());
        }
        if prev == allow_attr {
            return Ok(content.to_string());
        }
    }

    let mut result = String::new();
    for (i, line_text) in lines.iter().enumerate() {
        if i == insert_idx {
            result.push_str(&allow_attr);
            result.push('\n');
        }
        result.push_str(line_text);
        result.push('\n');
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-1".to_string(),
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
    fn go_to_definition_finds_fn_in_rust_file() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("example.rs");
        std::fs::write(
            &file,
            "fn hello() {}\nfn world() {}\nfn greet(name: &str) {}\n",
        )
        .unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "hello",
                "directory": tmp.path().to_string_lossy(),
                "language": "rust",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output = GoToDefinitionTool
            .run(&input)
            .expect("go_to_definition should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let defs = result["definitions"].as_array().unwrap();
        assert!(!defs.is_empty());
        let def = &defs[0];
        assert!(def["file"].as_str().unwrap().contains("example.rs"));
        assert_eq!(def["line"].as_i64().unwrap(), 1);
    }

    #[test]
    fn go_to_definition_finds_struct() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("example.rs");
        std::fs::write(&file, "pub struct Config {}\nfn main() {}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "Config",
                "directory": tmp.path().to_string_lossy(),
                "language": "rust",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output = GoToDefinitionTool
            .run(&input)
            .expect("go_to_definition should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let defs = result["definitions"].as_array().unwrap();
        assert!(!defs.is_empty());
    }

    #[test]
    fn go_to_definition_requires_symbol() {
        let input = tool_input(serde_json::json!({
            "directory": ".",
        }));
        let result = GoToDefinitionTool.run(&input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires arguments.symbol"));
    }

    #[test]
    fn go_to_definition_returns_empty_for_nonexistent_symbol() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "NonExistentSymbolXYZ",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };
        let output = GoToDefinitionTool
            .run(&input)
            .expect("go_to_definition should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["found"].as_bool(), Some(false));
    }

    #[test]
    fn find_references_finds_usages() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("example.rs");
        std::fs::write(&file, "fn helper() {}\nfn main() { helper(); helper(); }\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "helper",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output = FindReferencesTool
            .run(&input)
            .expect("find_references should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let refs = result["references"].as_array().unwrap();
        assert!(!refs.is_empty());
        assert!(result["files_scanned"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn find_references_requires_symbol() {
        let input = tool_input(serde_json::json!({
            "directory": ".",
        }));
        let result = FindReferencesTool.run(&input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires arguments.symbol"));
    }

    #[test]
    fn apply_code_action_add_import_rust() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": file.to_string_lossy(),
                "action": "add_import",
                "detail": "std::collections::HashMap",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output = ApplyCodeActionTool
            .run(&input)
            .expect("apply_code_action should succeed");
        assert!(output.success);
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("use std::collections::HashMap;"));
    }

    #[test]
    fn apply_code_action_add_import_idempotent() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("main.rs");
        std::fs::write(&file, "use std::collections::HashMap;\nfn main() {}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": file.to_string_lossy(),
                "action": "add_import",
                "detail": "std::collections::HashMap",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output = ApplyCodeActionTool
            .run(&input)
            .expect("apply_code_action should succeed");
        assert!(output.success);
        let content = std::fs::read_to_string(&file).unwrap();
        // Should still have exactly one occurrence
        assert_eq!(content.matches("use std::collections::HashMap;").count(), 1);
    }

    #[test]
    fn apply_code_action_fix_lint() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("main.rs");
        std::fs::write(&file, "fn main() {\n    let x = 1;\n}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": file.to_string_lossy(),
                "action": "fix_lint",
                "detail": "unused_variables",
                "line": 2,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output = ApplyCodeActionTool
            .run(&input)
            .expect("apply_code_action should succeed");
        assert!(output.success);
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("#[allow(unused_variables)]"));
    }

    #[test]
    fn apply_code_action_unknown_action() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let input = tool_input(serde_json::json!({
            "path": file.to_string_lossy(),
            "action": "unknown_action",
        }));
        let result = ApplyCodeActionTool.run(&input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown action"));
    }

    #[test]
    fn apply_code_action_requires_action() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let file = tmp.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let input = tool_input(serde_json::json!({
            "path": file.to_string_lossy(),
        }));
        let result = ApplyCodeActionTool.run(&input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires arguments.action"));
    }

    #[test]
    fn add_import_to_file_rust_adds_use() {
        let content = "fn main() {}";
        let result = add_import_to_file(content, "std::collections::HashMap", Path::new("x.rs"))
            .expect("add_import should work");
        assert!(result.contains("use std::collections::HashMap;"));
    }

    #[test]
    fn add_import_to_file_rust_after_existing_uses() {
        let content = "use std::fmt;\nfn main() {}";
        let result = add_import_to_file(content, "std::collections::HashMap", Path::new("x.rs"))
            .expect("add_import should work");
        assert!(result.contains("use std::fmt;\nuse std::collections::HashMap;"));
    }

    #[test]
    fn add_import_to_file_python() {
        let content = "def main(): pass";
        let result =
            add_import_to_file(content, "os", Path::new("x.py")).expect("add_import should work");
        assert!(result.contains("import os"));
    }

    #[test]
    fn add_lint_allow_adds_attribute() {
        let content = "fn main() {\n    let x = 1;\n}\n";
        let result = add_lint_allow(content, "unused_variables", 2, Path::new("x.rs")).unwrap();
        assert!(result.contains("#[allow(unused_variables)]"));
        assert!(result.contains("let x = 1;"));
    }

    #[test]
    fn add_lint_allow_already_exists() {
        let content = "#[allow(unused_variables)]\nfn main() {\n    let x = 1;\n}\n";
        let result = add_lint_allow(content, "unused_variables", 2, Path::new("x.rs")).unwrap();
        // Should not duplicate
        assert_eq!(result.matches("#[allow(unused_variables)]").count(), 1);
    }

    #[test]
    fn add_lint_allow_out_of_range() {
        let content = "fn main() {}";
        let result = add_lint_allow(content, "dead_code", 99, Path::new("x.rs"));
        assert!(result.is_err());
    }

    #[test]
    fn build_definition_patterns_includes_rust_keywords() {
        let patterns = build_definition_patterns("foo", "rust");
        let has_fn = patterns.iter().any(|p| p.contains("fn"));
        let has_struct = patterns.iter().any(|p| p.contains("struct"));
        assert!(has_fn);
        assert!(has_struct);
    }

    #[test]
    fn build_definition_patterns_includes_python() {
        let patterns = build_definition_patterns("bar", "python");
        let has_def = patterns.iter().any(|p| p.contains("def"));
        let has_class = patterns.iter().any(|p| p.contains("class"));
        assert!(has_def);
        assert!(has_class);
    }

    #[test]
    fn build_definition_patterns_includes_javascript() {
        let patterns = build_definition_patterns("baz", "typescript");
        let has_fn = patterns.iter().any(|p| p.contains("function"));
        let has_class = patterns.iter().any(|p| p.contains("class"));
        assert!(has_fn);
        assert!(has_class);
    }
}
