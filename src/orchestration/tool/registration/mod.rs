//! Registration of the built-in tool set into the [`ToolRegistry`].
//!
//! The registry core (struct, statics and runtime methods) stays in
//! `tool/mod.rs`; the population logic was split here by tool category so
//! each category can be reviewed and feature-gated independently. The call
//! order below preserves the original registration sequence from
//! `ToolRegistry::new` (the backward-compatibility alias block stays last).

use super::ToolRegistry;

mod cad_media;
mod core;
mod data;
mod documents;
mod fs_code;
mod ops;

use self::cad_media::register_cad_media;
use self::core::register_core;
use self::data::register_data;
use self::documents::register_documents;
use self::fs_code::register_fs_code;
use self::ops::register_ops;

/// Register every built-in tool into the given registry. Called once by
/// `ToolRegistry::new`.
pub(crate) fn register_all(registry: &mut ToolRegistry) {
    register_core(registry);
    register_fs_code(registry);
    register_documents(registry);
    register_data(registry);
    register_cad_media(registry);
    register_ops(registry);
}
