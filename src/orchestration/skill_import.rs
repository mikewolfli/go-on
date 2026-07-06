use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::i18n::runtime::tf;
use crate::orchestration::skill::SkillRegistry;

const SKILL_IMPORT_CONNECT_TIMEOUT_SECS: u64 = 10;
const SKILL_IMPORT_REQUEST_TIMEOUT_SECS: u64 = 30;
const SKILL_IMPORT_MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SkillImportPolicy {
    pub enabled: bool,
    pub allowed_sources: Vec<String>,
    pub require_sha256: bool,
    pub allow_floating_ref: bool,
    pub cache_dir: String,
}

impl SkillImportPolicy {
    pub fn from_runtime(runtime: &RuntimeConfig) -> Self {
        Self {
            enabled: runtime.skills_import_enabled,
            allowed_sources: runtime.skills_allowed_sources.clone(),
            require_sha256: runtime.skills_require_sha256,
            allow_floating_ref: runtime.skills_allow_floating_ref,
            cache_dir: runtime.skills_cache_dir.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillImportManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_manifest_schema")]
    pub input_schema: Value,
    /// Optional MCP endpoint for remote skill invocation.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Raw prompt template (populated when importing SKILL.md / skill.mdc).
    /// When present, the skill is registered as a prompt-based skill in SkillRegistry.
    #[serde(default)]
    pub prompt_template: Option<String>,
}

fn default_manifest_schema() -> Value {
    json!({"type": "object"})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedSkillRecord {
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    pub source_ref: String,
    pub sha256: String,
    pub manifest_path: String,
    pub enabled: bool,
    pub imported_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillImportRequest {
    pub source: SkillImportSource,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SkillImportSource {
    Github {
        repo: String,
        #[serde(rename = "ref")]
        reference: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        sha256: Option<String>,
    },
    Url {
        url: String,
        #[serde(default)]
        sha256: Option<String>,
    },
    Local {
        path: String,
        #[serde(default)]
        sha256: Option<String>,
    },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SkillImportIndex {
    skills: Vec<ImportedSkillRecord>,
}

pub struct SkillImportStore {
    policy: SkillImportPolicy,
    root_dir: PathBuf,
    index_path: PathBuf,
    records: HashMap<String, ImportedSkillRecord>,
    skill_registry: Arc<RwLock<SkillRegistry>>,
}

impl SkillImportStore {
    pub fn load(
        policy: SkillImportPolicy,
        skill_registry: Arc<RwLock<SkillRegistry>>,
    ) -> Result<Self> {
        let root_dir = PathBuf::from(&policy.cache_dir);
        let index_path = root_dir.join("index.json");
        let mut records = HashMap::new();
        if index_path.exists() {
            let raw = fs::read_to_string(&index_path)
                .with_context(|| format!("failed to read {}", index_path.display()))?;
            let parsed: SkillImportIndex =
                serde_json::from_str(&raw).context("failed to parse skill import index")?;
            for item in parsed.skills {
                records.insert(item.name.clone(), item);
            }
        }
        Ok(Self {
            policy,
            root_dir,
            index_path,
            records,
            skill_registry,
        })
    }

    pub fn list(&self) -> Vec<ImportedSkillRecord> {
        let mut items = self.records.values().cloned().collect::<Vec<_>>();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<ImportedSkillRecord> {
        let record = self
            .records
            .get_mut(name)
            .with_context(|| format!("imported skill '{}' not found", name))?;
        record.enabled = enabled;
        Ok(record.clone())
    }

    pub fn get(&self, name: &str) -> Option<ImportedSkillRecord> {
        self.records.get(name).cloned()
    }

    pub fn upsert_record(&mut self, record: ImportedSkillRecord) {
        self.records.insert(record.name.clone(), record);
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.records.remove(name).is_some()
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(&self.root_dir)
            .with_context(|| format!("failed to create {}", self.root_dir.display()))?;
        let payload = SkillImportIndex {
            skills: self.list(),
        };
        let serialized = serde_json::to_string_pretty(&payload)
            .context("failed to serialize skill import index")?;
        fs::write(&self.index_path, serialized)
            .with_context(|| format!("failed to write {}", self.index_path.display()))?;
        Ok(())
    }

    pub async fn import_skill(
        &mut self,
        request: SkillImportRequest,
    ) -> Result<ImportedSkillRecord> {
        if !self.policy.enabled {
            anyhow::bail!(
                "skill import is disabled by security policy (skills_import_enabled = false)"
            );
        }

        let fetched = fetch_source(&self.policy, &request.source).await?;

        let computed_sha = compute_sha256_hex(&fetched.payload);
        let expected_sha = request.source.expected_sha256();
        if self.policy.require_sha256 && expected_sha.is_none() {
            anyhow::bail!("{}", tf("error.missing_field", &[("field", "sha256")]));
        }
        if let Some(expected) = expected_sha {
            let normalized_expected = expected.to_ascii_lowercase();
            if normalized_expected != computed_sha {
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.missing_field",
                        &[(
                            "field",
                            &format!(
                                "sha256 mismatch: expected {}, got {}",
                                normalized_expected, computed_sha
                            )
                        )]
                    )
                );
            }
        }

        let manifest: SkillImportManifest = serde_json::from_slice(&fetched.payload)
            .context("failed to parse imported skill manifest")?;
        validate_manifest(&manifest)?;

        fs::create_dir_all(&self.root_dir)
            .with_context(|| format!("failed to create {}", self.root_dir.display()))?;
        let skill_dir = self.root_dir.join(&manifest.name).join(&manifest.version);
        fs::create_dir_all(&skill_dir)
            .with_context(|| format!("failed to create {}", skill_dir.display()))?;
        let manifest_path = skill_dir.join("manifest.json");
        fs::write(&manifest_path, &fetched.payload)
            .with_context(|| format!("failed to write {}", manifest_path.display()))?;

        let record = ImportedSkillRecord {
            name: manifest.name.clone(),
            version: manifest.version,
            description: manifest.description.clone(),
            source: fetched.source,
            source_ref: fetched.source_ref,
            sha256: computed_sha,
            manifest_path: manifest_path.display().to_string(),
            enabled: false,
            imported_at: now_ts(),
        };

        // If the manifest declares an MCP endpoint, validate that a RemoteSkill
        // can be constructed for it (connection is not made at import time).
        // The constructed skill is saved for potential registration below when
        // there is no prompt_template (endpoint-only skills).
        let remote_skill: Option<RemoteSkill> = if let Some(endpoint) = &manifest.endpoint {
            Some(
                RemoteSkill::new(
                    endpoint,
                    &manifest.name,
                    Some(&manifest.description),
                    Some(manifest.input_schema.clone()),
                )
                .context("failed to validate RemoteSkill endpoint")?,
            )
        } else {
            None
        };

        self.records.insert(record.name.clone(), record.clone());

        // Persist the updated index immediately.
        self.save()?;

        // Register the skill in the runtime SkillRegistry so it is immediately
        // executable by the skill engine.
        //
        // Two registration paths:
        //   - prompt_template present → register as a PromptBasedSkill
        //   - endpoint present (no prompt_template) → register as a RemoteSkill
        if let Some(prompt_template) = &manifest.prompt_template {
            let input_schema = match &manifest.input_schema {
                Value::Object(map) => map
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_string()))
                    .collect::<HashMap<String, String>>(),
                _ => HashMap::new(),
            };
            match self.skill_registry.write() {
                Ok(mut registry) => {
                    if let Err(e) = registry.create_skill_from_prompt(
                        &manifest.name,
                        &manifest.description,
                        prompt_template,
                        input_schema,
                    ) {
                        tracing::warn!(
                            "failed to register imported skill '{}': {}",
                            manifest.name,
                            e
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to acquire skill_registry lock for '{}': {}",
                        manifest.name,
                        e
                    );
                }
            }
        } else if let Some(remote) = remote_skill {
            // Endpoint-only skill: register as a RemoteSkill
            match self.skill_registry.write() {
                Ok(mut registry) => {
                    let skill: Arc<dyn crate::orchestration::skill::Skill> = Arc::new(remote);
                    if let Err(e) = registry.register(skill) {
                        tracing::warn!(
                            "failed to register remote imported skill '{}': {}",
                            manifest.name,
                            e
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to acquire skill_registry lock for '{}': {}",
                        manifest.name,
                        e
                    );
                }
            }
        }

        Ok(record)
    }
}

struct FetchedSource {
    payload: Vec<u8>,
    source: String,
    source_ref: String,
}

async fn fetch_source(
    policy: &SkillImportPolicy,
    source: &SkillImportSource,
) -> Result<FetchedSource> {
    match source {
        SkillImportSource::Github {
            repo,
            reference,
            path,
            ..
        } => {
            ensure_repo_and_ref(repo, reference, policy.allow_floating_ref)?;
            let source_label = format!("github.com/{}", repo);
            enforce_allowlist(policy, &source_label)?;
            // Try multiple manifest filenames in order of preference
            let manifest_candidates: Vec<String> = if let Some(p) = path {
                vec![p.clone()]
            } else {
                vec![
                    "manifest.json".to_string(),
                    "SKILL.md".to_string(),
                    "skill.mdc".to_string(),
                    "skill.json".to_string(),
                    "skill.yaml".to_string(),
                ]
            };
            let mut last_error = String::new();
            let mut payload = None;
            let mut fetched_path = String::new();
            for manifest_path in &manifest_candidates {
                let url = format!(
                    "https://raw.githubusercontent.com/{}/{}/{}",
                    repo, reference, manifest_path
                );
                match download_bytes(&url).await {
                    Ok(bytes) => {
                        fetched_path = manifest_path.clone();
                        payload = Some(bytes);
                        break;
                    }
                    Err(e) => {
                        last_error = format!("{}", e);
                    }
                }
            }
            let raw_payload = payload.ok_or_else(|| {
                anyhow::anyhow!(
                    "Failed to fetch skill from GitHub repo '{}' (ref: {}). Searched for: {}. Make sure this repo contains a go-on skill manifest file. Last error: {}",
                    repo, reference,
                    manifest_candidates.join(", "),
                    last_error
                )
            })?;
            let payload = if fetched_path.ends_with(".md") || fetched_path.ends_with(".mdc") {
                // SKILL.md / skill.mdc — parse and convert to JSON manifest
                let manifest =
                    parse_skill_md(&raw_payload).context("failed to parse SKILL.md / skill.mdc")?;
                serde_json::to_vec(&manifest)
                    .context("failed to serialize converted SKILL.md manifest")?
            } else {
                raw_payload
            };
            Ok(FetchedSource {
                payload,
                source: source_label,
                source_ref: reference.clone(),
            })
        }
        SkillImportSource::Url { url, .. } => {
            enforce_allowlist(policy, url)?;
            let payload = download_bytes(url).await?;
            Ok(FetchedSource {
                payload,
                source: url.clone(),
                source_ref: "url".to_string(),
            })
        }
        SkillImportSource::Local { path, .. } => {
            let resolved_path = resolve_local_manifest_path(path)?;
            let local_source = format!("local:{}", resolved_path.display());
            enforce_allowlist(policy, &local_source)?;
            let payload = fs::read(&resolved_path)
                .with_context(|| format!("failed to read {}", resolved_path.display()))?;
            Ok(FetchedSource {
                payload,
                source: local_source,
                source_ref: "local".to_string(),
            })
        }
    }
}

fn ensure_repo_and_ref(repo: &str, reference: &str, allow_floating_ref: bool) -> Result<()> {
    if repo.trim().is_empty() || !repo.contains('/') {
        anyhow::bail!("{}", tf("error.missing_field", &[("field", "github repo")]));
    }
    if reference.trim().is_empty() {
        anyhow::bail!("{}", tf("error.missing_field", &[("field", "github ref")]));
    }
    if !allow_floating_ref && is_floating_ref(reference) {
        anyhow::bail!(
            "{}",
            tf(
                "error.missing_field",
                &[("field", &format!("floating ref '{}'", reference))]
            )
        );
    }
    Ok(())
}

fn is_floating_ref(reference: &str) -> bool {
    let normalized = reference.to_ascii_lowercase();
    if matches!(normalized.as_str(), "main" | "master" | "latest" | "head") {
        return true;
    }
    let is_hex = reference.chars().all(|ch| ch.is_ascii_hexdigit());
    !is_hex || reference.len() < 7
}

fn resolve_local_manifest_path(path: &str) -> Result<PathBuf> {
    let path_buf = PathBuf::from(path);
    let candidate = if path_buf.is_dir() {
        path_buf.join("manifest.json")
    } else {
        path_buf
    };
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", candidate.display()))?;
    // canonicalize() already guarantees existence, so no redundant .exists() check.
    Ok(canonical)
}

fn enforce_allowlist(policy: &SkillImportPolicy, source: &str) -> Result<()> {
    if policy.allowed_sources.is_empty() {
        anyhow::bail!(
            "{}",
            tf(
                "error.missing_field",
                &[("field", "skills_allowed_sources")]
            )
        );
    }
    let allowed = policy
        .allowed_sources
        .iter()
        .any(|pattern| allowlist_match(pattern, source));
    if !allowed {
        anyhow::bail!(
            "{}",
            tf("error.command_not_allowed", &[("command", source)])
        );
    }
    Ok(())
}

fn allowlist_match(pattern: &str, value: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        value == pattern
    }
}

async fn download_bytes(url: &str) -> Result<Vec<u8>> {
    static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
    let client = HTTP_CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(Duration::from_secs(SKILL_IMPORT_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(SKILL_IMPORT_REQUEST_TIMEOUT_SECS))
            .build()
            .expect("failed to build reqwest client for skill import")
    });
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request failed for {}", url))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!(
            "HTTP {} when fetching {} — this URL does not contain a valid skill manifest.",
            status.as_u16(),
            url
        );
    }
    if let Some(content_length) = response.content_length() {
        if content_length > SKILL_IMPORT_MAX_BYTES as u64 {
            anyhow::bail!(
                "Response too large: {} bytes (max {}) for {}",
                content_length,
                SKILL_IMPORT_MAX_BYTES,
                url
            );
        }
    }

    let mut stream = response.bytes_stream();
    let mut payload = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("failed to read response body from {}", url))?;
        if payload.len() + chunk.len() > SKILL_IMPORT_MAX_BYTES {
            anyhow::bail!(
                "Response stream exceeded {} bytes for {}",
                SKILL_IMPORT_MAX_BYTES,
                url
            );
        }
        payload.extend_from_slice(&chunk);
    }
    Ok(payload)
}

