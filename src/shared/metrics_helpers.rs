//! Shared metrics formatting helpers.
//!
//! Extracted from `observability::observability` to break the circular
//! dependency: acp → observability → intelligence → acp.
//!
//! These helpers are standalone functions that do not depend on any
//! observability-specific types.
