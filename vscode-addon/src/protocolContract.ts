import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { Logger } from "./logger";

const log = Logger.forModule("protocolContract");

type SurfaceSupport = {
  openAiCompat: boolean;
  responsesNative: boolean;
};

type SurfaceContract = {
  checks: string[];
  supports: SurfaceSupport;
};

type ProtocolSupport = {
  supportedModes: string[];
  workflowControlModes?: string[];
  defaultWorkflowControlMode?: string;
  platformModes?: string[];
  defaultPlatformMode?: string;
  universalPlatformEnabled?: boolean;
  phaseCompatMappingEnabled?: boolean;
  universalExecutionCycleSchemaVersion?: string;
  universalGateModelCheckedInMainChain?: boolean;
  universalResponseSkeletonCheckedInMainChain?: boolean;
  universalSandboxProfileCheckedInMainChain?: boolean;
  universalApprovalCheckpointCheckedInMainChain?: boolean;
  universalCapabilityProfileCheckedInMainChain?: boolean;
  verification?: string;
  workflowProfileFields?: string[];
  defaultMode: string;
  autoModeSupportsAcpAndMcp: boolean;
  protocolCapabilityModel: string;
  adaptiveSelectionModel: string;
  adaptiveStartupTransportStrategy: string;
  fixedModesAreConfigDriven: boolean;
  acpInitializeProtocol: string;
  mcpInitializeProtocolVersion: string;
  coexistenceValidatedByRpcIntegration: boolean;
  triPathValidatedByIntegration: boolean;
  httpRootSupportsAcpHttp: boolean;
  httpRootAdvertisesResponsesEndpoints: boolean;
  guiDetectsAutoModeLiteral: boolean;
  guiParsesProtocolModeFromProtocolSection: boolean;
  guiTauriCompileCheckedInMainChain: boolean;
  vscodeAddonCompileCheckedInMainChain: boolean;
};

type ResponsesApiContract = {
  path: string;
  retrievalPath: string;
  listPath: string;
  r1Status: string;
  responseRetrievalSupport: boolean;
  responseListSupport: boolean;
  responseListObjectType: string;
  responseListDataContainsResponseObjects: boolean;
  responseListNewestFirst: boolean;
  responseIdsAreUniquePerRequest: boolean;
  responseIdHasTimestampAndSequence: boolean;
  responseStoreTracksCompleted: boolean;
  responseStoreTracksFailed: boolean;
  responseStatusLifecycle: string[];
  responseStatusHistoryField: string;
  responseHistoryIncludesTransitions: boolean;
  toolCallInitiationSupport: boolean;
  toolChoiceRequiredReturnsIncomplete: boolean;
  toolCallOutputItemSupport: boolean;
  toolResultContinuationSupport: boolean;
  previousResponseIdMustBeNonEmptyString: boolean;
  toolResultRequiresMatchingToolCallId: boolean;
  toolResultOutputItemSupport: boolean;
  failureCodeClasses: string[];
  toolLoopMissingResultUsesToolError: boolean;
  noPendingToolCallUsesToolError: boolean;
  upstreamTimeoutMapped: boolean;
  upstreamRateLimitMapped: boolean;
  retrievalNotFoundUsesResponsesErrorShape: boolean;
  streamSupport: boolean;
  streamEvents: string[];
  streamTerminatesWithDone: boolean;
  requestBodyMustBeObject: boolean;
  modelMustBeNonEmptyString: boolean;
  modelMustBeString: boolean;
  acceptedInputTypes: string[];
  inputMustProduceMessages: boolean;
  inputMustIncludeUserMessage: boolean;
  maxOutputTokensMin: number;
  maxOutputTokensMustBeInteger: boolean;
  temperatureMustBeNumber: boolean;
  temperatureRange: number[];
  metadataMustBeObject: boolean;
  reasoningMustBeObject: boolean;
  toolsMustBeArray: boolean;
  toolsEntriesMustBeObjects: boolean;
  toolsEntriesMustUseFunctionType: boolean;
  toolsEntriesRequireFunctionName: boolean;
  toolsFunctionDescriptionMustBeString: boolean;
  toolsFunctionParametersMustBeObject: boolean;
  toolsFunctionParametersTypeMustBeObject: boolean;
  toolsFunctionParametersPropertiesMustBeObject: boolean;
  toolsFunctionParametersRequiredMustBeStringArray: boolean;
  toolChoiceAllowedTypes: string[];
  toolChoiceStringValues: string[];
  toolChoiceRequiredNeedsTools: boolean;
  toolChoiceObjectMustUseFunctionType: boolean;
  toolChoiceObjectRequiresFunctionName: boolean;
  toolChoiceObjectRequiresTools: boolean;
  toolChoiceObjectMustReferenceDeclaredTool: boolean;
  emptyBodyUsesResponsesErrorShape: boolean;
  invalidJsonUsesResponsesErrorShape: boolean;
  requiredFields: string[];
  outputObjectType: string;
  errorHasCodeField: boolean;
  goldenCasesImplemented: boolean;
  responseRequiredFields: string[];
  errorRequiredFields: string[];
  streamEventOrder: string[];
  rootCapabilitiesPath: string;
  rootCapabilitiesAdvertisesResponsesEndpoints: boolean;
  deleteResponseMethodReturns405: boolean;
  streamSetupUnavailableDegradesToCompleted: boolean;
  streamSetupUnavailableUsesDegradedMessage: boolean;
  streamSetupUnavailableStoredAsCompleted: boolean;
  streamSetupUnavailableRetrievableById: boolean;
  nonStreamSetupUnavailableDegradesToCompleted: boolean;
  nonStreamSetupUnavailableUsesDegradedMessage: boolean;
  nonStreamSetupUnavailableStoredAsCompleted: boolean;
  nonStreamSetupUnavailableRetrievableById: boolean;
};

