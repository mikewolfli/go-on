use std::collections::HashSet;
use std::fs;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;

use super::*;
use crate::orchestration::skill_import::{ImportedSkillRecord, SkillImportRequest};
use crate::schema::{ImportedSkillRecordView, SkillActionResponse};

// Reuse the single-source-of-truth helper from `request::tools_pack`
// (the byte-identical private copies were removed).
use super::super::tools_pack::open_skill_import_store;

// ── Skill helper functions ──────────────────────────────────────────────

fn parse_skill_name_param(params: &Value) -> Result<String> {
    params
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: name"))
}

// `skill_import_policy` / `open_skill_import_store` live in
// `super::super::tools_pack` (single source of truth); the byte-identical
// copies previously defined here were removed.

fn normalize_imported_record(record: ImportedSkillRecord) -> Value {
    let resp = ImportedSkillRecordView {
        name: record.name,
        version: record.version,
        description: record.description,
        source: record.source,
        source_ref: record.source_ref,
        sha256: record.sha256,
        manifest_path: record.manifest_path,
        enabled: record.enabled,
        imported_at: record.imported_at,
    };
    serde_json::to_value(&resp).unwrap_or_default()
}

static SKILL_VERSION_HISTORY: OnceLock<StdMutex<HashMap<String, Vec<Value>>>> = OnceLock::new();

fn skill_version_history() -> &'static StdMutex<HashMap<String, Vec<Value>>> {
    SKILL_VERSION_HISTORY.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn load_skill_manifest(path: &str) -> Result<Value> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read skill manifest {}", path))?;
    serde_json::from_str::<Value>(&raw)
        .with_context(|| format!("failed to parse skill manifest {}", path))
}

fn save_skill_manifest(path: &str, manifest: &Value) -> Result<()> {
    let payload = serde_json::to_string_pretty(manifest)
        .context("failed to serialize skill manifest payload")?;
    fs::write(path, payload).with_context(|| format!("failed to write skill manifest {}", path))
}

