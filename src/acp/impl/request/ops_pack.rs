//! Operational status handlers — *migrated to sub-packs*.
//!
//! All operational status handlers have been split into dedicated modules:
//!
//! | Function | New Location |
//! |---|---|
//! | `handle_breaker_status` | `health_pack.rs` |
//! | `collect_degraded_services` | `health_pack.rs` |
//! | `handle_observability_alerts` | `diagnostic_pack.rs` |
//! | `handle_lock_status` | `diagnostic_pack.rs` |
//! | `summarize_lock_health` | `diagnostic_pack.rs` |
//!
//! This module is retained for backward compatibility with `request.rs`
//! where `mod ops_pack;` and `use self::ops_pack::*;` are declared.
