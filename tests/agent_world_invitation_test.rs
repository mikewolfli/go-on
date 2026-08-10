//! Agent World Invitation — End-to-End Pipeline Test
//!
//! Validates that the go-on backend correctly:
//! 1. Parses invitation URLs from user messages
//! 2. Extracts fragment params (task_access_token)
//! 3. Calls the Agent World API to fetch the task package
//! 4. Parses and validates the full response
//!
//! This tests the observe_phase pipeline up to the point where data
//! is injected into the AI context. The AI-driven plan-execute steps
//! require AI provider configuration and are tested separately.
//!
//! Note: the live external API call (`test_live_task_package_fetch`,
//! which fetched from agent-world-test.chuanshuo.com.cn with a hardcoded
//! one-time token) was removed — it is non-deterministic (external
//! token/invitation lifecycle) and was used for ad-hoc project-level
//! verification rather than the deterministic suite.
//!
//! The retained tests exercise the production `extract_url` helper
//! (`go_on::orchestration::tool_extended::http::extract_url`), which is the
//! function observe_phase actually calls. Earlier self-referential tests that
//! re-implemented the fragment/path/API-URL parsing inline (and therefore only
//! tested their own string constants rather than production code) were
//! removed: the fragment parsing and API URL construction live inline inside
//! the async `observe_phase` and are not exposed as callable functions.

use go_on::orchestration::tool_extended;

const TEST_INVITE_URL: &str = "https://agent-world-test.chuanshuo.com.cn/agent-invite/invite_8486d728a28f4c54a8188b8bebc0db3c#task_access_token=task_d2add96d25fd4a007a83cdff2134c8da3c6a7e4e235589bc";

// ============================================================================
// Phase 1: URL extraction from user messages (mimics observe_phase logic)
// ============================================================================

#[test]
fn test_extract_url_from_plain_text() {
    let msg = format!("请完成这个邀请：{}", TEST_INVITE_URL);
    let extracted = tool_extended::http::extract_url(&msg);
    assert!(
        extracted.is_some(),
        "URL should be extracted from plain text"
    );
    let url = extracted.unwrap();
    assert!(
        url.starts_with("https://agent-world-test.chuanshuo.com.cn"),
        "domain mismatch: got {}",
        url
    );
    assert!(
        url.contains("task_access_token"),
        "fragment preserved: got {}",
        url
    );
}

#[test]
fn test_extract_url_from_markdown_link() {
    let msg = format!("[点此加入]({})", TEST_INVITE_URL);
    let extracted = tool_extended::http::extract_url(&msg);
    assert!(extracted.is_some(), "URL from markdown link");
    let url = extracted.unwrap();
    assert!(url.starts_with("https://agent-world-test.chuanshuo.com.cn"));
}

#[test]
fn test_extract_url_from_code_block() {
    let msg = format!("```\n{}\n```", TEST_INVITE_URL);
    let extracted = tool_extended::http::extract_url(&msg);
    assert!(extracted.is_some(), "URL from code block");
}
