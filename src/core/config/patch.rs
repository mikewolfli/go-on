//! M1.2 — layered config merge with per-key source tracking.
//!
//! The loader resolves configuration as a stack of layers applied in order:
//!
//! 1. **builtin** — [`AppConfig::default()`] (the serde defaults).
//! 2. **project** — the resolved config file (`-c <path>`, `./config.toml`,
//!    or the platform config dir).
//! 3. **user** — `~/.config/go-on/config.toml` (honoring
//!    `GO_ON_CONFIG_DIR` / `XDG_CONFIG_HOME`), opt-in via `layered_merge`.
//! 4. **cli** — an inline `--patch` passed for one invocation only.
//!
//! Each layer is merged with [`deep_merge`]; every top-level key records
//! which layer supplied its winning value in a [`ConfigSource`]. Source
//! granularity is deliberately **top-level**: layers merge key-by-key at the
//! top level, so a layer that touches any nested field of a table is recorded
//! as the source of that whole top-level key (honest granularity documented
//! on [`ConfigSource::key`]).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::config::AppConfig;

/// Provenance of one top-level config key after a layered merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSource {
    /// Top-level config key this source describes (e.g. `"runtime"`,
    /// `"cache"`, `"default_phase"`). Granularity is deliberately top-level:
    /// nested per-field provenance is not tracked.
    pub key: String,
    /// Layer that supplied the winning value: `"builtin"` | `"project"` |
    /// `"user"` | `"cli"`.
    pub layer: &'static str,
    /// Config file path the value came from; `None` for the builtin layer
    /// (defaults) and inline CLI patches.
    pub path: Option<String>,
}

/// One config layer to merge: an in-memory TOML document plus provenance.
#[derive(Debug, Clone)]
pub struct LayerSource {
    /// Layer name: `"project"` | `"user"` | `"cli"` (the builtin layer is the
    /// [`AppConfig::default()`] base and is never a `LayerSource`).
    pub layer: &'static str,
    /// Config file path for provenance reporting; `None` for inline patches.
    pub path: Option<String>,
    /// The layer's content as a TOML document.
    pub toml: String,
}

/// Result of a layered load: the merged config, the layer-resolved view the
/// sources describe, per-key sources, and any non-fatal warnings.
#[derive(Debug, Clone)]
pub struct LayeredLoad {
    /// Final merged [`AppConfig`] (builtin + layers, then the normal
    /// legacy-sync / migration / auto-rules pipeline).
    pub config: AppConfig,
    /// Layer-resolved view (defaults + every applied layer, null values
    /// stripped) — the exact view `go-on config dump` prints and `sources`
    /// describes.
    pub merged: Value,
    /// Per-top-level-key provenance, sorted by key. Keys whose final value is
    /// null (unset `Option` fields) carry no meaningful source and are
    /// omitted.
    pub sources: Vec<ConfigSource>,
    /// Non-fatal problems encountered while applying layers (unreadable user
    /// config, unparseable layer, deserialize fallback, ...).
    pub warnings: Vec<String>,
}

/// Recursively merge `patch` into `base`.
///
/// - Objects are merged depth-wise (recursion into shared keys).
/// - Scalars and arrays are replaced wholesale by the patch (the patch wins).
pub fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                match base_map.get_mut(key) {
                    Some(base_value) => deep_merge(base_value, patch_value),
                    None => {
                        base_map.insert(key.clone(), patch_value.clone());
                    }
                }
            }
        }
        (base_slot, patch_value) => *base_slot = patch_value.clone(),
    }
}

/// Apply `layers` on top of `base` (the builtin defaults) and return the
/// merged config plus per-top-level-key sources.
///
/// Layered precedence: `base` (builtin) < `layers[0]` (project) < ... < the
/// last layer (cli). A top-level key is recorded as "overridden by layer L"
/// when its merged value differs from the pre-merge snapshot after merging L;
/// a key a later layer leaves untouched keeps its earlier source.
///
/// Never fails: a layer whose TOML does not parse is skipped (with a warning
/// logged), and a layer that deserializes to an invalid [`AppConfig`] drops
/// that layer and every later one, falling back to the last-good config (a
/// warning is logged). A bad layer must never crash the process.
pub fn apply_layers(base: AppConfig, layers: &[LayerSource]) -> (AppConfig, Vec<ConfigSource>) {
    let loaded = merge_layers(base, layers);
    (loaded.config, loaded.sources)
}

