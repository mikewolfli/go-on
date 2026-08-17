//! Unified ACP stream-event consumption (M0.1).
//!
//! Single classification + field-extraction implementation for the ACP stream
//! event protocol. The JSON-RPC dispatch loop (`request/dispatch.rs`), the
//! session bridge (`request/protocol_pack/session.rs`), and the OpenAI-compat
//! SSE consumer (`runtime/openai_compat/chat_completions.rs`) previously
//! matched event-name string literals and re-extracted payload fields inline,
//! and the consumers' match arm sets had already drifted (dispatch handles
//! `progress`/`phase_*`, the bridge silently ignores them). This module is the
//! src-side single source so the consumers cannot drift again.
//!
//! The GUI is a separate crate with its own consumer
//! (`gui/src/backend/state.rs`); this module is not shared across the crate
//! boundary (the `""` → chunk fallback in the GUI SSE parser is GUI-specific
//! and deliberately not mirrored here — src-side `StreamFrame.event` is always
//! a non-empty literal).

use serde_json::Value;

use crate::acp::r#impl::chat::streaming::{
    STREAM_EVENT_CHUNK, STREAM_EVENT_DONE, STREAM_EVENT_ERROR, STREAM_EVENT_PHASE_END,
    STREAM_EVENT_PHASE_START, STREAM_EVENT_PROGRESS, STREAM_EVENT_RESULT, STREAM_EVENT_STATUS,
    STREAM_EVENT_TELEMETRY, STREAM_EVENT_TOOL_APPROVAL,
};

/// Classified stream event types covering every production event name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamEventKind {
    Chunk,
    Done,
    Result,
    Error,
    Status,
    Progress,
    PhaseStart,
    PhaseEnd,
    Telemetry,
    ToolApproval,
    Unknown,
}

/// Classify a stream event name. Unknown names fall to [`StreamEventKind::Unknown`]
/// so consumers must still decide their fallback (ignore / generic notify).
///
/// Matches on the `STREAM_EVENT_*` constants from [`crate::acp::r#impl::chat::streaming`]
/// so emission and classification share one vocabulary and cannot drift.
pub(crate) fn classify_stream_event(event: &str) -> StreamEventKind {
    match event {
        STREAM_EVENT_CHUNK => StreamEventKind::Chunk,
        STREAM_EVENT_DONE => StreamEventKind::Done,
        STREAM_EVENT_RESULT => StreamEventKind::Result,
        STREAM_EVENT_ERROR => StreamEventKind::Error,
        STREAM_EVENT_STATUS => StreamEventKind::Status,
        STREAM_EVENT_PROGRESS => StreamEventKind::Progress,
        STREAM_EVENT_PHASE_START => StreamEventKind::PhaseStart,
        STREAM_EVENT_PHASE_END => StreamEventKind::PhaseEnd,
        STREAM_EVENT_TELEMETRY => StreamEventKind::Telemetry,
        STREAM_EVENT_TOOL_APPROVAL => StreamEventKind::ToolApproval,
        _ => StreamEventKind::Unknown,
    }
}

/// Fields of a `chunk` event.
#[derive(Debug, Default, Clone)]
pub(crate) struct ChunkFields {
    /// Visible content token (empty when the frame carries only reasoning).
    pub token: String,
    /// Reasoning text (empty for plain content tokens).
    pub reasoning: String,
}

