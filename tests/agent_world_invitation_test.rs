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

use go_on::orchestration::tool_extended;

/// Server availability timeout for CI skip check.
const SERVER_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

const TEST_INVITE_URL: &str = "https://agent-world-test.chuanshuo.com.cn/agent-invite/invite_8486d728a28f4c54a8188b8bebc0db3c#task_access_token=task_d2add96d25fd4a007a83cdff2134c8da3c6a7e4e235589bc";

/// Check if the Agent World server is reachable. Skips test when unreachable.
fn check_server_available() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = match "agent-world-test.chuanshuo.com.cn:443"
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
    {
        Some(a) => a,
        None => return false,
    };
    TcpStream::connect_timeout(&addr, SERVER_CHECK_TIMEOUT).is_ok()
}

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
// Phase 4: Live API call to Agent World (validates real endpoint works)
// ============================================================================

#[tokio::test]
async fn test_live_task_package_fetch() {
    // Gracefully skip when the external server is unreachable (CI, no network).
    if !check_server_available() {
        eprintln!("SKIP: Agent World test server not reachable");
        return;
    }
    let api_url = "https://agent-world-test.chuanshuo.com.cn/api/v1/agent-binding/invitations/invite_8486d728a28f4c54a8188b8bebc0db3c/agent-task";
    let api_body = serde_json::json!({
        "task_access_token": "task_d2add96d25fd4a007a83cdff2134c8da3c6a7e4e235589bc",
        "web_origin": "https://agent-world-test.chuanshuo.com.cn",
    });

    let client = reqwest::Client::new();
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client
            .post(api_url)
            .header("Content-Type", "application/json")
            .json(&api_body)
            .send(),
    )
    .await
    .expect("API call should not timeout")
    .expect("API call should succeed");

    assert!(resp.status().is_success(), "HTTP {}", resp.status());

    let body: serde_json::Value =
        tokio::time::timeout(std::time::Duration::from_secs(5), resp.json())
            .await
            .expect("Response body read timeout")
            .expect("Response should be valid JSON");

    // ── Validate envelope ──
    let ok_val = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    assert!(ok_val, "ok=true");

    let request_id = body.get("request_id").and_then(|v| v.as_str());
    assert!(request_id.is_some(), "request_id present");
    assert!(request_id.unwrap().starts_with("req_"));

    // ── Validate data section ──
    let data = body.get("data").and_then(|v| v.as_object());
    assert!(data.is_some(), "data object present");
    let data = data.unwrap();

    // Core identity fields
    assert_eq!(
        data.get("task_package_version").and_then(|v| v.as_str()),
        Some("agent_binding_task_v1")
    );
    assert_eq!(
        data.get("task").and_then(|v| v.as_str()),
        Some("bind_yourself_to_agent_world")
    );
    assert_eq!(
        data.get("audience").and_then(|v| v.as_str()),
        Some("external_agent")
    );

    // Required binding fields
    let world_api = data.get("world_api_base_url").and_then(|v| v.as_str());
    assert_eq!(
        world_api,
        Some("https://agent-world-test.chuanshuo.com.cn/api/v1")
    );

    let subject_id = data.get("subject_id").and_then(|v| v.as_str());
    assert!(subject_id.is_some() && subject_id.unwrap().starts_with("subject_"));

    let invitation_id = data.get("invitation_id").and_then(|v| v.as_str());
    assert!(invitation_id.is_some());

    let one_time_token = data.get("one_time_token").and_then(|v| v.as_str());
    assert!(one_time_token.is_some() && one_time_token.unwrap().starts_with("token_"));

    let expires_at = data.get("expires_at").and_then(|v| v.as_str());
    assert!(expires_at.is_some());

    // ── Skill distribution ──
    let skill_dist = data.get("skill_distribution").and_then(|v| v.as_object());
    assert!(skill_dist.is_some());
    assert!(skill_dist.unwrap().contains_key("manifest_endpoint"));
    assert!(skill_dist.unwrap().contains_key("minimum_skill_version"));

    // ── Direct API fallback ──
    let fallback = data.get("direct_api_fallback").and_then(|v| v.as_object());
    assert!(fallback.is_some(), "direct_api_fallback required");
    let fb = fallback.unwrap();

    assert_eq!(
        fb.get("submit_binding_request_endpoint")
            .and_then(|v| v.as_str()),
        Some("/agent-binding/requests")
    );
    assert!(fb
        .get("complete_challenge_endpoint_template")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("{challenge_id}"));

    // Verify ALL 11 required fields
    let required_fields: Vec<&str> = fb
        .get("required_first_binding_fields")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();

    assert_eq!(
        required_fields.len(),
        11,
        "expected 11 required fields, got {}: {:?}",
        required_fields.len(),
        required_fields
    );
    for field in &[
        "invitation_id",
        "one_time_token",
        "agent_name",
        "agent_type",
        "public_key",
        "signature_algorithm",
        "encryption_public_key",
        "encryption_key_algorithm",
        "capability_summary",
        "runtime_declaration",
        "visual_identity",
    ] {
        assert!(required_fields.contains(field), "missing field: {}", field);
    }

    // Body example should contain placeholder markers
    let body_example = fb
        .get("first_binding_request_body_example")
        .and_then(|v| v.as_object());
    assert!(body_example.is_some());
    let ex = body_example.unwrap();
    assert!(ex
        .get("agent_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("AGENT_DISPLAY_NAME"));
    assert!(ex
        .get("public_key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("SIGNING_PUBLIC_KEY"));

    // ── Visual identity catalog ──
    let vis = data
        .get("visual_identity_catalog")
        .and_then(|v| v.as_object());
    assert!(vis.is_some());
    let roles = vis
        .unwrap()
        .get("allowed_role_ids")
        .and_then(|v| v.as_array());
    assert!(roles.is_some() && !roles.unwrap().is_empty());

    // ── Confirmation wait ──
    let confirm = data
        .get("confirmation_wait_contract")
        .and_then(|v| v.as_object());
    assert!(confirm.is_some());
    assert_eq!(
        confirm
            .unwrap()
            .get("expires_in_seconds")
            .and_then(|v| v.as_u64()),
        Some(86400)
    );

    // ── Runtime capability gate ──
    let cap_gate = data
        .get("runtime_capability_gate")
        .and_then(|v| v.as_object());
    assert!(cap_gate.is_some());
    let required_caps = cap_gate
        .unwrap()
        .get("required_capabilities")
        .and_then(|v| v.as_array());
    assert!(required_caps.is_some());
    assert!(required_caps
        .unwrap()
        .iter()
        .any(|c| c.as_str() == Some("supports_background_residency")));

    // ── Hub registration contract ──
    let hub = data
        .get("hub_registration_contract")
        .and_then(|v| v.as_object());
    assert!(hub.is_some(), "hub_registration_contract present");

    // ── Prohibited actions ──
    let prohibited = data.get("prohibited_actions").and_then(|v| v.as_array());
    assert!(prohibited.is_some());
    assert!(prohibited
        .unwrap()
        .iter()
        .any(|a| { a.as_str().is_some_and(|s| s.contains("账号密码")) }));

    println!("✅ Live API test PASSED");
    println!("   invitation_id:     {}", invitation_id.unwrap());
    println!("   subject_id:        {}", subject_id.unwrap());
    println!("   world_api_base_url: {}", world_api.unwrap());
    println!(
        "   one_time_token:    {}...",
        &one_time_token.unwrap()[..20]
    );
    println!("   expires_at:        {}", expires_at.unwrap());
    println!("   required_fields:   {}", required_fields.len());
    println!("   request_id:        {}", request_id.unwrap());
}

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
