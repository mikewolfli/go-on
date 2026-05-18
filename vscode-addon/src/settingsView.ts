import * as vscode from "vscode";
import { spawn } from "child_process";
import * as fs from "fs/promises";
import * as http from "http";
import * as https from "https";
import * as path from "path";
import { i18n, MessageKeys } from "./i18n";
import { configManager } from "./configManager";
import { RuntimeManagerLike } from "./managerTypes";
import { normalizeProtocolMode } from "./protocolContract";
import { ensureGoOnBinary } from "./runtimeBinaryService";
import { getNonce, asRecord } from "./utils";

interface ProviderCatalogSpec {
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

interface ProviderCatalogEntry {
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

interface ProviderConfigSnapshot {
  model?: string;
  envVar?: string;
}

interface ProviderSecretTarget {
  name: string;
  envVar: string;
}

interface PersistedCopilotState {
  authMode?: string;
  accountLabel?: string;
  oauthClientId?: string;
  lastError?: string;
  lastStatus?: string;
}

interface CopilotAuthState {
  isAuthorized: boolean;
  authMode: string;
  accountLabel: string;
  oauthClientId: string;
  pending: boolean;
  statusMessage: string;
  lastError: string;
  userCode?: string;
  verificationUri?: string;
  expiresAt?: number;
  modelSource?: string;
  modelCount?: number;
}

interface ProviderModelResolution {
  modelOptions: string[];
  copilotAuth?: CopilotAuthState;
}

const BUILTIN_PROVIDER_CATALOG: ProviderCatalogSpec[] = [
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

interface CopilotTokenExchange {
  token: string;
  expiresAt: number;
}

interface CopilotModelCache {
  models: string[];
  fetchedAt: number;
}

interface PendingCopilotDeviceAuth {
  cancelRequested: boolean;
  userCode: string;
  verificationUri: string;
  expiresAt: number;
}

interface DeviceCodeResponse {
  device_code?: string;
  user_code?: string;
  verification_uri?: string;
  expires_in?: number;
  interval?: number;
}

interface HttpJsonResponse {
  status: number;
  bodyText: string;
  body: unknown;
}

const COPILOT_ENV_VAR = "GITHUB_COPILOT_TOKEN";
const COPILOT_SECRET_NAME = "github_copilot_token";
const COPILOT_TOKEN_URL = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_MODELS_URL = "https://api.githubcopilot.com/models";
const GITHUB_DEVICE_CODE_URL = "https://github.com/login/device/code";
const GITHUB_ACCESS_TOKEN_URL = "https://github.com/login/oauth/access_token";
const COPILOT_MODEL_CACHE_KEY = "go-on.copilot.modelsCache.v1";
const COPILOT_STATE_KEY = "go-on.copilot.authState.v1";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function createTransport(urlValue: URL): typeof http | typeof https {
  return urlValue.protocol === "http:" ? http : https;
}

async function requestJson(
  urlString: string,
  options: {
    method?: string;
    headers?: Record<string, string>;
    body?: string;
  } = {},
): Promise<HttpJsonResponse> {
  const target = new URL(urlString);
  const body = options.body ?? "";
  const headers: Record<string, string> = {
    Accept: "application/json",
    ...(options.headers || {}),
  };

  if (body && headers["Content-Length"] === undefined) {
    headers["Content-Length"] = Buffer.byteLength(body).toString();
  }

  return new Promise<HttpJsonResponse>((resolve, reject) => {
    const req = createTransport(target).request(
      target,
      {
        method: options.method || "GET",
        headers,
      },
      (res) => {
        let chunks = "";
        res.setEncoding("utf8");
        res.on("data", (chunk: string) => {
          chunks += chunk;
        });
        res.on("end", () => {
          let parsed: unknown = undefined;
          if (chunks.trim()) {
            try {
              parsed = JSON.parse(chunks);
            } catch {
              parsed = undefined;
            }
          }
          resolve({
            status: res.statusCode || 0,
            bodyText: chunks,
            body: parsed,
          });
        });
      },
    );

    req.on("error", reject);
    if (body) {
      req.write(body);
    }
    req.end();
  });
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function inferEnvVar(providerName: string): string {
  return `${providerName
    .trim()
    .toUpperCase()
    .replace(/[-\s]+/g, "_")}_API_KEY`;
}

function secretNameForEnvVar(envVar: string): string {
  const normalized = String(envVar || "").trim();
  if (!normalized) {
    return "";
  }
  if (normalized === "GITHUB_COPILOT_TOKEN") {
    return "github_copilot_token";
  }
  return normalized.toLowerCase();
}

function collectProviderSecretTargets(
  catalog: ProviderCatalogSpec[],
): ProviderSecretTarget[] {
  const targets = new Map<string, ProviderSecretTarget>();

  for (const spec of catalog) {
    for (const envVar of [spec.api_key_env, spec.secret_key_env]) {
      const normalized = String(envVar || "").trim();
      if (!normalized) {
        continue;
      }
      const secretName = secretNameForEnvVar(normalized);
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

function asCatalogSpec(value: unknown): ProviderCatalogSpec | null {
  const record = asRecord(value);
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

function dedupeCatalog(specs: ProviderCatalogSpec[]): ProviderCatalogSpec[] {
  const byName = new Map<string, ProviderCatalogSpec>();
  for (const spec of specs) {
    const key = spec.name.trim();
    if (!key || byName.has(key)) {
      continue;
    }
    byName.set(key, spec);
  }
  return Array.from(byName.values()).sort((a, b) =>
    a.name.localeCompare(b.name),
  );
}

function parseConfiguredAgents(
  content: string,
): Map<string, ProviderConfigSnapshot> {
  const result = new Map<string, ProviderConfigSnapshot>();
  const sectionRegex = /^\[agents\.([^\]]+)\]([\s\S]*?)(?=^\[[^\]]+\]|$)/gm;

  for (const match of content.matchAll(sectionRegex)) {
    const name = String(match[1] || "").trim();
    const section = String(match[2] || "");
    const model = section.match(/^model\s*=\s*"([^"]*)"\s*$/m)?.[1];
    const apiKeyEnv = section.match(/^api_key_env\s*=\s*"([^"]*)"\s*$/m)?.[1];
    const secretKeyEnv = section.match(
      /^secret_key_env\s*=\s*"([^"]*)"\s*$/m,
    )?.[1];

    result.set(name, {
      model,
      envVar: apiKeyEnv || secretKeyEnv,
    });
  }

  return result;
}

function formatTomlValue(value: unknown): string {
  if (typeof value === "string") {
    return `"${value.replace(/"/g, '\\"')}"`;
  }
  if (typeof value === "boolean" || typeof value === "number") {
    return String(value);
  }
  return `"${String(value)}"`;
}

function upsertSectionLine(
  section: string,
  key: string,
  value: unknown,
): string {
  const keyRegex = new RegExp(`^${escapeRegex(key)}\\s*=.*$`, "m");
  const line = `${key} = ${formatTomlValue(value)}`;
  if (keyRegex.test(section)) {
    return section.replace(keyRegex, line);
  }
  const lines = section.split("\n");
  lines.splice(1, 0, line);
  return lines.join("\n");
}

function removeSectionLine(section: string, key: string): string {
  const keyRegex = new RegExp(`^${escapeRegex(key)}\\s*=.*(?:\\r?\\n)?`, "m");
  return section.replace(keyRegex, "");
}

function upsertAgentSection(
  content: string,
  providerName: string,
  fields: Record<string, unknown>,
): string {
  const header = `[agents.${providerName}]`;
  const sectionRegex = new RegExp(
    `^${escapeRegex(header)}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );

  const applyFields = (section: string) => {
    let updated = section;
    for (const [key, value] of Object.entries(fields)) {
      if (value === undefined || value === null || value === "") {
        updated = removeSectionLine(updated, key);
      } else {
        updated = upsertSectionLine(updated, key, value);
      }
    }
    return updated;
  };

  if (sectionRegex.test(content)) {
    return content.replace(sectionRegex, (section) => applyFields(section));
  }

  let section = `${header}\n`;
  for (const [key, value] of Object.entries(fields)) {
    if (value !== undefined && value !== null && value !== "") {
      section += `${key} = ${formatTomlValue(value)}\n`;
    }
  }

  return `${content.trimEnd()}\n\n${section}`;
}

function upsertPhaseAgents(
  content: string,
  phase: string,
  agents: string[],
): string {
  const header = `[phases.${phase}]`;
  const sectionRegex = new RegExp(
    `^${escapeRegex(header)}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );
  const agentsLine = `agents = [${agents.map((agent) => `"${agent}"`).join(", ")}]`;

  if (sectionRegex.test(content)) {
    return content.replace(sectionRegex, (section) => {
      const agentsRegex = /^agents\s*=\s*\[[^\]]*\]\s*$/m;
      if (agentsRegex.test(section)) {
        return section.replace(agentsRegex, agentsLine);
      }
      const lines = section.split("\n");
      lines.splice(1, 0, agentsLine);
      return lines.join("\n");
    });
  }

