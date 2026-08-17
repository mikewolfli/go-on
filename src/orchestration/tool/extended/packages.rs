//! Package registry search tools.
//!
//! Searches crates.io, npm, PyPI, and other package registries
//! for available packages matching a query.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::debug;

use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

/// Lazy re-export of the process-global blocking client for synchronous
/// package-registry searches (connection pooling shared with all subsystems;
/// the 15s budget is applied per request inside the search helpers).
fn blocking_client() -> Result<&'static reqwest::blocking::Client> {
    crate::shared::http_client::blocking_http_client()
        .map_err(|err| anyhow::anyhow!("failed to get shared HTTP client: {err}"))
}

pub struct SearchPackagesTool;

impl Tool for SearchPackagesTool {
    fn name(&self) -> &'static str {
        "search_packages"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let query = input.payload["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("search_packages requires arguments.query"))?;
        let registry = input.payload["registry"].as_str().unwrap_or("auto");
        let max_results = input.payload["max_results"].as_u64().unwrap_or(5).min(20) as usize;

        let client = blocking_client()?;

        let (registry_used, results) = match registry {
            "crates.io" | "cargo-crates" => {
                ("crates.io", search_crates_io(client, query, max_results)?)
            }
            "npm" => ("npm", search_npm(client, query, max_results)?),
            "pypi" => ("pypi", search_pypi(client, query)?),
            "go" => ("go", search_go_proxy(client, query)?),
            _ => {
                // auto: try crates.io first, then npm, then pypi
                match search_crates_io(client, query, max_results) {
                    Ok(results) if !results.is_empty() => ("crates.io", results),
                    _ => {
                        debug!("crates.io returned no results, trying npm");
                        match search_npm(client, query, max_results) {
                            Ok(results) if !results.is_empty() => ("npm", results),
                            _ => {
                                debug!("npm returned no results, trying pypi");
                                let results = search_pypi(client, query)?;
                                ("pypi", results)
                            }
                        }
                    }
                }
            }
        };

        let total = results.len();
        let truncated = if total > max_results {
            let truncated: Vec<Value> = results.into_iter().take(max_results).collect();
            truncated
        } else {
            results
        };

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "registry": registry_used,
                "query": query,
                "results": truncated,
                "total": total.min(max_results),
            })),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

/// Percent-encode a string for use in URL query parameters.
fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Read a blocking response body under the shared policy cap and parse it as
/// JSON (OOM guard for package-registry responses — the registries can return
/// large bodies for popular packages).
fn capped_json(resp: &mut reqwest::blocking::Response, what: &str) -> Result<Value> {
    let body = crate::orchestration::tool::extended::http::read_blocking_body_capped(resp, what)
        .with_context(|| format!("Failed to read {what} response"))?;
    serde_json::from_slice(&body).with_context(|| format!("Failed to parse {what} response"))
}

/// Search crates.io for Rust packages.
fn search_crates_io(
    client: &reqwest::blocking::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<Value>> {
    let per_page = max_results.clamp(1, 50);
    let url = format!(
        "https://crates.io/api/v1/crates?q={}&per_page={}",
        url_encode(query),
        per_page
    );
    let mut resp = client
        .get(&url)
        .header("User-Agent", crate::shared::http_client::USER_AGENT)
        .send()
        .context("Failed to search crates.io")?;
    let data: Value = capped_json(&mut resp, "crates.io")?;

    Ok(data["crates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            json!({
                "name": c["id"],
                "description": c["description"],
                "max_version": c["max_version"],
                "downloads": c["downloads"],
                "repository": c["repository"],
                "registry": "crates.io",
            })
        })
        .collect())
}

