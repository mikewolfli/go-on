//! PostgreSQL connection pool (backend-postgres only).
//!
//! Provides a [`PgPoolPair`] that holds separate read/write pools, enabling
//! read-replica support. Both pools share the same [`PgClientManager`] factory.
//!
//! All pool operations must go through [`pool_get`] inside `spawn_blocking`
//! closures to bridge deadpool's async API with the synchronous `postgres::Client`.

use std::borrow::Cow;
use std::future::Future;

#[cfg(feature = "backend-postgres")]
use deadpool::managed::{Manager, Metrics, Pool, RecycleError, RecycleResult};
#[cfg(feature = "backend-postgres")]
use postgres::{Client, NoTls};
#[cfg(feature = "backend-postgres")]
use rustls::crypto::ring;
#[cfg(feature = "backend-postgres")]
use rustls::ClientConfig;
#[cfg(feature = "backend-postgres")]
use std::sync::Arc;
#[cfg(feature = "backend-postgres")]
use tokio_postgres_rustls::MakeRustlsConnect;

use anyhow::Result;

/// A [`Manager`] that creates and recycles `postgres::Client` connections.
///
/// deadpool's `Manager` trait uses `impl Future` return types, so we
/// perform the synchronous connection creation inline and wrap the result
/// in an immediately-resolved future via `async move { }`.
#[cfg(feature = "backend-postgres")]
pub(crate) struct PgClientManager {
    connect_fn: Box<dyn Fn() -> Result<Client> + Send + Sync>,
}

#[cfg(feature = "backend-postgres")]
impl Manager for PgClientManager {
    type Type = Client;
    type Error = anyhow::Error;

    fn create(&self) -> impl Future<Output = Result<Client, Self::Error>> + Send {
        let result = (self.connect_fn)();
        async move { result }
    }

    fn recycle(
        &self,
        conn: &mut Client,
        _metrics: &Metrics,
    ) -> impl Future<Output = RecycleResult<Self::Error>> + Send {
        let result = conn
            .simple_query("SELECT 1")
            .map(|_| ())
            .map_err(|e| RecycleError::Message(Cow::Owned(e.to_string())));
        async move { result }
    }
}

/// Pool type alias for convenience.
#[cfg(feature = "backend-postgres")]
pub(crate) type PgPool = Pool<PgClientManager>;

/// A pair of pools for read/write splitting.
///
/// When no read-replica is configured both fields point to the same pool.
#[cfg(feature = "backend-postgres")]
#[derive(Clone)]
pub(crate) struct PgPoolPair {
    pub write: PgPool,
    pub read: PgPool,
}

/// Acquire a connection from a pool by blocking the current thread.
///
/// Uses `Handle::try_current()` with a fallback to a temporary runtime when
/// no Tokio context is active (principle #24). Callers in async contexts should
/// use `spawn_blocking` + `pool_get`; sync callers during startup call it directly.
/// Shared fallback runtime for callers outside any Tokio context.
/// Created once and reused to avoid per-call runtime construction overhead.
#[cfg(feature = "backend-postgres")]
static FALLBACK_RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

#[cfg(feature = "backend-postgres")]
pub(crate) fn pool_get(pool: &PgPool) -> Result<deadpool::managed::Object<PgClientManager>> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle
            .block_on(pool.get())
            .map_err(|e| anyhow::anyhow!("pool get failed: {e}")),
        Err(_) => {
            // No runtime active — use the shared fallback runtime.
            // Created once and reused for all subsequent sync-path calls.
            let rt = FALLBACK_RT.get_or_init(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build fallback runtime for pool get")
            });
            rt.block_on(pool.get())
                .map_err(|e| anyhow::anyhow!("pool get failed: {e}"))
        }
    }
}

/// Build a single [`PgPool`] from a connection factory.
#[cfg(feature = "backend-postgres")]
pub(crate) fn create_pool<F>(connect_fn: F, max_size: usize) -> PgPool
where
    F: Fn() -> Result<Client> + Send + Sync + 'static,
{
    let manager = PgClientManager {
        connect_fn: Box::new(connect_fn),
    };
    Pool::builder(manager).max_size(max_size).build().unwrap()
}

/// Build a [`PgPoolPair`] from separate write and read factories.
///
/// When `read_replica_url` is `None`, the read pool is the same as the write pool.
#[cfg(feature = "backend-postgres")]
pub(crate) fn create_pool_pair<FW, FR>(
    write_connect_fn: FW,
    read_replica_url: Option<String>,
    read_connect_fn: FR,
    max_size: usize,
) -> PgPoolPair
where
    FW: Fn() -> Result<Client> + Send + Sync + 'static,
    FR: Fn() -> Result<Client> + Send + Sync + 'static,
{
    let write = create_pool(write_connect_fn, max_size);
    let read = match read_replica_url {
        Some(_) => create_pool(read_connect_fn, max_size),
        None => write.clone(),
    };
    PgPoolPair { write, read }
}

// ── TLS connect stack (single source of truth) ────────────────────────────
// Previously duplicated byte-for-byte in `memory/cache.rs` and
// `memory/vector.rs`; unified here so both store backends share one
// `sslmode`-aware connect path.

/// Parse the `sslmode` parameter from a PostgreSQL connection URL.
///
/// Returns `None` when no `sslmode` is present (defaults to NoTls).
#[cfg(feature = "backend-postgres")]
fn parse_sslmode(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "sslmode")
        .map(|(_, v)| v.to_string())
}

/// A [`ServerCertVerifier`] that accepts all certificates (for `sslmode=require`).
#[cfg(feature = "backend-postgres")]
#[derive(Debug)]
struct PermissiveVerifier;

#[cfg(feature = "backend-postgres")]
impl rustls::client::danger::ServerCertVerifier for PermissiveVerifier {
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }

    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
}

/// Connect to PostgreSQL with optional TLS based on the `sslmode` URL parameter.
///
/// Supports:
/// - `sslmode=require`     — TLS, no server certificate verification
/// - `sslmode=verify-ca`   — TLS, verify server certificate against CA
/// - `sslmode=verify-full` — TLS, verify server certificate AND hostname
/// - absent / `disable` / `allow` / `prefer` — No TLS (plain)
#[cfg(feature = "backend-postgres")]
pub(crate) fn connect_postgres(url: &str) -> Result<Client> {
    let provider = Arc::new(ring::default_provider());
    match parse_sslmode(url).as_deref() {
        Some("require") => {
            // TLS with no server certificate verification.
            let config = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| anyhow::anyhow!("TLS protocol config: {e}"))?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(PermissiveVerifier))
                .with_no_client_auth();
            let tls = MakeRustlsConnect::new(config);
            Ok(Client::connect(url, tls)?)
        }
        Some("verify-ca") | Some("verify-full") => {
            // TLS with CA certificate verification.
            let root_store =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| anyhow::anyhow!("TLS protocol config: {e}"))?
                .with_root_certificates(root_store)
                .with_no_client_auth();
            let tls = MakeRustlsConnect::new(config);
            Ok(Client::connect(url, tls)?)
        }
        _ => {
            // No TLS or sslmode=disable/allow/prefer
            Ok(Client::connect(url, NoTls)?)
        }
    }
}