fn validate_manifest(manifest: &SkillImportManifest) -> Result<()> {
    validate_skill_name(&manifest.name)?;
    if manifest.version.trim().is_empty() {
        anyhow::bail!(
            "{}",
            tf("error.missing_field", &[("field", "manifest version")])
        );
    }
    if !manifest.input_schema.is_object() {
        anyhow::bail!(
            "{}",
            tf("error.missing_field", &[("field", "manifest input_schema")])
        );
    }
    Ok(())
}

fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!(
            "{}",
            tf(
                "error.skill_name_length",
                &[("name", name), ("len", &name.len().to_string())]
            )
        );
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
    {
        anyhow::bail!(
            "{}",
            tf(
                "error.skill_name_invalid_chars",
                &[("name", name), ("chars", "invalid characters")]
            )
        );
    }
    Ok(())
}

fn compute_sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{:02x}", byte)).collect()
}

/// Parse a SKILL.md (Claude Code skill format) into a `SkillImportManifest`.
///
/// SKILL.md format (with optional YAML frontmatter):
/// ```markdown
/// ---
/// name: my-skill
/// description: Does something
/// version: 1.0.0
/// ---
///
/// # Skill Content
///
/// Instructions...
/// ```
///
/// If no YAML frontmatter exists, the first `#` heading is used as the name
/// (sanitised to match Go-On's skill naming rules), and the first paragraph
/// following a heading is used as the description.
///
/// The `prompt_template` field is always set to the full raw markdown,
/// so the skill can be registered as a prompt-based skill.
pub(crate) fn parse_skill_md(content: &[u8]) -> Result<SkillImportManifest> {
    let text = std::str::from_utf8(content).context("SKILL.md is not valid UTF-8")?;
    let full_text = text.to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut version = "1.0.0".to_string();
    let mut input_schema: Option<Value> = None;

    // Try to parse YAML frontmatter (between --- delimiters)
    let remaining = if let Some(after_prefix) = text.strip_prefix("---") {
        if let Some(end) = after_prefix.find("\n---") {
            let frontmatter = &after_prefix[..end];
            // Collect raw lines for multi-line value reconstruction
            let raw_lines: Vec<&str> = frontmatter.lines().collect();
            let mut i = 0;
            while i < raw_lines.len() {
                let line = raw_lines[i];
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_lowercase();
                    let value = value.trim().to_string();
                    if key == "input_schema" && !value.is_empty() {
                        // Collect multi-line JSON value
                        let mut json_str = value.clone();
                        i += 1;
                        while i < raw_lines.len() {
                            let continuation = raw_lines[i];
                            if continuation.starts_with(' ') || continuation.starts_with('\t') {
                                json_str.push_str(continuation.trim());
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        if let Ok(parsed) = serde_json::from_str(&json_str) {
                            input_schema = Some(parsed);
                        }
                        continue;
                    }
                    let value_clean = value.trim_matches('"').trim_matches('\'').to_string();
                    match key.as_str() {
                        "name" => name = value_clean,
                        "description" | "title" => description = value_clean,
                        "version" => version = value_clean,
                        _ => {}
                    }
                }
                i += 1;
            }
            Some(&text[3 + end + 5..]) // skip past closing ---
        } else {
            None
        }
    } else {
        Some(text)
    };

    // If no name from frontmatter, extract from first # heading
    if name.is_empty() {
        if let Some(remaining) = remaining {
            for line in remaining.lines() {
                if let Some(heading) = line.trim().strip_prefix("# ") {
                    name = heading.to_string();
                    // Take first part before dash or colon as name
                    for sep in &["—", "–", " - ", " – ", ": "] {
                        if let Some(idx) = name.find(sep) {
                            name = name[..idx].trim().to_string();
                            break;
                        }
                    }
                    break;
                }
            }
        }
    }

    // Fallback name if nothing found
    if name.is_empty() {
        anyhow::bail!("SKILL.md has no name (no YAML frontmatter and no # heading)");
    }

    // Sanitise name: lowercase, spaces→hyphens, strip non-allowed characters
    {
        let mut sanitised = String::with_capacity(name.len());
        for ch in name.chars() {
            match ch {
                c if c.is_ascii_lowercase() || c.is_ascii_digit() => sanitised.push(c),
                c if c.is_ascii_uppercase() => sanitised.push(c.to_ascii_lowercase()),
                ' ' | '_' | '-' | '.' => sanitised.push('-'),
                _ => {}
            }
        }
        // Trim leading/trailing hyphens and collapse runs
        let trimmed: String = sanitised
            .trim_matches('-')
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if trimmed.is_empty() {
            // If sanitisation emptied everything, keep original lowercased
            name = name.to_ascii_lowercase();
        } else {
            name = trimmed;
        }
    }

    // Description from frontmatter, or first non-heading, non-empty line
    if description.is_empty() {
        if let Some(remaining) = remaining {
            for line in remaining.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("---") {
                    description = trimmed.to_string();
                    if description.len() > 200 {
                        description = description[..197].to_string() + "...";
                    }
                    break;
                }
            }
        }
    }

    Ok(SkillImportManifest {
        name,
        version,
        description,
        input_schema: input_schema.unwrap_or_else(default_manifest_schema),
        endpoint: None,
        prompt_template: Some(full_text),
    })
}