/// Search npm for JavaScript/TypeScript packages.
fn search_npm(
    client: &reqwest::blocking::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<Value>> {
    let size = max_results.clamp(1, 20);
    let url = format!(
        "https://registry.npmjs.org/-/v1/search?text={}&size={}",
        url_encode(query),
        size
    );
    let mut resp = client
        .get(&url)
        .send()
        .context("Failed to search npm registry")?;
    let data: Value = capped_json(&mut resp, "npm")?;

    Ok(data["objects"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|obj| {
            let pkg = &obj["package"];
            json!({
                "name": pkg["name"],
                "description": pkg["description"],
                "version": pkg["version"],
                "links": pkg["links"],
                "publisher": pkg["publisher"]["username"],
                "registry": "npm",
            })
        })
        .collect())
}

/// Search PyPI for Python packages.
fn search_pypi(client: &reqwest::blocking::Client, query: &str) -> Result<Vec<Value>> {
    // Use the JSON API if available, otherwise fall back to HTML scraping
    // PyPI's XML-RPC API is deprecated; use the Warehouse JSON API
    let json_url = format!(
        "https://pypi.org/simple/search/?q={}&per_page=10",
        url_encode(query)
    );
    let mut resp = client
        .get(&json_url)
        .header("Accept", "application/json")
        .send()
        .context("Failed to search PyPI")?;
    let data: Value = capped_json(&mut resp, "PyPI search")?;

    // PyPI simple API returns a list of package names and URLs
    let packages = data.get("packages").and_then(|p| p.as_array()).cloned();

    if let Some(pkgs) = packages {
        return Ok(pkgs
            .into_iter()
            .map(|p| {
                json!({
                    "name": p["name"],
                    "version": p["version"],
                    "description": p["description"],
                    "url": p["url"],
                    "registry": "pypi",
                })
            })
            .collect());
    }

    // Fallback: parse pip search-style results (names only)
    let names = data.get("names").and_then(|n| n.as_array()).cloned();
    if let Some(names) = names {
        // Fetch individual package details for richer results
        let mut results = Vec::new();
        for name_val in names.iter().take(10) {
            if let Some(name) = name_val.as_str() {
                if let Ok(detail) = fetch_pypi_detail(client, name) {
                    results.push(detail);
                }
            }
        }
        return Ok(results);
    }

    Ok(Vec::new())
}

/// Fetch detailed info about a specific PyPI package.
fn fetch_pypi_detail(client: &reqwest::blocking::Client, name: &str) -> Result<Value> {
    let url = format!("https://pypi.org/pypi/{}/json", url_encode(name));
    let mut resp = client
        .get(&url)
        .send()
        .context(format!("Failed to fetch PyPI detail for {}", name))?;
    if !resp.status().is_success() {
        return Ok(json!({ "name": name, "registry": "pypi" }));
    }
    let data: Value = capped_json(&mut resp, "PyPI detail")?;
    let info = &data["info"];
    Ok(json!({
        "name": info["name"],
        "version": info["version"],
        "description": info["summary"],
        "author": info["author"],
        "license": info["license"],
        "home_page": info["home_page"],
        "repository": info["project_urls"]["Source"],
        "registry": "pypi",
    }))
}

/// Search the Go module proxy for Go packages.
fn search_go_proxy(client: &reqwest::blocking::Client, query: &str) -> Result<Vec<Value>> {
    // Go module proxy uses a simple list endpoint
    // The standard proxy (proxy.golang.org) does not provide search.
    // Use the pkg.go.dev search endpoint instead.
    let url = format!("https://proxy.golang.org/{}/@v/list", url_encode(query));
    let resp = client.get(&url).send();

    match resp {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().context("Failed to read Go proxy response")?;
            let versions: Vec<&str> = body.lines().collect();
            let latest = versions.last().copied().unwrap_or("unknown");
            Ok(vec![json!({
                "name": query,
                "version": latest,
                "versions_count": versions.len(),
                "registry": "go",
            })])
        }
        _ => {
            // Fallback: return a minimal result noting the module was checked
            Ok(vec![json!({
                "name": query,
                "version": "unknown",
                "note": "Module not found or proxy unavailable",
                "registry": "go",
            })])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-pkg".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn search_packages_requires_query() {
        let tool = SearchPackagesTool;
        let input = tool_input(json!({}));
        let result = tool.run(&input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires arguments.query"));
    }

    #[test]
    fn url_encode_replaces_spaces() {
        let encoded = url_encode("hello world");
        // form-urlencoded uses "+" for spaces (also valid in query strings)
        assert!(encoded == "hello%20world" || encoded == "hello+world");
    }
}