/// Shared implementation of the layered merge (also used by the config loader,
/// which needs the layer-resolved view and warnings in addition to the config).
pub(crate) fn merge_layers(base: AppConfig, layers: &[LayerSource]) -> LayeredLoad {
    let base_value =
        serde_json::to_value(&base).unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    let mut merged = base_value.clone();
    // Initial provenance: every key the builtin defaults define belongs to
    // the builtin layer (no path — they come from the `Default` impl).
    let mut source_map: HashMap<String, ConfigSource> = base_value
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(key, _)| {
                    (
                        key.clone(),
                        ConfigSource {
                            key: key.clone(),
                            layer: "builtin",
                            path: None,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut warnings = Vec::new();
    let mut last_good = base;

    for layer in layers {
        let Some(patch) = parse_layer_toml(layer, &mut warnings) else {
            continue;
        };
        let Value::Object(patch_map) = &patch else {
            continue;
        };

        // Pre-merge snapshot of the top-level keys the layer may touch.
        let snapshot: HashMap<String, Value> = merged
            .as_object()
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        deep_merge(&mut merged, &patch);

        match serde_json::from_value::<AppConfig>(merged.clone()) {
            Ok(cfg) => {
                for key in patch_map.keys() {
                    // "Overridden by this layer" = the merged value differs
                    // from the pre-merge snapshot at that top-level key.
                    if snapshot.get(key) != merged.get(key) {
                        source_map.insert(
                            key.clone(),
                            ConfigSource {
                                key: key.clone(),
                                layer: layer.layer,
                                path: layer.path.clone(),
                            },
                        );
                    }
                }
                last_good = cfg;
            }
            Err(err) => {
                let msg = format!(
                    "config layer '{}' produced an invalid merged config ({err}); \
                     keeping the last-good config and dropping this and all later layers",
                    layer.layer
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
                break;
            }
        }
    }

    // Only keys with a non-null final value get a source entry: unset
    // `Option` fields (null) have no meaningful provenance.
    let mut sources: Vec<ConfigSource> = merged
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(_, value)| !value.is_null())
                .filter_map(|(key, _)| source_map.get(key).cloned())
                .collect()
        })
        .unwrap_or_default();
    sources.sort_by(|a, b| a.key.cmp(&b.key));

    let mut merged_view = merged;
    strip_nulls(&mut merged_view);
    LayeredLoad {
        config: last_good,
        merged: merged_view,
        sources,
        warnings,
    }
}

/// Parse a layer's TOML into a [`serde_json::Value`] object.
///
/// Unparseable layers and layers containing values the config model cannot
/// represent (e.g. TOML datetimes, which `serde_json::Value` — and therefore
/// [`AppConfig`] — cannot hold) are skipped with a warning; a broken layer
/// must never take the process down.
fn parse_layer_toml(layer: &LayerSource, warnings: &mut Vec<String>) -> Option<Value> {
    match layer.toml.parse::<toml::Table>() {
        Ok(table) => match serde_json::to_value(&table) {
            Ok(value) => Some(value),
            Err(err) => {
                let msg = format!(
                    "config layer '{}' could not be converted to JSON ({err}); \
                     TOML datetimes are not supported by the config model — skipping layer",
                    layer.layer
                );
                tracing::warn!("{msg}");
                warnings.push(msg);
                None
            }
        },
        Err(err) => {
            let msg = format!(
                "config layer '{}' failed to parse as TOML ({err}); skipping layer",
                layer.layer
            );
            tracing::warn!("{msg}");
            warnings.push(msg);
            None
        }
    }
}

/// Resolve the user-layer config path.
///
/// Override order: `GO_ON_CONFIG_DIR/config.toml` → `XDG_CONFIG_HOME`-derived
/// `$XDG_CONFIG_HOME/go-on/config.toml` → `~/.config/go-on/config.toml`
/// (Windows: `%APPDATA%\go-on\config.toml`). Mirrors
/// [`crate::main::preferred_config_root`] semantics plus the explicit
/// `GO_ON_CONFIG_DIR` override, so the user layer lands next to the app's
/// default config file location.
pub fn user_config_path() -> Option<PathBuf> {
    user_config_path_from(
        std::env::var_os("GO_ON_CONFIG_DIR"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("APPDATA"),
        std::env::var_os("USERPROFILE"),
    )
}

fn user_config_path_from(
    go_on_dir: Option<std::ffi::OsString>,
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    appdata: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = go_on_dir {
        return Some(PathBuf::from(dir).join("config.toml"));
    }
    if cfg!(windows) {
        if let Some(dir) = appdata {
            return Some(PathBuf::from(dir).join("go-on").join("config.toml"));
        }
        if let Some(dir) = userprofile {
            return Some(
                PathBuf::from(dir)
                    .join("AppData")
                    .join("Roaming")
                    .join("go-on")
                    .join("config.toml"),
            );
        }
        return None;
    }
    if let Some(dir) = xdg_config_home {
        return Some(PathBuf::from(dir).join("go-on").join("config.toml"));
    }
    if let Some(dir) = home {
        return Some(
            PathBuf::from(dir)
                .join(".config")
                .join("go-on")
                .join("config.toml"),
        );
    }
    None
}

/// Read the user-layer config file, if one exists.
///
/// Returns `Ok(None)` when there is no user config (or it is blank), and
/// `Err` when the file exists but cannot be read — the caller surfaces that
/// as a warning rather than failing the load.
pub fn read_user_layer() -> Result<Option<LayerSource>, String> {
    let Some(path) = user_config_path() else {
        return Ok(None);
    };
    match fs::read_to_string(&path) {
        Ok(content) if content.trim().is_empty() => Ok(None),
        Ok(content) => Ok(Some(LayerSource {
            layer: "user",
            path: Some(path.display().to_string()),
            toml: content,
        })),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("user config {} unreadable: {err}", path.display())),
    }
}

/// Merge only the layer documents (no builtin defaults) and render the result
/// as a TOML string.
///
/// The loader feeds this to the legacy-key sync and schema-migration steps so
/// those see exactly the keys any layer set (never the materialized serde
/// defaults), preserving single-file semantics for the union of layers.
pub(crate) fn explicit_layers_toml(layers: &[LayerSource]) -> Option<String> {
    let mut explicit = Value::Object(serde_json::Map::new());
    for layer in layers {
        if let Ok(table) = layer.toml.parse::<toml::Table>() {
            if let Ok(value) = serde_json::to_value(&table) {
                deep_merge(&mut explicit, &value);
            }
        }
    }
    value_to_toml(&explicit)
}

/// The builtin layer: the config with no layers applied at all — i.e. what
/// an empty document parses to (serde defaults). This is what `go-on config
/// dump` reports as the `builtin` source, and it is what the loader merges
/// every layer on top of.
pub fn builtin_layer() -> AppConfig {
    toml::from_str("").unwrap_or_default()
}

/// Recursively remove `null` values (unset `Option` fields), which TOML
/// cannot represent.
pub fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            for v in map.values_mut() {
                strip_nulls(v);
            }
        }
        Value::Array(items) => {
            for v in items {
                strip_nulls(v);
            }
        }
        _ => {}
    }
}

