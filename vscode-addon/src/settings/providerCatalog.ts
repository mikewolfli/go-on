// ── Provider Catalog ──

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

export type ProviderGroupKey = "openai" | "chinese" | "other";

/**
 * Built-in provider catalog — the default set of AI providers supported
 * out of the box. Each entry includes connection details, env var names,
 * and default model selections.
 */
export const BUILTIN_PROVIDER_CATALOG: ProviderCatalogSpec[] = [
  {
    name: "openai",
    type: "openai",
    group: "openai",
    url: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    api_key_env: "OPENAI_API_KEY",
    supports_system: true,
  },
  {
    name: "openai_compatible",
    type: "openai_compatible",
    group: "openai",
    url: "http://127.0.0.1:8080/v1",
    model: "compatible-model",
    api_key_env: "OPENAI_COMPATIBLE_API_KEY",
    supports_system: true,
  },
  {
    name: "anthropic",
    type: "claude",
    group: "openai",
    url: "https://api.anthropic.com",
    model: "claude-sonnet-4-20250514",
    api_key_env: "ANTHROPIC_API_KEY",
    anthropic_version: "2023-06-01",
    max_tokens: 8192,
    supports_system: true,
  },
  {
    name: "cohere",
    type: "cohere",
    group: "openai",
    url: "https://api.cohere.ai/v1",
    model: "command-r-plus-08-2024",
    api_key_env: "COHERE_API_KEY",
    supports_system: true,
  },
  {
    name: "deepseek",
    type: "deepseek",
    group: "chinese",
    url: "https://api.deepseek.com",
    model: "deepseek-v4-flash",
    api_key_env: "DEEPSEEK_API_KEY",
    supports_system: true,
  },
  {
    name: "wenxin",
    type: "wenxin",
    group: "chinese",
    model: "ERNIE-4.5-8K",
    api_key_env: "WENXIN_API_KEY",
    secret_key_env: "WENXIN_SECRET_KEY",
  },
  {
    name: "qianfan",
    type: "qianfan",
    group: "chinese",
    model: "ERNIE-4.5-8K",
    api_key_env: "QIANFAN_API_KEY",
    secret_key_env: "QIANFAN_SECRET_KEY",
  },
  {
    name: "qwen",
    type: "qwen",
    group: "chinese",
    url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    model: "qwen-max",
    api_key_env: "QWEN_API_KEY",
    supports_system: true,
  },
  {
    name: "glm",
    type: "glm",
    group: "chinese",
    url: "https://open.bigmodel.cn/api/paas/v4",
    model: "glm-4-flash",
    api_key_env: "GLM_API_KEY",
  },
  {
    name: "yi",
    type: "yi",
    group: "chinese",
    url: "https://api.lingyiwanwu.com/v1",
    model: "yi-lightning",
    api_key_env: "YI_API_KEY",
  },
  {
    name: "hunyuan",
    type: "hunyuan",
    group: "chinese",
    url: "https://api.hunyuan.cloud.tencent.com/v1",
    model: "hunyuan-turbo-latest",
    api_key_env: "HUNYUAN_API_KEY",
  },
  {
    name: "doubao",
    type: "doubao",
    group: "chinese",
    url: "https://ark.cn-beijing.volces.com/api/v3",
    chat_path: "/chat/completions",
    model: "doubao-1.5-pro-256k-250115",
    api_key_env: "DOUBAO_API_KEY",
    supports_system: true,
  },
  {
    name: "facewall",
    type: "facewall",
    group: "chinese",
    url: "https://api.facewall.ai/v1",
    model: "facewall-chat",
    api_key_env: "FACEWALL_API_KEY",
  },
  {
    name: "langboat",
    type: "langboat",
    group: "chinese",
    url: "https://api.langboat.com/v1",
    model: "langboat-chat",
    api_key_env: "LANGBOAT_API_KEY",
  },
  {
    name: "skywork",
    type: "skywork",
    group: "chinese",
    url: "https://api.skywork.ai/v1",
    model: "skywork-chat",
    api_key_env: "SKYWORK_API_KEY",
  },
  {
    name: "stepfun",
    type: "stepfun",
    group: "chinese",
    url: "https://api.stepfun.com/v1",
    model: "step-2-16k",
    api_key_env: "STEPFUN_API_KEY",
  },
  {
    name: "xihu",
    type: "xihu",
    group: "chinese",
    url: "https://api.xihu.ai/v1",
    model: "xihu-chat",
    api_key_env: "XIHU_API_KEY",
  },
  {
    name: "moonshot",
    type: "moonshot",
    group: "chinese",
    url: "https://api.moonshot.cn/v1",
    model: "moonshot-v1-8k",
    api_key_env: "MOONSHOT_API_KEY",
  },
  {
    name: "minimax",
    type: "minimax",
    group: "chinese",
    url: "https://api.minimax.chat/v1",
    model: "MiniMax-Text-01",
    api_key_env: "MINIMAX_API_KEY",
  },
  {
    name: "ai21",
    type: "ai21",
    group: "other",
    url: "https://api.ai21.com/studio/v1",
    model: "jamba-1.5-mini",
    api_key_env: "AI21_API_KEY",
  },
  {
    name: "aleph",
    type: "aleph",
    group: "other",
    url: "https://api.aleph-alpha.com",
    model: "luminous-base",
    api_key_env: "ALEPH_API_KEY",
  },
  {
    name: "copilot",
    type: "copilot",
    group: "other",
    url: "http://127.0.0.1:8080",
    api_key_env: "GITHUB_COPILOT_TOKEN",
    supports_system: true,
  },
  {
    name: "deepquest",
    type: "deepquest",
    group: "other",
    url: "https://api.deepquest.ai/v1",
    model: "deepquest-chat",
    api_key_env: "DEEPQUEST_API_KEY",
  },
  {
    name: "fireworks",
    type: "fireworks",
    group: "other",
    url: "https://api.fireworks.ai/inference/v1",
    model: "accounts/fireworks/models/llama-v3p1-8b-instruct",
    api_key_env: "FIREWORKS_API_KEY",
  },
  {
    name: "gemini",
    type: "gemini",
    group: "other",
    url: "https://generativelanguage.googleapis.com/v1beta",
    model: "gemini-2.5-flash",
    api_key_env: "GEMINI_API_KEY",
  },
  {
    name: "groq",
    type: "groq",
    group: "other",
    url: "https://api.groq.com/openai/v1",
    model: "llama-3.3-70b-versatile",
    api_key_env: "GROQ_API_KEY",
  },
  {
    name: "llama",
    type: "llama",
    group: "other",
    url: "http://127.0.0.1:11434/v1",
    model: "llama3.2",
    supports_system: true,
  },
  {
    name: "loopai",
    type: "loopai",
    group: "other",
    url: "https://api.loopai.com/v1",
    model: "loopai-chat",
    api_key_env: "LOOPAI_API_KEY",
  },
  {
    name: "mistral",
    type: "mistral",
    group: "other",
    url: "https://api.mistral.ai/v1",
    model: "mistral-small-latest",
    api_key_env: "MISTRAL_API_KEY",
  },
  {
    name: "nim",
    type: "nim",
    group: "other",
    url: "https://integrate.api.nvidia.com/v1",
    model: "meta/llama-3.1-70b-instruct",
    api_key_env: "NIM_API_KEY",
  },
  {
    name: "perplexity",
    type: "perplexity",
    group: "other",
    url: "https://api.perplexity.ai",
    model: "sonar-pro",
    api_key_env: "PERPLEXITY_API_KEY",
  },
  {
    name: "replicate",
    type: "replicate",
    group: "other",
    url: "https://api.replicate.com/v1",
    model: "meta/meta-llama-3-70b-instruct",
    api_key_env: "REPLICATE_API_TOKEN",
  },
  {
    name: "titan",
    type: "titan",
    group: "other",
    url: "https://api.titanml.co/v1",
    model: "titan-chat",
    api_key_env: "TITAN_API_KEY",
  },
  {
    name: "together",
    type: "together",
    group: "other",
    url: "https://api.together.xyz/v1",
    model: "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
    api_key_env: "TOGETHER_API_KEY",
  },
  {
    name: "xai",
    type: "openai_compatible",
    group: "other",
    url: "https://api.x.ai/v1",
    model: "grok-3",
    api_key_env: "XAI_API_KEY",
    supports_system: true,
  },
];

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
