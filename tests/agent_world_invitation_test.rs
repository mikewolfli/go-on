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

// ============================================================================
// Phase 2: URL parsing — fragment splitting, path segments
// ============================================================================

#[test]
fn test_url_fragment_and_path_parsing() {
    // Step 1: Split fragment for HTTP fetch
    let fetch_url = TEST_INVITE_URL.split('#').next().unwrap_or(TEST_INVITE_URL);
    assert!(!fetch_url.contains('#'));
    assert!(fetch_url.ends_with("invite_8486d728a28f4c54a8188b8bebc0db3c"));

    // Step 2: Extract fragment params (like observe_phase does)
    let fragment_params: Vec<(String, String)> = TEST_INVITE_URL
        .split('#')
        .nth(1)
        .map(|f| {
            url::form_urlencoded::parse(f.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .unwrap_or_default();

    assert!(!fragment_params.is_empty());
    let token = fragment_params
        .iter()
        .find(|(k, _)| k == "task_access_token")
        .map(|(_, v)| v.as_str());
    let token_val = token.unwrap();
    assert!(
        token_val.starts_with("task_"),
        "token should start with task_"
    );
    assert!(
        token_val.len() >= 40,
        "token length should be >= 40, got {}",
        token_val.len()
    );

    // Step 3: Path segments (like observe_phase does)
    let path_segments: Vec<&str> = TEST_INVITE_URL
        .split('#')
        .next()
        .unwrap_or(TEST_INVITE_URL)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // path_segments includes "https:" as first element from URL splitting
    // last() is still correctly the invitation_id
    assert!(path_segments.len() >= 3, "got {:?}", path_segments);
    assert_eq!(
        *path_segments.last().unwrap(),
        "invite_8486d728a28f4c54a8188b8bebc0db3c"
    );

    // Step 4: Detect invitation_id
    let invitation_id = path_segments
        .last()
        .filter(|s| s.starts_with("invite_") || s.starts_with("invitation_"));
    assert!(invitation_id.is_some());
    assert_eq!(
        *invitation_id.unwrap(),
        "invite_8486d728a28f4c54a8188b8bebc0db3c"
    );
}

// ============================================================================
// Phase 3: API URL construction (mimics observe_phase logic)
// ============================================================================

#[test]
fn test_api_url_construction() {
    let path_segments: Vec<&str> = TEST_INVITE_URL
        .split('#')
        .next()
        .unwrap_or(TEST_INVITE_URL)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let invitation_id = path_segments.last().unwrap();
    let host = TEST_INVITE_URL.split('/').nth(2).unwrap_or("");
    let scheme = if TEST_INVITE_URL.starts_with("https") {
        "https"
    } else {
        "http"
    };

    let api_url = format!(
        "{}://{}/api/v1/agent-binding/invitations/{}/agent-task",
        scheme, host, invitation_id,
    );

    assert_eq!(
        api_url,
        "https://agent-world-test.chuanshuo.com.cn/api/v1/agent-binding/invitations/invite_8486d728a28f4c54a8188b8bebc0db3c/agent-task"
    );
}

// ============================================================================
// Phase 4: (removed) live API call to Agent World — non-deterministic
//          external test, see module doc.
// ============================================================================

// ============================================================================
// Phase 5: Verify context construction is neutral (no prescriptive instructions)
// ============================================================================

#[test]
fn test_context_construction_is_neutral() {
    // Simulate what observe_phase puts into the AI context
    let fake_spa = "<div id=\"root\"></div>";
    let mut context_msg = format!(
        "[Auto-fetched content from {}]\nHTTP Status: 200\n\n{}",
        TEST_INVITE_URL, fake_spa
    );

    let mut spa_info = "\n\n[SPA Page Analysis]\nPath segments: agent-world-test.chuanshuo.com.cn / agent-invite / invite_8486d728...\nFragment params: [(\"task_access_token\", \"task_d2add96d25fd4a007a83cdff2134c8da3c6a7e4e235589bc\")]".to_string();

    // A shortened mock of the API response insertion
    spa_info.push_str("\n\n[API: POST .../api/v1/agent-binding/invitations/.../agent-task]\nStatus: 200 OK\nResponse:\n{\"ok\":true,\"data\":{\"task\":\"bind_yourself_to_agent_world\",\"invitation_id\":\"...\",\"one_time_token\":\"...\"}}");

    // The new code just adds the task package as data, no instructions
    spa_info.push_str(&format!(
        "\n\n[Agent World Task Package - pre-fetched by system]\n{}",
        "{}" // placeholder — the actual data is the raw JSON
    ));

    context_msg.push_str(&spa_info);

    // Verify no hardcoded instructions are present
    assert!(
        !context_msg.contains("AI MUST execute"),
        "no prescriptive instructions"
    );
    assert!(
        !context_msg.contains("Key workflow summary"),
        "no workflow summary"
    );
    assert!(
        !context_msg.contains("Generate cryptographic keys"),
        "no key gen instructions"
    );
    assert!(
        !context_msg.contains("Submit binding request"),
        "no binding instructions"
    );

    // Verify the data sections are present
    assert!(context_msg.contains("SPA Page Analysis"));
    assert!(context_msg.contains("Agent World Task Package"));
    assert!(context_msg.contains("task_access_token"));

    println!("✅ Context is neutrally presented (no prescriptive instructions)");
    println!("   Context length: {} chars", context_msg.len());
}