/** A message in the framed protocol, with an optional message_id for deduplication. */
export type FramedMessage = {
  message_id?: string;
  type?: string;
  [key: string]: unknown;
};

export type ProtocolContract = {
  version: string;
  runtime: {
    baseUrl: string;
    healthPath: string;
  };
  /** Negotiated ACP protocol version from the backend, or 0 if unknown. */
  backendProtocolVersion: number;
  protocol: ProtocolSupport;
  openai: {
    modelsPath: string;
    modelAliasPath: string;
    chatCompletionsPath: string;
    streamDoneMarker: string;
    commonForwardedFields: string[];
    requiredAssertions: {
      modelsDataNonEmpty: boolean;
      chatContentNonEmpty: boolean;
      sseHasDataFrame: boolean;
      sseHasDoneMarker: boolean;
    };
  };
  statusTerms: {
    healthy: string;
    healthCheckFailed: string;
    running: string;
    stopped: string;
  };
  errors: {
    providerNotReady: string;
    setupWizardOpened: string;
    setupWizardPrompt: string;
    runtimeProbePassed: string;
    requestErrorKinds: string[];
    requestErrorContextPrefix: string;
  };
  surfaces: {
    gui: SurfaceContract;
    vscodeAddon: SurfaceContract;
  };
  responsesApi: ResponsesApiContract;
};

