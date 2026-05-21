use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

/// Serializes concurrent `/rpc` calls to prevent pipe-swapping race conditions.
/// `server.output` is a global singleton — without this guard, two concurrent
/// `/rpc` requests would corrupt each other's response capture pipes.
pub(crate) static RPC_SERIAL: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

pub(crate) static RESPONSES_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn next_responses_api_id(prefix: &str) -> String {
    let seq = RESPONSES_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", prefix, crate::acp::prelude::now_ts_ms(), seq)
}
