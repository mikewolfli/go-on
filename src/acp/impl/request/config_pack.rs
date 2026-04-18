use super::*;
use crate::protocol::access_mode::{normalize_protocol_mode, resolve_access_selection};

pub(super) fn governance_rule_fingerprint(config_path: Option<&str>) -> Value {
    let base_dir = config_path
        .map(PathBuf::from)
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let mut files = Vec::new();

    let shared = [
        base_dir.join("RULES.md"),
        base_dir.join("RULES").join("global.md"),
        base_dir.join("RULES").join("common.md"),
        base_dir.join("RULES").join("local.md"),
    ];

    for path in shared {
        if let Some(item) = build_rule_file_info(&base_dir, &path) {
            files.push(item);
        }
    }

    let rules_dir = base_dir.join("RULES");
    if let Ok(read_dir) = fs::read_dir(&rules_dir) {
        let mut dynamic_paths = read_dir
            .filter_map(|entry| entry.ok().map(|item| item.path()))
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
            .collect::<Vec<_>>();
        dynamic_paths.sort();

        for path in dynamic_paths {
            if let Some(item) = build_rule_file_info(&base_dir, &path) {
                let already_known = files
                    .iter()
                    .any(|existing| existing.get("path") == item.get("path"));
                if !already_known {
                    files.push(item);
                }
            }
        }
    }

    let file_count = files.len() as u64;
    let total_bytes = files
        .iter()
        .filter_map(|item| item.get("size_bytes").and_then(Value::as_u64))
        .sum::<u64>();
    let latest_mtime_ts = files
        .iter()
        .filter_map(|item| item.get("mtime_ts").and_then(Value::as_i64))
        .max()
        .unwrap_or(0);

    json!({
        "version": format!("r{}-{}-{}", file_count, latest_mtime_ts, total_bytes),
        "file_count": file_count,
        "latest_mtime_ts": latest_mtime_ts,
        "total_bytes": total_bytes,
        "files": files,
    })
}

fn build_rule_file_info(base_dir: &Path, path: &Path) -> Option<Value> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok();
    let mtime_ts = modified
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);

    let relative = path
        .strip_prefix(base_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    Some(json!({
        "path": relative,
        "size_bytes": metadata.len(),
        "mtime_ts": mtime_ts,
    }))
}

pub(super) fn governance_config_summary(config_path: Option<&str>) -> Value {
    let Some(config_path) = config_path else {
        return json!({
            "loaded": false,
            "production_strict": false,
            "entry_auth_enabled": false,
            "entry_auth_api_key_env": "GO_ON_ENTRY_API_KEY",
            "entry_auth_key_configured": false,
            "entry_rate_limit_rpm": 240,
            "entry_rate_limit_burst": 60,
            "strict_violation_count": 0,
            "strict_violations": [],
            "warning_count": 0,
            "warnings": [],
        });
    };

    let config_path_buf = PathBuf::from(config_path);
    let config = match AppConfig::load(&config_path_buf) {
        Ok(config) => config,
        Err(err) => {
            return json!({
                "loaded": false,
                "production_strict": false,
                "entry_auth_enabled": false,
                "entry_auth_api_key_env": "GO_ON_ENTRY_API_KEY",
                "entry_auth_key_configured": false,
                "entry_rate_limit_rpm": 240,
                "entry_rate_limit_burst": 60,
                "strict_violation_count": 1,
                "strict_violations": [format!("failed_to_load_config:{}", err)],
                "warning_count": 1,
                "warnings": [format!("failed_to_load_config:{}", err)],
            });
        }
    };

    let warnings = collect_config_warnings(&config_path_buf, &config);
    let strict_enabled = config
        .runtime
        .as_ref()
        .map(|runtime| runtime.production_strict)
        .unwrap_or(false);
    let entry_auth_enabled = config
        .runtime
        .as_ref()
        .map(|runtime| runtime.entry_auth_enabled)
        .unwrap_or(false);
    let entry_auth_api_key_env = config
        .runtime
        .as_ref()
        .map(|runtime| runtime.entry_auth_api_key_env.clone())
        .unwrap_or_else(|| "GO_ON_ENTRY_API_KEY".to_string());
    let entry_auth_key_configured = std::env::var(&entry_auth_api_key_env)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let entry_rate_limit_rpm = config
        .runtime
        .as_ref()
        .map(|runtime| runtime.entry_rate_limit_rpm)
        .unwrap_or(240);
    let entry_rate_limit_burst = config
        .runtime
        .as_ref()
        .map(|runtime| runtime.entry_rate_limit_burst)
        .unwrap_or(60);
    let strict_violations = collect_production_strict_violations(&config);
    json!({
        "loaded": true,
        "production_strict": strict_enabled,
        "entry_auth_enabled": entry_auth_enabled,
        "entry_auth_api_key_env": entry_auth_api_key_env,
        "entry_auth_key_configured": entry_auth_key_configured,
        "entry_rate_limit_rpm": entry_rate_limit_rpm,
        "entry_rate_limit_burst": entry_rate_limit_burst,
        "strict_violation_count": strict_violations.len(),
        "strict_violations": strict_violations,
        "warning_count": warnings.len(),
        "warnings": warnings,
    })
}

