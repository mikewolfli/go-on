//! Chat UI rendering module.
//!
//! This module was extracted from the monolithic `ui.rs` (2,043 lines).
//! Sub-modules are being created to split concerns:
//!
//! - `messages` - message bubble rendering, token stats, avatars
//! - `input` - input area, send button, keyboard shortcuts
//! - `model_picker` - agent/model selection combos
//! - `attachments` - file attachment display and handling
//!
//! Currently the old monolithic content is included for backward compatibility.
//! As sub-modules are stabilized, code will be migrated out of this file.

pub mod attachments;
pub mod input;
pub mod messages;
pub mod model_picker;

// Re-export public sub-module items that don't conflict with the legacy content.
// Note: mode_display_key, draw_role_avatar, render_token_stats, render_collapsed_bubble
// are now wired from their respective sub-modules.

// ═══════════════════════════════════════════════════════════════════════════
// Legacy content from the old monolithic ui.rs — will be migrated to
// sub-modules as part of ongoing decomposition (BLUE65).
// ═══════════════════════════════════════════════════════════════════════════

include!("old_ui_content.rs");