pub fn parse_semver_patch(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn bump_patch_version(version: &str) -> String {
    parse_semver_patch(version)
        .map(|(major, minor, patch)| format!("{}.{}.{}", major, minor, patch + 1))
        .unwrap_or_else(|| "1.0.0".to_string())
}

fn build_skill_version_snapshot(
    record: &ImportedSkillRecord,
    manifest: &Value,
    updated_by: &str,
    change_summary: &str,
) -> Value {
    let updated_at = crate::acp::prelude::now_ts().to_string();
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or(record.version.as_str())
        .to_string();
    let description = manifest
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or(record.description.as_str())
        .to_string();
    let default_schema = {
        let mut m = serde_json::Map::new();
        m.insert("type".to_string(), Value::String("object".to_string()));
        Value::Object(m)
    };
    let input_schema = manifest
        .get("input_schema")
        .cloned()
        .unwrap_or(default_schema);
    let prompt_template = manifest.get("prompt_template").cloned();

    let snapshot = crate::schema::SkillVersionSnapshot {
        name: record.name.clone(),
        version,
        description,
        input_schema: Some(input_schema),
        prompt_template,
        manifest_path: record.manifest_path.clone(),
        saved_at: updated_at.clone(),
        updated_at,
        updated_by: updated_by.to_string(),
        change_summary: change_summary.to_string(),
    };
    serde_json::to_value(&snapshot).unwrap_or_default()
}

fn push_skill_version_snapshot(name: &str, snapshot: Value) {
    let mut history = skill_version_history().lock().unwrap_or_else(|poisoned| {
        warn!("Skill version history lock poisoned in push_skill_version_snapshot, recovering");
        poisoned.into_inner()
    });
    let entries = history.entry(name.to_string()).or_default();
    entries.push(snapshot);
    if entries.len() > 100 {
        let overflow = entries.len() - 100;
        entries.drain(0..overflow);
    }
}

// ── Skill admin audit helpers ───────────────────────────────────────────

fn record_skill_admin_audit(action: &str, target: &str, success: bool, reason: &str) {
    record_skill_admin_audit_with_protocol(action, target, success, reason, "acp_stdio");
}

fn record_skill_admin_audit_with_protocol(
    action: &str,
    target: &str,
    success: bool,
    reason: &str,
    protocol: &str,
) {
    use crate::governance::hardening::AutonomousEditAuditEntry;
    let entry = AutonomousEditAuditEntry {
        timestamp: crate::acp::prelude::now_ts().to_string(),
        agent: format!("skill.{}", action),
        file_path: target.to_string(),
        change_summary: format!(
            "action={} status={} protocol={}",
            action,
            if success { "ok" } else { "error" },
            protocol,
        ),
        approval_reason: reason.to_string(),
        confidence_score: if success { 1.0 } else { 0.0 },
        reversible: action != "import",
    };
    if let Err(err) = super::super::mcp_audit_logger().record(&entry) {
        debug!("failed to record skill admin audit: {}", err);
    }
}

// ── Skill handlers ──────────────────────────────────────────────────────

pub async fn skill_import_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let request: SkillImportRequest =
        serde_json::from_value(params).context("invalid params for skill.import")?;
    let mut store = open_skill_import_store(server)?;
    let imported = match store.import_skill(request).await {
        Ok(record) => record,
        Err(err) => {
            record_skill_admin_audit("import", "skill.import", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    store.save()?;
    let imported_name = imported.name.clone();

    record_skill_admin_audit(
        "import",
        &imported.name,
        true,
        "imported skill manifest with supply-chain checks",
    );
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "import".to_string(),
        name: Some(imported_name),
        skill: Some(normalize_imported_record(imported)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub async fn skill_list_imported_payload(server: &AcpServer) -> Result<Value> {
    let store = open_skill_import_store(server)?;
    let imported_skills = store.list();
    let imported_names: HashSet<String> = imported_skills.iter().map(|r| r.name.clone()).collect();

    let mut skills: Vec<Value> = imported_skills
        .into_iter()
        .map(normalize_imported_record)
        .collect();

    if let Ok(registry) = server.orchestration_deps.skill_registry.read() {
        for (name, data) in registry.prompt_skill_data() {
            if !imported_names.contains(&name) {
                let view = ImportedSkillRecordView {
                    name: data.name,
                    version: "1.0".to_string(),
                    description: data.description,
                    source: "prompt".to_string(),
                    source_ref: String::new(),
                    sha256: String::new(),
                    manifest_path: String::new(),
                    enabled: true,
                    imported_at: data.created_at,
                };
                skills.push(serde_json::to_value(&view).unwrap_or_default());
            }
        }
    }

    skills.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
    });

    let total = skills.len();
    let enabled = skills
        .iter()
        .filter(|skill| skill.get("enabled").and_then(Value::as_bool) == Some(true))
        .count();
    let disabled = total.saturating_sub(enabled);

    record_skill_admin_audit(
        "list_imported",
        "skill.list_imported",
        true,
        "listed imported skills",
    );
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "list_imported".to_string(),
        name: None,
        skill: None,
        total: Some(total),
        enabled: Some(enabled),
        disabled: Some(disabled),
        skills: Some(skills),
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub async fn skill_enabled_toggle_payload(
    server: &AcpServer,
    params: Value,
    enabled: bool,
) -> Result<Value> {
    let action = if enabled { "enable" } else { "disable" };
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(action, "skill.toggle", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    let mut store = open_skill_import_store(server)?;
    let updated = match store.set_enabled(&name, enabled) {
        Ok(record) => {
            store.save()?;
            record
        }
        Err(_) => {
            let is_prompt_skill = server
                .orchestration_deps
                .skill_registry
                .read()
                .map(|r| r.prompt_skill_data().contains_key(&name))
                .unwrap_or(false);
            if is_prompt_skill {
                record_skill_admin_audit(
                    action,
                    &name,
                    true,
                    "prompt skill toggle (always enabled)",
                );
                let payload = serde_json::to_value(SkillActionResponse {
                    ok: true,
                    action: action.to_string(),
                    name: Some(name),
                    skill: None,
                    total: None,
                    enabled: None,
                    disabled: None,
                    skills: None,
                    removed: None,
                    unregistered: None,
                    version: None,
                    versions: None,
                })
                .unwrap_or_default();
                return Ok(payload);
            }
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit(action, &name, false, &reason);
            return Err(anyhow::anyhow!(reason));
        }
    };
    record_skill_admin_audit(action, &name, true, "updated imported skill state");
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: action.to_string(),
        name: Some(name),
        skill: Some(normalize_imported_record(updated)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub async fn skill_remove_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("remove", "skill.remove", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    let mut store = open_skill_import_store(server)?;
    let removed = store.remove(&name);
    if !removed {
        let registry_removed = server
            .orchestration_deps
            .skill_registry
            .write()
            .map(|mut registry| {
                let r = registry.unregister(&name);
                if let Err(e) = registry.save_prompt_skills_to_disk() {
                    tracing::warn!("Failed to persist prompt skills after removal: {}", e);
                }
                r
            })
            .unwrap_or(false);
        if registry_removed {
            record_skill_admin_audit("remove", &name, true, "removed prompt skill");
            let payload = serde_json::to_value(SkillActionResponse {
                ok: true,
                action: "remove".to_string(),
                name: Some(name),
                skill: None,
                total: None,
                enabled: None,
                disabled: None,
                skills: None,
                removed: Some(false),
                unregistered: Some(true),
                version: None,
                versions: None,
            })
            .unwrap_or_default();
            return Ok(payload);
        }
        let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
        record_skill_admin_audit("remove", &name, false, &reason);
        return Err(anyhow::anyhow!(reason));
    }
    let unregistered = server
        .orchestration_deps
        .skill_registry
        .write()
        .map(|mut registry| {
            let unregistered = registry.unregister(&name);
            if let Err(e) = registry.save_prompt_skills_to_disk() {
                tracing::warn!("Failed to persist prompt skills after removal: {}", e);
            }
            unregistered
        })
        .unwrap_or(false);
    store.save()?;
    record_skill_admin_audit("remove", &name, true, "removed imported skill record");

    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "remove".to_string(),
        name: Some(name),
        skill: None,
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: Some(removed),
        unregistered: Some(unregistered),
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub async fn skill_create_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("create", "skill.create", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    let description = params
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: description"))?;
    let prompt_template = params
        .get("prompt_template")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: prompt_template"))?;
    // Bound the template size: an oversized template would bloat the skill
    // persistence file and be re-sent to the model on every invocation.
    const MAX_PROMPT_TEMPLATE_BYTES: usize = 1024 * 1024;
    if prompt_template.len() > MAX_PROMPT_TEMPLATE_BYTES {
        anyhow::bail!(
            "prompt_template too large: {} bytes > {} max",
            prompt_template.len(),
            MAX_PROMPT_TEMPLATE_BYTES
        );
    }
    let input_schema: std::collections::HashMap<String, String> = params
        .get("input_schema")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let result = {
        let mut registry = server
            .orchestration_deps
            .skill_registry
            .write()
            .map_err(|err| anyhow::anyhow!("skill registry write-lock error: {}", err))?;
        registry.create_skill_from_prompt(&name, &description, &prompt_template, input_schema)
    };
    if let Err(err) = result {
        record_skill_admin_audit("create", &name, false, &err.to_string());
        return Err(anyhow::anyhow!(err.to_string()));
    }

    record_skill_admin_audit("create", &name, true, "created skill from prompt template");
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "create".to_string(),
        name: Some(name),
        skill: None,
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub(crate) fn skill_update_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("update", "skill.update", false, &err.to_string());
            return Err(err);
        }
    };

    let mut store = open_skill_import_store(server)?;
    let Some(mut record) = store.get(&name) else {
        let has_prompt_skill = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().contains_key(&name))
            .unwrap_or(false);
        if !has_prompt_skill {
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit("update", &name, false, &reason);
            anyhow::bail!(reason);
        }
        let current = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().get(&name).cloned())
            .ok()
            .flatten()
            .context("skill not found in registry")?;
        let description = params
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or(current.description);
        let prompt_template = params
            .get("prompt_template")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or(current.prompt_template);
        let input_schema: std::collections::HashMap<String, String> = params
            .get("input_schema")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(current.input_schema);

        {
            let mut registry = server
                .orchestration_deps
                .skill_registry
                .write()
                .map_err(|err| anyhow::anyhow!("skill registry write-lock error: {}", err))?;
            registry.create_skill_from_prompt(
                &name,
                &description,
                &prompt_template,
                input_schema,
            )?;
        }

        record_skill_admin_audit("update", &name, true, "updated prompt skill");
        return Ok(serde_json::to_value(SkillActionResponse {
            ok: true,
            action: "update".to_string(),
            name: Some(name),
            skill: None,
            total: None,
            enabled: None,
            disabled: None,
            skills: None,
            removed: None,
            unregistered: None,
            version: None,
            versions: None,
        })
        .unwrap_or_default());
    };

    let mut manifest = load_skill_manifest(&record.manifest_path)?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(&record, &manifest, "system", "initial skill import"),
    );

    if let Some(description) = params.get("description").and_then(Value::as_str) {
        manifest["description"] = Value::String(description.to_string());
        record.description = description.to_string();
    }
    if let Some(schema) = params.get("input_schema") {
        manifest["input_schema"] = schema.clone();
    }
    if let Some(prompt) = params.get("prompt_template").and_then(Value::as_str) {
        manifest["prompt_template"] = Value::String(prompt.to_string());
    }

    let current_version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or(record.version.as_str());
    let target_version = params
        .get("version")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| bump_patch_version(current_version));
    manifest["version"] = Value::String(target_version.clone());
    record.version = target_version;

    save_skill_manifest(&record.manifest_path, &manifest)?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(
            &record,
            &manifest,
            "system",
            "updated imported skill manifest",
        ),
    );

    store.upsert_record(record.clone());
    store.save()?;

    record_skill_admin_audit("update", &name, true, "updated imported skill manifest");
    Ok(serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "update".to_string(),
        name: Some(name),
        skill: Some(normalize_imported_record(record)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default())
}

pub(crate) fn skill_version_list_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(
                "version.list",
                "skill.version.list",
                false,
                &err.to_string(),
            );
            return Err(err);
        }
    };

    let store = open_skill_import_store(server)?;
    let Some(record) = store.get(&name) else {
        let has_prompt_skill = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().contains_key(&name))
            .unwrap_or(false);
        if !has_prompt_skill {
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit("version.list", &name, false, &reason);
            anyhow::bail!(reason);
        }
        record_skill_admin_audit("version.list", &name, true, "listed prompt skill versions");
        return Ok(serde_json::to_value(SkillActionResponse {
            ok: true,
            action: "version.list".to_string(),
            name: Some(name),
            skill: None,
            total: None,
            enabled: None,
            disabled: None,
            skills: None,
            removed: None,
            unregistered: None,
            version: None,
            versions: Some(vec![serde_json::json!({"version": "1.0"})]),
        })
        .unwrap_or_default());
    };

    let manifest = load_skill_manifest(&record.manifest_path)?;
    let mut versions = skill_version_history()
        .lock()
        .ok()
        .and_then(|history| history.get(&name).cloned())
        .unwrap_or_default();
    versions.push(build_skill_version_snapshot(
        &record,
        &manifest,
        "system",
        "current imported skill snapshot",
    ));

    record_skill_admin_audit("version.list", &name, true, "listed skill versions");
    Ok(serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "version.list".to_string(),
        name: Some(name),
        skill: None,
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: Some(versions),
    })
    .unwrap_or_default())
}