fn normalize_protocol_mode_for_baseline(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    match normalize_protocol_mode(raw).unwrap_or(lowered.as_str()) {
        "adaptive" => "adaptive".to_string(),
        "acp_stdio" | "acp_http" => "acp".to_string(),
        "mcp_stdio" | "mcp_http" => "mcp".to_string(),
        other => other.to_string(),
    }
}

fn load_config_document(config_path: &Path) -> std::result::Result<toml::Value, String> {
    let raw =
        fs::read_to_string(config_path).map_err(|err| format!("failed_to_read_config:{}", err))?;
    raw.parse::<toml::Value>()
        .map_err(|err| format!("failed_to_parse_toml:{}", err))
}

fn extract_runtime_explicit_keys(document: &toml::Value) -> HashSet<String> {
    document
        .get("runtime")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

fn extract_protocol_mode_from_protocol_table(document: &toml::Value) -> Option<String> {
    document
        .get("protocol")
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("mode"))
        .and_then(toml::Value::as_str)
        .map(|value| value.to_string())
}

fn extract_runtime_protocol_mode_legacy(document: &toml::Value) -> Option<String> {
    document
        .get("runtime")
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("protocol_mode"))
        .and_then(toml::Value::as_str)
        .map(|value| value.to_string())
}

fn detect_legacy_config_keys(document: &toml::Value) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    let mut push_item = |old_path: &str, new_path: &str, reason: &str| {
        if seen.insert(old_path.to_string()) {
            items.push(json!({
                "old_path": old_path,
                "new_path": new_path,
                "reason": reason,
            }));
        }
    };

    if let Some(runtime) = document.get("runtime").and_then(toml::Value::as_table) {
        for (old_key, new_key, reason) in [
            (
                "auth_enabled",
                "runtime.entry_auth_enabled",
                "legacy auth switch renamed",
            ),
            (
                "auth_api_key_env",
                "runtime.entry_auth_api_key_env",
                "legacy env key renamed",
            ),
            (
                "rate_limit_rpm",
                "runtime.entry_rate_limit_rpm",
                "legacy rate limit key renamed",
            ),
            (
                "rate_limit_burst",
                "runtime.entry_rate_limit_burst",
                "legacy burst key renamed",
            ),
            (
                "http_bind_addr",
                "runtime.acp_http_bind_addr",
                "legacy bind key renamed",
            ),
            (
                "strict_mode",
                "runtime.production_strict",
                "legacy strict key renamed",
            ),
            (
                "protocol_mode",
                "protocol.mode",
                "protocol mode moved from runtime to protocol table",
            ),
        ] {
            if runtime.contains_key(old_key) {
                push_item(&format!("runtime.{}", old_key), new_key, reason);
            }
        }
    }

    if document
        .as_table()
        .map(|table| table.contains_key("protocol_mode"))
        .unwrap_or(false)
    {
        push_item(
            "protocol_mode",
            "protocol.mode",
            "root-level protocol mode is deprecated",
        );
    }

    items
}

