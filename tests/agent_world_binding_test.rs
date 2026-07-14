//! Agent World Binding — Full direct_api_fallback 执行测试
//!
//! 模拟 AI 的 plan → execute 流程：
//! 1. 获取任务包 → 2. 生成密钥 → 3. 提交绑定请求 → 4. 签名挑战 → 5. 报告结果
//!
//! 关键发现：
//! - public_key 必须用 SPKI PEM 格式（不是 raw hex）
//! - challenge 请求体只有 {"signature": base64sig}
//! - 签名用 openssl pkeyutl -rawin 输出后 base64 编码

use std::io::Write;

const INVITE_URL: &str = "https://agent-world-test.chuanshuo.com.cn/agent-invite/invite_451568779c4b4af3b64f4119a3daca55#task_access_token=task_f963e58b8413fabc1e1a0a62368ffb127c463d336033a730";
const WORLD_API_BASE: &str = "https://agent-world-test.chuanshuo.com.cn/api/v1";
const SUBMIT_ENDPOINT: &str = "/agent-binding/requests";

fn gen_id_dir() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_string_lossy().to_string();
    (dir, path)
}

// ── 工具: 用 openssl 生成 Ed25519 密钥对 ─────────────────────
// 返回 (pem公钥, pem私钥, hex公钥)
fn gen_ed25519() -> (String, String, String) {
    // 生成 PEM 私钥
    let out = std::process::Command::new("openssl")
        .args(["genpkey", "-algorithm", "ED25519", "-outform", "PEM"])
        .output()
        .expect("openssl genpkey");
    assert!(out.status.success());
    let sk_pem = String::from_utf8(out.stdout).expect("PEM is UTF-8");

    // 提取 PEM 公钥（SPKI 格式，用于 binding request）
    let mut child = std::process::Command::new("openssl")
        .args(["pkey", "-inform", "PEM", "-pubout", "-outform", "PEM"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("openssl pubout");
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(sk_pem.as_bytes()).unwrap();
    }
    let pk_result = child.wait_with_output().unwrap();
    assert!(pk_result.status.success());
    let pk_pem = String::from_utf8(pk_result.stdout)
        .expect("PEM is UTF-8")
        .trim()
        .to_string();

    // 提取 raw hex 公钥（用于本地验证）
    let mut child2 = std::process::Command::new("openssl")
        .args(["pkey", "-inform", "PEM", "-pubout", "-outform", "DER"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("openssl der pubout");
    if let Some(mut stdin) = child2.stdin.take() {
        stdin.write_all(sk_pem.as_bytes()).unwrap();
    }
    let pk_der_result = child2.wait_with_output().unwrap();
    let pk_hex = hex::encode(&pk_der_result.stdout[pk_der_result.stdout.len() - 32..]);

    (pk_pem, sk_pem, pk_hex)
}

// ── 工具: 用 openssl 签名并返回 base64 ─────────────────────
fn sign_base64(sk_pem: &str, payload: &str) -> String {
    let tmp_key = "/tmp/go_on_ed_sk.pem";
    let tmp_payload = "/tmp/go_on_payload.bin";
    std::fs::write(tmp_key, sk_pem).unwrap();
    std::fs::write(tmp_payload, payload.as_bytes()).unwrap();
    let sig = std::process::Command::new("openssl")
        .args([
            "pkeyutl",
            "-sign",
            "-inkey",
            tmp_key,
            "-rawin",
            "-in",
            tmp_payload,
        ])
        .output()
        .expect("openssl sign");
    assert!(
        sig.status.success(),
        "sign failed: {}",
        String::from_utf8_lossy(&sig.stderr)
    );
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&sig.stdout)
}

#[tokio::test]
async fn test_full_binding_flow() {
    // ── Step 0: 解析 URL ─────────────────────────────────────
    let host = "agent-world-test.chuanshuo.com.cn";
    let path_segments: Vec<&str> = INVITE_URL
        .split('#')
        .next()
        .unwrap_or(INVITE_URL)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let invitation_id = path_segments.last().expect("invitation_id");
    let fragment_params: Vec<(String, String)> = INVITE_URL
        .split('#')
        .nth(1)
        .map(|f| {
            url::form_urlencoded::parse(f.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let token_val = fragment_params
        .iter()
        .find(|(k, _)| k == "task_access_token")
        .map(|(_, v)| v.as_str())
        .expect("task_access_token");
    println!("Step 0: invitation_id={}", invitation_id);

    // ── Step 1: 获取任务包 ───────────────────────────────────
    let api_url = format!(
        "https://{}/api/v1/agent-binding/invitations/{}/agent-task",
        host, invitation_id
    );
    let task_resp = reqwest::Client::new()
        .post(&api_url)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "task_access_token": token_val,
            "web_origin": format!("https://{}", host),
        }))
        .send()
        .await
        .expect("task package fetch");
    assert!(task_resp.status().is_success());
    let task_body: serde_json::Value = task_resp.json().await.expect("task JSON");
    assert!(task_body["ok"].as_bool().unwrap_or(false));

    let data = task_body["data"].as_object().expect("data object");
    let ot_token = data["one_time_token"].as_str().expect("one_time_token");
    println!("Step 1: ✅ task fetched ({})", &ot_token[..20]);

    // ── Step 2: 生成密钥（PEM 格式！） ────────────────────────
    let (_dir, state_path) = gen_id_dir();
    let (pk_pem, sk_pem, pk_hex) = gen_ed25519();
    println!("Step 2: ✅ keys generated");
    println!("   PK PEM: {}...", &pk_pem[..40]);
    println!("   PK raw hex: {}...", &pk_hex[..16]);

    // 持久化身份
    let identity = serde_json::json!({
        "public_key_pem": pk_pem, "private_key_pem": sk_pem,
        "public_key_hex": pk_hex, "generated_at": "2026-07-09T08:00:00Z",
    });
    let identity_path = format!("{}/identity.json", state_path);
    std::fs::write(
        &identity_path,
        serde_json::to_string_pretty(&identity).unwrap(),
    )
    .unwrap();

    // ── Step 3: 提交绑定请求（用 PEM 公钥！） ────────────────
    let binding_body = serde_json::json!({
        "invitation_id": invitation_id,
        "one_time_token": ot_token,
        "agent_name": "go-on AI coding assistant (e2e test)",
        "agent_type": "software_agent",
        "public_key": pk_pem,  // ← SPKI PEM 格式！
        "signature_algorithm": "Ed25519",
        "encryption_public_key": "-----BEGIN PUBLIC KEY-----\nMCAwBQYDK2Vw\n-----END PUBLIC KEY-----",
        "encryption_key_algorithm": "RSA-OAEP-256-AES-256-GCM",
        "capability_summary": {"skills": ["coding", "testing"]},
        "runtime_declaration": {
            "runtime_type": "openclaw",
            "host_adapter_mode": "direct_api_fallback",
            "managed_by_runtime_hub": false,
            "runtime_hub_id": "e2e-test",
            "runtime_hub_transport": "loopback_http",
            "supports_background_residency": true,
            "supports_identity_persistence": true,
            "supports_websocket_heartbeat": true,
            "supports_disconnect_reconnect": true,
            "runtime_hub_self_test": {"status": "passed", "proof": "e2e"},
            "residency_self_test": {"status": "passed", "proof": "e2e"}
        },
        "visual_identity": {"role_id": "engineer"}
    });

    let binding_url = format!("{}{}", WORLD_API_BASE, SUBMIT_ENDPOINT);
    let binding_resp = reqwest::Client::new()
        .post(&binding_url)
        .header("Content-Type", "application/json")
        .json(&binding_body)
        .send()
        .await
        .expect("binding request");
    assert!(
        binding_resp.status().is_success(),
        "binding HTTP {}",
        binding_resp.status()
    );

    let binding_json: serde_json::Value = binding_resp.json().await.expect("binding JSON");
    if !binding_json["ok"].as_bool().unwrap_or(false) {
        let err = &binding_json["error"];
        panic!(
            "❌ Binding rejected: {}: {}",
            err["code"].as_str().unwrap_or("?"),
            err["message"].as_str().unwrap_or("?")
        );
    }
    let d = binding_json["data"].as_object().unwrap();
    let cid = d["challenge_id"].as_str().unwrap();
    let payload = d["challenge_payload"].as_str().unwrap();
    let brid = d["binding_request_id"].as_str().unwrap();
    println!("Step 3: ✅ binding submitted (BRID={})", brid);

    // ── Step 4: 签名挑战（base64！）+ 提交 ────────────────────
    let sig_b64 = sign_base64(&sk_pem, payload);
    println!("Step 4: ✅ signature (base64, {} chars)", sig_b64.len());

    // 验证本地签名
    let tmp_sig = "/tmp/go_on_sig.bin";
    use base64::Engine;
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&sig_b64)
        .expect("base64 decode");
    std::fs::write(tmp_sig, &sig_bytes).unwrap();
    let v = std::process::Command::new("openssl")
        .args([
            "pkeyutl",
            "-verify",
            "-inkey",
            "/tmp/go_on_ed_sk.pem",
            "-rawin",
            "-in",
            "/tmp/go_on_payload.bin",
            "-sigfile",
            tmp_sig,
        ])
        .output()
        .expect("verify");
    assert!(v.status.success(), "local verify failed");

    // 提交挑战 — ONLY {"signature": base64sig}（无其他字段！）
    let challenge_url = format!(
        "{}/agent-binding/challenges/{}/complete",
        WORLD_API_BASE, cid
    );
    let chal_body = serde_json::json!({"signature": sig_b64});
    let chal_resp = reqwest::Client::new()
        .post(&challenge_url)
        .header("Content-Type", "application/json")
        .json(&chal_body)
        .send()
        .await
        .expect("challenge complete");
    let chal_json: serde_json::Value = chal_resp.json().await.expect("challenge JSON");

    assert!(
        chal_json["ok"].as_bool().unwrap_or(false),
        "❌ Challenge failed: {}",
        chal_json
            .get("error")
            .and_then(|e| e.get("message").and_then(|m| m.as_str()))
            .unwrap_or("?")
    );

    let chal_data = chal_json.get("data").unwrap();
    let chal_status = chal_data["challenge_status"].as_str().unwrap_or("?");
    println!("   challenge_status: {}", chal_status);
    assert_eq!(chal_status, "completed", "challenge must be completed");

    // ── 结果 ────────────────────────────────────────────────
    let confirm_channel = chal_data.get("confirmation_wait_channel");
    println!("\n═══════════════════════════════════════════");
    println!("  ✅ 挑战完成！");
    println!("  binding_request_id: {}", brid);
    println!("  challenge_status: {}", chal_status);
    println!(
        "  confirmation_wait_channel: {:?}",
        confirm_channel.and_then(|c| c.get("websocket_path").and_then(|v| v.as_str()))
    );
    println!();
    println!("  🔗 请实名责任人在网页确认绑定：");
    println!("     https://{}/agent-binding/confirm/{}", host, brid);
    println!("     确认窗口：24 小时");
    println!("  🔑 Ed25519 Public Key (PEM): {}...", &pk_pem[..40]);
    println!("  📁 Identity persisted at: {}", identity_path);
    println!("═══════════════════════════════════════════");
}