  return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\n${agentsLine}\nfallback = true\n`;
}

export class GoOnSettingsViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "go-on-settings";
  private _view?: vscode.WebviewView;
  private _runtimeFeatures: Record<string, boolean> = {};
  private _messageSubscription?: vscode.Disposable;
  private _pendingCopilotDeviceAuth?: PendingCopilotDeviceAuth;
  private readonly manager: RuntimeManagerLike;
  private readonly context: vscode.ExtensionContext;
  private readonly _commandMessageMap: Record<string, string> = {
    startGoOn: "go-on.start",
    stopGoOn: "go-on.stop",
    healthCheck: "go-on.healthCheck",
    healthProbes: "go-on.healthProbes",
    clearCache: "go-on.cacheClear",
    breakerStatus: "go-on.breakerStatus",
    breakerRecovery: "go-on.breakerRecovery",
    observabilityAlerts: "go-on.observabilityAlerts",
    securityBaseline: "go-on.securityBaseline",
    harnessStatus: "go-on.harnessStatus",
    clearVector: "go-on.vectorClear",
    reloadConfig: "go-on.configReload",
    workflowExecute: "go-on.workflowExecute",
    taskPlan: "go-on.taskPlan",
    taskExecute: "go-on.taskExecute",
    learningSummary: "go-on.learningSummary",
    learningGuardrail: "go-on.learningGuardrail",
    learningReplay: "go-on.learningReplay",
    knowledgeDistill: "go-on.knowledgeDistill",
    rlAlignmentEval: "go-on.rlAlignmentEval",
    hardnessStatus: "go-on.hardnessStatus",
    costStatus: "go-on.costStatus",
    configBaseline: "go-on.configBaseline",
    errorContract: "go-on.errorContract",
    buildRepro: "go-on.buildRepro",
    dataLifecycle: "go-on.dataLifecycle",
    optimizationPeak: "go-on.optimizationPeak",
    releaseReadiness: "go-on.releaseReadiness",
    runtimeStability: "go-on.runtimeStability",
    autotuneStatus: "go-on.autotuneStatus",
    governanceStatus: "go-on.governanceStatus",
    governancePlanGet: "go-on.governancePlanGet",
    governanceAuditRecent: "go-on.governanceAuditRecent",
    lockStatus: "go-on.lockStatus",
    debugPanelGet: "go-on.debugPanelGet",
    actionCheck: "go-on.actionCheck",
  };

  constructor(
    private readonly _extensionUri: vscode.Uri,
    _manager: RuntimeManagerLike,
    _context: vscode.ExtensionContext,
  ) {
    this.manager = _manager;
    this.context = _context;
    this.context.subscriptions.push(
      new vscode.Disposable(() => this._messageSubscription?.dispose()),
    );
  }

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken,
  ) {
    this._view = webviewView;

    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this._extensionUri],
    };

    webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);

    this._messageSubscription?.dispose();
    this._messageSubscription = webviewView.webview.onDidReceiveMessage(
      async (message: Record<string, unknown>) => {
        try {
          await this._handleWebviewMessage(message);
        } catch (error: unknown) {
          this._postMessage({
            type: "settingsActionError",
            message: this._getErrorMessage(error),
          });
        }
      },
      undefined,
    );

    this._sendCurrentSettings();
    // Request webview to scroll to credentials section
    this._postMessage({ type: "focusCredentials" });
    if (this.manager.isRunning?.()) {
      this._refreshRuntimeFeatures().catch(() => undefined);
    }
  }

  private async _refreshRuntimeFeatures(): Promise<void> {
    try {
      const response = (await this.manager.sendRequest(
        "runtime.features",
        {},
      )) as Record<string, unknown>;
      const features = (
        typeof response === "object" && response !== null
          ? response["features"]
          : undefined
      ) as Record<string, boolean> | undefined;
      if (features && typeof features === "object") {
        this._runtimeFeatures = features;
        this._view?.webview.postMessage({
          type: "runtimeFeatures",
          features: this._runtimeFeatures,
        });
      }
    } catch {
      // not fatal — keep previous features
    }
  }

  private _getErrorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  private async _handleWebviewMessage(message: Record<string, unknown>) {
    const messageType = String(message.type ?? "");
    const handlers: Record<
      string,
      (_msg: Record<string, unknown>) => Promise<void> | void
    > = {
      requestSettings: async (_message) => this._sendCurrentSettings(),
      openConfigWizard: async () => this.showConfigWizard(),
      updateSetting: async (msg) =>
        this._handleGenericSettingUpdate(String(msg.key ?? ""), msg.value),
      updateRuntimeSetting: async (msg) =>
        this._updateRuntimeSetting(String(msg.key ?? ""), msg.value),
      updateCacheSetting: async (msg) =>
        this._updateCacheSetting(String(msg.key ?? ""), msg.value),
      updateVectorSetting: async (msg) =>
        this._updateVectorSetting(String(msg.key ?? ""), msg.value),
      updateAutotuneSetting: async (msg) =>
        this._updateAutotuneSetting(String(msg.key ?? ""), msg.value),
      requestProviderModels: async (msg) =>
        this._sendProviderModels(String(msg.provider ?? "")),
      authorizeCopilotGitHubSession: async () =>
        this._authorizeCopilotWithGitHubSession(),
      authorizeCopilotDeviceFlow: async (msg) =>
        this._startCopilotDeviceAuthorization(String(msg.oauthClientId ?? "")),
      cancelCopilotDeviceFlow: async () =>
        this._cancelCopilotDeviceAuthorization(),
      deleteCopilotAuthorization: async () =>
        this._deleteCopilotAuthorization(),
      saveProviderSelection: async (msg) =>
        this._saveProviderSelection(
          String(msg.provider ?? ""),
          String(msg.model ?? "auto"),
          msg.envVar ? String(msg.envVar) : undefined,
        ),
      addAgent: async (msg) =>
        this._addAgent(String(msg.name ?? ""), msg.config),
      deleteAgent: async (msg) => this._deleteAgent(String(msg.name ?? "")),
      updatePhase: async (msg) =>
        this._updatePhase(String(msg.name ?? ""), msg.config),
      setLanguage: async (msg) => this._setLanguage(String(msg.language ?? "")),
      setKeyringSecret: async (msg) =>
        this._handleKeyringSet(String(msg.name ?? ""), String(msg.value ?? "")),
      getKeyringSecret: async (msg) =>
        this._handleKeyringGet(String(msg.name ?? "")),
      deleteKeyringSecret: async (msg) =>
        this._handleKeyringDelete(String(msg.name ?? "")),
      listKeyringSecrets: async () => this._handleKeyringList(),
      quickSetupProvider: async (msg) =>
        this._handleQuickSetupProvider(
          String(msg.provider ?? ""),
          String(msg.apiKey ?? ""),
        ),
      applyDefaultConfigTemplate: async (msg) =>
        this._handleApplyDefaultConfigTemplate(String(msg.template ?? "")),
      applyRulesSettings: async (msg) =>
        this._handleApplyRulesSettings(
          (msg.payload as {
            globalRules?: string[];
            commonRules?: string[];
            phaseRules?: Record<string, string[]>;
          }) || {},
        ),
      applyWorkflowMapping: async (msg) =>
        this._handleApplyWorkflowMapping(
          (msg.payload as {
            defaultPhase?: string;
            phases?: Record<
              string,
              {
                agents: string[];
                fallback?: boolean;
                principles?: string[];
                switchRules?: {
                  circuitBreakerFailures?: number;
                  circuitBreakerOpenSeconds?: number;
                };
              }
            >;
          }) || {},
        ),
    };

    const messageHandler = handlers[messageType];
    if (messageHandler) {
      await messageHandler(message);
      return;
    }

    const command = this._commandMessageMap[messageType];
    if (command) {
      await vscode.commands.executeCommand(command);
      return;
    }
  }

  private async _handleGenericSettingUpdate(key: string, value: unknown) {
    if (!key.startsWith("go-on.")) {
      return;
    }

    const goOnConfig = vscode.workspace.getConfiguration("go-on");
    const relativeKey = key.replace(/^go-on\./, "");

    // Keys with runtime./cache./vector./autotune. prefixes are TOML-only settings;
    // skip the VS Code config update to avoid dual-write race conditions.
    const isTomlOnly =
      relativeKey.startsWith("runtime.") ||
      relativeKey.startsWith("cache.") ||
      relativeKey.startsWith("vector.") ||
      relativeKey.startsWith("autotune.");

    if (!isTomlOnly) {
      await goOnConfig.update(
        relativeKey,
        value,
        vscode.ConfigurationTarget.Workspace,
      );
    }

    if (relativeKey.startsWith("runtime.")) {
      await this._updateRuntimeSetting(
        relativeKey.replace(/^runtime\./, ""),
        value,
      );
      return;
    }
    if (relativeKey.startsWith("cache.")) {
      await this._updateCacheSetting(
        relativeKey.replace(/^cache\./, ""),
        value,
      );
      return;
    }
    if (relativeKey.startsWith("vector.")) {
      await this._updateVectorSetting(
        relativeKey.replace(/^vector\./, ""),
        value,
      );
      return;
    }
    if (relativeKey.startsWith("autotune.")) {
      await this._updateAutotuneSetting(
        relativeKey.replace(/^autotune\./, ""),
        value,
      );
      return;
    }

    vscode.window.showInformationMessage(
      i18n.getMessage(MessageKeys.successfullySaved),
    );
  }

  private _workspaceRoot(): string {
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!root) {
      throw new Error("No workspace folder open.");
    }
    return root;
  }

  private _resolveConfigPath(): string {
    const root = this._workspaceRoot();
    const configured =
      vscode.workspace
        .getConfiguration("go-on")
        .get<string>("configPath", "./config.toml") || "./config.toml";
    return path.isAbsolute(configured)
      ? configured
      : path.resolve(root, configured);
  }

  private async _runSecretCommand(
    action: "set" | "get" | "delete",
    secretName: string,
    secretValue?: string,
  ): Promise<string> {
    const config = vscode.workspace.getConfiguration("go-on");
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const runtime = await ensureGoOnBinary(workspaceRoot, config, this.context);

    const args: string[] = ["--secret", action, "--secret-name", secretName];
    if (secretValue !== undefined) {
      args.push("--secret-value", secretValue);
    }

    return new Promise<string>((resolve, reject) => {
      const proc = spawn(runtime.executablePath, args, {
        cwd: workspaceRoot || runtime.runtimeDir,
        stdio: ["ignore", "pipe", "pipe"],
      });

      let stdout = "";
      let stderr = "";

      proc.stdout?.on("data", (chunk: Buffer) => {
        stdout += chunk.toString();
      });

      proc.stderr?.on("data", (chunk: Buffer) => {
        stderr += chunk.toString();
      });

      proc.on("error", reject);
      proc.on("close", (code) => {
        if (code === 0) {
          resolve(stdout.trim());
          return;
        }
        reject(
          new Error(
            `go-on secret command failed: ${(stderr || stdout || `exit code ${code}`).trim()}`,
          ),
        );
      });
    });
  }

  private async _readCopilotToken(): Promise<string | undefined> {
    try {
      const token = await this._runSecretCommand("get", COPILOT_SECRET_NAME);
      return token.trim() || undefined;
    } catch {
      return undefined;
    }
  }

  private async _writeCopilotToken(token: string): Promise<void> {
    await this._runSecretCommand("set", COPILOT_SECRET_NAME, token);
    this.manager.setRuntimeEnvOverrides?.({
      [COPILOT_ENV_VAR]: token,
    });
  }

  private _loadPersistedCopilotState(): PersistedCopilotState {
    return (
      this.context.workspaceState.get<PersistedCopilotState>(
        COPILOT_STATE_KEY,
      ) || {}
    );
  }

  private async _updatePersistedCopilotState(
    patch: Partial<PersistedCopilotState>,
  ): Promise<void> {
    await this.context.workspaceState.update(COPILOT_STATE_KEY, {
      ...this._loadPersistedCopilotState(),
      ...patch,
    });
  }

  private _baseCopilotAuthState(
    partial?: Partial<CopilotAuthState>,
  ): CopilotAuthState {
    const stored = this._loadPersistedCopilotState();
    return {
      isAuthorized: false,
      authMode: stored.authMode || "none",
      accountLabel: stored.accountLabel || "",
      oauthClientId: stored.oauthClientId || "",
      pending: false,
      statusMessage: stored.lastStatus || "",
      lastError: stored.lastError || "",
      ...partial,
    };
  }

  private async _currentCopilotAuthState(
    partial?: Partial<CopilotAuthState>,
  ): Promise<CopilotAuthState> {
    const token = await this._readCopilotToken();
    const pending = this._pendingCopilotDeviceAuth;
    const state = this._baseCopilotAuthState({
      isAuthorized: Boolean(token),
      pending: Boolean(pending),
      userCode: pending?.userCode,
      verificationUri: pending?.verificationUri,
      expiresAt: pending?.expiresAt,
      ...partial,
    });

    if (!state.statusMessage) {
      state.statusMessage = state.isAuthorized
        ? "GitHub token is stored and ready for Copilot exchange."
        : "Authorize GitHub Copilot to fetch models and enable runtime requests.";
    }

    return state;
  }

  private async _postCopilotAuthState(
    partial?: Partial<CopilotAuthState>,
  ): Promise<void> {
    this._postMessage({
      type: "copilotAuthState",
      auth: await this._currentCopilotAuthState(partial),
    });
  }

  private async _exchangeCopilotToken(
    githubToken: string,
  ): Promise<CopilotTokenExchange> {
    const response = await requestJson(COPILOT_TOKEN_URL, {
      headers: {
        Authorization: `token ${githubToken}`,
        "User-Agent": "go-on-vscode/1.0",
      },
    });

    if (response.status < 200 || response.status >= 300) {
      throw new Error(
        `Copilot token exchange failed (${response.status}): ${response.bodyText || "empty response"}`,
      );
    }

    const body = asRecord(response.body);
    const token = typeof body.token === "string" ? body.token.trim() : "";
    if (!token) {
      throw new Error("Copilot token exchange returned no token field.");
    }

    return {
      token,
      expiresAt:
        typeof body.expires_at === "number"
          ? body.expires_at
          : Math.floor(Date.now() / 1000) + 1500,
    };
  }

  private _extractCopilotModelIds(payload: unknown): string[] {
    const result = new Set<string>();
    const root = asRecord(payload);
    const candidates = Array.isArray(payload)
      ? payload
      : Array.isArray(root.data)
        ? root.data
        : Array.isArray(root.models)
          ? root.models
          : [];

    for (const candidate of candidates) {
      if (typeof candidate === "string" && candidate.trim()) {
        result.add(candidate.trim());
        continue;
      }
      const record = asRecord(candidate);
      const fields = [record.id, record.model, record.model_id, record.name];
      for (const field of fields) {
        if (typeof field === "string" && field.trim()) {
          result.add(field.trim());
          break;
        }
      }
    }

    return Array.from(result.values()).sort((left, right) =>
      left.localeCompare(right),
    );
  }

  private _loadCopilotModelCache(): CopilotModelCache | undefined {
    return this.context.workspaceState.get<CopilotModelCache>(
      COPILOT_MODEL_CACHE_KEY,
    );
  }

  private async _storeCopilotModelCache(models: string[]): Promise<void> {
    await this.context.workspaceState.update(COPILOT_MODEL_CACHE_KEY, {
      models,
      fetchedAt: Date.now(),
    });
  }

  private async _resolveCopilotModels(): Promise<ProviderModelResolution> {
    const githubToken = await this._readCopilotToken();
    if (!githubToken) {
      const cached = this._loadCopilotModelCache();
      return {
        modelOptions: cached?.models || [],
        copilotAuth: await this._currentCopilotAuthState({
          modelSource: cached?.models?.length ? "cache" : "config",
          modelCount: cached?.models?.length || 0,
        }),
      };
    }

    try {
      const copilotToken = await this._exchangeCopilotToken(githubToken);
      const response = await requestJson(COPILOT_MODELS_URL, {
        headers: {
          Authorization: `Bearer ${copilotToken.token}`,
          "User-Agent": "go-on-vscode/1.0",
          "Editor-Version": "vscode/1.90.0",
          "Editor-Plugin-Version": "copilot-chat/0.17.0",
          "Copilot-Integration-Id": "copilot-chat",
        },
      });

      if (response.status < 200 || response.status >= 300) {
        throw new Error(
          `Copilot model request failed (${response.status}): ${response.bodyText || "empty response"}`,
        );
      }

      const models = this._extractCopilotModelIds(response.body);
      if (models.length === 0) {
        throw new Error("Copilot model request returned no model identifiers.");
      }

      await this._storeCopilotModelCache(models);
      await this._updatePersistedCopilotState({
        lastError: "",
        lastStatus: `Fetched ${models.length} Copilot models from GitHub.`,
      });
      return {
        modelOptions: models,
        copilotAuth: await this._currentCopilotAuthState({
          isAuthorized: true,
          modelSource: "network",
          modelCount: models.length,
          statusMessage: `Fetched ${models.length} Copilot models from GitHub.`,
          lastError: "",
        }),
      };
    } catch (error: unknown) {
      const cached = this._loadCopilotModelCache();
      const message = errorMessage(error);
      await this._updatePersistedCopilotState({
        lastError: message,
        lastStatus: cached?.models?.length
          ? "Using cached Copilot models after refresh failure."
          : "Copilot model refresh failed.",
      });
      return {
        modelOptions: cached?.models || [],
        copilotAuth: await this._currentCopilotAuthState({
          isAuthorized: true,
          modelSource: cached?.models?.length ? "cache" : "config",
          modelCount: cached?.models?.length || 0,
          statusMessage: cached?.models?.length
            ? "Using cached Copilot models after refresh failure."
            : "Copilot model refresh failed.",
          lastError: message,
        }),
      };
    }
  }

  private async _authorizeCopilotWithGitHubSession(): Promise<void> {
    const session = await vscode.authentication.getSession(
      "github",
      ["read:user"],
      { createIfNone: true },
    );
    if (!session?.accessToken) {
      throw new Error("GitHub authentication did not return an access token.");
    }

    await this._exchangeCopilotToken(session.accessToken);
    await this._writeCopilotToken(session.accessToken);
    await this._updatePersistedCopilotState({
      authMode: "github-session",
      accountLabel: session.account.label,
      lastError: "",
      lastStatus: `Authenticated as ${session.account.label}.`,
    });
    await this._postCopilotAuthState({
      isAuthorized: true,
      authMode: "github-session",
      accountLabel: session.account.label,
      statusMessage: `Authenticated as ${session.account.label}.`,
      lastError: "",
    });
    await this._sendProviderModels("copilot");
  }

  private async _startCopilotDeviceAuthorization(
    oauthClientId: string,
  ): Promise<void> {
    const clientId = oauthClientId.trim();
    if (!clientId) {
      throw new Error(
        "GitHub OAuth client ID is required for device authorization.",
      );
    }

    const response = await requestJson(GITHUB_DEVICE_CODE_URL, {
      method: "POST",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        "User-Agent": "go-on-vscode/1.0",
      },
      body: new URLSearchParams({
        client_id: clientId,
        scope: "read:user",
      }).toString(),
    });

    if (response.status < 200 || response.status >= 300) {
      throw new Error(
        `GitHub device code request failed (${response.status}): ${response.bodyText || "empty response"}`,
      );
    }

    const payload = response.body as DeviceCodeResponse;
    const deviceCode = String(payload.device_code || "").trim();
    const userCode = String(payload.user_code || "").trim();
    const verificationUri = String(payload.verification_uri || "").trim();
    const expiresIn = Number(payload.expires_in || 900);
    let intervalSeconds = Math.max(1, Number(payload.interval || 5));

    if (!deviceCode || !userCode || !verificationUri) {
      throw new Error("GitHub device authorization response is incomplete.");
    }

    const expiresAt = Date.now() + expiresIn * 1000;
    this._pendingCopilotDeviceAuth = {
      cancelRequested: false,
      userCode,
      verificationUri,
      expiresAt,
    };

    await this._updatePersistedCopilotState({
      oauthClientId: clientId,
      lastError: "",
      lastStatus: `Open ${verificationUri} and enter code ${userCode}.`,
    });
    await vscode.env.openExternal(vscode.Uri.parse(verificationUri));
    await this._postCopilotAuthState({
      authMode: "device-flow",
      oauthClientId: clientId,
      pending: true,
      userCode,
      verificationUri,
      expiresAt,
      statusMessage: `Open ${verificationUri} and enter code ${userCode}.`,
      lastError: "",
    });

    const poll = async (): Promise<void> => {
      while (
        this._pendingCopilotDeviceAuth &&
        !this._pendingCopilotDeviceAuth.cancelRequested
      ) {
        if (Date.now() >= expiresAt) {
          this._pendingCopilotDeviceAuth = undefined;
          await this._updatePersistedCopilotState({
            lastError: "Device authorization expired before completion.",
            lastStatus: "GitHub device authorization expired.",
          });
          await this._postCopilotAuthState({
            pending: false,
            authMode: "device-flow",
            oauthClientId: clientId,
            statusMessage: "GitHub device authorization expired.",
            lastError: "Device authorization expired before completion.",
          });
          return;
        }

        await new Promise((resolve) =>
          setTimeout(resolve, intervalSeconds * 1000),
        );
        if (
          !this._pendingCopilotDeviceAuth ||
          this._pendingCopilotDeviceAuth.cancelRequested
        ) {
          break;
        }

        const tokenResponse = await requestJson(GITHUB_ACCESS_TOKEN_URL, {
          method: "POST",
          headers: {
            "Content-Type": "application/x-www-form-urlencoded",
            "User-Agent": "go-on-vscode/1.0",
          },
          body: new URLSearchParams({
            client_id: clientId,
            device_code: deviceCode,
            grant_type: "urn:ietf:params:oauth:grant-type:device_code",
          }).toString(),
        });

        if (tokenResponse.status < 200 || tokenResponse.status >= 300) {
          throw new Error(
            `GitHub access token polling failed (${tokenResponse.status}): ${tokenResponse.bodyText || "empty response"}`,
          );
        }

        const tokenPayload = asRecord(tokenResponse.body);
        const accessToken =
          typeof tokenPayload.access_token === "string"
            ? tokenPayload.access_token.trim()
            : "";
        if (accessToken) {
          await this._exchangeCopilotToken(accessToken);
          await this._writeCopilotToken(accessToken);
          this._pendingCopilotDeviceAuth = undefined;
          await this._updatePersistedCopilotState({
            authMode: "device-flow",
            lastError: "",
            lastStatus: "GitHub device authorization completed.",
          });
          await this._postCopilotAuthState({
            isAuthorized: true,
            authMode: "device-flow",
            oauthClientId: clientId,
            pending: false,
            statusMessage: "GitHub device authorization completed.",
            lastError: "",
          });
          await this._sendProviderModels("copilot");
          return;
        }

        const pollError =
          typeof tokenPayload.error === "string" ? tokenPayload.error : "";
        if (pollError === "authorization_pending") {
          continue;
        }
        if (pollError === "slow_down") {
          intervalSeconds += 5;
          continue;
        }

        this._pendingCopilotDeviceAuth = undefined;
        const description =
          typeof tokenPayload.error_description === "string"
            ? tokenPayload.error_description
            : pollError || "unknown error";
        await this._updatePersistedCopilotState({
          lastError: description,
          lastStatus: "GitHub device authorization failed.",
        });
        await this._postCopilotAuthState({
          pending: false,
          authMode: "device-flow",
          oauthClientId: clientId,
          statusMessage: "GitHub device authorization failed.",
          lastError: description,
        });
        return;
      }
    };

    void poll().catch(async (error: unknown) => {
      this._pendingCopilotDeviceAuth = undefined;
      const message = errorMessage(error);
      await this._updatePersistedCopilotState({
        lastError: message,
        lastStatus: "GitHub device authorization failed.",
      });
      await this._postCopilotAuthState({
        pending: false,
        authMode: "device-flow",
        oauthClientId: clientId,
        statusMessage: "GitHub device authorization failed.",
        lastError: message,
      });
    });
  }

  private async _cancelCopilotDeviceAuthorization(): Promise<void> {
    if (this._pendingCopilotDeviceAuth) {
      this._pendingCopilotDeviceAuth.cancelRequested = true;
    }
    this._pendingCopilotDeviceAuth = undefined;
    await this._updatePersistedCopilotState({
      lastStatus: "Canceled pending GitHub device authorization.",
    });
    await this._postCopilotAuthState({
      pending: false,
      statusMessage: "Canceled pending GitHub device authorization.",
    });
  }

  private async _deleteCopilotAuthorization(): Promise<void> {
    try {
      await this._runSecretCommand("delete", COPILOT_SECRET_NAME);
    } catch {
      // ignore missing secrets
    }
    this.manager.setRuntimeEnvOverrides?.({
      [COPILOT_ENV_VAR]: "",
    });
    await this._updatePersistedCopilotState({
      authMode: "none",
      accountLabel: "",
      lastError: "",
      lastStatus: "Removed stored GitHub Copilot authorization.",
    });
    await this._postCopilotAuthState({
      isAuthorized: false,
      authMode: "none",
      accountLabel: "",
      pending: false,
      statusMessage: "Removed stored GitHub Copilot authorization.",
      lastError: "",
    });
    await this._sendProviderModels("copilot");
  }

  private async _loadProviderCatalog(): Promise<ProviderCatalogSpec[]> {
    const runtimeCatalog = await this._loadProviderCatalogFromRuntime();
    const configuredMap = await this._loadConfiguredAgentMap();
    const configuredCatalog = Array.from(configuredMap.entries()).map(
      ([name, snapshot]) => ({
        name,
        type: name,
        model: snapshot.model,
        api_key_env: snapshot.envVar || inferEnvVar(name),
      }),
    );

    const merged = dedupeCatalog([
      ...runtimeCatalog,
      ...configuredCatalog,
      ...BUILTIN_PROVIDER_CATALOG,
    ]);
    return merged;
  }

  private async _loadProviderCatalogFromRuntime(): Promise<
    ProviderCatalogSpec[]
  > {
    if (!this.manager.isRunning()) {
      return [];
    }

    try {
      const result = await this.manager.sendRequest("provider.catalog", {});
      const record = asRecord(result);
      // Backend returns { "ok": true, "catalog": [...], "total": N }
      const providers = Array.isArray(record.catalog) ? record.catalog : [];
      const parsed = providers
        .map((item) => asCatalogSpec(item))
        .filter((item): item is ProviderCatalogSpec => Boolean(item));
      if (parsed.length > 0) {
        return parsed;
      }
    } catch {
      // Fallback to models/list and built-in catalog when provider.catalog is unavailable.
    }

    try {
      const result = await this.manager.sendRequest("models/list", {});
      const payload = asRecord(result);
      const groups = Array.isArray(payload.models) ? payload.models : [];
      const byProvider = new Map<string, string[]>();
      for (const group of groups) {
        const item = asRecord(group);
        const name = String(item.provider || item.agent || "").trim();
        const modelId = this._modelIdFromRuntime(
          item.id || item.model_id || item.name,
        );
        if (!name) {
          continue;
        }
        if (!byProvider.has(name)) {
          byProvider.set(name, []);
        }
        if (modelId && !byProvider.get(name)?.includes(modelId)) {
          byProvider.get(name)?.push(modelId);
        }
      }
      const parsed = Array.from(byProvider.entries()).map(
        ([name, models]) =>
          ({
            name,
            type: name,
            model: models[0],
            api_key_env: inferEnvVar(name),
          }) as ProviderCatalogSpec,
      );
      return parsed;
    } catch {
      return [];
    }
  }

  private async _loadConfiguredAgentMap(): Promise<
    Map<string, ProviderConfigSnapshot>
  > {
    try {
      const configPath = this._resolveConfigPath();
      const content = await fs.readFile(configPath, "utf8");
      return parseConfiguredAgents(content);
    } catch {
      return new Map();
    }
  }

  private _modelIdFromRuntime(value: unknown): string | undefined {
    if (typeof value === "string" && value.trim()) {
      return value.trim();
    }
    if (typeof value !== "object" || value === null) {
      return undefined;
    }
    const record = value as Record<string, unknown>;
    const candidates = [
      record.id,
      record.model_id,
      record.modelId,
      record.name,
    ];
    for (const candidate of candidates) {
      if (typeof candidate === "string" && candidate.trim()) {
        return candidate.trim();
      }
    }
    return undefined;
  }

  private async _resolveProviderModels(
    providerName: string,
    spec?: ProviderCatalogEntry,
  ): Promise<ProviderModelResolution> {
    const modelSet = new Set<string>();
    modelSet.add("auto");
    if (spec?.defaultModel) {
      modelSet.add(spec.defaultModel);
    }

    let copilotAuth: CopilotAuthState | undefined;

    if (providerName === "copilot") {
      const copilotModels = await this._resolveCopilotModels();
      copilotAuth = copilotModels.copilotAuth;
      for (const model of copilotModels.modelOptions) {
        modelSet.add(model);
      }
    }

    if (this.manager.isRunning()) {
      try {
        const response = await this.manager.sendRequest(
          "provider.list_models",
          {
            provider: providerName,
          },
        );
        const payload = asRecord(response);
        const ids = Array.isArray(payload.model_ids) ? payload.model_ids : [];
        for (const item of ids) {
          const modelId = this._modelIdFromRuntime(item);
          if (modelId) {
            modelSet.add(modelId);
          }
        }

        const runtimeModels = Array.isArray(payload.models)
          ? payload.models
          : [];
        for (const runtimeModel of runtimeModels) {
          const modelId = this._modelIdFromRuntime(runtimeModel);
          if (modelId) {
            modelSet.add(modelId);
          }
        }

        const defaultModel = this._modelIdFromRuntime(payload.default_model);
        if (defaultModel) {
          modelSet.add(defaultModel);
        }
      } catch {
        try {
          const response = await this.manager.sendRequest("models/list", {});
          const payload = asRecord(response);
          const groups = Array.isArray(payload.models) ? payload.models : [];
          for (const group of groups) {
            const record = asRecord(group);
            const groupProvider = String(
              record.provider || record.agent || "",
            ).trim();
            if (groupProvider !== providerName) {
              continue;
            }
            const modelId = this._modelIdFromRuntime(
              record.id || record.model_id || record.name,
            );
            if (modelId) {
              modelSet.add(modelId);
            }
          }
        } catch {
          // Keep catalog-only models when runtime endpoint is unavailable.
        }
      }
    }

    return {
      modelOptions: Array.from(modelSet.values()),
      copilotAuth,
    };
  }

  private async _buildProviderSettingsPayload() {
    const catalog = await this._loadProviderCatalog();
    const configured = await this._loadConfiguredAgentMap();
    const secretTargets = collectProviderSecretTargets(catalog);
    const providers: ProviderCatalogEntry[] = catalog
      .map((spec) => {
        const configuredValue = configured.get(spec.name);
        return {
          name: spec.name,
          agentType: spec.type,
          group: spec.group,
          defaultModel: spec.model,
          apiKeyEnv: spec.api_key_env,
          secretKeyEnv: spec.secret_key_env,
          url: spec.url,
          chatPath: spec.chat_path,
          supportsSystem: spec.supports_system,
          configuredModel: configuredValue?.model,
          configuredEnvVar: configuredValue?.envVar,
        };
      })
      .sort((a, b) => a.name.localeCompare(b.name));

    const selectedProvider =
      providers.find((item) => item.configuredModel || item.configuredEnvVar)
        ?.name ||
      providers[0]?.name ||
      "copilot";
    const selectedSpec = providers.find(
      (item) => item.name === selectedProvider,
    );
    const selectedModel =
      selectedSpec?.configuredModel || selectedSpec?.defaultModel || "auto";
    const selectedEnvVar =
      selectedSpec?.configuredEnvVar ||
      selectedSpec?.apiKeyEnv ||
      inferEnvVar(selectedProvider);
    const modelResolution = await this._resolveProviderModels(
      selectedProvider,
      selectedSpec,
    );

    return {
      providers,
      selectedProvider,
      selectedModel,
      selectedEnvVar,
      modelOptions: modelResolution.modelOptions,
      secretTargets,
      copilotAuth:
        modelResolution.copilotAuth || (await this._currentCopilotAuthState()),
    };
  }

  private async _sendProviderModels(providerName: string) {
    if (!providerName.trim()) {
      return;
    }

    const payload = await this._buildProviderSettingsPayload();
    const selectedSpec = payload.providers.find(
      (item) => item.name === providerName,
    );
    const modelResolution = await this._resolveProviderModels(
      providerName,
      selectedSpec,
    );

    this._postMessage({
      type: "providerModelsData",
      provider: providerName,
      modelOptions: modelResolution.modelOptions,
      selectedModel:
        selectedSpec?.configuredModel || selectedSpec?.defaultModel || "auto",
      selectedEnvVar:
        selectedSpec?.configuredEnvVar ||
        selectedSpec?.apiKeyEnv ||
        inferEnvVar(providerName),
      copilotAuth: modelResolution.copilotAuth,
    });
  }

  private async _saveProviderSelection(
    providerName: string,
    modelName: string,
    envVar?: string,
  ) {
    const provider = providerName.trim();
    if (!provider) {
      throw new Error("Provider cannot be empty.");
    }

    const catalog = await this._loadProviderCatalog();
    const matched = catalog.find((item) => item.name === provider);
    if (!matched) {
      throw new Error(`Unknown provider: ${provider}`);
    }

    const normalizedModel = modelName.trim() || "auto";
    const normalizedEnvVar =
      (envVar || "").trim() || matched.api_key_env || inferEnvVar(provider);
    const copilotConfigEnvVar =
      provider === "copilot" && (await this._readCopilotToken())
        ? `keyring://go-on/${COPILOT_SECRET_NAME}`
        : normalizedEnvVar;