fn runtime_field_source(explicit_runtime_keys: &HashSet<String>, field: &str) -> &'static str {
    if explicit_runtime_keys.contains(field) {
        "config_file"
    } else {
        "default"
    }
}

fn resolve_protocol_source(
    server_protocol_mode: Option<&str>,
    protocol_mode_from_file: Option<&str>,
) -> &'static str {
    match (server_protocol_mode, protocol_mode_from_file) {
        (Some(server_mode), Some(file_mode)) => {
            let normalized_server = normalize_protocol_mode_for_baseline(server_mode);
            let normalized_file = normalize_protocol_mode_for_baseline(file_mode);
            if normalized_server != normalized_file {
                "cli_override"
            } else {
                "config_file"
            }
        }
        (Some(_), None) => "default",
        (None, Some(_)) => "config_file",
        (None, None) => "default",
    }
}

pub(super) async fn handle_config_reload(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let path = server
        .config_path
        .clone()
        .unwrap_or_else(|| "config.toml".to_string());
    let config_path = std::path::PathBuf::from(&path);
    let config = AppConfig::load(&config_path)?;
    let report = validate_runtime_readiness(&config_path, &config)?;
    let warnings = report.warning_messages();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "note": "flow/registry/cache/vector/autotune resources reloaded",
            "path": config_path.display().to_string(),
            "warning_count": warnings.len(),
            "warnings": warnings,
            "profile_recommendation": report.profile_recommendation,
            "recommendations": report.recommendations,
            "health": {
                "score": report.score,
                "critical_count": report.critical_count,
                "warn_count": report.warn_count,
                "info_count": report.info_count,
            }
        }),
    )
    .await
}