fn now_ts() -> i64 {
    crate::acp::prelude::now_ts()
}

/// A remote skill that wraps an MCP endpoint as a Skill trait implementation.
///
/// This allows remote MCP skills to be registered in the SkillRegistry and
/// invoked through the same interface as local skills.
pub struct RemoteSkill {
    name: String,
    description: String,
    input_schema: Value,
    endpoint: String,
    client: reqwest::Client,
}

impl RemoteSkill {
    /// Create a new RemoteSkill that proxies tool calls to an MCP endpoint.
    ///
    /// The endpoint should point to an MCP-compatible server that exposes
    /// a `/tools/call` endpoint accepting `{"name": "...", "arguments": {...}}`.
    ///
    /// `description` and `input_schema` override the defaults when provided.
    /// When `None`, a default description ("Remote MCP skill at ...") and a
    /// generic JSON object schema are used.
    pub fn new(
        endpoint: &str,
        skill_name: &str,
        description: Option<&str>,
        input_schema: Option<Value>,
    ) -> Result<Self> {
        let connect_timeout = Duration::from_secs(SKILL_IMPORT_CONNECT_TIMEOUT_SECS);
        let request_timeout = Duration::from_secs(SKILL_IMPORT_REQUEST_TIMEOUT_SECS);
        let client = Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .build()
            .context("failed to build HTTP client for RemoteSkill")?;

        Ok(Self {
            name: skill_name.to_string(),
            description: description
                .unwrap_or(&format!("Remote MCP skill at {}", endpoint))
                .to_string(),
            input_schema: input_schema.unwrap_or_else(|| json!({"type": "object"})),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            client,
        })
    }