    const configPath = this._resolveConfigPath();
    let content = "";
    try {
      content = await fs.readFile(configPath, "utf8");
    } catch {
      content = "";
    }

    content = upsertAgentSection(content, provider, {
      type: matched.type,
      url: matched.url,
      chat_path: matched.chat_path,
      api_key_env: copilotConfigEnvVar,
      secret_key_env: matched.secret_key_env,
      anthropic_version: matched.anthropic_version,
      model: normalizedModel,
      max_tokens: matched.max_tokens,
      supports_system: matched.supports_system,
    });

    const defaultPhase = content.match(
      /^default_phase\s*=\s*"([^"]+)"\s*$/m,
    )?.[1];
    if (defaultPhase) {
      content = upsertPhaseAgents(content, defaultPhase, [provider]);
    }

    await fs.writeFile(configPath, `${content.trimEnd()}\n`, "utf8");
    this._postMessage({
      type: "settingsActionResult",
      message: `Saved provider=${provider}, model=${normalizedModel} to ${configPath}`,
    });
    await this._sendCurrentSettings();
  }

  // Settings update methods
  private async _updateRuntimeSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`runtime.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _updateCacheSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`cache.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _updateVectorSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`vector.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _updateAutotuneSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`autotune.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _addAgent(name: string, config: unknown) {
    try {
      configManager.setConfigValue(`agents.${name}`, config);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _deleteAgent(name: string) {
    try {
      const config = configManager.getConfig();
      delete config.agents[name];
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _updatePhase(name: string, config: unknown) {
    try {
      configManager.setConfigValue(`phases.${name}`, config);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _setLanguage(language: string) {
    try {
      const config = vscode.workspace.getConfiguration("go-on");
      await config.update(
        "language",
        language,
        vscode.ConfigurationTarget.Global,
      );
      configManager.setConfigValue("language", language);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _sendCurrentSettings() {
    if (!this._view) return;

    const config = configManager.getConfig();
    const vsCodeConfig = vscode.workspace.getConfiguration("go-on");
    let providerSettings: {
      providers: ProviderCatalogEntry[];
      selectedProvider: string;
      selectedModel: string;
      selectedEnvVar: string;
      modelOptions: string[];
      secretTargets: ProviderSecretTarget[];
      copilotAuth: CopilotAuthState;
    } = {
      providers: [],
      selectedProvider: "copilot",
      selectedModel: "auto",
      selectedEnvVar: inferEnvVar("copilot"),
      modelOptions: ["auto"],
      secretTargets: [],
      copilotAuth: await this._currentCopilotAuthState(),
    };

    try {
      providerSettings = await this._buildProviderSettingsPayload();
    } catch {
      // Keep fallback provider payload if catalog/config discovery fails.
    }

    const settings = {
      language: i18n.getCurrentLanguage(),
      runtime: config.runtime,
      cache: config.cache,
      vector: config.vector,
      autotune: config.autotune,
      agents: config.agents,
      phases: config.phases,
      flow: config.flow,
      executablePath: vsCodeConfig.get("executablePath"),
      autoStart: vsCodeConfig.get("autoStart"),
      isRunning: this.manager.isRunning?.() || false,
      providerSettings,
    };

    this._view.webview.postMessage({
      type: "settingsData",
      data: settings,
      translations: this._getTranslations(),
      language: i18n.getCurrentLanguage(),
    });
  }

  private _getTranslations() {
    return {
      general: {
        goOn: i18n.getMessage(MessageKeys.goOn),
        settings: i18n.getMessage(MessageKeys.settings),
        start: i18n.getMessage(MessageKeys.start),
        stop: i18n.getMessage(MessageKeys.stop),
        status: i18n.getMessage(MessageKeys.status),
        running: i18n.getMessage(MessageKeys.running),
        stopped: i18n.getMessage(MessageKeys.stopped),
      },
      runtime: {
        runtime: i18n.getMessage(MessageKeys.runtime),
        runtimeSettings: i18n.getMessage(MessageKeys.runtimeSettings),
        maintenanceInterval: i18n.getMessage(MessageKeys.maintenanceInterval),
        healthInterval: i18n.getMessage(MessageKeys.healthInterval),
        shutdownDrain: i18n.getMessage(MessageKeys.shutdownDrain),
      },
      execution: {
        executionSettings: i18n.getMessage(MessageKeys.executionSettings),
        startGoOn: i18n.getMessage(MessageKeys.startGoOn),
        stopGoOn: i18n.getMessage(MessageKeys.stopGoOn),
        healthCheck: i18n.getMessage(MessageKeys.healthCheck),
        clearCache: i18n.getMessage(MessageKeys.clearCache),
      },
      workflow: {
        workflow: i18n.getMessage(MessageKeys.workflow),
        phases: i18n.getMessage(MessageKeys.phases),
        agents: i18n.getMessage(MessageKeys.agents),
        addPhase: i18n.getMessage(MessageKeys.addPhase),
        editPhase: i18n.getMessage(MessageKeys.editPhase),
        deletePhase: i18n.getMessage(MessageKeys.deletePhase),
      },
      buttons: {
        save: i18n.getMessage(MessageKeys.save),
        cancel: i18n.getMessage(MessageKeys.cancel),
        reset: i18n.getMessage(MessageKeys.reset),
        apply: i18n.getMessage(MessageKeys.apply),
        delete: i18n.getMessage(MessageKeys.delete),
        edit: i18n.getMessage(MessageKeys.edit),
        add: i18n.getMessage(MessageKeys.add),
      },
      messages: {
        successfullySaved: i18n.getMessage(MessageKeys.successfullySaved),
        errorSaving: i18n.getMessage(MessageKeys.errorSaving),
      },
      language: {
        language: i18n.getMessage(MessageKeys.language),
        simplifiedChinese: i18n.getMessage(MessageKeys.simplifiedChinese),
        traditionalChinese: i18n.getMessage(MessageKeys.traditionalChinese),
        english: i18n.getMessage(MessageKeys.english),
      },
      credentials: {
        credentials: i18n.getMessage(MessageKeys.credentials),
        apiKey: i18n.getMessage(MessageKeys.apiKey),
        secretKey: i18n.getMessage(MessageKeys.secretKey),
      },
    };
  }

  private async _handleKeyringSet(name: string, value: string) {
    try {
      await vscode.commands.executeCommand("go-on.keyringSet", { name, value });
      this._postMessage({
        type: "keyringResult",
        message: `Saved secret '${name}' to system keyring.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: this._getErrorMessage(error),
      });
    }
  }

  private async _handleKeyringGet(name: string) {
    try {
      const value = await vscode.commands.executeCommand<string>(
        "go-on.keyringGet",
        { name },
      );
      this._postMessage({
        type: "keyringResult",
        message: `Fetched secret '${name}' from system keyring.`,
        value: value ?? "",
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: this._getErrorMessage(error),
      });
    }
  }

  private async _handleKeyringDelete(name: string) {
    try {
      await vscode.commands.executeCommand("go-on.keyringDelete", { name });
      this._postMessage({
        type: "keyringResult",
        message: `Deleted secret '${name}' from system keyring.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: this._getErrorMessage(error),
      });
    }
  }

  private async _handleKeyringList() {
    try {
      const output =
        await vscode.commands.executeCommand<string>("go-on.keyringList");
      this._postMessage({
        type: "keyringResult",
        message: "Listed keyring secret status.",
        value: output ?? "",
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: this._getErrorMessage(error),
      });
    }
  }

  private async _handleQuickSetupProvider(
    provider: string,
    apiKey: string,
  ): Promise<void> {
    try {
      // Infer env var name from provider
      const envVarName = inferEnvVar(provider);

      // 1. Store API key in keyring
      await vscode.commands.executeCommand("go-on.keyringSet", {
        name: secretNameForEnvVar(envVarName),
        value: apiKey,
      });

      // 2. Save provider selection to config.toml
      await this._saveProviderSelection(provider, "auto", envVarName);

      this._postMessage({
        type: "quickSetupResult",
        message: `✅ ${provider} configured successfully. API key saved to keyring.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "quickSetupError",
        message: `Setup failed: ${this._getErrorMessage(error)}`,
      });
    }
  }

  private async _handleApplyDefaultConfigTemplate(template: string) {
    try {
      const configPath = await vscode.commands.executeCommand<string>(
        "go-on.applyDefaultConfigTemplate",
        { template },
      );
      this._postMessage({
        type: "settingsActionResult",
        message: `Applied template '${template}' to ${configPath}.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "settingsActionError",
        message: this._getErrorMessage(error),
      });
    }
  }

  private async _handleApplyRulesSettings(payload: {
    globalRules?: string[];
    commonRules?: string[];
    phaseRules?: Record<string, string[]>;
  }) {
    try {
      const rulesDir = await vscode.commands.executeCommand<string>(
        "go-on.updateRules",
        payload,
      );
      this._postMessage({
        type: "settingsActionResult",
        message: `Rules updated in ${rulesDir}.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "settingsActionError",
        message: this._getErrorMessage(error),
      });
    }
  }

  private async _handleApplyWorkflowMapping(payload: {
    defaultPhase?: string;
    phases?: Record<
      string,
      {
        agents: string[];
        fallback?: boolean;
        principles?: string[];
        switchRules?: {
          circuitBreakerFailures?: number;
          circuitBreakerOpenSeconds?: number;
        };
      }
    >;
  }) {
    try {
      const configPath = await vscode.commands.executeCommand<string>(
        "go-on.updateWorkflowMapping",
        payload,
      );
      this._postMessage({
        type: "settingsActionResult",
        message: `Workflow mapping saved to ${configPath}.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "settingsActionError",
        message: this._getErrorMessage(error),
      });
    }
  }

  private _postMessage(message: unknown) {
    this._view?.webview.postMessage(message);
  }

  public async showConfigWizard() {
    const panel = vscode.window.createWebviewPanel(
      "goOnConfigWizard",
      i18n.getMessage(MessageKeys.configWizardTitle),
      vscode.ViewColumn.One,
      { enableScripts: true },
    );

    panel.webview.html = this._getConfigWizardHtml(panel.webview);

    // Store the message listener disposable so it can be cleaned up when the panel closes
    const messageSubscription = panel.webview.onDidReceiveMessage(
      async (message: Record<string, unknown>) => {
        const command = String(message.command ?? "");
        if (command === "cancel") {
          panel.dispose();
          return;
        }
        if (command !== "saveConfig") {
          return;
        }

        const payload = (message.config ?? {}) as Record<string, unknown>;
        const goOnConfig = vscode.workspace.getConfiguration("go-on");
        const rawProtocolMode = String(payload.protocolMode ?? "from_config");
        const protocolMode =
          rawProtocolMode === "from_config"
            ? "from_config"
            : normalizeProtocolMode(rawProtocolMode);

        await Promise.all([
          goOnConfig.update(
            "configPath",
            String(payload.configPath ?? "./config.toml"),
            vscode.ConfigurationTarget.Workspace,
          ),
          goOnConfig.update(
            "executablePath",
            String(payload.executablePath ?? ""),
            vscode.ConfigurationTarget.Workspace,
          ),
          goOnConfig.update(
            "autoStart",
            Boolean(payload.autoStart),
            vscode.ConfigurationTarget.Workspace,
          ),
          goOnConfig.update(
            "runtime.protocolMode",
            protocolMode,
            vscode.ConfigurationTarget.Workspace,
          ),
        ]);

        configManager.setConfigValue("runtime.protocolMode", protocolMode);
        await configManager.saveToFile();
        await this._sendCurrentSettings();

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.successfullySaved),
        );
        panel.dispose();
      },
    );

    // Clean up the message subscription when the panel is disposed
    panel.onDidDispose(() => {
      messageSubscription.dispose();
    });
  }

  private _getConfigWizardHtml(webview: vscode.Webview) {
    const nonce = getNonce();
    const config = vscode.workspace.getConfiguration("go-on");
    const configPath = String(config.get("configPath", "./config.toml"));
    const executablePath = String(config.get("executablePath", ""));
    const autoStart = Boolean(config.get("autoStart", false));
    const protocolMode = String(
      config.get("runtime.protocolMode", "from_config"),
    );

    const payload = JSON.stringify({
      configPath,
      executablePath,
      autoStart,
      protocolMode,
    }).replace(/</g, "\\u003c");

    return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>${i18n.getMessage(MessageKeys.configWizardTitle)}</title>
    <style>
        body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); background: var(--vscode-editor-background); padding: 20px; }
        .header { margin-bottom: 16px; }
        .title { font-size: 22px; font-weight: 700; }
        .subtitle { color: var(--vscode-descriptionForeground); margin-top: 6px; }
        .steps { display: flex; gap: 8px; margin: 18px 0 20px; }
        .step { flex: 1; border: 1px solid var(--vscode-panel-border); border-radius: 8px; padding: 10px; color: var(--vscode-descriptionForeground); }
        .step.active { border-color: var(--vscode-focusBorder); color: var(--vscode-foreground); }
        .cards, .modes { display: grid; gap: 12px; }
        .cards { grid-template-columns: repeat(3, minmax(0, 1fr)); }
        .modes { grid-template-columns: repeat(2, minmax(0, 1fr)); }
        .card { border: 1px solid var(--vscode-panel-border); border-radius: 10px; padding: 14px; cursor: pointer; background: var(--vscode-sideBar-background); }
        .card.selected { border-color: var(--vscode-focusBorder); background: var(--vscode-list-activeSelectionBackground); }
        .card-title { font-weight: 700; margin-bottom: 8px; }
        .card-desc { color: var(--vscode-descriptionForeground); line-height: 1.6; font-size: 12px; }
        .recommended { display: inline-block; margin-top: 8px; color: var(--vscode-testing-iconPassed); font-size: 12px; }
        .review { display: grid; gap: 10px; }
        .review-item { border: 1px solid var(--vscode-panel-border); border-radius: 8px; padding: 10px; }
        .review-label { font-size: 12px; color: var(--vscode-descriptionForeground); margin-bottom: 4px; }
        .review-value { font-weight: 600; word-break: break-all; }
        .actions { display: flex; justify-content: space-between; margin-top: 20px; }
        button { border: none; border-radius: 6px; padding: 8px 14px; cursor: pointer; }
        .ghost { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); }
        .primary { background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
        @media (max-width: 760px) { .cards, .modes { grid-template-columns: 1fr; } }
    </style>
</head>
<body>
    <div class="header">
        <div class="title">${i18n.getMessage(MessageKeys.configWizardTitle)}</div>
        <div class="subtitle">${i18n.getMessage(MessageKeys.configWizardSubtitle)}</div>
    </div>
    <div class="steps">
        <div class="step active" data-step-indicator="0">${i18n.getMessage(MessageKeys.configWizardStep1)}</div>
        <div class="step" data-step-indicator="1">${i18n.getMessage(MessageKeys.configWizardStep2)}</div>
        <div class="step" data-step-indicator="2">${i18n.getMessage(MessageKeys.configWizardStep3)}</div>
    </div>
    <div id="step0">
        <div class="cards">
            <div class="card selected" data-scenario="local">
                <div class="card-title">${i18n.getMessage(MessageKeys.configWizardLocalTitle)}</div>
                <div class="card-desc">${i18n.getMessage(MessageKeys.configWizardLocalDesc)}</div>
            </div>
            <div class="card" data-scenario="shared">
                <div class="card-title">${i18n.getMessage(MessageKeys.configWizardSharedTitle)}</div>
                <div class="card-desc">${i18n.getMessage(MessageKeys.configWizardSharedDesc)}</div>
            </div>
            <div class="card" data-scenario="editor">
                <div class="card-title">${i18n.getMessage(MessageKeys.configWizardEditorTitle)}</div>
                <div class="card-desc">${i18n.getMessage(MessageKeys.configWizardEditorDesc)}</div>
            </div>
        </div>
    </div>
    <div id="step1" hidden>
        <div class="modes">
            <div class="card" data-mode="from_config"><div class="card-title">from_config</div><div class="card-desc">Follow project config.toml</div></div>
            <div class="card selected" data-mode="adaptive"><div class="card-title">adaptive</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardAdaptiveDesc)}</div><span class="recommended">${i18n.getMessage(MessageKeys.configWizardRecommended)}</span></div>
            <div class="card" data-mode="acp_stdio"><div class="card-title">acp_stdio</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardAcpStdioDesc)}</div></div>
            <div class="card" data-mode="acp_http"><div class="card-title">acp_http</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardAcpHttpDesc)}</div></div>
            <div class="card" data-mode="mcp_stdio"><div class="card-title">mcp_stdio</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardMcpStdioDesc)}</div></div>
            <div class="card" data-mode="mcp_http"><div class="card-title">mcp_http</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardMcpHttpDesc)}</div></div>
        </div>
    </div>
    <div id="step2" hidden>
        <div class="review">
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.configPath)}</div><div class="review-value" id="review-config-path"></div></div>
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.executablePath)}</div><div class="review-value" id="review-executable-path"></div></div>
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.autoStart)}</div><div class="review-value" id="review-auto-start"></div></div>
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.configWizardProtocolMode)}</div><div class="review-value" id="review-protocol-mode"></div></div>
        </div>
    </div>
    <div class="actions">
        <button class="ghost" id="cancel-btn">${i18n.getMessage(MessageKeys.cancel)}</button>
        <div>
            <button class="ghost" id="prev-btn" disabled>${i18n.getMessage(MessageKeys.configWizardPrevious)}</button>
            <button class="primary" id="next-btn">${i18n.getMessage(MessageKeys.configWizardNext)}</button>
        </div>
    </div>
    <script nonce="${nonce}">
        const vscode = acquireVsCodeApi();
        const initial = ${payload};
        const state = {
            step: 0,
            scenario: 'local',
            configPath: initial.configPath,
            executablePath: initial.executablePath,
            autoStart: initial.autoStart,
            protocolMode: initial.protocolMode || 'adaptive',
        };

        const recommendations = {
            local: 'adaptive',
            shared: 'acp_http',
            editor: 'acp_stdio',
        };

        function render() {
            document.querySelectorAll('[data-step-indicator]').forEach((el, index) => {
                el.classList.toggle('active', index === state.step);
            });
            document.getElementById('step0').hidden = state.step !== 0;
            document.getElementById('step1').hidden = state.step !== 1;
            document.getElementById('step2').hidden = state.step !== 2;
            document.getElementById('prev-btn').disabled = state.step === 0;
            document.getElementById('next-btn').textContent = state.step === 2 ? '${i18n.getMessage(MessageKeys.save)}' : '${i18n.getMessage("configuration.wizard.next")}';
            document.querySelectorAll('[data-scenario]').forEach((el) => {
                el.classList.toggle('selected', el.dataset.scenario === state.scenario);
            });
            document.querySelectorAll('[data-mode]').forEach((el) => {
                el.classList.toggle('selected', el.dataset.mode === state.protocolMode);
            });
            document.getElementById('review-config-path').textContent = state.configPath || './config.toml';
            document.getElementById('review-executable-path').textContent = state.executablePath || '(empty)';
            document.getElementById('review-auto-start').textContent = state.autoStart ? 'true' : 'false';
            document.getElementById('review-protocol-mode').textContent = state.protocolMode;
        }

        document.querySelectorAll('[data-scenario]').forEach((el) => {
            el.addEventListener('click', () => {
                state.scenario = el.dataset.scenario;
                state.protocolMode = recommendations[state.scenario] || 'adaptive';
                state.autoStart = state.scenario === 'shared';
                render();
            });
        });

        document.querySelectorAll('[data-mode]').forEach((el) => {
            el.addEventListener('click', () => {
                state.protocolMode = el.dataset.mode;
                render();
            });
        });

        document.getElementById('cancel-btn').addEventListener('click', () => {
            vscode.postMessage({ command: 'cancel' });
        });

        document.getElementById('prev-btn').addEventListener('click', () => {
            if (state.step > 0) state.step -= 1;
            render();
        });

        document.getElementById('next-btn').addEventListener('click', () => {
            if (state.step < 2) {
                state.step += 1;
                render();
                return;
            }
            vscode.postMessage({ command: 'saveConfig', config: state });
        });

        window.addEventListener('message', (event) => {
            if (event.data?.command === 'close') {
                window.close();
            }
        });

        render();
    </script>
