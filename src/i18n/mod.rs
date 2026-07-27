//! Internationalization (i18n) modules for multi-language support and localization.
//!
//! This module contains components responsible for providing internationalization
//! capabilities in the ACP proxy system, including:
//!
//! - **Runtime**: Core i18n runtime for loading and managing translations
//! - **Watcher**: File system watcher for hot-reloading translation files
//!
//! The i18n system supports dynamic language switching and hot-reloading
//! of translation files without requiring server restart.

pub mod runtime;
pub mod watcher;

// Re-export commonly used i18n functions so consumers can write
// `use go_on::i18n::{t, tf, Language, …}` instead of drilling into `runtime`.
// The `allow` is needed because the binary (`main.rs`) has its own `mod i18n`
// compilation unit that doesn't reference these re-exports directly;
// they are consumed by library consumers (e.g. `test_i18n`).
#[allow(unused_imports)]
pub use runtime::{current_language, init_i18n, set_language, t, tf, Language};
