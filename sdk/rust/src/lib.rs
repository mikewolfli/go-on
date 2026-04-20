use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct GoOnClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceStatusResponse {
    pub ok: bool,
    pub governance: Value,
}

impl GoOnClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    /// S15 SDK stub: fetch governance.status via JSON-RPC
    pub async fn governance_status(&self) -> Result<GovernanceStatusResponse, SdkError> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "governance.status",
            "params": {}
        });

        let resp: Value = self
            .http
            .post(format!("{}/v1/responses", self.base_url))
            .json(&payload)
            .send()
            .await?
            .json()
            .await?;

        Ok(GovernanceStatusResponse {
            ok: resp.get("result").and_then(|r| r.get("ok")).and_then(Value::as_bool).unwrap_or(false),
            governance: resp
                .get("result")
                .and_then(|r| r.get("governance"))
                .cloned()
                .unwrap_or(Value::Null),
        })
    }
}
