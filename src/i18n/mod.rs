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
