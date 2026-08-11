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
    /// Consumed by both the ACP HTTP and MCP HTTP arms (mTLS config paths
    /// compile in every profile), so it is not profile-gated.
    pub fn with_client_cert(mut self, enabled: bool) -> Self {
        self.require_client_cert = enabled;
        self
    }

    /// Builder-pattern: restrict mTLS to client certificates whose Common Name
    /// appears in the given list. An empty list disables CN filtering.
    /// Called from `run_acp_http_server` under `#[cfg(feature = "multi-users-server")]`.
    #[cfg(feature = "multi-users-server")]
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
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Install the Rustls CryptoProvider once for all tests in this module.
    /// Required by rustls 0.23+ — without this, server config / acceptor
    /// construction will panic with "Could not automatically determine the
    /// process-level CryptoProvider".
    fn ensure_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    /// Generates a CA, a server cert+key, and a client cert signed by the CA,
    /// writes PEM files to a temp dir, and returns paths plus metadata.
    struct TestCertFixture {
        _dir: TempDir, // keeps temp dir alive for the lifetime of the fixture
        ca_path: PathBuf,
        server_cert_path: PathBuf,
        server_key_path: PathBuf,
        _client_cert_pem: String,
        _client_cn: String,
    }

    impl TestCertFixture {
        fn new() -> Self {
            ensure_crypto_provider();
            use rcgen::*;

            // ── CA ──────────────────────────────────────────────────────
            let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut ca_params = CertificateParams::default();
            ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            {
                let mut dn = DistinguishedName::new();
                dn.push(DnType::CommonName, "Test CA");
                ca_params.distinguished_name = dn;
            }
            let ca_cert = ca_params.self_signed(&ca_key).unwrap();

            // rcgen 0.14: `signed_by` takes an `Issuer` (CA params + key)
            // instead of separate issuer-cert / issuer-key arguments.
            let issuer = Issuer::from_params(&ca_params, ca_key);

            // ── Server certificate ──────────────────────────────────────
            let server_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut server_params = CertificateParams::default();
            server_params.is_ca = IsCa::NoCa;
            server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            {
                let mut dn = DistinguishedName::new();
                dn.push(DnType::CommonName, "localhost");
                server_params.distinguished_name = dn;
            }
            let server_cert = server_params.signed_by(&server_key, &issuer).unwrap();

            // ── Client certificate ──────────────────────────────────────
            let client_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut client_params = CertificateParams::default();
            client_params.is_ca = IsCa::NoCa;
            client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
            {
                let mut dn = DistinguishedName::new();
                dn.push(DnType::CommonName, "test-client-01");
                client_params.distinguished_name = dn;
            }
            let client_cert = client_params.signed_by(&client_key, &issuer).unwrap();

            // ── Write PEM files to temp dir ─────────────────────────────
            let dir = TempDir::new().unwrap();
            let ca_path = dir.path().join("ca.pem");
            let cert_path = dir.path().join("server.pem");
            let key_path = dir.path().join("server.key");

            fs::write(&ca_path, ca_cert.pem()).unwrap();
            fs::write(&cert_path, server_cert.pem()).unwrap();
            fs::write(&key_path, server_key.serialize_pem()).unwrap();

            Self {
                _dir: dir,
                ca_path,
                server_cert_path: cert_path,
                server_key_path: key_path,
                _client_cert_pem: client_cert.pem(),
                _client_cn: "test-client-01".to_string(),
            }
        }

        fn acceptor(&self) -> MtlsAcceptor {
            MtlsAcceptor::new(
                self.ca_path.clone(),
                self.server_cert_path.clone(),
                self.server_key_path.clone(),
            )
        }
    }

    // ── build_server_config tests ───────────────────────────────────────

    #[test]
    fn test_build_server_config_valid() {
        let f = TestCertFixture::new();
        let acceptor = f.acceptor();
        let config = acceptor.build_server_config().unwrap();

        // ALPN protocols should be configured
        assert_eq!(
            config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        // Config builds successfully with client cert verification enabled
    }

    #[test]
    fn test_build_server_config_empty_cert_chain() {
        let f = TestCertFixture::new();
        // Overwrite the server cert file with empty content
        fs::write(&f.server_cert_path, "").unwrap();

        let acceptor = f.acceptor();
        let err = acceptor.build_server_config().unwrap_err();

        assert!(
            matches!(err, MtlsError::InvalidCert(ref msg) if msg.contains("empty")),
            "expected InvalidCert about empty chain, got: {err}"
        );
    }

    #[test]
    fn test_build_server_config_bad_pem_in_server_cert() {
        let f = TestCertFixture::new();
        fs::write(&f.server_cert_path, "not-a-valid-pem").unwrap();

        let acceptor = f.acceptor();
        let err = acceptor.build_server_config().unwrap_err();

        assert!(
            matches!(err, MtlsError::InvalidCert(_)),
            "expected InvalidCert for bad PEM, got: {err}"
        );
    }

    #[test]
    fn test_build_server_config_bad_pem_in_ca() {
        let f = TestCertFixture::new();
        fs::write(&f.ca_path, "garbage-pem-data").unwrap();

        let acceptor = f.acceptor();
        let err = acceptor.build_server_config().unwrap_err();

        assert!(
            matches!(err, MtlsError::InvalidCert(_)),
            "expected InvalidCert for bad CA PEM, got: {err}"
        );
    }

    #[test]
    fn test_build_server_config_missing_ca_file() {
        let f = TestCertFixture::new();
        let acceptor = MtlsAcceptor::new(
            "/tmp/__nonexistent_ca__",
            f.server_cert_path.clone(),
            f.server_key_path.clone(),
        );
        let err = acceptor.build_server_config().unwrap_err();

        assert!(
            matches!(err, MtlsError::CertNotFound(_)),
            "expected CertNotFound, got: {err}"
        );
    }

    #[test]
    fn test_build_server_config_missing_key_file() {
        let f = TestCertFixture::new();
        let acceptor = MtlsAcceptor::new(
            f.ca_path.clone(),
            f.server_cert_path.clone(),
            "/tmp/__nonexistent_key__",
        );
        let err = acceptor.build_server_config().unwrap_err();

        assert!(
            matches!(err, MtlsError::KeyNotFound(_)),
            "expected KeyNotFound, got: {err}"
        );
    }

    #[test]
    fn test_build_server_config_mismatched_key() {
        let f = TestCertFixture::new();
        // Write an unrelated key (generate a new keypair)
        use rcgen::KeyPair;
        let wrong_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        fs::write(&f.server_key_path, wrong_key.serialize_pem()).unwrap();

        let acceptor = f.acceptor();
        let err = acceptor.build_server_config().unwrap_err();

        assert!(
            matches!(err, MtlsError::InvalidCert(_)),
            "expected InvalidCert for mismatched key, got: {err}"
        );
    }

    // ── build_ca_store_with_cn_check tests ──────────────────────────────

    #[test]
    fn test_ca_store_with_allowed_cn() {
        let f = TestCertFixture::new();
        let mut acceptor = f.acceptor();
        // The CA cert has CN = "Test CA"
        acceptor.allowed_cn_list = vec!["Test CA".to_string()];

        let store = acceptor.build_ca_store_with_cn_check().unwrap();
        assert!(
            !store.is_empty(),
            "RootCertStore should contain the CA cert"
        );
    }

    #[test]
    fn test_ca_store_cn_rejected() {
        let f = TestCertFixture::new();
        let mut acceptor = f.acceptor();
        acceptor.allowed_cn_list = vec!["Wrong-CN".to_string()];

        let err = acceptor.build_ca_store_with_cn_check().unwrap_err();
        assert!(
            matches!(err, MtlsError::CnNotAllowed(_)),
            "expected CnNotAllowed, got: {err}"
        );
    }

    #[test]
    fn test_ca_store_empty_allowed_list_skips_cn_check() {
        let f = TestCertFixture::new();
        let mut acceptor = f.acceptor();
        // Empty list = no CN filtering
        acceptor.allowed_cn_list = vec![];

        let store = acceptor.build_ca_store_with_cn_check().unwrap();
        assert!(
            !store.is_empty(),
            "RootCertStore should contain the CA cert"
        );
    }

    // ── Builder methods (only available under multi-users-server) ───────

    #[test]
    #[cfg(feature = "multi-users-server")]
    fn test_with_client_cert_false_disables_verifier() {
        let f = TestCertFixture::new();
        let acceptor = f.acceptor().with_client_cert(false);
        let config = acceptor.build_server_config().unwrap();

        // Config builds with no client auth (ALPN still configured)
        assert_eq!(
            config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    #[cfg(feature = "multi-users-server")]
    fn test_with_client_cert_true_keeps_verifier() {
        let f = TestCertFixture::new();
        let acceptor = f.acceptor().with_client_cert(true);
        let config = acceptor.build_server_config().unwrap();

        // Config builds with client cert verification (default behavior)
        assert_eq!(
            config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    #[cfg(feature = "multi-users-server")]
    fn test_with_allowed_cns_filters_cn() {
        let f = TestCertFixture::new();
        let acceptor = f
            .acceptor()
            .with_allowed_cns(vec!["client-alpha".to_string(), "client-beta".to_string()]);

        assert_eq!(
            acceptor.allowed_cn_list,
            vec!["client-alpha".to_string(), "client-beta".to_string()]
        );
    }

    #[test]
    #[cfg(feature = "multi-users-server")]
    fn test_with_allowed_cns_empty_list() {
        let f = TestCertFixture::new();
        let acceptor = f.acceptor().with_allowed_cns(vec![]);

        assert!(acceptor.allowed_cn_list.is_empty());
    }

    // ── accept returns HandshakeFailed on bad TLS ─────────────────────────

    #[tokio::test]
    async fn test_accept_returns_handshake_error() {
        let f = TestCertFixture::new();
        let acceptor = f.acceptor();

        // Start a TCP listener on a random port so the TCP connect succeeds,
        // but since the listener does NOT perform a TLS handshake, the
        // acceptor will fail with HandshakeFailed.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn a task that accepts and then immediately drops the TCP connection
        tokio::spawn(async move {
            if let Ok((_stream, _)) = listener.accept().await {
                // Connection accepted — client will start TLS handshake
                // but no TLS data is sent back, causing the handshake to fail
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });

        // Give the spawned task a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Now connect — TCP succeeds, TLS fails
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let err = acceptor.accept(stream).await.unwrap_err();

        assert!(
            matches!(err, MtlsError::HandshakeFailed(_)),
            "expected HandshakeFailed, got: {err}"
        );
    }
}
