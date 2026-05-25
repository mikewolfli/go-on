#![recursion_limit = "512"]

//! go-on – ACP runtime proxy with integrated multi-agent orchestration.
//!
//! This library crate re-exports modules so that external consumers
//! (for example integration tests) can import them via `use go_on::…`.

pub mod i18n;
pub mod optimization;
pub mod resilience;

// ── Profile mutual exclusion ──────────────────────────────────────────────
// Exactly ONE profile must be selected. The Cargo feature system does not
// enforce mutual exclusion at the dependency level, so we check explicitly.

// Fail if no profile is selected (e.g. `cargo build --no-default-features`).
#[cfg(not(any(
    feature = "profile-local",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server",
)))]
compile_error!(
    "No profile feature is enabled. Exactly one must be selected: \
     profile-local, profile-simple-server, or profile-multi-users-server"
);

// Fail if more than one profile is selected simultaneously.
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
