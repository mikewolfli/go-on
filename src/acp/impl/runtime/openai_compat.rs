//! OpenAI API compatibility and Responses API handlers.
//!
//! Split (M0.4) into two single-responsibility submodules:
//! - [`chat_completions`] — `/v1/chat/completions`-compatible endpoints and
//!   the `OpenAiChatRequest` / `OpenAiChatMessage` types;
//! - [`responses`] — the Responses API implementation (input conversion,
//!   response builders, payload store, tool helpers, validation, handlers).
//!
//! This facade re-exports the `pub(crate)` surface so external consumers
//! (`runtime/http.rs`) keep their `use super::openai_compat::{...}` imports.

pub(crate) mod chat_completions;
pub(crate) mod responses;

pub(crate) use chat_completions::{
    build_openai_models_response, handle_openai_chat_completions,
};
pub(crate) use responses::{
    extract_response_id_from_path, handle_response_get, handle_responses_api,
    list_responses_api_payloads,
};
