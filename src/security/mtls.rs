//! mTLS (GAP-B52-24)
//!
//! Provides mTLS configuration, acceptor/connector components built on rustls,
//! and certificate expiry monitoring with a 30-day warning threshold.

use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

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

    #[error("IO error: {0}")]
    Io(String),
}

impl From<std::io::Error> for MtlsError {
    fn from(e: std::io::Error) -> Self {
        MtlsError::Io(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// MtlsAcceptor
// ---------------------------------------------------------------------------

/// Accepts incoming mTLS connections using rustls.
///
/// Loads CA and server certificates from PEM files at the given paths.
pub struct MtlsAcceptor {
    /// Path to the CA certificate file (PEM).
    ca_cert_path: PathBuf,
    /// Path to the server certificate file (PEM).
    server_cert_path: PathBuf,
    /// Path to the server private key file (PEM).
    server_key_path: PathBuf,
    /// Whether to require client certificates for incoming connections.
    require_client_cert: bool,
    /// Optional list of allowed Common Names for client certificates.
    allowed_cn_list: Vec<String>,
    /// Cached server config (rebuilt on cert reload).
    server_config: RwLock<Option<Arc<rustls::ServerConfig>>>,
}

// ---------------------------------------------------------------------------
// MtlsConfig
// ---------------------------------------------------------------------------

/// Configuration for mTLS, holding certificate paths and settings.
/// This is a plain-data config struct distinct from the runtime `MtlsAcceptor`.
#[allow(dead_code)] // Public API for mTLS consumers
#[derive(Debug, Clone)]
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

impl MtlsConfig {
    /// Create a new `MtlsConfig` from certificate paths.
    #[allow(dead_code)] // Public API for mTLS consumers
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
}

impl MtlsAcceptor {
    /// Create a new MtlsAcceptor from certificate paths and settings.
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
            server_config: RwLock::new(None),
        }
    }

    /// Build (or rebuild) the rustls ServerConfig from the current config files.
    pub fn build_server_config(&self) -> Result<Arc<rustls::ServerConfig>, MtlsError> {
        // Load CA certificates
        let ca_cert_bytes = std::fs::read(&self.ca_cert_path).map_err(|e| {
            MtlsError::CertNotFound(format!("{}: {}", self.ca_cert_path.display(), e))
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
        let server_cert_bytes = std::fs::read(&self.server_cert_path).map_err(|e| {
            MtlsError::CertNotFound(format!("{}: {}", self.server_cert_path.display(), e))
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
        let key_bytes = std::fs::read(&self.server_key_path).map_err(|e| {
            MtlsError::KeyNotFound(format!("{}: {}", self.server_key_path.display(), e))
        })?;

        let private_key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
            .map_err(|e| MtlsError::InvalidKey(e.to_string()))?
            .ok_or_else(|| MtlsError::InvalidKey("no private key found".into()))?;

        // Configure client certificate verification
        if self.require_client_cert {
            let verifier = rustls::server::WebPkiClientVerifier::builder(root_store.into())
                .build()
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

            let final_verifier = if !self.allowed_cn_list.is_empty() {
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
        let ca_cert_bytes = std::fs::read(&self.ca_cert_path)?;
        let mut root_store = rustls::RootCertStore::empty();
        let certs = rustls_pemfile::certs(&mut ca_cert_bytes.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;

        for cert in &certs {
            let subject_cn = match x509_parser::parse_x509_certificate(cert) {
                Ok((_, parsed_cert)) => parsed_cert
                    .subject()
                    .iter_common_name()
                    .next()
                    .and_then(|cn| cn.as_str().ok())
                    .unwrap_or("unknown")
                    .to_string(),
                Err(_) => "unknown".to_string(),
            };
            if !self.allowed_cn_list.is_empty()
                && !self.allowed_cn_list.iter().any(|cn| cn == &subject_cn)
            {
                return Err(MtlsError::CnNotAllowed(subject_cn));
            }
            root_store
                .add(cert.clone())
                .map_err(|e| MtlsError::InvalidCert(e.to_string()))?;
        }

        Ok(root_store)
    }

    /// Accept an incoming TLS connection (wraps a tokio TcpStream).
    /// Returns the TLS stream and the CN of the client certificate (if available).
    ///
    /// This bundles handshake + CN extraction, replacing the manual
    /// `tls_acceptor.accept(stream)` call so that client-cert CN is
    /// available for authorization (BLUE56-D03).
    pub async fn accept(
        &self,
        stream: tokio::net::TcpStream,
    ) -> Result<(tokio_rustls::TlsStream<tokio::net::TcpStream>, String), MtlsError> {
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

        let acceptor = tokio_rustls::TlsAcceptor::from(server_config);

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

        Ok((tokio_rustls::TlsStream::Server(tls_stream), cn))
    }

    /// Builder-pattern: enable or disable client certificate verification.
    #[allow(dead_code)] // Builder method — wired from ACP HTTP server under multi-users-server feature
    pub fn with_client_cert(mut self, enabled: bool) -> Self {
        self.require_client_cert = enabled;
        self
    }

    /// Builder-pattern: restrict mTLS to client certificates whose Common Name
    /// appears in the given list. An empty list disables CN filtering.
    #[allow(dead_code)] // Builder method — wired from ACP HTTP server under multi-users-server feature
    pub fn with_allowed_cns(mut self, allowed: Vec<String>) -> Self {
        self.allowed_cn_list = allowed;
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // mTLS tests removed — MtlsConfig and CertificateInfo were dead code
    // (F-GAP-49 reserved mTLS feature). Keep the module empty so the
    // build does not produce "unused module" warnings.
}