const fallbackContract: ProtocolContract = {
  version: "2026-04-14-blue13-r4.10-fallback",
  runtime: {
    baseUrl: "http://127.0.0.1:8090",
    healthPath: "/health",
  },
  backendProtocolVersion: 0,
  protocol: {
    supportedModes: [
      "adaptive",
      "acp_stdio",
      "acp_http",
      "mcp_stdio",
      "mcp_http",
    ],
    workflowControlModes: ["manual", "assisted", "autonomous"],
    defaultWorkflowControlMode: "assisted",
    platformModes: ["universal", "phase_compat"],
    defaultPlatformMode: "phase_compat",
    universalPlatformEnabled: true,
    phaseCompatMappingEnabled: true,
    universalExecutionCycleSchemaVersion: "blue23-universal-cycle-v1",
    universalGateModelCheckedInMainChain: true,
    universalResponseSkeletonCheckedInMainChain: true,
    universalSandboxProfileCheckedInMainChain: true,
    universalApprovalCheckpointCheckedInMainChain: true,
    universalCapabilityProfileCheckedInMainChain: true,
    defaultMode: "adaptive",
    autoModeSupportsAcpAndMcp: true,
    protocolCapabilityModel: "capability_plus_transport",
    adaptiveSelectionModel: "client_type_routed",
    adaptiveStartupTransportStrategy: "http_if_bind_else_stdio",
    fixedModesAreConfigDriven: true,
    acpInitializeProtocol: "acp",
    mcpInitializeProtocolVersion: "2024-11-05",
    coexistenceValidatedByRpcIntegration: true,
    triPathValidatedByIntegration: true,
    httpRootSupportsAcpHttp: true,
    httpRootAdvertisesResponsesEndpoints: true,
    guiDetectsAutoModeLiteral: true,
    guiParsesProtocolModeFromProtocolSection: true,
    guiTauriCompileCheckedInMainChain: true,
    vscodeAddonCompileCheckedInMainChain: true,
  },
  openai: {
    modelsPath: "/v1/models",
    modelAliasPath: "/v1/model",
    chatCompletionsPath: "/v1/chat/completions",
    streamDoneMarker: "[DONE]",
    commonForwardedFields: [
      "temperature",
      "top_p",
      "max_tokens",
      "n",
      "stop",
      "presence_penalty",
      "frequency_penalty",
      "logit_bias",
      "user",
      "seed",
      "response_format",
      "tools",
      "tool_choice",
      "parallel_tool_calls",
      "function_call",
      "functions",
    ],
    requiredAssertions: {
      modelsDataNonEmpty: true,
      chatContentNonEmpty: true,
      sseHasDataFrame: true,
      sseHasDoneMarker: true,
    },
  },
  statusTerms: {
    healthy: "Healthy",
    healthCheckFailed: "Health check failed",
    running: "Running",
    stopped: "Stopped",
  },
  errors: {
    providerNotReady: "No runtime-ready AI provider configured.",
    setupWizardOpened: "Setup wizard opened.",
    setupWizardPrompt:
      "No runtime-ready AI provider is configured. Opening Go-On setup wizard now.",
    runtimeProbePassed: "runtime.health semantic probe passed",
    requestErrorKinds: ["PuaViolation", "BudgetExceeded", "SandboxBlocked"],
    requestErrorContextPrefix: "acp.handle_request.dispatch",
  },
  surfaces: {
    gui: {
      checks: ["health", "models"],
      supports: {
        openAiCompat: true,
        responsesNative: false,
      },
    },
    vscodeAddon: {
      checks: ["runtime.health"],
      supports: {
        openAiCompat: true,
        responsesNative: false,
      },
    },
  },
  responsesApi: {
    path: "/v1/responses",
    retrievalPath: "/v1/responses/{id}",
    listPath: "/v1/responses",
    r1Status: "baseline",
    responseRetrievalSupport: true,
    responseListSupport: true,
    responseListObjectType: "list",
    responseListDataContainsResponseObjects: true,
    responseListNewestFirst: true,
    responseIdsAreUniquePerRequest: true,
    responseIdHasTimestampAndSequence: true,
    responseStoreTracksCompleted: true,
    responseStoreTracksFailed: true,
    responseStatusLifecycle: ["queued", "in_progress", "completed", "failed"],
    responseStatusHistoryField: "status_history",
    responseHistoryIncludesTransitions: true,
    toolCallInitiationSupport: true,
    toolChoiceRequiredReturnsIncomplete: true,
    toolCallOutputItemSupport: true,
    toolResultContinuationSupport: true,
    previousResponseIdMustBeNonEmptyString: true,
    toolResultRequiresMatchingToolCallId: true,
    toolResultOutputItemSupport: true,
    failureCodeClasses: [
      "timeout",
      "rate_limit",
      "tool_error",
      "upstream_error",
    ],
    toolLoopMissingResultUsesToolError: true,
    noPendingToolCallUsesToolError: true,
    upstreamTimeoutMapped: true,
    upstreamRateLimitMapped: true,
    retrievalNotFoundUsesResponsesErrorShape: true,
    streamSupport: true,
    streamEvents: [
      "response.created",
      "response.output_text.delta",
      "response.completed",
      "response.failed",
    ],
    streamTerminatesWithDone: true,
    requestBodyMustBeObject: true,
    modelMustBeNonEmptyString: true,
    modelMustBeString: true,
    acceptedInputTypes: ["string", "array"],
    inputMustProduceMessages: true,
    inputMustIncludeUserMessage: true,
    maxOutputTokensMin: 1,
    maxOutputTokensMustBeInteger: true,
    temperatureMustBeNumber: true,
    temperatureRange: [0, 2],
    metadataMustBeObject: true,
    reasoningMustBeObject: true,
    toolsMustBeArray: true,
    toolsEntriesMustBeObjects: true,
    toolsEntriesMustUseFunctionType: true,
    toolsEntriesRequireFunctionName: true,
    toolsFunctionDescriptionMustBeString: true,
    toolsFunctionParametersMustBeObject: true,
    toolsFunctionParametersTypeMustBeObject: true,
    toolsFunctionParametersPropertiesMustBeObject: true,
    toolsFunctionParametersRequiredMustBeStringArray: true,
    toolChoiceAllowedTypes: ["string", "object"],
    toolChoiceStringValues: ["auto", "none", "required"],
    toolChoiceRequiredNeedsTools: true,
    toolChoiceObjectMustUseFunctionType: true,
    toolChoiceObjectRequiresFunctionName: true,
    toolChoiceObjectRequiresTools: true,
    toolChoiceObjectMustReferenceDeclaredTool: true,
    emptyBodyUsesResponsesErrorShape: true,
    invalidJsonUsesResponsesErrorShape: true,
    requiredFields: [
      "id",
      "object",
      "created_at",
      "model",
      "status",
      "output",
      "usage",
      "error",
      "incomplete_details",
    ],
    outputObjectType: "response",
    errorHasCodeField: true,
    goldenCasesImplemented: true,
    responseRequiredFields: [
      "id",
      "object",
      "created_at",
      "model",
      "status",
      "output",
      "usage",
      "error",
      "incomplete_details",
    ],
    errorRequiredFields: ["code", "type", "message"],
    streamEventOrder: [
      "response.created",
      "response.output_text.delta",
      "response.completed",
    ],
    rootCapabilitiesPath: "/",
    rootCapabilitiesAdvertisesResponsesEndpoints: true,
    deleteResponseMethodReturns405: true,
    streamSetupUnavailableDegradesToCompleted: true,
    streamSetupUnavailableUsesDegradedMessage: true,
    streamSetupUnavailableStoredAsCompleted: true,
    streamSetupUnavailableRetrievableById: true,
    nonStreamSetupUnavailableDegradesToCompleted: true,
    nonStreamSetupUnavailableUsesDegradedMessage: true,
    nonStreamSetupUnavailableStoredAsCompleted: true,
    nonStreamSetupUnavailableRetrievableById: true,
  },
};

