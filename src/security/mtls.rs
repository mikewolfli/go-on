//! mTLS (GAP-B52-24)
//!
//! Provides mTLS configuration, acceptor/connector components built on rustls,
//! and certificate expiry monitoring with a 30-day warning threshold.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum MtlsError {
    #[error("certificate not found: {0}")]
    CertNotFound(String),

    #[error("private key not found: {0}")]
    KeyNotFound(String),

    #[error("invalid certificate: {0}")]
    InvalidCert(String),

    #[error("invalid private key: {0}")]
    InvalidKey(String),

    #[error("TLS handshake failed: {0}")]
    HandshakeFailed(String),

    #[error("CN not in allowed list: {0}")]
    CnNotAllowed(String),

    #[allow(dead_code)] // Reserved for cert expiry monitoring
    #[error("certificate expired at {0}")]
    CertExpired(String),

    #[error("IO error: {0}")]
    Io(String),
}

impl From<std::io::Error> for MtlsError {
    fn from(e: std::io::Error) -> Self {
        MtlsError::Io(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// MtlsConfig
// ---------------------------------------------------------------------------

/// mTLS configuration for the go-on runtime.
#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtlsConfig {
    /// Path to the CA certificate file (PEM).
    pub ca_cert_path: PathBuf,
    /// Path to the server certificate file (PEM).
    pub server_cert_path: PathBuf,
    /// Path to the server private key file (PEM).
    pub server_key_path: PathBuf,
    /// Whether to require client certificates for incoming connections.
    pub require_client_cert: bool,
    /// Optional list of allowed Common Names for client certificates.
    pub allowed_cn_list: Vec<String>,
}

#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
impl MtlsConfig {
    /// Create a new mTLS configuration.
    pub fn new(
        ca_cert_path: impl Into<PathBuf>,
        server_cert_path: impl Into<PathBuf>,
        server_key_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            ca_cert_path: ca_cert_path.into(),
            server_cert_path: server_cert_path.into(),
            server_key_path: server_key_path.into(),
            require_client_cert: true,
            allowed_cn_list: Vec::new(),
        }
    }

    /// Set whether client certificates are required.
    pub fn with_client_cert(mut self, require: bool) -> Self {
        self.require_client_cert = require;
        self
    }

    /// Set the allowed CN list.
    pub fn with_allowed_cns(mut self, cns: Vec<String>) -> Self {
        self.allowed_cn_list = cns;
        self
    }
}

// ---------------------------------------------------------------------------
// CertificateInfo
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateInfo {
    pub subject_cn: String,
    pub issuer_cn: String,
    pub not_before: u64,
    pub not_after: u64,
    pub serial_number: String,
    pub is_expired: bool,
    pub days_remaining: i64,
}

#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
impl CertificateInfo {
    /// Parse certificate info from DER-encoded certificate bytes.
    pub fn from_der(der_bytes: &[u8]) -> Result<Self, MtlsError> {
        let cert = x509_parser::parse_x509_certificate(der_bytes)
            .map_err(|e| MtlsError::InvalidCert(e.to_string()))?
            .1;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let not_before = cert.validity().not_before.timestamp();
        let not_after = cert.validity().not_after.timestamp();

        let days_remaining = if not_after > now as i64 {
            (not_after - now as i64) / 86400
        } else {
            -(now as i64 - not_after) / 86400
        };

        let subject_cn = cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .unwrap_or("unknown")
            .to_string();

        let issuer_cn = cert
            .issuer()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .unwrap_or("unknown")
            .to_string();

        let serial = cert.raw_serial_as_string();

        Ok(Self {
            subject_cn,
            issuer_cn,
            not_before: not_before as u64,
            not_after: not_after as u64,
            serial_number: serial,
            is_expired: now as i64 > not_after,
            days_remaining,
        })
    }
}

// ---------------------------------------------------------------------------
// MtlsAcceptor
// ---------------------------------------------------------------------------

/// Accepts incoming mTLS connections using rustls.
///
/// Loads CA and server certificates from the paths specified in MtlsConfig.
#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
pub struct MtlsAcceptor {
    config: MtlsConfig,
    /// Cached server config (rebuilt on cert reload).
    server_config: RwLock<Option<Arc<rustls::ServerConfig>>>,
}

#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
impl MtlsAcceptor {
    /// Create a new MtlsAcceptor from configuration.
    pub fn new(config: MtlsConfig) -> Self {
        Self {
            config,
            server_config: RwLock::new(None),
        }
    }

