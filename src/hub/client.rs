//! Runtime Hub client — connects to a local Hub via JSON-RPC over HTTP.
//!
//! Discovery flow:
//!   1. Check $GO_ON_HUB_URL env var for direct address.
//!   2. Read discovery file from default path.
//!   3. Verify the hub process is alive.
//!   4. Perform JSON-RPC handshake.
//!   5. Return a connected client handle.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

use super::discovery::HubDiscovery;

/// A connected Hub client.
pub struct HubClient {
    /// Hub endpoint URL.
    endpoint: String,
    /// Hub ID from handshake.
    pub hub_id: String,
    /// HTTP client (reused for keep-alive).
    client: reqwest::Client,
}

impl HubClient {
    /// Discover and connect to a running Hub.
    ///
    /// Returns `None` if no Hub is available (no discovery file, stale hub, etc.).
    pub async fn discover() -> Result<Option<Self>> {
        // 1. Check env var for direct URL.
        if let Ok(url) = std::env::var("GO_ON_HUB_URL") {
            if !url.is_empty() {
                return Ok(Some(Self::connect_direct(&url).await?));
            }
        }

        // 2. Read discovery file.
        let path = HubDiscovery::default_path();
        if !path.exists() {
            return Ok(None);
        }
        let discovery = HubDiscovery::read(&path)?;

        // 3. Verify process alive.
        if !discovery.is_alive() {
            // Stale discovery file — clean it up.
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }

        // 4. Connect.
        Ok(Some(Self::connect_direct(&discovery.endpoint).await?))
    }

    /// Connect to a Hub at the given endpoint URL.
    /// Used both in tests and when GO_ON_HUB_URL env var is set.
    pub async fn connect_direct(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;

        // Perform handshake.
        let handshake = Self::json_rpc_call(
            &client,
            endpoint,
            "hub.handshake",
            json!({"nonce": hex::encode(rand::random::<[u8; 16]>())}),
        )
        .await?;

        let hub_id = handshake
            .get("hub_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(Self {
            endpoint: endpoint.to_string(),
            hub_id,
            client,
        })
    }

    /// Get Hub status.
    pub async fn status(&self) -> Result<Value> {
        Self::json_rpc_call(&self.client, &self.endpoint, "hub.status", json!({})).await
    }

    /// Store a value in the Hub's vault.
    pub async fn store(&self, key: &str, value: Value) -> Result<Value> {
        Self::json_rpc_call(
            &self.client,
            &self.endpoint,
            "hub.store",
            json!({"key": key, "value": value}),
        )
        .await
    }

    /// Retrieve a value from the Hub's vault.
    pub async fn retrieve(&self, key: &str) -> Result<Value> {
        Self::json_rpc_call(
            &self.client,
            &self.endpoint,
            "hub.retrieve",
            json!({"key": key}),
        )
        .await
    }

    /// List all keys in the Hub's vault.
    pub async fn list_keys(&self) -> Result<Value> {
        Self::json_rpc_call(&self.client, &self.endpoint, "hub.list", json!({})).await
    }

    /// Low-level JSON-RPC call.
    async fn json_rpc_call(
        client: &reqwest::Client,
        endpoint: &str,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1,
        });

        let resp = client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("hub RPC {} failed", method))?;

        let result: Value = resp.json().await?;

        // Check for JSON-RPC error.
        if let Some(error) = result.get("error") {
            anyhow::bail!("hub RPC {} error: {:?}", method, error);
        }

        Ok(result.get("result").cloned().unwrap_or_default())
    }
}