/// Extract `chunk` fields. `token`/`reasoning` mirror the `text`/`reasoning`
/// split the GUI consumer uses; the src side has no `text` producer today but
/// the fallback is kept for parity.
pub(crate) fn extract_chunk_fields(payload: &Value) -> ChunkFields {
    ChunkFields {
        token: payload
            .get("token")
            .or_else(|| payload.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        reasoning: payload
            .get("reasoning")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

/// Fields of a `result`/`done` completion event.
#[derive(Debug, Default, Clone)]
pub(crate) struct ResultFields {
    /// Final response text (`response`, falling back to `content`).
    pub response: Option<String>,
}

/// Extract the final response text of a `result`/`done` completion event
/// (`response`, falling back to `content`).
pub(crate) fn extract_result_fields(payload: &Value) -> ResultFields {
    let response = payload
        .get("response")
        .and_then(Value::as_str)
        .or_else(|| payload.get("content").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    ResultFields { response }
}

/// Stream error message with `error` → `message` dual-field fallback.
///
/// The production error event carries the resolved human text under `error`
/// (and historically also a raw i18n key under `message`); consumers must
/// prefer `error` so a raw key can never leak to the client.
pub(crate) fn extract_error_message(payload: &Value) -> Option<String> {
    payload
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Fields of a `tool_approval` event.
#[derive(Debug, Clone)]
pub(crate) struct ToolApprovalFields {
    pub tool_name: String,
    pub tool_args: Value,
    pub mode: String,
    pub risk_score: f64,
}

/// Extract `tool_approval` fields (the same four the bridge registers as a
/// pending permission request).
pub(crate) fn extract_tool_approval_fields(payload: &Value) -> ToolApprovalFields {
    ToolApprovalFields {
        tool_name: payload
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tool_args: payload.get("tool_args").cloned().unwrap_or(Value::Null),
        mode: payload
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        risk_score: payload
            .get("risk_score")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_every_stream_event_constant() {
        // The STREAM_EVENT_* constants in `streaming` are the src-side emission
        // vocabulary; each one must resolve to a concrete kind (never Unknown)
        // so a constant emitted by any producer is always consumed correctly.
        for name in [
            STREAM_EVENT_CHUNK,
            STREAM_EVENT_DONE,
            STREAM_EVENT_RESULT,
            STREAM_EVENT_ERROR,
            STREAM_EVENT_STATUS,
            STREAM_EVENT_PROGRESS,
            STREAM_EVENT_PHASE_START,
            STREAM_EVENT_PHASE_END,
            STREAM_EVENT_TELEMETRY,
            STREAM_EVENT_TOOL_APPROVAL,
        ] {
            assert_ne!(
                classify_stream_event(name),
                StreamEventKind::Unknown,
                "constant {name} must classify to a concrete kind"
            );
        }
    }

    #[test]
    fn classifies_all_production_event_names() {
        for (name, kind) in [
            ("chunk", StreamEventKind::Chunk),
            ("done", StreamEventKind::Done),
            ("result", StreamEventKind::Result),
            ("error", StreamEventKind::Error),
            ("status", StreamEventKind::Status),
            ("progress", StreamEventKind::Progress),
            ("phase_start", StreamEventKind::PhaseStart),
            ("phase_end", StreamEventKind::PhaseEnd),
            ("telemetry", StreamEventKind::Telemetry),
            ("tool_approval", StreamEventKind::ToolApproval),
        ] {
            assert_eq!(classify_stream_event(name), kind, "event {name}");
        }
        assert_eq!(
            classify_stream_event("unknown_event"),
            StreamEventKind::Unknown
        );
    }

    #[test]
    fn extract_error_prefers_error_over_message() {
        // The production site historically emitted a raw i18n key under
        // `message`; the resolved text under `error` must win.
        let payload = serde_json::json!({
            "error": "pipeline produced no response",
            "message": "error.chat.no_response_from_pipeline",
        });
        assert_eq!(
            extract_error_message(&payload).as_deref(),
            Some("pipeline produced no response")
        );
        // message-only events (GUI/proxy style) still resolve.
        let msg_only = serde_json::json!({ "message": "stream failed" });
        assert_eq!(
            extract_error_message(&msg_only).as_deref(),
            Some("stream failed")
        );
        // Empty strings are treated as absent.
        let empty = serde_json::json!({ "error": "" });
        assert_eq!(extract_error_message(&empty), None);
    }

    #[test]
    fn extract_chunk_fields_parses_token_and_reasoning() {
        let payload = serde_json::json!({ "token": "hello", "reasoning": "think" });
        let fields = extract_chunk_fields(&payload);
        assert_eq!(fields.token, "hello");
        assert_eq!(fields.reasoning, "think");
        let reasoning_only = serde_json::json!({ "token": "", "reasoning": "r" });
        assert_eq!(extract_chunk_fields(&reasoning_only).token, "");
    }

    #[test]
    fn extract_result_fields_falls_back_across_keys() {
        let payload = serde_json::json!({ "content": "fallback", "agent": "coder" });
        let fields = extract_result_fields(&payload);
        assert_eq!(fields.response.as_deref(), Some("fallback"));
        // Missing response/content → None (the extra payload keys are ignored).
        let empty = serde_json::json!({ "agent": "coder", "selected_model": "gpt-4o" });
        assert_eq!(extract_result_fields(&empty).response, None);
    }

    #[test]
    fn extract_tool_approval_fields_parses_four_fields() {
        let payload = serde_json::json!({
            "tool_name": "write_file",
            "tool_args": {"path": "a.txt"},
            "mode": "edit",
            "risk_score": 42.5,
        });
        let fields = extract_tool_approval_fields(&payload);
        assert_eq!(fields.tool_name, "write_file");
        assert_eq!(fields.tool_args["path"], "a.txt");
        assert_eq!(fields.mode, "edit");
        assert_eq!(fields.risk_score, 42.5);
    }
}
