//! Runtime Hub HTTP JSON-RPC server.
//!
//! Lightweight background daemon providing local RPC services.
//! Pattern matches Zed/VSCode: single binary, random loopback port,
//! discovery file, Bearer token auth.
//!
//! JSON-RPC 2.0 methods:
//! - hub.handshake  — echo hub identity (hub_id + nonce + version; no
//!   signature verification yet — see `HubDiscovery::public_key`)
//! - hub.status     — get runtime status
//! - hub.store      — persist a value
//! - hub.retrieve   — read a persisted value
//! - hub.list       — list persisted keys
//! - memory.ingest  — store shared memory entries synced from another node
//!   (used by the DistributedMemoryBus HTTP transport)
//!
//! # Wiring note
//! This module is live via the `go-on hub` CLI command (see `src/main/mod.rs`);
//! core server paths are not yet connected. It remains a design reserve for the
//! future multi-process architecture — see parent `hub/mod.rs` for the full
//! rationale.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use super::discovery::HubDiscovery;
use crate::mcp::error_codes::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR};

/// Hub JSON-RPC server.
pub struct HubServer {
    hub_id: String,
    api_token: String,
    bind_addr: String,
    discovery_path: PathBuf,
    vault: Arc<Mutex<HashMap<String, Value>>>,
    start_time: Instant,
    rpc_count: Arc<Mutex<u64>>,
}

impl HubServer {
    /// Create a new Hub server bound to `127.0.0.1:<port>` (0 = auto-assign).
    pub fn with_port(port: u16) -> Result<Self> {
        let mut seed = [0u8; 8];
        use rand::Rng;
        rand::rng().fill_bytes(&mut seed);
        let hub_id = format!("hub_{}", hex::encode(seed));
        let api_token = hex::encode(rand::random::<[u8; 32]>());

        Ok(Self {
            hub_id,
            api_token,
            bind_addr: if port == 0 {
                String::new()
            } else {
                format!("127.0.0.1:{}", port)
            },
            discovery_path: HubDiscovery::default_path(),
            vault: Arc::new(Mutex::new(HashMap::new())),
            start_time: Instant::now(),
            rpc_count: Arc::new(Mutex::new(0)),
        })
    }

    /// Start the Hub server. Returns the bound address.
    pub async fn start(&mut self) -> Result<String> {
        let bind = if self.bind_addr.is_empty() {
            "127.0.0.1:0".to_string()
        } else {
            self.bind_addr.clone()
        };
        let listener = TcpListener::bind(&bind).context("bind loopback port")?;
        let addr = listener.local_addr()?;
        self.bind_addr = format!("http://{}", addr);
        info!("Hub {} starting on {}", self.hub_id, self.bind_addr);

        // Random opaque identity token. Not derived from a keypair and not
        // yet verified by clients: handshake is an identity echo until the
        // signed-handshake design (hub failover roadmap) is implemented.
        let discovery = HubDiscovery {
            hub_id: self.hub_id.clone(),
            transport: "loopback_http".to_string(),
            endpoint: self.bind_addr.clone(),
            public_key: hex::encode(rand::random::<[u8; 32]>()),
            pid: std::process::id(),
            created_at: iso_now(),
        };
        discovery.write(&self.discovery_path)?;
        info!("Hub discovery: {}", self.discovery_path.display());

        let listener = tokio::net::TcpListener::from_std(listener)?;

        // Extract owned copies for the background task.
        let vault = self.vault.clone();
        let api_token = self.api_token.clone();
        let hub_id = self.hub_id.clone();
        let rpc_count = self.rpc_count.clone();
        let start_time = self.start_time;
        let discovery_path = self.discovery_path.clone();

        info!("Hub binding: {}", self.bind_addr);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        let vault = vault.clone();
                        let api_token = api_token.clone();
                        let hub_id = hub_id.clone();
                        let rpc_count = rpc_count.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_rpc(
                                stream, peer, vault, api_token, hub_id, rpc_count, start_time,
                            )
                            .await
                            {
                                warn!("Hub RPC error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Hub accept: {}", e);
                        break;
                    }
                }
            }
            let _ = std::fs::remove_file(&discovery_path);
            info!("Hub stopped");
        });

        info!(
            "Hub ready: {} (token: {}...)",
            self.bind_addr,
            &self.api_token[..8]
        );
        Ok(self.bind_addr.clone())
    }
}