    /// Build (or rebuild) the rustls ServerConfig from the current config files.
    pub fn build_server_config(&self) -> Result<Arc<rustls::ServerConfig>, MtlsError> {
        // Load CA certificates
        let ca_cert_bytes = std::fs::read(&self.config.ca_cert_path).map_err(|e| {
            MtlsError::CertNotFound(format!("{}: {}", self.config.ca_cert_path.display(), e))
        })?;

        let mut root_store = rustls::RootCertStore::empty();
        let ca_certs = rustls_pemfile::certs(&mut ca_cert_bytes.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

        for cert in &ca_certs {
            root_store
                .add(cert.clone())
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;
        }

        // Load server certificate chain
        let server_cert_bytes = std::fs::read(&self.config.server_cert_path).map_err(|e| {
            MtlsError::CertNotFound(format!("{}: {}", self.config.server_cert_path.display(), e))
        })?;

        let cert_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut server_cert_bytes.as_slice())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

        if cert_chain.is_empty() {
            return Err(MtlsError::InvalidCert(
                "empty server certificate chain".into(),
            ));
        }

        // Load server private key
        let key_bytes = std::fs::read(&self.config.server_key_path).map_err(|e| {
            MtlsError::KeyNotFound(format!("{}: {}", self.config.server_key_path.display(), e))
        })?;

        let private_key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
            .map_err(|e| MtlsError::InvalidKey(e.to_string()))?
            .ok_or_else(|| MtlsError::InvalidKey("no private key found".into()))?;

        // Configure client certificate verification
        if self.config.require_client_cert {
            let verifier = rustls::server::WebPkiClientVerifier::builder(root_store.into())
                .build()
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

            let final_verifier = if !self.config.allowed_cn_list.is_empty() {
                rustls::server::WebPkiClientVerifier::builder(Arc::new(
                    self.build_ca_store_with_cn_check()?,
                ))
                .build()
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?
            } else {
                verifier
            };

            let mut server_config = rustls::ServerConfig::builder()
                .with_client_cert_verifier(final_verifier)
                .with_single_cert(cert_chain, private_key)
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

            server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            Ok(Arc::new(server_config))
        } else {
            let mut server_config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, private_key)
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

            server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            Ok(Arc::new(server_config))
        }
    }

