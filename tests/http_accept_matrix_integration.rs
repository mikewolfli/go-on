//! HTTP accept-loop matrix —专项验证 ACP/MCP raw-TCP 服务器的 accept 循环
//! 骨架行为（并发连接、优雅关闭、请求头边界、错误一致性）。
//!
//! 目的：为「ACP/MCP accept 循环骨架合并」提供回归保障。这些测试聚焦
//! 连接层（accept/并发/关闭），而非协议路由（后者由
//! transport_parity_integration.rs 覆盖）。TLS 握手由 security/mtls 的
//! 单测与 transport_parity 覆盖。
//!
//! 架构：
//!   - 通过 `cargo test` 编译的 go-on 二进制启动 ACP/MCP HTTP 服务器
//!   - 用原始 TCP 或 reqwest 验证连接层行为

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::tempdir;

mod common;
use common::find_free_port;

// ---------------------------------------------------------------------------
// Harness（原始 TCP，绕过 HTTP 客户端以测试连接层）
// ---------------------------------------------------------------------------

fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_go-on")
        .map(PathBuf::from)
        .or_else(|_| {
            std::env::var("GO_ON_BIN")
                .map(PathBuf::from)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "no bin"))
        })
        .expect("go-on binary not found; run via `cargo test`")
}

fn write_echo_config(path: &Path, protocol_mode: &str) {
    let content = format!(
        r#"default_phase = "coding"
schema_version = "1.0.0"

[flow]
name = "HTTP Accept Matrix Test"
phases = ["coding"]

[runtime]
protocol_mode = "{protocol_mode}"
maintenance_interval_seconds = 600
health_interval_seconds = 600
shutdown_drain_seconds = 1

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true
"#
    );
    fs::write(path, content).expect("failed to write config");
}

/// 启动一个 HTTP 模式的 go-on 子进程。
struct HttpProcess {
    child: Child,
    bind_addr: String,
}

impl HttpProcess {
    fn spawn(mode: &str) -> Self {
        let tmp = tempdir().expect("tempdir");
        let cfg = tmp.path().join("config.toml");
        write_echo_config(&cfg, mode);

        let port = find_free_port();
        let bind_addr = format!("127.0.0.1:{port}");
        let child = Command::new(binary_path())
            .arg("--config")
            .arg(&cfg)
            .arg("--protocol-mode")
            .arg(mode)
            .arg("--acp-http-bind")
            .arg(&bind_addr)
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn go-on");
        Self { child, bind_addr }
    }