</body>
</html>`;
  }

  private _getHtmlForWebview(webview: vscode.Webview) {
    const styleResetUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "reset.css"),
    );
    const styleVSCodeUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "vscode.css"),
    );
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "settings.js"),
    );

    const nonce = getNonce();

    return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <link href="${styleResetUri}" rel="stylesheet">
                <link href="${styleVSCodeUri}" rel="stylesheet">
                <title>Go-On Settings</title>
                <style>
                    .settings-container {
                        padding: 10px;
                        height: 100%;
                        overflow-y: auto;
                    }
                    .setting-group {
                        margin-bottom: 20px;
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        padding: 10px;
                    }
                    .setting-group h3 {
                        margin: 0 0 10px 0;
                        color: var(--vscode-textLink-foreground);
                        border-bottom: 1px solid var(--vscode-panel-border);
                        padding-bottom: 5px;
                    }
                    .setting-item {
                        margin-bottom: 10px;
                    }
                    .setting-item label {
                        display: block;
                        margin-bottom: 4px;
                        font-weight: bold;
                    }
                    .setting-item input, .setting-item select {
                        width: 100%;
                        padding: 4px 8px;
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        background: var(--vscode-input-background);
                        color: var(--vscode-input-foreground);
                    }
                    .setting-item input[type="checkbox"] {
                        width: auto;
                        margin-right: 8px;
                    }
                    .setting-item input[type="number"] {
                        width: 80px;
                    }
                    .action-buttons {
                        margin-top: 20px;
                        display: flex;
                        flex-wrap: wrap;
                        gap: 5px;
                    }
                    .action-button {
                        padding: 6px 12px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.9em;
                    }
                    .action-button:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .action-button.danger {
                        background: var(--vscode-notificationsErrorIcon-foreground);
                    }
                    .status-indicator {
                        display: inline-block;
                        width: 8px;
                        height: 8px;
                        border-radius: 50%;
                        margin-right: 5px;
                    }
                    .status-indicator.connected {
                        background: var(--vscode-charts-green);
                    }
                    .status-indicator.disconnected {
                        background: var(--vscode-notificationsErrorIcon-foreground);
                    }
                </style>
            </head>
            <body>
                <div class="settings-container">
                    <div class="setting-group">
                        <h3>🖥️ System Configuration</h3>
                        <div class="setting-item">
                            <label for="configPath">Config File Path:</label>
                            <input type="text" id="configPath" data-setting="go-on.configPath">
                        </div>
                        <div class="setting-item">
                            <label for="executablePath">Executable Path:</label>
                            <input type="text" id="executablePath" data-setting="go-on.executablePath">
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="autoDownloadBinary" data-setting="go-on.autoDownloadBinary">
                                Auto-download app binary when missing
                            </label>
                        </div>
                        <div class="setting-item">
                            <label for="releaseRepository">Release Repository (owner/repo):</label>
                            <input type="text" id="releaseRepository" data-setting="go-on.releaseRepository">
                        </div>
                        <div class="setting-item">
                            <label for="releaseTag">Release Tag:</label>
                            <input type="text" id="releaseTag" data-setting="go-on.releaseTag">
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="autoStart" data-setting="go-on.autoStart">
                                Auto-start Go-On on workspace open
                            </label>
                        </div>
                        <div class="setting-item">
                            <label for="defaultTemplate">Default Config Template:</label>
                            <select id="defaultTemplate">
                                <option value="config.toml.autopilot-adaptive">autopilot-adaptive</option>
                            </select>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyDefaultTemplate">Apply As Active config.toml</button>
                            <button class="action-button" id="openConfigWizard">Open Config Wizard</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🔐 System Keyring (Preferred)</h3>
                        <div class="setting-item">
                            <label for="secretName">Secret Name:</label>
                          <select id="secretName"></select>
                        </div>
                        <div class="setting-item">
                            <label for="secretValue">Secret Value:</label>
                            <input type="password" id="secretValue" autocomplete="off" placeholder="Enter API key or token">
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="setKeyringSecret">Save to Keyring</button>
                            <button class="action-button" id="getKeyringSecret">Read from Keyring</button>
                            <button class="action-button" id="listKeyringSecrets">List Key Status</button>
                            <button class="action-button danger" id="deleteKeyringSecret">Delete Key</button>
                        </div>
                        <div class="setting-item" style="margin-top: 8px;">
                            <label for="keyringOutput">Keyring Output:</label>
                            <textarea id="keyringOutput" rows="5" style="width: 100%;"></textarea>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>📜 Rules Settings</h3>
                        <div class="setting-item">
                            <label for="globalRules">Global Rules (RULES/global.md, one per line):</label>
                            <textarea id="globalRules" rows="5" style="width: 100%;" placeholder="Rule line 1&#10;Rule line 2"></textarea>
                        </div>
                        <div class="setting-item">
                            <label for="commonRules">Common Rules (RULES/common.md, one per line):</label>
                            <textarea id="commonRules" rows="5" style="width: 100%;" placeholder="Rule line 1&#10;Rule line 2"></textarea>
                        </div>
                        <div class="setting-item">
                            <label for="phaseRules">Per-Phase Rules (format: phase|rule text):</label>
                            <textarea id="phaseRules" rows="6" style="width: 100%;" placeholder="coding|Must include tests&#10;review|Fail closed on uncertainty"></textarea>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyRulesSettings">Save Rules</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🧭 Workflow And AI Routing</h3>
                        <div class="setting-item">
                            <label for="defaultPhaseInput">Default Phase:</label>
                            <input type="text" id="defaultPhaseInput" placeholder="coding">
                        </div>
                        <div class="setting-item">
                            <label for="workflowMapping">Node Mapping JSON:</label>
                            <textarea id="workflowMapping" rows="12" style="width: 100%;" placeholder='{"coding":{"agents":["copilot","deepseek"],"fallback":true,"principles":["Prefer safe changes"],"switchRules":{"circuitBreakerFailures":3,"circuitBreakerOpenSeconds":30}}}'></textarea>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyWorkflowMapping">Save Workflow Mapping</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🤖 Provider Model Routing</h3>
                        <div class="setting-item">
                            <label for="providerSelect">Provider:</label>
                            <select id="providerSelect"></select>
                        </div>
                        <div class="setting-item">
                            <label for="providerModelSelect">Model:</label>
                            <select id="providerModelSelect"></select>
                        </div>
                        <div class="setting-item">
                            <label for="providerEnvVar">API Key Env Var:</label>
                            <input type="text" id="providerEnvVar" placeholder="Optional, inferred when empty">
                        </div>
                        <div class="setting-item" id="copilotAuthPanel" style="display: none;">
                          <label for="copilotOauthClientId">GitHub OAuth Client ID For Device Flow:</label>
                          <input type="text" id="copilotOauthClientId" placeholder="Required only for device flow">
                          <div class="action-buttons" style="margin-top: 8px;">
                            <button class="action-button" id="authorizeCopilotGitHubSession">Authorize With GitHub Login</button>
                            <button class="action-button" id="authorizeCopilotDeviceFlow">Authorize With Device Code</button>
                            <button class="action-button" id="refreshCopilotModels">Refresh Copilot Models</button>
                            <button class="action-button danger" id="cancelCopilotDeviceFlow">Cancel Device Flow</button>
                            <button class="action-button danger" id="deleteCopilotAuthorization">Delete Stored Copilot Token</button>
                          </div>
                          <div class="setting-item" style="margin-top: 8px;">
                            <label for="copilotAuthOutput">Copilot Authorization And Model Status:</label>
                            <textarea id="copilotAuthOutput" rows="6" style="width: 100%;"></textarea>
                          </div>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyProviderSelection">Apply Provider/Model To config.toml</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>💬 Chat Settings</h3>
                        <div class="setting-item">
                            <label for="maxHistory">Max Chat History:</label>
                            <input type="number" id="maxHistory" min="1" max="1000" data-setting="go-on.chat.maxHistory">
                        </div>
                        <div class="setting-item">
                            <label for="model">Default Model:</label>
                            <select id="model" data-setting="go-on.chat.model">
                                <option value="auto">Auto</option>
                                <option value="copilot">GitHub Copilot</option>
                                <option value="deepseek">DeepSeek</option>
                                <option value="wenxin">Wenxin</option>
                                <option value="openai_compatible">OpenAI Compatible</option>
                                <option value="doubao">Doubao</option>
                                <option value="claude">Claude</option>
                            </select>
                        </div>
                        <div class="setting-item">
                            <label for="temperature">Temperature:</label>
                            <input type="number" id="temperature" min="0" max="2" step="0.1" data-setting="go-on.chat.temperature">
                        </div>
                        <div class="setting-item">
                            <label for="maxTokens">Max Tokens:</label>
                            <input type="number" id="maxTokens" min="1" max="32768" data-setting="go-on.chat.maxTokens">
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="streaming" data-setting="go-on.chat.streaming">
                                Enable streaming responses
                            </label>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🧠 Memory & Cache</h3>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="cacheEnabled" data-setting="go-on.cache.enabled">
                                Enable response caching
                            </label>
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="vectorEnabled" data-setting="go-on.vector.enabled">
                                Enable vector memory
                            </label>
                        </div>
                        <div class="setting-item">
                            <label for="healthInterval">Health Check Interval (seconds):</label>
                            <input type="number" id="healthInterval" min="30" max="3600" data-setting="go-on.health.interval">
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🎨 UI Settings</h3>
                        <div class="setting-item">
                            <label for="uiTheme">Theme:</label>
                            <select id="uiTheme" data-setting="go-on.ui.theme">
                                <option value="auto">Auto (Follow VS Code)</option>
                                <option value="light">Light</option>
                                <option value="dark">Dark</option>
                            </select>
                        </div>
                        <div class="setting-item">
                            <label for="fontSize">Font Size:</label>
                            <input type="number" id="fontSize" min="8" max="24" data-setting="go-on.ui.fontSize">
                        </div>
                    </div>

                    <div class="action-buttons">
                        <button class="action-button" id="startGoOn">Start Go-On</button>
                        <button class="action-button" id="stopGoOn">Stop Go-On</button>
                        <button class="action-button" id="healthCheck">Health Check</button>
                        <button class="action-button" id="healthProbes">Health Probes</button>
                        <button class="action-button" id="lockStatus">Lock Status</button>
                        <button class="action-button" id="observabilityAlerts">Observability Alerts</button>
                        <button class="action-button" id="securityBaseline" data-feature="entry_auth,production_strict">Security Baseline</button>
                        <button class="action-button" id="harnessStatus" data-feature="harness_bus">Harness Status</button>
                        <button class="action-button" id="breakerStatus">Breaker Status</button>
                        <button class="action-button" id="breakerRecovery">Breaker Recovery</button>
                        <button class="action-button danger" id="clearCache" data-feature="response_cache">Clear Cache</button>
                        <button class="action-button danger" id="clearVector" data-feature="vector_store">Clear Vector</button>
                        <button class="action-button" id="reloadConfig">Reload Config</button>
                        <button class="action-button" id="workflowExecute" data-feature="skills_enabled,skills_import">Workflow Execute</button>
                        <button class="action-button" id="taskPlan">Task Plan</button>
                        <button class="action-button" id="taskExecute">Task Execute</button>
                        <button class="action-button" id="learningSummary">Learning Summary</button>
                        <button class="action-button" id="learningGuardrail">Learning Guardrail</button>
                        <button class="action-button" id="learningReplay">Learning Replay</button>
                        <button class="action-button" id="knowledgeDistill">Knowledge Distill</button>
                        <button class="action-button" id="rlAlignmentEval">RL Alignment Eval</button>
                        <button class="action-button" id="hardnessStatus">Hardness Status</button>
                        <button class="action-button" id="costStatus">Cost Status</button>
                        <button class="action-button" id="configBaseline">Config Baseline</button>
                        <button class="action-button" id="errorContract">Error Contract</button>
                        <button class="action-button" id="buildRepro">Build Repro</button>
                        <button class="action-button" id="dataLifecycle">Data Lifecycle</button>
                        <button class="action-button" id="optimizationPeak">Optimization Peak</button>
                        <button class="action-button" id="releaseReadiness">Release Readiness</button>
                        <button class="action-button" id="runtimeStability">Runtime Stability</button>
                        <button class="action-button" id="autotuneStatus" data-feature="autotune">Autotune Status</button>
                        <button class="action-button" id="governanceStatus">Governance Status</button>
                        <button class="action-button" id="governancePlanGet">Governance Plan</button>
                        <button class="action-button" id="governanceAuditRecent">Governance Audit</button>
                        <button class="action-button" id="debugPanelGet">Debug Panel</button>
                        <button class="action-button" id="actionCheck">Action Check</button>
                    </div>

                    <div style="margin-top: 20px; padding: 10px; background: var(--vscode-textBlockQuote-background); border-left: 3px solid var(--vscode-textBlockQuote-border);">
                        <strong>Status:</strong>
                        <span class="status-indicator ${this.manager.isRunning() ? "connected" : "disconnected"}"></span>
                        ${this.manager.isRunning() ? "Connected" : "Disconnected"}
                    </div>

                    <div class="setting-item" style="margin-top: 8px;">
                        <label for="settingsActionOutput">Settings Action Output:</label>
                        <textarea id="settingsActionOutput" rows="4" style="width: 100%;"></textarea>
                    </div>
                </div>
                <script nonce="${nonce}" src="${scriptUri}"></script>
            </body>
            </html>`;
  }
}
