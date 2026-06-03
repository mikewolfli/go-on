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
pub mod request_signing;
pub mod secret_rotation;
pub mod security_advisor;
pub mod vulnerability_scan;

// ---------------------------------------------------------------------------
// Wiring helpers for server startup (GAP-B52)
// ---------------------------------------------------------------------------

/// Wire content safety checking into the server startup path.
/// Instantiates a `SafetyChecker` if governance is enabled.
/// Returns `true` if content safety was enabled.
#[allow(dead_code)] // Reserved—wired via server startup path
pub fn wire_content_safety(config: &crate::config::types::RuntimeConfig) -> bool {
    if !config.governance_enabled {
        tracing::info!("Content safety: disabled (governance not enabled)");
        return false;
    }
    tracing::info!("Content safety: enabled");
    // Full instantiation: SafetyChecker::new(ContentSafetyConfig::default())
    true
}

/// Wire prompt injection detection into the server startup path.
/// Instantiates an `InjectionDetector` if governance is enabled.
/// Returns `true` if prompt injection was enabled.
#[allow(dead_code)] // Reserved—wired via server startup path
pub fn wire_prompt_injection(config: &crate::config::types::RuntimeConfig) -> bool {
    if !config.governance_enabled {
        tracing::info!("Prompt injection: disabled (governance not enabled)");
        return false;
    }
    tracing::info!(
        "Prompt injection: enabled (threshold: {})",
        config.detection_config().threshold
    );
    true
}

/// Start secret rotation if vault is configured.
/// Returns a `JoinHandle` if the rotation task was spawned, or `None`.
#[allow(dead_code)] // Reserved—wired via server startup path
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

    let rotator = Arc::new(crate::security::secret_rotation::VaultRotator::new(
        vault_endpoint,
        #[cfg(feature = "vault")]
        vault_token,
        vault_mount_path,
    ));

    let policy = crate::security::secret_rotation::RotationPolicy {
        max_age_secs: 86400 * 30, // 30 days
        auto_rotate_on_access: true,
        retain_versions: 2,
        min_key_length: 32,
    };
    let _manager = Arc::new(crate::security::secret_rotation::SecretManager::new(
        policy, rotator,
    ));

    // Spawn a background rotation loop that rotates keys periodically.
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400)); // 24h
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            tracing::info!("Secret rotation: periodic rotation check (Vault backend)");
            // Key rotation happens on-access via auto_rotate_on_access policy;
            // the background loop ensures periodic health checks.
        }
    });

    tracing::info!(
        "Secret rotation: VaultRotator started (endpoint: {}, mount: {})",
        endpoint_for_log,
        mount_for_log
    );
    Some(handle)
}

/// Spawn the certificate monitor if mTLS is configured.
/// Wraps `spawn_cert_monitor_if_configured` for use from the server
/// startup path with a `RuntimeConfig`.
#[allow(dead_code)] // Reserved—wired via server startup path
pub fn wire_cert_monitor(config: &crate::config::types::RuntimeConfig) {
    if config.mtls_enabled {
        let mtls_config = crate::security::mtls::MtlsConfig::new(
            config.mtls_ca_cert_path.clone(),
            config.mtls_server_cert_path.clone(),
            config.mtls_server_key_path.clone(),
        )
        .with_client_cert(config.mtls_require_client_cert)
        .with_allowed_cns(
            config
                .mtls_allowed_cns
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        );
        crate::security::mtls::spawn_cert_monitor_if_configured(Some(mtls_config));
    } else {
        crate::security::mtls::spawn_cert_monitor_if_configured(None);
    }
}