/**
 * Resolve the backend base URL using the following priority:
 *   1. VS Code config `go-on.baseUrl`
 *   2. Environment variable `GOON_BASE_URL`
 *   3. Hardcoded fallback `http://127.0.0.1:8090`
 */
function resolveBaseUrl(): string {
  // Priority 1: VS Code configuration (go-on.baseUrl)
  try {
    const configured = vscode.workspace
      .getConfiguration("go-on")
      .get<string>("baseUrl");
    if (configured && configured.trim().length > 0) {
      return configured.trim();
    }
  } catch {
    // Not running inside VS Code (e.g., tests) — fall through
  }

  // Priority 2: Environment variable
  const envUrl = process.env["GOON_BASE_URL"];
  if (envUrl && envUrl.trim().length > 0) {
    return envUrl.trim();
  }

  // Priority 3: Hardcoded default
  return "http://127.0.0.1:8090";
}

/**
 * The GUI-supported protocol versions that this addon can handle (ascending).
 * Used when negotiating with the backend's /protocol/version endpoint.
 */
const CLIENT_SUPPORTED_VERSIONS: number[] = [1, 2, 3];

/**
 * Select the highest protocol version common to both the client and server
 * lists. Iterates `clientVersions` in descending order and returns the first
 * version found in `serverVersions`, or `0` when there is no overlap.
 */
function selectHighestCommon(
  clientVersions: number[],
  serverVersions: number[],
): number {
  for (let i = clientVersions.length - 1; i >= 0; i--) {
    if (serverVersions.includes(clientVersions[i])) {
      return clientVersions[i];
    }
  }
  return 0;
}

/**
 * Map a negotiated protocol version number to the chat completions path.
 * V2+ → /v1/chat/completions
 * V1   → /chat/stream
 */
function chatPathForVersion(version: number): string {
  return version >= 2 ? "/v1/chat/completions" : "/chat/stream";
}

/**
 * Fetch the backend's protocol version from `/protocol/version` and re-negotiate
 * the `protocolContract` fields (`backendProtocolVersion`, chat path, etc.).
 *
 * Returns the negotiated version number (0 on failure).
 */