/// Serializes tests that mutate the `GO_ON_CONFIG_DIR` environment variable:
/// the user-layer path resolution reads process-global state, so the tests
/// that point it at their own temp dirs must not interleave.
#[cfg(test)]
pub(crate) static USER_CONFIG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serialize a JSON value to a TOML document string, omitting null values.
/// Returns `None` when the value cannot be expressed in TOML (e.g. mixed-type
/// arrays) — callers fall back to JSON rendering.
pub fn value_to_toml(value: &Value) -> Option<String> {
    let mut clone = value.clone();
    strip_nulls(&mut clone);
    toml::to_string(&clone).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(layer: &'static str, toml_str: &str) -> LayerSource {
        LayerSource {
            layer,
            path: Some(format!("{layer}.toml")),
            toml: toml_str.to_string(),
        }
    }

    fn source_by_key<'a>(sources: &'a [ConfigSource], key: &str) -> &'a ConfigSource {
        sources
            .iter()
            .find(|s| s.key == key)
            .unwrap_or_else(|| panic!("no source for {key}"))
    }

    #[test]
    fn deep_merge_merges_objects_recursively() {
        let mut base: Value = serde_json::from_str(r#"{"a": {"b": 1, "c": 2}, "d": 3}"#).unwrap();
        let patch: Value = serde_json::from_str(r#"{"a": {"c": 9, "e": 4}, "f": [1, 2]}"#).unwrap();
        deep_merge(&mut base, &patch);
        assert_eq!(
            base,
            serde_json::json!({"a": {"b": 1, "c": 9, "e": 4}, "d": 3, "f": [1, 2]})
        );
    }

    #[test]
    fn deep_merge_patch_wins_on_scalars_and_arrays() {
        let mut base: Value = serde_json::json!({"a": 1, "b": [1, 2], "c": {"x": 1}});
        let patch: Value = serde_json::json!({"a": "replaced", "b": [9], "c": 7});
        deep_merge(&mut base, &patch);
        assert_eq!(base, serde_json::json!({"a": "replaced", "b": [9], "c": 7}));
    }

    #[test]
    fn layered_override_precedence_builtin_project_user_cli() {
        let base = AppConfig::default();
        let layers = [
            layer("project", "[cache]\nenabled = true"),
            layer("user", "[cache]\nenabled = false"),
            layer("cli", "[cache]\nenabled = true"),
        ];
        let (cfg, sources) = apply_layers(base, &layers);

        // Highest-precedence layer (cli) wins over project and user.
        assert!(cfg.cache.expect("cache should exist").enabled);
        let source = source_by_key(&sources, "cache");
        assert_eq!(source.layer, "cli");
        assert_eq!(source.path.as_deref(), Some("cli.toml"));
    }

    #[test]
    fn key_untouched_by_later_layers_keeps_earlier_source() {
        let base = AppConfig::default();
        let layers = [
            layer("project", "[cache]\nenabled = true"),
            layer("user", "[runtime]\nhealth_interval_seconds = 5"),
            layer("cli", "default_phase = \"delivery\""),
        ];
        let (_, sources) = apply_layers(base, &layers);

        // cache was only touched by the project layer; later layers that do
        // not touch it must not steal its source.
        assert_eq!(source_by_key(&sources, "cache").layer, "project");
        assert_eq!(source_by_key(&sources, "runtime").layer, "user");
        assert_eq!(source_by_key(&sources, "default_phase").layer, "cli");
    }

    #[test]
    fn nested_key_override_tracks_top_level_source() {
        // A layer that touches one nested key of a table overrides the whole
        // top-level key (honest top-level granularity).
        let base = AppConfig::default();
        let layers = [
            layer("project", "[runtime]\nmaintenance_interval_seconds = 60"),
            layer("user", "[runtime]\nhealth_interval_seconds = 5"),
        ];
        let (cfg, sources) = apply_layers(base, &layers);

        let rt = cfg.runtime.expect("runtime should exist");
        assert_eq!(rt.maintenance_interval_seconds, 60);
        assert_eq!(rt.health_interval_seconds, 5);
        assert_eq!(source_by_key(&sources, "runtime").layer, "user");
    }

    #[test]
    fn deserialize_failure_falls_back_to_last_good_config() {
        let base = AppConfig::default();
        let layers = [
            layer("project", "[cache]\nenabled = true"),
            // runtime must be a table; a string makes the merged config fail
            // to deserialize — the load must fall back, never crash.
            layer("user", "runtime = \"bogus\""),
            layer("cli", "default_phase = \"delivery\""),
        ];
        let (cfg, sources) = apply_layers(base, &layers);

        // Project layer applied; the bad user layer drops it and the cli layer.
        assert!(cfg.cache.expect("cache should exist").enabled);
        assert!(cfg.runtime.is_none());
        assert_eq!(cfg.provider.default_phase, "");
        assert_eq!(source_by_key(&sources, "cache").layer, "project");
        assert!(!sources
            .iter()
            .any(|s| s.key == "default_phase" && s.layer == "cli"));
    }

    #[test]
    fn unparseable_layer_is_skipped_but_later_layers_apply() {
        let base = AppConfig::default();
        let layers = [
            layer("project", "this is not {{{ valid toml"),
            layer("cli", "default_phase = \"delivery\""),
        ];
        let (cfg, sources) = apply_layers(base, &layers);

        assert_eq!(cfg.provider.default_phase, "delivery");
        assert_eq!(source_by_key(&sources, "default_phase").layer, "cli");
    }

    #[test]
    fn builtin_layer_equals_parsing_an_empty_document() {
        // The builtin layer is the serde defaults — the same config a parse
        // of an empty document yields, NOT the `Default` impl (whose fields
        // are the plain Rust defaults, e.g. schema_version = "").
        let parsed: AppConfig = toml::from_str("").unwrap();
        assert_eq!(builtin_layer().schema_version, parsed.schema_version);
        assert_eq!(builtin_layer().schema_version, "1.0.0");
    }

    #[test]
    fn untouched_keys_report_builtin_as_their_source() {
        let (cfg, sources) = apply_layers(builtin_layer(), &[]);

        // Builtin layer = the serde defaults (an empty document parses to
        // them); with no layers the merged config equals that baseline.
        assert_eq!(cfg.schema_version, "1.0.0");
        assert!(!sources.is_empty());
        assert!(sources
            .iter()
            .all(|s| s.layer == "builtin" && s.path.is_none()));
    }

    #[test]
    fn user_config_path_prefers_override_then_xdg_then_home() {
        assert_eq!(
            user_config_path_from(
                Some("/o".into()),
                Some("/x".into()),
                Some("/h".into()),
                None,
                None
            ),
            Some(PathBuf::from("/o").join("config.toml"))
        );
        assert_eq!(
            user_config_path_from(None, Some("/x".into()), Some("/h".into()), None, None),
            Some(PathBuf::from("/x").join("go-on").join("config.toml"))
        );
        assert_eq!(
            user_config_path_from(None, None, Some("/h".into()), None, None),
            Some(
                PathBuf::from("/h")
                    .join(".config")
                    .join("go-on")
                    .join("config.toml")
            )
        );
        assert_eq!(user_config_path_from(None, None, None, None, None), None);
    }
}
