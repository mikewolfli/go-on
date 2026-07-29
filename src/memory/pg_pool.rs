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
use postgres::Client;

use anyhow::Result;

/// A [`Manager`] that creates and recycles `postgres::Client` connections.
///
/// deadpool 0.12's `Manager` trait uses `impl Future` return types, so we
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
#[cfg(feature = "backend-postgres")]
pub(crate) fn pool_get(pool: &PgPool) -> Result<deadpool::managed::Object<PgClientManager>> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| {
            handle
                .block_on(pool.get())
                .map_err(|e| anyhow::anyhow!("pool get failed: {e}"))
        }),
        Err(_) => {
            // No runtime active — create a temporary one for this one call.
            // This can happen during early-startup sync init before the runtime is built.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| anyhow::anyhow!("failed to build temp runtime for pool get: {e}"))?;
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