export async function renegotiateWithBackend(): Promise<number> {
  const baseUrl = protocolContract.runtime.baseUrl;
  const protoUrl = `${baseUrl}/protocol/version`;

  try {
    const response = await fetch(protoUrl);
    if (!response.ok) {
      log.warn(`renegotiate: backend returned ${response.status}`);
      return 0;
    }
    const data = (await response.json()) as {
      supported_versions?: number[];
      latest?: number;
    };
    const serverVersions: number[] = data.supported_versions ?? [];
    if (serverVersions.length === 0) {
      log.warn("renegotiate: no supported_versions in response");
      return 0;
    }

    const version = selectHighestCommon(
      CLIENT_SUPPORTED_VERSIONS,
      serverVersions,
    );
    if (version === 0) {
      log.warn(
        `renegotiate: no common version (client=${CLIENT_SUPPORTED_VERSIONS}, server=${serverVersions})`,
      );
      return 0;
    }

    // Update the live contract with the negotiated version and matching chat path.
    protocolContract.backendProtocolVersion = version;
    protocolContract.openai.chatCompletionsPath = chatPathForVersion(version);

    log.info(
      `renegotiate: negotiated version ${version}, chat path ${protocolContract.openai.chatCompletionsPath}`,
    );
    return version;
  } catch (err) {
    log.warn(`renegotiate: fetch failed: ${String(err)}`);
    return 0;
  }
}

/** @returns {ProtocolContract} the parsed contract, or fallback on failure. */
function loadProtocolContract(): ProtocolContract {
  // NOTE: __dirname is used because VS Code extensions use CommonJS.
  // If migrating to ESM, use: `import.meta.url` + `fileURLToPath`.
  const contractPath = path.resolve(
    __dirname,
    "..",
    "..",
    "contracts",
    "editor-capability-matrix.json",
  );

  try {
    const raw = fs.readFileSync(contractPath, "utf8");
    const contract = JSON.parse(raw) as ProtocolContract;
    // Override baseUrl with resolved value so consumers get the correct backend address.
    contract.runtime.baseUrl = resolveBaseUrl();
    // Default backendProtocolVersion to 0 when the file doesn't carry it yet.
    contract.backendProtocolVersion ??= 0;
    return contract;
  } catch (err) {
    log.warn("load failed:", err);
    return {
      ...fallbackContract,
      runtime: { ...fallbackContract.runtime, baseUrl: resolveBaseUrl() },
      backendProtocolVersion: 0,
    };
  }
}

export let protocolContract = loadProtocolContract();

/**
 * Refresh interval for protocol contract (milliseconds).
 * Re-fetches the contract every 5 minutes so the extension picks up
 * backend API upgrades without requiring a reload.
 */
const REFRESH_INTERVAL_MS = 5 * 60 * 1000;

/** Reload the protocol contract from disk, updating the exported reference. */
function refreshProtocolContract(): void {
  try {
    const contract = loadProtocolContract();
    protocolContract = contract;
  } catch (err) {
    log.warn("refresh failed, keeping stale contract:", err);
  }
  // Re-resolve baseUrl on refresh so config/env changes are picked up.
  const newBaseUrl = resolveBaseUrl();
  protocolContract.runtime.baseUrl = newBaseUrl;

  // Fire-and-forget backend renegotiation so the chat path and protocol version
  // stay in sync with the running backend instance.
  renegotiateWithBackend().catch((err: unknown) => {
    log.warn("renegotiate on refresh failed:", String(err));
  });
}

// Periodically refresh the contract so the extension adapts to backend upgrades.
// The interval is intentionally not cleared — the extension lives for the lifetime
// of the VS Code window, so the refresh runs until the window closes.
setInterval(refreshProtocolContract, REFRESH_INTERVAL_MS);

const protocolModeAliases: Record<string, string> = {
  adaptive: "adaptive",
  auto: "adaptive",
  acp_stdio: "acp_stdio",
  "acp+stdio": "acp_stdio",
  "acp-stdio": "acp_stdio",
  acp: "acp_stdio",
  acp_http: "acp_http",
  "acp+http": "acp_http",
  "acp-http": "acp_http",
  mcp_stdio: "mcp_stdio",
  "mcp+stdio": "mcp_stdio",
  "mcp-stdio": "mcp_stdio",
  mcp: "mcp_stdio",
  mcp_http: "mcp_http",
  "mcp+http": "mcp_http",
  "mcp-http": "mcp_http",
  from_config: "from_config",
};

export function normalizeProtocolMode(mode: string): string {
  const normalized = protocolModeAliases[mode.trim().toLowerCase()];
  return normalized ?? mode.trim().toLowerCase();
}

export function isAllowedProtocolMode(mode: string): boolean {
  if (mode === "from_config") {
    return true;
  }
  return protocolContract.protocol.supportedModes.includes(mode);
}