pub(crate) fn skill_version_rollback_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(
                "version.rollback",
                "skill.version.rollback",
                false,
                &err.to_string(),
            );
            return Err(err);
        }
    };
    let Some(target_version) = params.get("version").and_then(Value::as_str) else {
        anyhow::bail!("version is required");
    };

    let mut store = open_skill_import_store(server)?;
    let Some(mut record) = store.get(&name) else {
        let has_prompt_skill = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().contains_key(&name))
            .unwrap_or(false);
        if !has_prompt_skill {
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit("version.rollback", &name, false, &reason);
            anyhow::bail!(reason);
        }
        record_skill_admin_audit(
            "version.rollback",
            &name,
            true,
            "prompt skill has no version history",
        );
        return Ok(serde_json::to_value(SkillActionResponse {
            ok: true,
            action: "rollback".to_string(),
            name: Some(name),
            skill: None,
            total: None,
            enabled: None,
            disabled: None,
            skills: None,
            removed: None,
            unregistered: None,
            version: Some(target_version.to_string()),
            versions: None,
        })
        .unwrap_or_default());
    };

    let history = skill_version_history()
        .lock()
        .ok()
        .and_then(|entries| entries.get(&name).cloned())
        .unwrap_or_default();
    let Some(snapshot) = history.into_iter().rev().find(|entry| {
        entry
            .get("version")
            .and_then(Value::as_str)
            .map(|version| version == target_version)
            .unwrap_or(false)
    }) else {
        anyhow::bail!(
            "version '{}' not found for skill '{}'",
            target_version,
            name
        );
    };

    let mut manifest = load_skill_manifest(&record.manifest_path)?;
    if let Some(description) = snapshot.get("description") {
        manifest["description"] = description.clone();
        if let Some(text) = description.as_str() {
            record.description = text.to_string();
        }
    }
    if let Some(schema) = snapshot.get("input_schema") {
        manifest["input_schema"] = schema.clone();
    }
    if let Some(prompt_template) = snapshot.get("prompt_template") {
        manifest["prompt_template"] = prompt_template.clone();
    }
    manifest["version"] = Value::String(target_version.to_string());
    record.version = target_version.to_string();

    save_skill_manifest(&record.manifest_path, &manifest)?;
    store.upsert_record(record.clone());
    store.save()?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(
            &record,
            &manifest,
            "system",
            "rolled back imported skill version",
        ),
    );

    record_skill_admin_audit(
        "version.rollback",
        &name,
        true,
        "rolled back imported skill version",
    );
    Ok(serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "rollback".to_string(),
        name: Some(name),
        skill: Some(normalize_imported_record(record)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: Some(target_version.to_string()),
        versions: None,
    })
    .unwrap_or_default())
}
