//! Security Module (GAP-B52)
//!
//! Provides request signing, mTLS configuration, prompt injection detection,
//! audit integrity with hash chains, and content safety checking for the
//! go-on runtime.

pub mod audit_integrity;
pub mod content_safety;
pub mod mtls;
pub mod prompt_injection;
pub mod request_signing;
pub mod security_advisor;
pub mod severity;
pub mod vulnerability_scan;

// ---------------------------------------------------------------------------
// Wiring helpers for server startup (GAP-B52)
// ---------------------------------------------------------------------------

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
            let cp = cert_path.clone();
            tokio::task::spawn_blocking(move || check_cert_expiry(&cp))
                .await
                .ok();
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