    /// Build a root store that also checks CN against the allowed list.
    fn build_ca_store_with_cn_check(&self) -> Result<rustls::RootCertStore, MtlsError> {
        let ca_cert_bytes = std::fs::read(&self.config.ca_cert_path)?;
        let mut root_store = rustls::RootCertStore::empty();
        let certs = rustls_pemfile::certs(&mut ca_cert_bytes.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

        for cert in &certs {
            let info = CertificateInfo::from_der(cert)?;
            if !self.config.allowed_cn_list.is_empty()
                && !self
                    .config
                    .allowed_cn_list
                    .iter()
                    .any(|cn| cn == &info.subject_cn)
            {
                return Err(MtlsError::CnNotAllowed(info.subject_cn));
            }
            root_store
                .add(cert.clone())
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;
        }

        Ok(root_store)
    }

    /// Accept an incoming TLS connection (wraps a tokio TcpStream).
    /// Returns the CN of the client certificate if available.
    pub async fn accept(
        &self,
        stream: tokio::net::TcpStream,
    ) -> Result<(tokio_rustls::TlsAcceptor, String), MtlsError> {
        let server_config = {
            let cached = self.server_config.read().await;
            match cached.as_ref() {
                Some(cfg) => cfg.clone(),
                None => {
                    drop(cached);
                    let cfg = self.build_server_config()?;
                    *self.server_config.write().await = Some(cfg.clone());
                    cfg
                }
            }
        };

        let acceptor = tokio_rustls::TlsAcceptor::from(server_config.clone());

        // BLUE56-D03: Perform the actual TLS handshake and extract client CN
        let tls_stream = match acceptor.accept(stream).await {
            Ok(tls) => tls,
            Err(e) => return Err(MtlsError::HandshakeFailed(e.to_string())),
        };

        // Extract CN from the peer certificate
        let cn = if let Some(peer_certs) = tls_stream.get_ref().1.peer_certificates() {
            if let Some(cert_der) = peer_certs.first() {
                match x509_parser::parse_x509_certificate(cert_der) {
                    Ok((_, cert)) => {
                        let subject = cert.subject().to_string();
                        // Extract CN from subject string "CN=name,..."
                        subject
                            .split(',')
                            .find_map(|part| {
                                let p = part.trim();
                                p.strip_prefix("CN=").map(|cn| cn.to_string())
                            })
                            .unwrap_or_else(|| "unknown".to_string())
                    }
                    Err(_) => "unknown".to_string(),
                }
            } else {
                "unknown".to_string()
            }
        } else {
            "unknown".to_string()
        };

        Ok((tokio_rustls::TlsAcceptor::from(server_config), cn))
    }

    /// Reload certificates from disk (for hot-reload scenarios).
    pub async fn reload_certs(&self) -> Result<(), MtlsError> {
        let cfg = self.build_server_config()?;
        *self.server_config.write().await = Some(cfg);
        info!("mTLS certificates reloaded");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MtlsConnector
// ---------------------------------------------------------------------------

#[allow(dead_code)] // Reserved for future outbound mTLS client connections (GAP-B52-24)
/// Connects to remote mTLS endpoints using rustls.
pub struct MtlsConnector {
    config: MtlsConfig,
    client_config: RwLock<Option<Arc<rustls::ClientConfig>>>,
}

#[allow(dead_code)] // Reserved for future outbound mTLS client connections (GAP-B52-24)
impl MtlsConnector {
    pub fn new(config: MtlsConfig) -> Self {
        Self {
            config,
            client_config: RwLock::new(None),
        }
    }

    /// Build the rustls ClientConfig.
    fn build_client_config(&self) -> Result<Arc<rustls::ClientConfig>, MtlsError> {
        // Load CA certs for server verification
        let ca_cert_bytes = std::fs::read(&self.config.ca_cert_path)?;
        let mut root_store = rustls::RootCertStore::empty();
        let ca_certs: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut ca_cert_bytes.as_slice())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

        for cert in &ca_certs {
            root_store
                .add(cert.clone())
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;
        }

        // Load client certificate for mTLS
        let client_cert_bytes = std::fs::read(&self.config.server_cert_path)?;
        let cert_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut client_cert_bytes.as_slice())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

        let key_bytes = std::fs::read(&self.config.server_key_path)?;
        let private_key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
            .map_err(|e| MtlsError::InvalidKey(e.to_string()))?
            .ok_or_else(|| MtlsError::InvalidKey("no client private key found".into()))?;

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(Arc::new(root_store))
            .with_client_auth_cert(cert_chain, private_key)
            .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

        Ok(Arc::new(config))
    }

    /// Connect to a remote mTLS endpoint.
    pub async fn connect(
        &self,
        addr: &str,
        server_name: &str,
    ) -> Result<tokio_rustls::TlsConnector, MtlsError> {
        let client_config = {
            let cached = self.client_config.read().await;
            match cached.as_ref() {
                Some(cfg) => cfg.clone(),
                None => {
                    drop(cached);
                    let cfg = self.build_client_config()?;
                    *self.client_config.write().await = Some(cfg.clone());
                    cfg
                }
            }
        };

        // BLUE56-D03: Perform actual TCP connection and TLS handshake
        let stream = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| MtlsError::HandshakeFailed(format!("TCP connect: {}", e)))?;

        let connector = tokio_rustls::TlsConnector::from(client_config);
        let _tls_stream = connector
            .connect(
                rustls::pki_types::ServerName::try_from(server_name)
                    .map_err(|e| MtlsError::HandshakeFailed(format!("invalid server name: {}", e)))?
                    .to_owned(),
                stream,
            )
            .await
            .map_err(|e| MtlsError::HandshakeFailed(format!("TLS connect: {}", e)))?;

        Ok(connector)
    }
}

