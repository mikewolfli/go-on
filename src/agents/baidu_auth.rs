//! Shared Baidu OAuth 2.0 token authentication
//!
//! Both the Wenxin and Qianfan variants of
//! [`BaiduErnieAgent`](super::ernie::BaiduErnieAgent) use the same Baidu
//! OAuth 2.0 `client_credentials` flow: an `api_key` + `secret_key` are
//! exchanged for a short-lived `access_token`. This module provides a single,
//! reusable client that fetches and caches that token, and automatically
//! refreshes it on expiry.

use anyhow::Result;
use serde::Deserialize;

use crate::agent::resolve_secret;
use crate::agents::agent::token_request_failed_msg;
use crate::agents::TokenCache;

/// The raw response from Baidu's OAuth token endpoint.
#[derive(Debug, Deserialize)]
pub struct BaiduTokenResponse {
    pub access_token: String,
    /// Time-to-live in seconds (typically 2592000 for 30 days).
    pub expires_in: Option<u64>,
}

/// A client that manages a cached Baidu access token obtained via the
/// `client_credentials` OAuth 2.0 grant.
///
/// The cache automatically refreshes the token when it is about to expire
/// (with a safety margin of up to 120 seconds).
pub struct BaiduAuthClient {
    api_key_env: String,
    secret_key_env: String,
    client: reqwest::Client,
    cache: TokenCache,
}

impl BaiduAuthClient {
    /// Create a new auth client.
    ///
    /// * `api_key_env`    – Name of the environment variable holding the API key
    ///   (used as `client_id`).
    /// * `secret_key_env` – Name of the environment variable holding the secret
    ///   (used as `client_secret`).
    /// * `client`         – A shared `reqwest::Client`.
    pub fn new(api_key_env: String, secret_key_env: String, client: reqwest::Client) -> Self {
        Self {
            api_key_env,
            secret_key_env,
            client,
            cache: TokenCache::new(),
        }
    }

    /// Return a valid access token, fetching (or re-fetching) one from Baidu's
    /// OAuth endpoint if the cached token is missing or expired.
    ///
    /// `service_name` is used in error messages and secret resolution contexts
    /// (e.g. `"wenxin"`, `"qianfan"`).
    pub async fn get_access_token(&self, service_name: &str) -> Result<String> {
        // Fast path: cached token still valid (120s safety margin).
        if let Some(token) = self.cache.fresh(120) {
            return Ok(token);
        }

        let api_key = resolve_secret(&self.api_key_env, &format!("{service_name}.api_key_env"))?;
        let secret_key = resolve_secret(
            &self.secret_key_env,
            &format!("{service_name}.secret_key_env"),
        )?;

        let mut url = reqwest::Url::parse("https://aip.baidubce.com/oauth/2.0/token")?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("grant_type", "client_credentials");
            pairs.append_pair("client_id", api_key.as_str());
            pairs.append_pair("client_secret", secret_key.as_str());
        }
        let response = self.client.get(url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                token_request_failed_msg(service_name, &status.to_string(), &body)
            );
        }

        let token_response: BaiduTokenResponse = response.json().await?;
        let ttl_seconds = token_response.expires_in.unwrap_or(1800);
        let safety_margin = ttl_seconds.min(120);
        let expires_at = unix_now_secs() + ttl_seconds - safety_margin;

        self.cache
            .store(token_response.access_token.clone(), expires_at);

        Ok(token_response.access_token)
    }
}

/// Current Unix time in seconds.
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
