use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;

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
}

impl SkillImportStore {
    pub fn load(policy: SkillImportPolicy) -> Result<Self> {
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
            anyhow::bail!("skills import is disabled by runtime.skills_import_enabled");
        }

        let fetched = fetch_source(&self.policy, &request.source).await?;

        let computed_sha = compute_sha256_hex(&fetched.payload);
        let expected_sha = request.source.expected_sha256();
        if self.policy.require_sha256 && expected_sha.is_none() {
            anyhow::bail!("sha256 is required by policy but not provided");
        }
        if let Some(expected) = expected_sha {
            let normalized_expected = expected.to_ascii_lowercase();
            if normalized_expected != computed_sha {
                anyhow::bail!(
                    "sha256 mismatch: expected {}, got {}",
                    normalized_expected,
                    computed_sha
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
            description: manifest.description,
            source: fetched.source,
            source_ref: fetched.source_ref,
            sha256: computed_sha,
            manifest_path: manifest_path.display().to_string(),
            enabled: false,
            imported_at: now_ts(),
        };

        self.records.insert(record.name.clone(), record.clone());
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
            let manifest_path = path.clone().unwrap_or_else(|| "manifest.json".to_string());
            let source_label = format!("github.com/{}", repo);
            enforce_allowlist(policy, &source_label)?;
            let url = format!(
                "https://raw.githubusercontent.com/{}/{}/{}",
                repo, reference, manifest_path
            );
            let payload = download_bytes(&url).await?;
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
        anyhow::bail!("github repo must be formatted as owner/repo");
    }
    if reference.trim().is_empty() {
        anyhow::bail!("github ref must not be empty");
    }
    if !allow_floating_ref && is_floating_ref(reference) {
        anyhow::bail!(
            "floating ref '{}' is denied by policy (pin to immutable commit SHA)",
            reference
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
    if !canonical.exists() {
        anyhow::bail!("manifest path not found: {}", canonical.display());
    }
    Ok(canonical)
}

fn enforce_allowlist(policy: &SkillImportPolicy, source: &str) -> Result<()> {
    if policy.allowed_sources.is_empty() {
        anyhow::bail!("skills import allowlist is empty; configure runtime.skills_allowed_sources");
    }
    let allowed = policy
        .allowed_sources
        .iter()
        .any(|pattern| allowlist_match(pattern, source));
    if !allowed {
        anyhow::bail!(
            "source '{}' is not allowed by runtime.skills_allowed_sources",
            source
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
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(SKILL_IMPORT_CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(SKILL_IMPORT_REQUEST_TIMEOUT_SECS))
        .build()
        .context("failed to build reqwest client")?;
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request failed for {}", url))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("request failed for {} with status {}", url, status);
    }
    if let Some(content_length) = response.content_length() {
        if content_length > SKILL_IMPORT_MAX_BYTES as u64 {
            anyhow::bail!(
                "response body too large for {}: {} bytes (max {})",
                url,
                content_length,
                SKILL_IMPORT_MAX_BYTES
            );
        }
    }

    let mut stream = response.bytes_stream();
    let mut payload = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("failed to read response body from {}", url))?;
        if payload.len() + chunk.len() > SKILL_IMPORT_MAX_BYTES {
            anyhow::bail!(
                "response body too large for {}: exceeded {} bytes",
                url,
                SKILL_IMPORT_MAX_BYTES
            );
        }
        payload.extend_from_slice(&chunk);
    }
    Ok(payload)
}

fn validate_manifest(manifest: &SkillImportManifest) -> Result<()> {
    validate_skill_name(&manifest.name)?;
    if manifest.version.trim().is_empty() {
        anyhow::bail!("manifest version must not be empty");
    }
    if !manifest.input_schema.is_object() {
        anyhow::bail!("manifest input_schema must be a JSON object");
    }
    Ok(())
}

fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("skill name length must be within [1, 64]");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
    {
        anyhow::bail!("skill name contains invalid characters: {}", name);
    }
    Ok(())
}

fn compute_sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{:02x}", byte)).collect()
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
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

    #[tokio::test]
    async fn local_import_requires_matching_sha_when_enabled() {
        let temp = tempfile::tempdir().expect("temp dir");
        let manifest = json!({
            "name": "local.echo",
            "version": "1.0.0",
            "description": "local skill",
            "input_schema": {"type": "object"}
        });
        let manifest_path = temp.path().join("manifest.json");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let policy = SkillImportPolicy {
            enabled: true,
            allowed_sources: vec!["local:*".to_string()],
            require_sha256: true,
            allow_floating_ref: false,
            cache_dir: temp.path().join("cache").display().to_string(),
        };
        let mut store = SkillImportStore::load(policy).unwrap();

        let err = store
            .import_skill(SkillImportRequest {
                source: SkillImportSource::Local {
                    path: manifest_path.display().to_string(),
                    sha256: None,
                },
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("sha256 is required"));
    }

    #[tokio::test]
    async fn local_import_succeeds_and_persists_disabled_record() {
        let temp = tempfile::tempdir().expect("temp dir");
        let manifest = json!({
            "name": "local.echo",
            "version": "1.0.1",
            "description": "local skill",
            "input_schema": {"type": "object", "properties": {"message": {"type": "string"}}}
        });
        let payload = serde_json::to_vec(&manifest).unwrap();
        let sha = compute_sha256_hex(&payload);
        let manifest_path = temp.path().join("manifest.json");
        fs::write(&manifest_path, payload).unwrap();

        let policy = SkillImportPolicy {
            enabled: true,
            allowed_sources: vec!["local:*".to_string()],
            require_sha256: true,
            allow_floating_ref: false,
            cache_dir: temp.path().join("cache").display().to_string(),
        };
        let mut store = SkillImportStore::load(policy).unwrap();
        let imported = store
            .import_skill(SkillImportRequest {
                source: SkillImportSource::Local {
                    path: manifest_path.display().to_string(),
                    sha256: Some(sha),
                },
            })
            .await
            .unwrap();
        assert_eq!(imported.name, "local.echo");
        assert!(!imported.enabled);

        store.save().unwrap();
        let reloaded = SkillImportStore::load(store.policy.clone()).unwrap();
        assert_eq!(reloaded.list().len(), 1);
    }
}
