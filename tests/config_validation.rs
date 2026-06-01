/// Config Validation Test (GAP-B54-084)
///
/// Reads `config/config.toml`, parses it as `AppConfig`, and asserts key
/// fields have reasonable values. This guards against regressions where a
/// typo or structural change silently breaks the default configuration.
use std::path::Path;

fn test_config_path() -> &'static Path {
    // Integration tests run from the workspace root, so `config/config.toml`
    // is directly accessible via the relative path shown below.
    Path::new("config/config.toml")
}

#[test]
fn config_file_exists() {
    let path = test_config_path();
    assert!(
        path.exists(),
        "config/config.toml must exist at workspace root"
    );
    assert!(
        path.metadata().unwrap().len() > 100,
        "config file should be non-trivial"
    );
}

#[test]
fn config_parses_successfully() {
    let cfg = go_on::config::AppConfig::load(test_config_path())
        .expect("default config/config.toml must parse without errors");
    assert_eq!(cfg.schema_version, "1.0.0");
}

#[test]
fn config_has_schema_version() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    // Schema version must be a non-empty semver-ish string
    assert!(
        !cfg.schema_version.is_empty(),
        "schema_version must not be empty"
    );
    assert!(
        cfg.schema_version.contains('.'),
        "schema_version should be a dotted version like 1.0.0"
    );
}

#[test]
fn config_has_default_phase() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    assert!(!cfg.default_phase.is_empty(), "default_phase must be set");
}

#[test]
fn config_has_model_selection_mode() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    assert!(
        !cfg.model_selection_mode.is_empty(),
        "model_selection_mode must be set"
    );
}

#[test]
fn config_has_at_least_one_agent() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    assert!(
        !cfg.agents.is_empty(),
        "config must define at least one agent; got {}",
        cfg.agents.len()
    );
}

#[test]
fn config_has_runtime_config() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    let runtime = cfg
        .runtime
        .as_ref()
        .expect("config must have [runtime] section");
    assert!(
        runtime.acp_http_bind_addr.is_some(),
        "acp_http_bind_addr must be set"
    );
    let bind_addr = runtime.acp_http_bind_addr.as_deref().unwrap();
    assert!(!bind_addr.is_empty(), "acp_http_bind_addr must not be empty");
    assert!(
        bind_addr.contains(':'),
        "acp_http_bind_addr should contain port separator ':'"
    );
}

#[test]
fn config_has_cache_config() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    let cache = cfg
        .cache
        .as_ref()
        .expect("config must have [cache] section");
    assert!(cache.enabled, "cache should be enabled in default config");
    assert!(
        cache.default_ttl_seconds >= 60,
        "cache TTL should be at least 60s; got {}",
        cache.default_ttl_seconds
    );
}

#[test]
fn config_has_vector_config() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    let vector = cfg
        .vector
        .as_ref()
        .expect("config must have [vector] section");
    assert!(vector.enabled, "vector store should be enabled");
    assert!(
        vector.dimensions >= 64,
        "vector dimensions should be >= 64; got {}",
        vector.dimensions
    );
}

#[test]
fn config_has_flow_with_phases() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    assert!(
        !cfg.flow.phases.is_empty(),
        "flow must define at least one phase; got {}",
        cfg.flow.phases.len()
    );
}

#[test]
fn config_has_phase_configs() {
    let cfg = go_on::config::AppConfig::load(test_config_path()).expect("config must parse");
    assert!(
        !cfg.phases.is_empty(),
        "config must define at least one [phases.*] section"
    );
    // Every phase referenced in the flow should have a corresponding section
    for phase_name in &cfg.flow.phases {
        assert!(
            cfg.phases.contains_key(phase_name.as_str()),
            "flow references phase '{}' but no [phases.{}] section exists",
            phase_name,
            phase_name
        );
    }
}
