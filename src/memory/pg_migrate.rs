//! Versioned database migrations for PostgreSQL (backend-postgres only).
//!
//! Each migration is an ordered SQL string stored in the `MIGRATIONS` slice.
//! The runner tracks applied versions in a `_schema_version` table and applies
//! any pending migrations up to the requested target version.
//!
//! **Important:** The DDL strings in `MIGRATIONS` serve as the canonical schema
//! baseline. Each `new()` method in `cache.rs` / `vector.rs` may also run
//! additional dynamic DDL (e.g. vector dimensions, HNSW indexes) that is
//! applied *in addition* to the migration DDL.  Because all DDL uses
//! `CREATE TABLE IF NOT EXISTS`, running both is safe — the migration creates
//! the base schema, and the dynamic DDL adds anything the migration does not
//! cover (such as the HNSW index with configurable dimensions).
//!
//! **`warm_memory` is intentionally NOT in `MIGRATIONS`:** the table is created
//! inline by `WarmStore::new` in `memory_persistence.rs` (both the sqlite and
//! postgres variants). The warm tier has no versioned schema-evolution
//! requirements — its DDL is idempotent (`CREATE TABLE IF NOT EXISTS` + `CREATE
//! INDEX IF NOT EXISTS`) and is owned by the store that uses it, so adding it
//! here would create a second, duplicate DDL path. Keep it inline unless the
//! warm tier gains real migration needs.

#![cfg_attr(not(feature = "backend-postgres"), allow(unused_imports))]

#[cfg(feature = "backend-postgres")]
use postgres::Client;

use anyhow::Result;

/// Ordered list of SQL migration strings.
///
/// Index 0 → v1, index 1 → v2, etc.  Append new migrations at the end.
#[cfg(feature = "backend-postgres")]
pub(crate) const MIGRATIONS: &[&str] = &[
    // v1: response_cache table
    "CREATE TABLE IF NOT EXISTS response_cache (
        cache_key        TEXT    PRIMARY KEY,
        response_text    TEXT    NOT NULL,
        agent_name       TEXT,
        created_at       BIGINT  NOT NULL,
        updated_at       BIGINT  NOT NULL,
        expires_at       BIGINT  NOT NULL,
        hit_count        BIGINT  NOT NULL DEFAULT 0,
        last_hit_at      BIGINT
    );
    CREATE INDEX IF NOT EXISTS idx_response_cache_expires_at
        ON response_cache(expires_at);",
    // v2: vector_memory + phase_summary (base tables; HNSW index is added
    //     dynamically in VectorStore::new() since dimensions are configurable).
    //     NOTE: `embedding vector(768)` is a static baseline only —
    //     VectorStore::new_with_replica re-types the column to the runtime
    //     embedding-provider dimensions at every startup, so this fixed width
    //     never blocks a non-768 provider.
    "CREATE TABLE IF NOT EXISTS vector_memory (
        memory_key      TEXT PRIMARY KEY,
        phase           TEXT NOT NULL,
        query_text      TEXT NOT NULL,
        response_text   TEXT NOT NULL,
        embedding       vector(768) NOT NULL,
        created_at      BIGINT NOT NULL,
        updated_at      BIGINT NOT NULL,
        hit_count       BIGINT NOT NULL DEFAULT 0,
        last_hit_at     BIGINT,
        user_id         TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_vector_memory_phase_updated_at
        ON vector_memory(phase, updated_at DESC);
    CREATE INDEX IF NOT EXISTS idx_vector_memory_user_id
        ON vector_memory(user_id);
    CREATE TABLE IF NOT EXISTS phase_summary (
        phase           TEXT PRIMARY KEY,
        summary_text    TEXT NOT NULL,
        updated_at      BIGINT NOT NULL
    );",
    // v3: session_store
    "CREATE TABLE IF NOT EXISTS session_store (
        session_id      TEXT PRIMARY KEY,
        session_data    TEXT NOT NULL,
        created_at      BIGINT NOT NULL,
        updated_at      BIGINT NOT NULL
    );",
];

/// Run all pending migrations up to (and including) `target_version`.
///
/// `target_version` is 1-based and is clamped to `MIGRATIONS.len()`.
#[cfg(feature = "backend-postgres")]
pub(crate) fn run_migrations(client: &mut Client, target_version: usize) -> Result<()> {
    let target = target_version.min(MIGRATIONS.len());
    if target == 0 {
        return Ok(());
    }

    // Ensure the schema version tracking table exists.
    client.batch_execute(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version     INTEGER PRIMARY KEY,
            applied_at  BIGINT NOT NULL
        )",
    )?;

    // Read the highest already-applied version.
    let current: usize = client
        .query("SELECT COALESCE(MAX(version), 0) FROM _schema_version", &[])
        .ok()
        .and_then(|rows| rows.first().map(|r| r.get::<_, i32>(0) as usize))
        .unwrap_or(0);

    // Apply each pending migration in order.
    for v in (current + 1)..=target {
        let sql = MIGRATIONS[v - 1];
        client.batch_execute(sql)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        client.execute(
            "INSERT INTO _schema_version (version, applied_at) VALUES ($1, $2)",
            &[&(v as i32), &now],
        )?;
    }

    Ok(())
}
