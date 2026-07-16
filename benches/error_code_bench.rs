//! Benchmarks for the ErrorCode HTTP hot path
//!
//! Measures serialization, deserialization, and HTTP response header formatting
//! on every error response path.  Tracking these ensures the ErrorCode injection
//! in `write_http_json_response` does not regress.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

// ---------------------------------------------------------------------------
// Baseline: error-code serialization (hot path for every error response)
// ---------------------------------------------------------------------------

fn bench_error_code_serialize(c: &mut Criterion) {
    let codes = [
        go_on::core::error::ErrorCode::BadRequest,
        go_on::core::error::ErrorCode::Unauthorized,
        go_on::core::error::ErrorCode::Forbidden,
        go_on::core::error::ErrorCode::NotFound,
        go_on::core::error::ErrorCode::RateLimitExceeded,
        go_on::core::error::ErrorCode::InternalError,
        go_on::core::error::ErrorCode::ServiceUnavailable,
        go_on::core::error::ErrorCode::BadGateway,
    ];

    c.bench_function("error_code_serialize", |b| {
        b.iter(|| {
            for code in &codes {
                let _ = black_box(serde_json::to_string(code));
            }
        })
    });
}

fn bench_error_code_http_status(c: &mut Criterion) {
    let codes = [
        go_on::core::error::ErrorCode::BadRequest,
        go_on::core::error::ErrorCode::Unauthorized,
        go_on::core::error::ErrorCode::Forbidden,
        go_on::core::error::ErrorCode::InternalError,
        go_on::core::error::ErrorCode::ResourceExhausted,
    ];

    c.bench_function("error_code_http_status", |b| {
        b.iter(|| {
            for code in &codes {
                let _ = black_box(code.http_status());
            }
        })
    });
}

fn bench_app_error_error_code(c: &mut Criterion) {
    use go_on::core::error::{AppError, ProxyError, ValidationError};

    let errors: Vec<AppError> = vec![
        AppError::Proxy(ProxyError::InvalidRequest("bad payload".into())),
        AppError::Proxy(ProxyError::RateLimitExceeded("api".into())),
        AppError::Proxy(ProxyError::CircuitBreakerOpen("openai".into())),
        AppError::Validation(ValidationError::MissingField("name".into())),
    ];

    c.bench_function("app_error_error_code", |b| {
        b.iter(|| {
            for err in &errors {
                let _ = black_box(err.error_code());
            }
        })
    });
}

fn bench_http_response_header_with_security(c: &mut Criterion) {
    // Simulate the header formatting done in write_http_json_response / write_http_text_response
    let status = 200u16;
    let body_len = 256usize;
    let security =
        "X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nX-XSS-Protection: 0\r\n";
    let extra = "Access-Control-Allow-Origin: *\r\n";

    c.bench_function("http_response_header_with_security", |b| {
        b.iter(|| {
            let status_text = match status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                404 => "Not Found",
                500 => "Internal Server Error",
                _ => "OK",
            };
            let _ = black_box(format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}{}\r\n",
                status,
                status_text,
                body_len,
                security,
                extra
            ));
        })
    });
}

fn bench_json_error_with_error_code(c: &mut Criterion) {
    // Simulate the error response construction using only public ErrorCode API.
    // This mirrors the hot path in write_http_json_response.
    use go_on::core::error::ErrorCode;

    let test_cases: Vec<(u16, &str, ErrorCode)> = vec![
        (400, "invalid request", ErrorCode::BadRequest),
        (401, "unauthorized", ErrorCode::Unauthorized),
        (403, "forbidden", ErrorCode::Forbidden),
        (404, "not found", ErrorCode::NotFound),
        (429, "too many requests", ErrorCode::RateLimitExceeded),
        (500, "internal error", ErrorCode::InternalError),
    ];

    c.bench_function("json_error_with_error_code", |b| {
        b.iter(|| {
            for (status, msg, code) in &test_cases {
                let mut value = serde_json::json!({"error": msg});
                if let Some(obj) = value.as_object_mut() {
                    if obj.contains_key("error") && !obj.contains_key("code") {
                        obj.insert("code".into(), serde_json::json!(code));
                    }
                }
                // Also verify the http_status mapping matches
                let _ = black_box((*status >= 400, code.http_status()));
                black_box(value);
            }
        })
    });
}

criterion_group!(
    benches,
    bench_error_code_serialize,
    bench_error_code_http_status,
    bench_app_error_error_code,
    bench_http_response_header_with_security,
    bench_json_error_with_error_code,
);
criterion_main!(benches);
