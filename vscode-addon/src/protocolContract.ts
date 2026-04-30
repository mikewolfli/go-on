import * as fs from "fs";
import * as path from "path";

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

export type ProtocolContract = {
  version: string;
  runtime: {
    baseUrl: string;
    healthPath: string;
  };
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
    return JSON.parse(raw) as ProtocolContract;
  } catch {
    return fallbackContract;
  }
}

export const protocolContract = loadProtocolContract();
export const workflowControlModes = protocolContract.protocol
  .workflowControlModes ?? ["manual", "assisted", "autonomous"];
export const defaultWorkflowControlMode =
  protocolContract.protocol.defaultWorkflowControlMode ?? "assisted";
export const platformModes = protocolContract.protocol.platformModes ?? [
  "universal",
  "phase_compat",
];
export const defaultPlatformMode =
  protocolContract.protocol.defaultPlatformMode ?? "phase_compat";

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