    async fn call_remote(&self, input: &Value) -> Result<Value> {
        let url = format!("{}/tools/call", self.endpoint);
        let payload = json!({
            "name": self.name,
            "arguments": input,
        });

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("failed to call remote skill at {}", url))?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!(
                "{}",
                tf(
                    "error.tool_not_found",
                    &[(
                        "name",
                        &format!("{} returned status {} from {}", self.name, status, url)
                    )]
                )
            );
        }

        let body: Value = response
            .json()
            .await
            .with_context(|| format!("failed to parse response from {}", url))?;

        Ok(body)
    }
}

#[async_trait::async_trait]
impl crate::orchestration::skill::Skill for RemoteSkill {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        self.call_remote(input).await
    }
}

impl SkillImportSource {
    fn expected_sha256(&self) -> Option<&str> {
        match self {
            SkillImportSource::Github { sha256, .. } => sha256.as_deref(),
            SkillImportSource::Url { sha256, .. } => sha256.as_deref(),
            SkillImportSource::Local { sha256, .. } => sha256.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_workspace(name: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("skill_import_test_ws")
            .join(format!("{}-{}", name, now_ts()));
        fs::create_dir_all(&root).expect("create test workspace");
        root
    }

    #[test]
    fn allowlist_supports_wildcard_prefix() {
        assert!(allowlist_match(
            "https://artifacts.example.com/skills/*",
            "https://artifacts.example.com/skills/demo/manifest.json"
        ));
        assert!(!allowlist_match(
            "https://artifacts.example.com/skills/*",
            "https://evil.example.com/demo"
        ));
    }

    #[test]
    fn floating_ref_detection_blocks_branch_names() {
        assert!(is_floating_ref("main"));
        assert!(is_floating_ref("latest"));
        assert!(!is_floating_ref("d34db33fd34db33fd34db33fd34db33fd34db33f"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Miri on Windows does not support filesystem directory creation APIs"
    )]
    fn local_import_requires_matching_sha_when_enabled() {
        let root = test_workspace("requires_sha");
        let manifest = json!({
            "name": "local.echo",
            "version": "1.0.0",
            "description": "local skill",
            "input_schema": {"type": "object"}
        });
        let manifest_path = root.join("manifest.json");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let policy = SkillImportPolicy {
            enabled: true,
            allowed_sources: vec!["local:*".to_string()],
            require_sha256: true,
            allow_floating_ref: false,
            cache_dir: root.join("cache").display().to_string(),
        };
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        let mut store = SkillImportStore::load(policy, registry).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime for test");
        let err = runtime
            .block_on(store.import_skill(SkillImportRequest {
                source: SkillImportSource::Local {
                    path: manifest_path.display().to_string(),
                    sha256: None,
                },
            }))
            .unwrap_err();

        assert!(err.to_string().contains("error.missing_field"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Miri on Windows does not support filesystem directory creation APIs"
    )]
    fn local_import_succeeds_and_persists_disabled_record() {
        let root = test_workspace("persist_record");
        let manifest = json!({
            "name": "local.echo",
            "version": "1.0.1",
            "description": "local skill",
            "input_schema": {"type": "object", "properties": {"message": {"type": "string"}}}
        });
        let payload = serde_json::to_vec(&manifest).unwrap();
        let sha = compute_sha256_hex(&payload);
        let manifest_path = root.join("manifest.json");
        fs::write(&manifest_path, payload).unwrap();

        let policy = SkillImportPolicy {
            enabled: true,
            allowed_sources: vec!["local:*".to_string()],
            require_sha256: true,
            allow_floating_ref: false,
            cache_dir: root.join("cache").display().to_string(),
        };
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        let mut store = SkillImportStore::load(policy, registry.clone()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime for test");
        let imported = runtime
            .block_on(store.import_skill(SkillImportRequest {
                source: SkillImportSource::Local {
                    path: manifest_path.display().to_string(),
                    sha256: Some(sha),
                },
            }))
            .unwrap();
        assert_eq!(imported.name, "local.echo");
        assert!(!imported.enabled);

        store.save().unwrap();
        let reloaded = SkillImportStore::load(store.policy.clone(), registry).unwrap();
        assert_eq!(reloaded.list().len(), 1);
    }
}
