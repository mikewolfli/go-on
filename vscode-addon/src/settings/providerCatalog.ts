// ── Provider Catalog ──
//
// The built-in provider DATA lives in `providerCatalog.generated.ts`,
// regenerated from the backend's `built_in_provider_specs()` in
// `src/core/providers.rs` (the single source of truth) by
// `scripts/gen-provider-catalog.py`. This file only keeps the types,
// helpers, and the re-export — do not hand-edit the catalog entries here.

export interface ProviderCatalogSpec {
  name: string;
  type: string;
  group?: string;
  model?: string;
  api_key_env?: string;
  secret_key_env?: string;
  url?: string;
  chat_path?: string;
  anthropic_version?: string;
  max_tokens?: number;
  supports_system?: boolean;
}

export interface ProviderCatalogEntry {
  name: string;
  agentType: string;
  group?: string;
  defaultModel?: string;
  apiKeyEnv?: string;
  secretKeyEnv?: string;
  url?: string;
  chatPath?: string;
  supportsSystem?: boolean;
  configuredModel?: string;
  configuredEnvVar?: string;
}

export interface ProviderConfigSnapshot {
  model?: string;
  envVar?: string;
}

export interface ProviderSecretTarget {
  name: string;
  envVar: string;
}

/**
 * Built-in provider catalog — the default set of AI providers supported
 * out of the box. Each entry includes connection details, env var names,
 * and default model selections.
 *
 * Generated from the backend authority — see
 * `vscode-addon/src/settings/providerCatalog.generated.ts`.
 */
export { BUILTIN_PROVIDER_CATALOG } from "./providerCatalog.generated";

/** Infer an env var name from a provider name (e.g. "openai" → "OPENAI_API_KEY"). */
export function inferEnvVar(providerName: string): string {
  return `${providerName
    .trim()
    .toUpperCase()
    .replace(/[-\s]+/g, "_")}_API_KEY`;
}

/** Parse an unknown value into a ProviderCatalogSpec (or null). */
export function asCatalogSpec(value: unknown): ProviderCatalogSpec | null {
  const record: Record<string, unknown> =
    typeof value === "object" && value !== null
      ? (value as Record<string, unknown>)
      : {};
  const name = String(record.name || "").trim();
  const type = String(record.type || record.agent_type || "").trim();
  if (!name || !type) {
    return null;
  }
  const parseString = (raw: unknown): string | undefined => {
    const normalized = String(raw || "").trim();
    return normalized ? normalized : undefined;
  };
  const parseBool = (raw: unknown): boolean | undefined =>
    typeof raw === "boolean" ? raw : undefined;
  const parseNumber = (raw: unknown): number | undefined =>
    typeof raw === "number" && Number.isFinite(raw) ? raw : undefined;

  return {
    name,
    type,
    group: parseString(record.group),
    model: parseString(record.model),
    api_key_env: parseString(record.api_key_env),
    secret_key_env: parseString(record.secret_key_env),
    url: parseString(record.url),
    chat_path: parseString(record.chat_path),
    anthropic_version: parseString(record.anthropic_version),
    max_tokens: parseNumber(record.max_tokens),
    supports_system: parseBool(record.supports_system),
  };
}

/** Deduplicate a catalog by provider name (last wins). */
export function dedupeCatalog(
  catalog: ProviderCatalogSpec[],
): ProviderCatalogSpec[] {
  const byName = new Map<string, ProviderCatalogSpec>();
  for (const spec of catalog) {
    const key = spec.name || spec.type;
    byName.set(key, spec);
  }
  return Array.from(byName.values());
}

/**
 * Collect all unique secret targets from a catalog.
 * Returns sorted array of {name, envVar} for keyring operations.
 */
export function collectProviderSecretTargets(
  catalog: ProviderCatalogSpec[],
): ProviderSecretTarget[] {
  const targets = new Map<string, ProviderSecretTarget>();

  for (const spec of catalog) {
    for (const envVar of [spec.api_key_env, spec.secret_key_env]) {
      const normalized = String(envVar || "").trim();
      if (!normalized) {
        continue;
      }
      // Use the direct env var name as the secret name
      const secretName = normalized.toLowerCase();
      if (!secretName || targets.has(secretName)) {
        continue;
      }
      targets.set(secretName, {
        name: secretName,
        envVar: normalized,
      });
    }
  }

  return Array.from(targets.values()).sort((left, right) =>
    left.name.localeCompare(right.name),
  );
}
