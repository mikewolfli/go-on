//! go-on – ACP runtime proxy with integrated multi-agent orchestration.
//!
//! This library crate re-exports the `i18n` module so that external consumers
//! (for example the `test_i18n` test harness) can import it via
//! `use go_on::i18n::{…}`.

pub mod i18n;

// ── Profile mutual exclusion ──────────────────────────────────────────────
// Exactly ONE profile must be selected. The Cargo feature system does not
// enforce mutual exclusion at the dependency level, so we check explicitly.
#[cfg(any(
    all(feature = "profile-local", feature = "profile-simple-server"),
    all(feature = "profile-local", feature = "profile-multi-users-server"),
    all(
        feature = "profile-simple-server",
        feature = "profile-multi-users-server"
    ),
))]
compile_error!(
    "Exactly one profile feature must be enabled. \
     Choose one of: profile-local, profile-simple-server, profile-multi-users-server"
);
