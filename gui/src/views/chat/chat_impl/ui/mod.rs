//! Chat UI rendering module.
//!
//! This module was extracted from the monolithic `ui.rs` (2,043 lines).
//! Sub-modules have been created to split concerns:
//!
//! - `messages` - message bubble rendering, token stats, avatars
//! - `input` - input area, send button, keyboard shortcuts
//! - `model_picker` - agent/model selection combos
//! - `attachments` - file attachment display and handling
//!
//! ## Current status (as of this writing)
//!
//! The sub-modules contain rendering *helpers* that are called from within
//! `old_ui_content.rs` (which is included below). The old file still holds
//! all the top-level methods on `ChatView`:
//!
//! | Method | Location | Size | Notes |
//! |--------|----------|------|-------|
//! | `ChatView::show()` | old_ui_content.rs:4 | ~130 lines | Main entry, delegates to show_safe_chat_layout |
//! | `show_safe_chat_layout()` | old_ui_content.rs:132 | ~500 lines | Stable layout: mode row, messages scroll, input area |
//! | `show_sidebar()` | old_ui_content.rs:625 | ~330 lines | Session list sidebar |
//! | `show_messages()` | old_ui_content.rs:953 | ~670 lines | Message bubble rendering, markdown, token stats |
//!
//! ## Migration plan (incremental)
//!
//! Phase 1 (current): Pull pure rendering helpers into sub-modules ✓ DONE
//!   - `messages::draw_role_avatar`, `render_token_stats`, `render_collapsed_bubble`
//!   - `attachments::render_attachments`
//!   - `input::render_send_button`, `render_mode_row`, `handle_input_shortcuts`
//!   - `model_picker::render_model_picker`
//!
//! Phase 2 (next): Extract `show_messages()` into `messages.rs`
//!   - Move `ChatView::show_messages()` to `messages::show_messages()`
//!   - Keep `use super::*` import for ChatView access
//!
//! Phase 3 (next): Extract `show_sidebar()` into a new `sidebar.rs` sub-module
//!   - Creates `sidebar.rs` with `pub fn show_sidebar()`
//!
//! Phase 4 (final): Extract `show()` and `show_safe_chat_layout()` into `mod.rs` directly
//!   - Remove `include!("old_ui_content.rs")`
//!   - Delete old_ui_content.rs
//!
//! Once all phases are complete, the `include!` on the last line can be removed.

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