pub(super) async fn handle_config_baseline(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let config_summary = governance_config_summary(server.config_path.as_deref());

    let config_path = server
        .config_path
        .clone()
        .unwrap_or_else(|| "config.toml".to_string());
    let config_path_buf = PathBuf::from(&config_path);

    let mut document_warnings = Vec::new();
    let mut explicit_runtime_keys = HashSet::new();
    let mut protocol_mode_from_protocol_table = None::<String>;
    let mut protocol_mode_from_runtime_legacy = None::<String>;
    let mut legacy_mappings = Vec::new();

    match load_config_document(&config_path_buf) {
        Ok(document) => {
            explicit_runtime_keys = extract_runtime_explicit_keys(&document);
            protocol_mode_from_protocol_table =
                extract_protocol_mode_from_protocol_table(&document);
            protocol_mode_from_runtime_legacy = extract_runtime_protocol_mode_legacy(&document);
            legacy_mappings = detect_legacy_config_keys(&document);
        }
        Err(err) => {
            document_warnings.push(err);
        }
    }

    let mut explicit_runtime_fields = explicit_runtime_keys.iter().cloned().collect::<Vec<_>>();
    explicit_runtime_fields.sort();

    let protocol_mode_from_file = protocol_mode_from_protocol_table
        .clone()
        .or(protocol_mode_from_runtime_legacy.clone());
    let protocol_source = resolve_protocol_source(
        server.runtime_config.protocol_mode.as_deref(),
        protocol_mode_from_file.as_deref(),
    );
    let access_selection = resolve_access_selection(
        server.runtime_config.protocol_mode.as_deref(),
        server.runtime_config.acp_http_bind_addr.as_deref(),
    );
    let entry_auth_key_env = server.runtime_config.entry_auth_api_key_env.clone();
    let entry_auth_key_configured = std::env::var(&entry_auth_key_env)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "baseline": {
                "status": if legacy_mappings.is_empty() { "frozen" } else { "migration_required" },
                "source_precedence": ["cli_override", "env", "config_file", "default"],
                "effective": {
                    "configured_mode": access_selection.configured_mode,
                    "protocol_mode": server.runtime_config.protocol_mode.clone().unwrap_or_else(|| "adaptive".to_string()),
                    "protocol_capability": access_selection.protocol_capability.as_str(),
                    "request_dispatch_mode": access_selection.request_dispatch_mode.as_str(),
                    "startup_transport": access_selection.startup_transport.as_str(),
                    "transport_strategy": access_selection.transport_strategy,
                    "selection_reason": access_selection.selection_reason,
                    "maintenance_interval_seconds": server.runtime_config.maintenance_interval_seconds,
                    "health_interval_seconds": server.runtime_config.health_interval_seconds,
                    "shutdown_drain_seconds": server.runtime_config.shutdown_drain_seconds,
                    "acp_http_bind_addr": server.runtime_config.acp_http_bind_addr.clone(),
                    "entry_auth_enabled": server.runtime_config.entry_auth_enabled,
                    "entry_auth_api_key_env": entry_auth_key_env,
                    "entry_auth_key_configured": entry_auth_key_configured,
                    "entry_rate_limit_rpm": server.runtime_config.entry_rate_limit_rpm,
                    "entry_rate_limit_burst": server.runtime_config.entry_rate_limit_burst,
                    "production_strict": server.runtime_config.production_strict,
                    "sqlite_vacuum_interval_cycles": server.runtime_config.sqlite_vacuum_interval_cycles,
                    "trace_slow_top_n": server.runtime_config.trace_slow_top_n,
                },
                "sources": {
                    "protocol_mode": protocol_source,
                    "maintenance_interval_seconds": runtime_field_source(&explicit_runtime_keys, "maintenance_interval_seconds"),
                    "health_interval_seconds": runtime_field_source(&explicit_runtime_keys, "health_interval_seconds"),
                    "shutdown_drain_seconds": runtime_field_source(&explicit_runtime_keys, "shutdown_drain_seconds"),
                    "acp_http_bind_addr": runtime_field_source(&explicit_runtime_keys, "acp_http_bind_addr"),
                    "entry_auth_enabled": runtime_field_source(&explicit_runtime_keys, "entry_auth_enabled"),
                    "entry_auth_api_key_env": runtime_field_source(&explicit_runtime_keys, "entry_auth_api_key_env"),
                    "entry_auth_key_configured": "env",
                    "entry_rate_limit_rpm": runtime_field_source(&explicit_runtime_keys, "entry_rate_limit_rpm"),
                    "entry_rate_limit_burst": runtime_field_source(&explicit_runtime_keys, "entry_rate_limit_burst"),
                    "production_strict": runtime_field_source(&explicit_runtime_keys, "production_strict"),
                    "sqlite_vacuum_interval_cycles": runtime_field_source(&explicit_runtime_keys, "sqlite_vacuum_interval_cycles"),
                    "trace_slow_top_n": runtime_field_source(&explicit_runtime_keys, "trace_slow_top_n"),
                },
                "config": config_summary,
                "migration": {
                    "legacy_key_count": legacy_mappings.len(),
                    "legacy_keys": legacy_mappings,
                    "compatibility_window": "v0.6.x",
                    "replacement_map": [
                        {"from": "runtime.auth_enabled", "to": "runtime.entry_auth_enabled"},
                        {"from": "runtime.auth_api_key_env", "to": "runtime.entry_auth_api_key_env"},
                        {"from": "runtime.rate_limit_rpm", "to": "runtime.entry_rate_limit_rpm"},
                        {"from": "runtime.rate_limit_burst", "to": "runtime.entry_rate_limit_burst"},
                        {"from": "runtime.http_bind_addr", "to": "runtime.acp_http_bind_addr"},
                        {"from": "runtime.strict_mode", "to": "runtime.production_strict"},
                        {"from": "runtime.protocol_mode", "to": "protocol.mode"}
                    ],
                    "next_actions": [
                        "Replace deprecated keys with replacement_map equivalents",
                        "Keep only one protocol source: prefer [protocol].mode",
                        "Run config.reload and runtime.health after migration"
                    ]
                },
                "file": {
                    "path": config_path,
                    "runtime_explicit_field_count": explicit_runtime_fields.len(),
                    "runtime_explicit_fields": explicit_runtime_fields,
                    "protocol_mode_from_protocol_table": protocol_mode_from_protocol_table,
                    "protocol_mode_from_runtime_legacy": protocol_mode_from_runtime_legacy,
                    "warnings": document_warnings,
                }
            }
        }),
    )
    .await
}
