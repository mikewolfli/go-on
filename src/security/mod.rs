//! Security Module (GAP-B52)
//!
//! Provides request signing, mTLS configuration, prompt injection detection,
//! secret rotation, audit integrity with hash chains, and content safety
//! checking for the go-on runtime.

use std::sync::Arc;

pub mod audit_integrity;
pub mod content_safety;
pub mod mtls;
pub mod prompt_injection;
pub mod rate_limiter;
pub mod request_signing;
pub mod secret_rotation;
pub mod security_advisor;
pub mod vulnerability_scan;

// ---------------------------------------------------------------------------
// Wiring helpers for server startup (GAP-B52)
// ---------------------------------------------------------------------------

/// Start secret rotation if vault is configured.
/// Returns a `JoinHandle` if the rotation task was spawned, or `None`.
pub fn start_secret_rotation_if_configured(
    config: &crate::config::types::RuntimeConfig,
) -> Option<tokio::task::JoinHandle<()>> {
    if !config.governance_enabled {
        tracing::info!("Secret rotation: disabled (governance not enabled)");
        return None;
    }

    // Read vault configuration from environment variables.
    let vault_endpoint = std::env::var("VAULT_ADDR").ok()?;
    let vault_mount_path =
        std::env::var("VAULT_MOUNT_PATH").unwrap_or_else(|_| "secret".to_string());
    #[cfg(feature = "vault")]
    let vault_token = std::env::var("VAULT_TOKEN").ok()?;

    // Clone for logging after ownership moves into VaultRotator::new
    let endpoint_for_log = vault_endpoint.clone();
    let mount_for_log = vault_mount_path.clone();

    let rotator = match crate::security::secret_rotation::VaultRotator::new(
        vault_endpoint,
        #[cfg(feature = "vault")]
        vault_token,
        vault_mount_path,
    ) {
        Ok(r) => Arc::new(r),
        Err(e) => {
            tracing::warn!("Secret rotation: failed to create VaultRotator: {e}");
            return None;
        }
    };

    let policy = crate::security::secret_rotation::RotationPolicy {
        max_age_secs: 86400 * 30, // 30 days
        auto_rotate_on_access: true,
        retain_versions: 2,
        min_key_length: 32,
    };
    // Use underscore prefix to suppress unused warning — the SecretManager
    // is captured by the background spawn to keep it alive.
    let manager = Arc::new(crate::security::secret_rotation::SecretManager::new(
        policy, rotator,
    ));

    // Spawn a background rotation loop that keeps the SecretManager alive
    // and performs periodic rotation for all registered keys.
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400)); // 24h
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let rotated = manager.rotate_all_expired().await;
            if rotated > 0 {
                tracing::info!("Secret rotation: rotated {rotated} expired keys");
            } else {
                tracing::debug!("Secret rotation: no expired keys");
            }
        }
    });

    tracing::info!(
        "Secret rotation: VaultRotator started (endpoint: {}, mount: {})",
        endpoint_for_log,
        mount_for_log
    );
    Some(handle)
}

/// Monitor mTLS server certificate expiry and log warnings with days-remaining.
///
/// Reads `mtls_server_cert_path` from the runtime config, parses the
/// certificate using x509-parser, and checks its `not_after` time.
/// A background task re-checks daily. Returns `None` when mTLS is disabled.
pub fn wire_cert_monitor(
    config: &crate::config::types::RuntimeConfig,
) -> Option<tokio::task::JoinHandle<()>> {
    if !config.mtls_enabled {
        return None;
    }

    let cert_path = config.mtls_server_cert_path.clone();

    // Initial check at startup.
    check_cert_expiry(&cert_path);

    // Spawn background task that re-checks daily.
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            check_cert_expiry(&cert_path);
        }
    });

    tracing::info!(
        "Certificate monitor: watching {} for expiry (daily check)",
        config.mtls_server_cert_path
    );
    Some(handle)
}

/// Read a PEM certificate file, parse with x509-parser, and log expiry status.
fn check_cert_expiry(cert_path: &str) {
    let data = match std::fs::read(cert_path) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Cert monitor: cannot read {}: {}", cert_path, e);
            return;
        }
    };

    // Decode PEM to DER certificates.
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        match rustls_pemfile::certs(&mut data.as_slice()).collect() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "Cert monitor: failed to decode PEM from {}: {}",
                    cert_path,
                    e
                );
                return;
            }
        };

    if certs.is_empty() {
        tracing::warn!("Cert monitor: no certificates found in {}", cert_path);
        return;
    }

    match x509_parser::parse_x509_certificate(&certs[0]) {
        Ok((_, parsed)) => {
            let odt = parsed.validity().not_after.to_datetime();
            let not_after: std::time::SystemTime = odt.into();

            let now = std::time::SystemTime::now();
            if not_after <= now {
                let ago = now.duration_since(not_after).unwrap_or_default().as_secs() / 86400;
                tracing::error!(
                    "Certificate at {} has EXPIRED ({} day(s) ago)",
                    cert_path,
                    ago
                );
            } else {
                let remaining = not_after.duration_since(now).unwrap_or_default();
                let days = remaining.as_secs() / 86400;
                if days < 30 {
                    tracing::warn!(
                        "Certificate at {} expires in {} day(s) — renew soon",
                        cert_path,
                        days
                    );
                } else {
                    tracing::info!(
                        "Certificate at {} is valid — {} day(s) until expiry",
                        cert_path,
                        days
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                "Cert monitor: failed to parse certificate at {}: {}",
                cert_path,
                e
            );
        }
    }
}