/// Handle a single JSON-RPC request over HTTP.
async fn handle_rpc(
    mut stream: TcpStream,
    _peer: std::net::SocketAddr,
    vault: Arc<Mutex<HashMap<String, Value>>>,
    api_token: String,
    hub_id: String,
    rpc_count: Arc<Mutex<u64>>,
    start_time: Instant,
) -> Result<()> {
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    if !request_line.trim().starts_with("POST") {
        return write_json(
            &mut stream,
            405,
            json_rpc_error(None, INVALID_REQUEST, "Only POST"),
        )
        .await;
    }

    // Parse headers.
    let mut content_length: usize = 0;
    let mut auth = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            content_length = line
                .split(':')
                .nth(1)
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
        }
        if lower.starts_with("authorization:") {
            auth = line.split(':').nth(1).unwrap_or("").trim().to_string();
        }
    }

    // Validate Bearer token.
    if !auth.contains(&format!("Bearer {}", &api_token)) {
        return write_json(&mut stream, 401, json!({"error":"unauthorized"})).await;
    }

    // Read body.
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).await?;
    }
    let body_str = String::from_utf8_lossy(&body);

    // Parse JSON-RPC request.
    let req: Value = match serde_json::from_str(&body_str) {
        Ok(v) => v,
        Err(_) => {
            return write_json(
                &mut stream,
                400,
                json_rpc_error(None, PARSE_ERROR, "Parse error"),
            )
            .await
        }
    };
    let req_id = req.get("id").cloned().unwrap_or(json!(null));
    let req_method = req
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let params = req.get("params").cloned().unwrap_or(json!({}));

    // Increment RPC counter.
    {
        let mut count = rpc_count.lock().await;
        *count += 1;
    }

    // Route to handler.
    let result = match req_method.as_str() {
        "hub.handshake" => {
            let nonce = params.get("nonce").and_then(|v| v.as_str()).unwrap_or("");
            json!({
                "hub_id": hub_id,
                "nonce": nonce,
                "transport": "loopback_http",
                "server_version": env!("CARGO_PKG_VERSION"),
            })
        }
        "hub.status" => {
            let count = *rpc_count.lock().await;
            let vlen = vault.lock().await.len();
            json!({
                "hub_id": hub_id,
                "running": true,
                "uptime_seconds": start_time.elapsed().as_secs(),
                "rpc_count": count,
                "vault_keys": vlen,
            })
        }
        "hub.store" => {
            let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let value = params.get("value");
            if key.is_empty() || value.is_none() {
                // Top-level JSON-RPC error object (previously the error was
                // wrongly nested inside `result` for this arm).
                let err = json_rpc_error(
                    Some(req_id.clone()),
                    INVALID_PARAMS,
                    "key and value required",
                );
                write_json(&mut stream, 200, err).await?;
                return Ok(());
            }
            let value = value.cloned().unwrap_or(json!(null));
            vault.lock().await.insert(key.to_string(), value);
            json!({"ok": true, "key": key})
        }
        "hub.retrieve" => {
            let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let v = vault.lock().await;
            if let Some(val) = v.get(key) {
                json!({"ok": true, "key": key, "value": val})
            } else {
                json!({"ok": false, "key": key, "error": "not_found"})
            }
        }
        "hub.list" => {
            let keys: Vec<String> = vault.lock().await.keys().cloned().collect();
            json!({"ok": true, "keys": keys})
        }
        "memory.ingest" => {
            // Receiving side of the DistributedMemoryBus HTTP transport: store
            // the synced entries under a per-source key so they are observable
            // via hub.list / hub.retrieve.
            //
            // NOTE: this arm compiles whenever `sub-bus-distributed-memory` is
            // enabled (the gate on `hub/mod.rs`). The ACP /rpc counterpart in
            // src/acp/impl/request.rs uses the same gate (`#[cfg(feature =
            // "sub-bus-distributed-memory")]`), so both sides are active under
            // simple-server / multi-users-server / full — aligned as of the
            // 2026-08-10 #2 consolidation.
            let entries = params.get("entries").cloned().unwrap_or(json!([]));
            let source = params
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("remote")
                .to_string();
            let count = entries.as_array().map(|a| a.len()).unwrap_or(0);
            let key = format!("memory.entries.{}", source);
            vault.lock().await.insert(key.clone(), entries);
            json!({"ok": true, "stored": count, "key": key})
        }
        _ => {
            // Unknown method → JSON-RPC error object in the top-level
            // `error` field. Both this branch and the `hub.store`
            // INVALID_PARAMS branch write the error at the top level
            // (previously the error objects were wrongly nested inside
            // `result`).
            let err = json_rpc_error(
                Some(req_id.clone()),
                METHOD_NOT_FOUND,
                format!("Method not found: {}", req_method),
            );
            write_json(&mut stream, 200, err).await?;
            return Ok(());
        }
    };

    let response = json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": result,
    });
    write_json(&mut stream, 200, response).await
}

/// Write an HTTP JSON response.
async fn write_json(stream: &mut TcpStream, status: u16, body: Value) -> Result<()> {
    let body_str = serde_json::to_string(&body)?;
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status, status_text, body_str.len(), body_str
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Build a JSON-RPC error response.
fn json_rpc_error(id: Option<Value>, code: i32, msg: impl ToString) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(json!(null)),
        "error": { "code": code, "message": msg.to_string() }
    })
}

/// Returns the current UTC time as an RFC 3339 / ISO-8601 string.
fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339()
}
