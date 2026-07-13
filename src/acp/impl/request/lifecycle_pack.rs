use super::*;

fn directory_usage(path: &Path) -> (u64, u64, u64, u64) {
    let mut total_bytes = 0u64;
    let mut file_count = 0u64;
    let mut dir_count = 0u64;
    let mut unreadable_count = 0u64;

    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            unreadable_count += 1;
            continue;
        };

        for entry in entries {
            let Ok(entry) = entry else {
                unreadable_count += 1;
                continue;
            };

            let entry_path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                unreadable_count += 1;
                continue;
            };

            if metadata.is_dir() {
                dir_count += 1;
                stack.push(entry_path);
            } else if metadata.is_file() {
                file_count += 1;
                total_bytes += metadata.len();
            }
        }
    }

    (total_bytes, file_count, dir_count, unreadable_count)
}

fn waterline_status(bytes: u64, warn_bytes: u64, critical_bytes: u64) -> &'static str {
    if bytes >= critical_bytes {
        "critical"
    } else if bytes >= warn_bytes {
        "warn"
    } else {
        "ok"
    }
}

fn select_ledger_dir(base_dir: &Path) -> PathBuf {
    let candidates = [
        base_dir.join("artifacts"),
        base_dir.join("target").join("artifacts"),
        base_dir.join(".artifacts"),
    ];
    candidates
        .iter()
        .find(|path| path.exists())
        .cloned()
        .unwrap_or_else(|| base_dir.join("artifacts"))
}

fn storage_target_report(
    base_dir: &Path,
    name: &str,
    path: &Path,
    is_dir: bool,
    warn_bytes: u64,
    critical_bytes: u64,
) -> Value {
    let relative = path
        .strip_prefix(base_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    if !path.exists() {
        return json!({
            "name": name,
            "path": relative,
            "present": false,
            "is_dir": is_dir,
            "total_bytes": 0,
            "file_count": 0,
            "directory_count": 0,
            "unreadable_count": 0,
            "waterline": {
                "warn_bytes": warn_bytes,
                "critical_bytes": critical_bytes,
                "status": "ok",
            }
        });
    }

    let (total_bytes, file_count, directory_count, unreadable_count) = if is_dir {
        directory_usage(path)
    } else {
        match fs::metadata(path) {
            Ok(metadata) => (metadata.len(), 1, 0, 0),
            Err(_) => (0, 0, 0, 1),
        }
    };

    json!({
        "name": name,
        "path": relative,
        "present": true,
        "is_dir": is_dir,
        "total_bytes": total_bytes,
        "file_count": file_count,
        "directory_count": directory_count,
        "unreadable_count": unreadable_count,
        "waterline": {
            "warn_bytes": warn_bytes,
            "critical_bytes": critical_bytes,
            "status": waterline_status(total_bytes, warn_bytes, critical_bytes),
        }
    })
}

pub(super) async fn data_lifecycle_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let execute_gc = params
        .get("execute_gc")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let gc_cycle = if execute_gc {
        Some(run_maintenance_cycle(server).await?)
    } else {
        None
    };

    let maintenance = server
        .resilience
        .maintenance_tracker
        .read()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();

    let base_dir = super::repro_pack::resolve_workspace_root(server.config_path.as_deref());
    let ledger_dir = select_ledger_dir(&base_dir);

    let targets = vec![
        storage_target_report(
            &base_dir,
            "cache",
            &base_dir.join("acp_cache.sqlite3"),
            false,
            512 * 1024 * 1024,
            1024 * 1024 * 1024,
        ),
        storage_target_report(
            &base_dir,
            "vector",
            &base_dir.join("acp_vector.sqlite3"),
            false,
            1024 * 1024 * 1024,
            4 * 1024 * 1024 * 1024,
        ),
        storage_target_report(
            &base_dir,
            "ledger",
            &ledger_dir,
            true,
            256 * 1024 * 1024,
            1024 * 1024 * 1024,
        ),
    ];

    let total_bytes = targets
        .iter()
        .filter_map(|target| target.get("total_bytes").and_then(Value::as_u64))
        .sum::<u64>();

    let waterline_status = waterline_status(
        total_bytes,
        2_u64 * 1024 * 1024 * 1024,
        6_u64 * 1024 * 1024 * 1024,
    );
    let alerts = targets
        .iter()
        .filter(|target| {
            target
                .get("waterline")
                .and_then(|waterline| waterline.get("status"))
                .and_then(Value::as_str)
                .map(|status| status == "warn" || status == "critical")
                .unwrap_or(false)
        })
        .filter_map(|target| {
            let name = target.get("name").and_then(Value::as_str)?;
            let status = target
                .get("waterline")
                .and_then(|waterline| waterline.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("ok");
            Some(format!("{}:{}", name, status))
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": true,
        "lifecycle": {
            "version": "x10-data-lifecycle-v1",
            "policy": {
                "cache": {
                    "retention": "ttl_and_expired_cleanup",
                    "cleanup_frequency_seconds": server.runtime_config.maintenance_interval_seconds,
                    "archive_rule": "expired cache records removed by maintenance.gc; sqlite vacuum follows maintenance policy",
                },
                "vector": {
                    "retention": "explicit_clear_or_maintenance_vacuum",
                    "cleanup_frequency_seconds": server.runtime_config.maintenance_interval_seconds,
                    "archive_rule": "vector index compaction handled by maintenance cycle and manual vector.clear",
                },
                "ledger": {
                    "retention": "latest_snapshots_plus_auditable_history",
                    "cleanup_frequency_seconds": server.runtime_config.maintenance_interval_seconds,
                    "archive_rule": "latest pointers rotate while historical artifacts remain replayable",
                }
            },
            "storage": {
                "workspace": base_dir.to_string_lossy().to_string(),
                "targets": targets,
                "total_bytes": total_bytes,
                "waterline": {
                    "warn_bytes": 2_u64 * 1024 * 1024 * 1024,
                    "critical_bytes": 6_u64 * 1024 * 1024 * 1024,
                    "status": waterline_status,
                    "alerts": alerts,
                }
            },
            "cleanup": {
                "execute_gc": execute_gc,
                "cycle": gc_cycle.as_ref().map(|cycle| json!({
                    "memory_expired_removed": cycle.memory_expired_removed,
                })),
            },
            "audit": {
                "maintenance": maintenance,
                "replay_sequence": [
                    "data.lifecycle",
                    "maintenance.gc",
                    "runtime.health",
                    "data.lifecycle"
                ],
                "next_actions": [
                    "Check storage.waterline alerts and execute maintenance.gc when warn/critical",
                    "Run runtime.health after cleanup to verify runtime remains healthy",
                    "Archive release artifacts together with build.repro metadata for rollback traceability"
                ]
            }
        }
    }))
}