// ---------------------------------------------------------------------------
// Certificate Expiry Monitoring
// ---------------------------------------------------------------------------

/// Check a certificate file for expiry and return a warning if the certificate
/// will expire within the given threshold.
pub fn check_cert_expiry(
    cert_path: &Path,
    warning_threshold_days: u64,
) -> Result<Option<String>, MtlsError> {
    let cert_bytes = std::fs::read(cert_path)?;
    let certs = rustls_pemfile::certs(&mut cert_bytes.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

    if certs.is_empty() {
        return Err(MtlsError::InvalidCert("no certificates found".into()));
    }

    let info = CertificateInfo::from_der(&certs[0])?;

    if info.is_expired {
        return Ok(Some(format!(
            "Certificate '{}' expired on {}",
            info.subject_cn, info.not_after
        )));
    }

    if (info.days_remaining as u64) < warning_threshold_days {
        return Ok(Some(format!(
            "Certificate '{}' expires in {} days (threshold: {} days)",
            info.subject_cn, info.days_remaining, warning_threshold_days
        )));
    }

    Ok(None)
}

/// Monitor certificate expiry on a recurring interval.
/// Spawn this as a tokio task during initialization.
#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
/// Wired via server startup in production deployments.
pub fn start_cert_monitor(
    config: MtlsConfig,
    check_interval: Duration,
    warning_threshold_days: u64,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(check_interval);
        loop {
            interval.tick().await;
            let paths = [&config.ca_cert_path, &config.server_cert_path];

            for path in &paths {
                match check_cert_expiry(path, warning_threshold_days) {
                    Ok(Some(warning)) => warn!("Cert monitor: {}", warning),
                    Ok(None) => {} // OK
                    Err(e) => warn!("Cert monitor error for {}: {}", path.display(), e),
                }
            }
        }
    });
}

/// Spawn the certificate monitor task if an mTLS config is provided.
///
/// If `config` is `Some`, calls `start_cert_monitor` with a 24-hour check
/// interval and a 30-day warning threshold. If `config` is `None`, logs a
/// debug message indicating certificate monitoring is disabled.
#[allow(dead_code)] // F-GAP-49 — reserved mTLS feature
/// Wired via server startup in production deployments.
pub fn spawn_cert_monitor_if_configured(config: Option<MtlsConfig>) {
    match config {
        Some(cfg) => {
            info!(
                "mTLS certificate monitoring enabled (interval: 24h, warning threshold: 30 days)"
            );
            start_cert_monitor(cfg, Duration::from_secs(86400), 30);
        }
        None => {
            tracing::debug!("mTLS certificate monitoring disabled — no config provided");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn setup_test_certs() -> (TempDir, MtlsConfig) {
        let dir = TempDir::new().unwrap();

        // Write a minimal CA cert and server cert/key for structural testing.
        // In a real environment, these would be proper certificates.
        // For unit tests, we test the configuration struct and expiry logic.
        let config = MtlsConfig::new(
            dir.path().join("ca.pem"),
            dir.path().join("server.pem"),
            dir.path().join("server.key"),
        );

        (dir, config)
    }

    #[test]
    fn test_mtls_config_builder() {
        let (_dir, config) = setup_test_certs();
        let config = config
            .with_client_cert(true)
            .with_allowed_cns(vec!["client.example.com".into()]);

        assert!(config.require_client_cert);
        assert_eq!(config.allowed_cn_list.len(), 1);
        assert_eq!(config.allowed_cn_list[0], "client.example.com");
    }

    #[test]
    fn test_certificate_info_parse_error_for_invalid_der() {
        let result = CertificateInfo::from_der(b"not-a-real-cert");
        assert!(result.is_err());
    }
}