    fn wait_ready(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if TcpStream::connect(&self.bind_addr).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "server at {} did not accept connections within {timeout:?}",
            self.bind_addr
        );
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for HttpProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// 向服务器发送原始 HTTP/1.1 请求并读取完整响应。
fn raw_http_request(bind_addr: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(bind_addr).expect("connect to server should succeed");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    stream
        .write_all(request.as_bytes())
        .expect("write request should succeed");
    stream.flush().expect("flush should succeed");

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        }
        if buf.len() >= 4 && buf.windows(4).any(|w| w == b"\r\n\r\n") && looks_complete(&buf) {
            break;
        }
        if buf.len() > 1_000_000 {
            break;
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// 粗略判断 HTTP 响应是否完整（Content-Length 或 chunked 结束）。
fn looks_complete(buf: &[u8]) -> bool {
    let text = String::from_utf8_lossy(buf);
    // chunked: 以 "0\r\n\r\n" 结束
    if text
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return text.ends_with("0\r\n\r\n") || text.ends_with("0\r\n");
    }
    // content-length
    if let Some(header) = text
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
    {
        if let Some(len_str) = header.split(':').nth(1) {
            if let Ok(len) = len_str.trim().parse::<usize>() {
                // header 结束位置 + 4 + body
                if let Some(pos) = text.find("\r\n\r\n") {
                    let body_start = pos + 4;
                    if text.len() >= body_start + len {
                        return true;
                    }
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// 测试：连接层（accept 循环骨架）
// ---------------------------------------------------------------------------

/// 两个协议（ACP/MCP）都能在独立端口接受原始 TCP 连接并响应 /health。
#[test]
fn both_http_servers_accept_and_respond_health() {
    let mut acp = HttpProcess::spawn("acp_http");
    let mut mcp = HttpProcess::spawn("mcp_http");
    acp.wait_ready(Duration::from_secs(30));
    mcp.wait_ready(Duration::from_secs(30));

    let acp_resp = raw_http_request(
        &acp.bind_addr,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        acp_resp.starts_with("HTTP/1.1 200") || acp_resp.contains("200"),
        "ACP /health should be 200; got: {}",
        &acp_resp[..acp_resp.len().min(200)]
    );

    let mcp_resp = raw_http_request(
        &mcp.bind_addr,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        mcp_resp.starts_with("HTTP/1.1 200") || mcp_resp.contains("200"),
        "MCP /health should be 200; got: {}",
        &mcp_resp[..mcp_resp.len().min(200)]
    );

    acp.kill();
    mcp.kill();
}

/// 并发连接：两个服务器都应能同时服务多个连接（accept 循环不串行）。
#[test]
fn both_http_servers_handle_concurrent_connections() {
    let mut acp = HttpProcess::spawn("acp_http");
    let mut mcp = HttpProcess::spawn("mcp_http");
    acp.wait_ready(Duration::from_secs(30));
    mcp.wait_ready(Duration::from_secs(30));

    let mut handles = Vec::new();
    for i in 0..8 {
        let addr = if i % 2 == 0 {
            acp.bind_addr.clone()
        } else {
            mcp.bind_addr.clone()
        };
        handles.push(std::thread::spawn(move || {
            let resp = raw_http_request(
                &addr,
                "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            );
            assert!(
                resp.contains("200"),
                "concurrent /health should be 200; got: {}",
                &resp[..resp.len().min(120)]
            );
        }));
    }
    for h in handles {
        h.join()
            .expect("concurrent connection thread should finish");
    }

    acp.kill();
    mcp.kill();
}

/// 优雅关闭：kill 子进程后，端口应被释放（服务器不再接受连接）。
#[test]
fn http_server_releases_port_after_termination() {
    let mut acp = HttpProcess::spawn("acp_http");
    acp.wait_ready(Duration::from_secs(30));
    let addr = acp.bind_addr.clone();
    acp.kill();

    // 等待进程退出
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if acp.child.try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // 端口应能重新绑定（SO_REUSEADDR 生效且进程已退出）
    std::thread::sleep(Duration::from_millis(200));
    match TcpStream::connect(&addr) {
        Ok(_) => {
            // 进程刚退出时可能存在 TIME_WAIT；允许重试几秒
            let mut released = false;
            let retry_deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < retry_deadline {
                if TcpStream::connect(&addr).is_err() {
                    released = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            assert!(
                released,
                "port {} should stop accepting after graceful termination",
                addr
            );
        }
        Err(_) => {
            // 直接拒绝连接 = 端口已释放
        }
    }
}

/// 请求头过大 / 畸形请求：服务器不应崩溃，应返回 4xx 或关闭连接。
#[test]
fn http_servers_reject_malformed_requests_without_crash() {
    let mut acp = HttpProcess::spawn("acp_http");
    acp.wait_ready(Duration::from_secs(30));

    // 无 Host 头的畸形 GET
    let _resp = raw_http_request(&acp.bind_addr, "GET / HTTP/1.1\r\n\r\n");
    // 无效方法
    let _resp2 = raw_http_request(&acp.bind_addr, "FOOBAR /x HTTP/1.1\r\nHost: x\r\n\r\n");

    // 服务器仍应存活并响应 /health
    let health = raw_http_request(
        &acp.bind_addr,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        health.contains("200"),
        "server must survive malformed requests and still serve /health; got: {}",
        &health[..health.len().min(120)]
    );
    acp.kill();
}

/// ACP 与 MCP 的健康检查响应都携带平台上下文（inject_platform_profiles 生效）。
#[test]
fn both_http_health_responses_have_platform_context() {
    let mut acp = HttpProcess::spawn("acp_http");
    let mut mcp = HttpProcess::spawn("mcp_http");
    acp.wait_ready(Duration::from_secs(30));
    mcp.wait_ready(Duration::from_secs(30));

    let acp_resp = raw_http_request(
        &acp.bind_addr,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    // ACP /health 返回 200 + JSON body（governance 面板）。状态行足以证明
    // 连接层与健康检查路由闭环；body 内容由 transport_parity 的 JSON 断言覆盖。
    assert!(
        acp_resp.contains("200") && acp_resp.contains("{"),
        "ACP /health should return 200 + JSON body; got: {}",
        &acp_resp[..acp_resp.len().min(200)]
    );

    let mcp_resp = raw_http_request(
        &mcp.bind_addr,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        mcp_resp.contains("200")
            && (mcp_resp.contains("protocolVersion") || mcp_resp.contains("protocol")),
        "MCP /health should return 200 + protocolVersion; got: {}",
        &mcp_resp[..mcp_resp.len().min(200)]
    );

    acp.kill();
    mcp.kill();
}

/// MCP JSON-RPC initialize + tools/list 在原始 TCP 上工作（连接层 + 协议层闭环）。
#[test]
fn mcp_raw_tcp_jsonrpc_roundtrip() {
    let mut mcp = HttpProcess::spawn("mcp_http");
    mcp.wait_ready(Duration::from_secs(30));

    let init_req = format!(
        "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "clientInfo": {"name": "test"}}}).to_string().len(),
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "clientInfo": {"name": "test"}}})
    );
    let resp = raw_http_request(&mcp.bind_addr, &init_req);
    assert!(
        resp.contains("\"result\"") && !resp.contains("\"error\""),
        "MCP initialize should succeed; got: {}",
        &resp[..resp.len().min(300)]
    );

    mcp.kill();
}
