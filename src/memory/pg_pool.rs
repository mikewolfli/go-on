//! PostgreSQL connection pool (backend-postgres only).
//!
//! Provides a `PgPoolPair` that holds separate read/write pools, enabling
//! read-replica support. Both pools share the same `PgClientManager` factory.
//!
//! All pool operations must go through `pool_get` inside `spawn_blocking`
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
/// Always blocks on the dedicated shared fallback runtime rather than on
/// `Handle::try_current().block_on(...)`. `try_current()` succeeds inside an
/// async context (e.g. a synchronous constructor such as `ResponseCache::
/// new_with_replica` invoked from a runtime worker thread), and `Handle::
/// block_on` panics there with "Cannot block the current thread from within a
/// runtime" (principle #20). A separate current-thread runtime is safe to
/// block on from any thread — runtime worker, blocking pool, or plain sync.
/// Created once and reused to avoid per-call runtime construction overhead.
///
/// This function is **not re-entrant**: while the current thread is blocked on
/// [`FALLBACK_RT`] it *is* the runtime's worker, and a nested `pool_get` on
/// that same thread would deadlock the current-thread runtime. `pool_get`
/// detects that case (see [`IN_FALLBACK_RT`]) and panics with a clear message
/// so the misuse fails fast instead of hanging.
#[cfg(feature = "backend-postgres")]
static FALLBACK_RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

// Guards [`FALLBACK_RT`] against re-entrant `pool_get` calls.
//
// `FALLBACK_RT` is a current-thread runtime: while a thread is blocked on it,
// that thread *is* the runtime's worker. If code running inside that
// `block_on` (directly or transitively) called `pool_get` again, the nested
// `block_on` on the same thread would deadlock (the worker waits on itself)
// or panic with tokio's "Cannot start a runtime from within a runtime".
// `pool_get` sets this marker for the duration of the outer `block_on` and
// rejects any nested call it observes.
// NOTE: this is a plain comment, not a doc comment — `thread_local!` expands
// to a macro invocation and cannot carry `///` documentation.
#[cfg(feature = "backend-postgres")]
thread_local! {
    static IN_FALLBACK_RT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Clears the [`IN_FALLBACK_RT`] marker on drop, including during unwinding.
#[cfg(feature = "backend-postgres")]
struct FallbackRtMarker;

#[cfg(feature = "backend-postgres")]
impl FallbackRtMarker {
    fn enter() -> Self {
        IN_FALLBACK_RT.with(|flag| flag.set(true));
        FallbackRtMarker
    }
}

#[cfg(feature = "backend-postgres")]
impl Drop for FallbackRtMarker {
    fn drop(&mut self) {
        IN_FALLBACK_RT.with(|flag| flag.set(false));
    }
}

#[cfg(feature = "backend-postgres")]
pub(crate) fn pool_get(pool: &PgPool) -> Result<deadpool::managed::Object<PgClientManager>> {
    // Non-reentrancy guard: if the current thread is already the FALLBACK_RT
    // worker (this call originates from inside a future driven by
    // `FALLBACK_RT.block_on`), blocking on the same current-thread runtime
    // again would deadlock. Fail fast with a clear message instead.
    if IN_FALLBACK_RT.with(|flag| flag.get()) {
        panic!(
            "pool_get called re-entrantly from inside the fallback runtime worker thread; \
             blocking on the same current-thread runtime would deadlock. \
             Call pool_get from a spawn_blocking closure instead."
        );
    }
    let rt = FALLBACK_RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build fallback runtime for pool get")
    });
    // Mark the calling thread as the FALLBACK_RT worker for the duration of
    // `block_on` so a nested `pool_get` is rejected by the guard above. The
    // marker is cleared on drop, including during unwinding.
    let _marker = FallbackRtMarker::enter();
    rt.block_on(pool.get())
        .map_err(|e| anyhow::anyhow!("pool get failed: {e}"))
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

/// Resolve the PostgreSQL connection string from a single canonical source.
///
/// Priority: explicit config `connection_string` → `GO_ON_PG_CONNECTION_STRING`
/// → `DATABASE_URL` → `PG_DSN` → `GO_ON_DATABASE_URL`.
///
/// The cache, vector store, and memory warm tier all resolve their DSN through
/// this one function so the fallback order stays consistent across backends.
#[cfg(feature = "backend-postgres")]
pub(crate) fn resolve_pg_dsn(config_connection_string: Option<&str>) -> Option<String> {
    if let Some(dsn) = config_connection_string.map(str::trim) {
        if !dsn.is_empty() {
            return Some(dsn.to_string());
        }
    }
    for var in [
        "GO_ON_PG_CONNECTION_STRING",
        "DATABASE_URL",
        "PG_DSN",
        "GO_ON_DATABASE_URL",
    ] {
        if let Ok(value) = std::env::var(var) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}
