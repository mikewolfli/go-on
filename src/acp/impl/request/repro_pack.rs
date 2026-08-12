use super::*;

pub(super) fn resolve_workspace_root(config_path: Option<&str>) -> PathBuf {
    config_path
        .map(PathBuf::from)
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

fn read_git_head_commit(base_dir: &Path) -> Option<String> {
    let git_dir = base_dir.join(".git");
    let head_path = git_dir.join("HEAD");
    let head_content = fs::read_to_string(&head_path).ok()?;
    let head = head_content.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let ref_path = git_dir.join(reference.trim());
        return fs::read_to_string(ref_path)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    if head.len() >= 7 {
        return Some(head.to_string());
    }
    None
}

fn build_repro_file_info(
    base_dir: &Path,
    path: &Path,
    id: &str,
    category: &str,
    required: bool,
) -> Value {
    let relative = path
        .strip_prefix(base_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");

    let Ok(metadata) = fs::metadata(path) else {
        return json!({
            "id": id,
            "category": category,
            "path": relative,
            "required": required,
            "present": false,
        });
    };

    let modified = metadata.modified().ok();
    let mtime_ts = modified
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    let size_bytes = metadata.len();

    let (hash_fnv1a64, hash_source) = if size_bytes <= 8 * 1024 * 1024 {
        let hash = fs::read(path)
            .ok()
            .map(|bytes| fnv1a64_hex(&bytes))
            .unwrap_or_else(|| fnv1a64_hex(format!("{}:{}", size_bytes, mtime_ts).as_bytes()));
        (hash, "content")
    } else {
        (
            fnv1a64_hex(format!("{}:{}", size_bytes, mtime_ts).as_bytes()),
            "size_mtime",
        )
    };

    json!({
        "id": id,
        "category": category,
        "path": relative,
        "required": required,
        "present": true,
        "size_bytes": size_bytes,
        "mtime_ts": mtime_ts,
        "hash_fnv1a64": hash_fnv1a64,
        "hash_source": hash_source,
    })
}

pub(super) fn reproducible_build_summary(config_path: Option<&str>) -> Value {
    let base_dir = resolve_workspace_root(config_path);
    let config_snapshot = super::config_pack::governance_config_summary(config_path);

    let lock_files = vec![
        build_repro_file_info(
            &base_dir,
            &base_dir.join("Cargo.lock"),
            "cargo_lock",
            "dependency_lock",
            true,
        ),
        build_repro_file_info(
            &base_dir,
            &base_dir.join("vscode-addon").join("package-lock.json"),
            "addon_package_lock",
            "dependency_lock",
            true,
        ),
    ];

    let manifest_files = vec![
        build_repro_file_info(
            &base_dir,
            &base_dir.join("Cargo.toml"),
            "cargo_manifest",
            "manifest",
            true,
        ),
        // The GUI is a Rust crate (gui/Cargo.toml) that shares the workspace
        // Cargo.lock above — it has no package.json/package-lock.json, so the
        // previous GUI/ (uppercase) paths were never found and the report was
        // permanently "reproducible_incomplete".
        build_repro_file_info(
            &base_dir,
            &base_dir.join("gui").join("Cargo.toml"),
            "gui_manifest",
            "manifest",
            true,
        ),
        build_repro_file_info(
            &base_dir,
            &base_dir.join("vscode-addon").join("package.json"),
            "addon_manifest",
            "manifest",
            true,
        ),
    ];

    let release_artifacts = vec![
        build_repro_file_info(
            &base_dir,
            &base_dir.join("target").join("release").join("go-on.exe"),
            "binary_windows",
            "artifact",
            false,
        ),
        build_repro_file_info(
            &base_dir,
            &base_dir.join("target").join("release").join("go-on"),
            "binary_unix",
            "artifact",
            false,
        ),
        build_repro_file_info(
            &base_dir,
            &base_dir.join("gui").join("src").join("main.rs"),
            "gui_source",
            "artifact",
            false,
        ),
        build_repro_file_info(
            &base_dir,
            &base_dir
                .join("vscode-addon")
                .join("out")
                .join("extension.js"),
            "addon_extension_js",
            "artifact",
            false,
        ),
    ];

    let mut manifest = Vec::new();
    manifest.extend(lock_files.clone());
    manifest.extend(manifest_files.clone());
    manifest.extend(release_artifacts.clone());

    let required_total = manifest
        .iter()
        .filter(|item| item.get("required").and_then(Value::as_bool) == Some(true))
        .count() as u64;
    let required_present = manifest
        .iter()
        .filter(|item| {
            item.get("required").and_then(Value::as_bool) == Some(true)
                && item.get("present").and_then(Value::as_bool) == Some(true)
        })
        .count() as u64;

    let missing_required = manifest
        .iter()
        .filter(|item| {
            item.get("required").and_then(Value::as_bool) == Some(true)
                && item.get("present").and_then(Value::as_bool) != Some(true)
        })
        .filter_map(|item| item.get("path").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();

    let git_commit = option_env!("VERGEN_GIT_SHA")
        .map(str::to_string)
        .or_else(|| read_git_head_commit(&base_dir))
        .unwrap_or_else(|| "unknown".to_string());

    let rustflags = std::env::var("RUSTFLAGS").unwrap_or_else(|_| "".to_string());
    let cargo_target = std::env::var("CARGO_BUILD_TARGET").unwrap_or_else(|_| "native".to_string());
    let cargo_profile = std::env::var("CARGO_PROFILE").unwrap_or_else(|_| "dev".to_string());
    let protocol_mode =
        std::env::var("GO_ON_PROTOCOL_MODE").unwrap_or_else(|_| "from_config".to_string());

    json!({
        "version": "x9-build-repro-v1",
        "status": if missing_required.is_empty() { "reproducible_ready" } else { "reproducible_incomplete" },
        "reproducibility": {
            "required_total": required_total,
            "required_present": required_present,
            "missing_required": missing_required,
        },
        "build": {
            "package_version": env!("CARGO_PKG_VERSION"),
            "git_commit": git_commit,
            "git_commit_short": git_commit.chars().take(12).collect::<String>(),
            "build_parameters": {
                "rustflags": rustflags,
                "cargo_build_target": cargo_target,
                "cargo_profile": cargo_profile,
                "protocol_mode": protocol_mode,
            },
            "upgrade_tracks": {
                "security_patch": "weekly",
                "feature_upgrade": "biweekly",
                "rollback_policy": "pin lockfiles and restore previous release manifest",
            }
        },
        "config_snapshot": config_snapshot,
        "dependency_locks": lock_files,
        "manifests": manifest_files,
        "release_manifest": {
            "items": release_artifacts,
            "all_items": manifest,
        }
    })
}

pub(super) async fn build_repro_payload(server: &AcpServer) -> Result<Value> {
    let summary = reproducible_build_summary(server.config_path.as_deref());

    Ok(json!({
        "ok": true,
        "build": summary,
    }))
}
