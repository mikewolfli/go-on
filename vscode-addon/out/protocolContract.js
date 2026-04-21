"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isAllowedProtocolMode = exports.normalizeProtocolMode = exports.defaultPlatformMode = exports.platformModes = exports.defaultWorkflowControlMode = exports.workflowControlModes = exports.protocolContract = void 0;
const fs = require("fs");
const path = require("path");
const fallbackContract = {
    version: '2026-04-14-blue13-r4.10-fallback',
    runtime: {
        baseUrl: 'http://127.0.0.1:8090',
        healthPath: '/health',
    },
    protocol: {
        supportedModes: ['adaptive', 'acp_stdio', 'acp_http', 'mcp_stdio', 'mcp_http'],
        workflowControlModes: ['manual', 'assisted', 'autonomous'],
        defaultWorkflowControlMode: 'assisted',
        platformModes: ['universal', 'phase_compat'],
        defaultPlatformMode: 'phase_compat',
        universalPlatformEnabled: true,
        phaseCompatMappingEnabled: true,
        universalExecutionCycleSchemaVersion: 'blue23-universal-cycle-v1',
        universalGateModelCheckedInMainChain: true,
        universalResponseSkeletonCheckedInMainChain: true,
        universalSandboxProfileCheckedInMainChain: true,
        universalApprovalCheckpointCheckedInMainChain: true,
        universalCapabilityProfileCheckedInMainChain: true,
        defaultMode: 'adaptive',
        autoModeSupportsAcpAndMcp: true,
        protocolCapabilityModel: 'capability_plus_transport',
        adaptiveSelectionModel: 'client_type_routed',
        adaptiveStartupTransportStrategy: 'http_if_bind_else_stdio',
        fixedModesAreConfigDriven: true,
        acpInitializeProtocol: 'acp',
        mcpInitializeProtocolVersion: '2024-11-05',
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
        modelsPath: '/v1/models',
        modelAliasPath: '/v1/model',
        chatCompletionsPath: '/v1/chat/completions',
        streamDoneMarker: '[DONE]',
        commonForwardedFields: [
            'temperature',
            'top_p',
            'max_tokens',
            'n',
            'stop',
            'presence_penalty',
            'frequency_penalty',
            'logit_bias',
            'user',
            'seed',
            'response_format',
            'tools',
            'tool_choice',
            'parallel_tool_calls',
            'function_call',
            'functions',
        ],
        requiredAssertions: {
            modelsDataNonEmpty: true,
            chatContentNonEmpty: true,
            sseHasDataFrame: true,
            sseHasDoneMarker: true,
        },
    },
    statusTerms: {
        healthy: 'Healthy',
        healthCheckFailed: 'Health check failed',
        running: 'Running',
        stopped: 'Stopped',
    },
    errors: {
        providerNotReady: 'No runtime-ready AI provider configured.',
        setupWizardOpened: 'Setup wizard opened.',
        setupWizardPrompt: 'No runtime-ready AI provider is configured. Opening Go-On setup wizard now.',
        runtimeProbePassed: 'runtime.health semantic probe passed',
        requestErrorKinds: ['PuaViolation', 'BudgetExceeded', 'SandboxBlocked'],
        requestErrorContextPrefix: 'acp.handle_request.dispatch',
    },
    surfaces: {
        gui: {
            checks: ['health', 'models'],
            supports: {
                openAiCompat: true,
                responsesNative: false,
            },
        },
        vscodeAddon: {
            checks: ['runtime.health'],
            supports: {
                openAiCompat: true,
                responsesNative: false,
            },
        },
    },
    responsesApi: {
        path: '/v1/responses',
        retrievalPath: '/v1/responses/{id}',
        listPath: '/v1/responses',
        r1Status: 'baseline',
        responseRetrievalSupport: true,
        responseListSupport: true,
        responseListObjectType: 'list',
        responseListDataContainsResponseObjects: true,
        responseListNewestFirst: true,
        responseIdsAreUniquePerRequest: true,
        responseIdHasTimestampAndSequence: true,
        responseStoreTracksCompleted: true,
        responseStoreTracksFailed: true,
        responseStatusLifecycle: ['queued', 'in_progress', 'completed', 'failed'],
        responseStatusHistoryField: 'status_history',
        responseHistoryIncludesTransitions: true,
        toolCallInitiationSupport: true,
        toolChoiceRequiredReturnsIncomplete: true,
        toolCallOutputItemSupport: true,
        toolResultContinuationSupport: true,
        previousResponseIdMustBeNonEmptyString: true,
        toolResultRequiresMatchingToolCallId: true,
        toolResultOutputItemSupport: true,
        failureCodeClasses: ['timeout', 'rate_limit', 'tool_error', 'upstream_error'],
        toolLoopMissingResultUsesToolError: true,
        noPendingToolCallUsesToolError: true,
        upstreamTimeoutMapped: true,
        upstreamRateLimitMapped: true,
        retrievalNotFoundUsesResponsesErrorShape: true,
        streamSupport: true,
        streamEvents: ['response.created', 'response.output_text.delta', 'response.completed', 'response.failed'],
        streamTerminatesWithDone: true,
        requestBodyMustBeObject: true,
        modelMustBeNonEmptyString: true,
        modelMustBeString: true,
        acceptedInputTypes: ['string', 'array'],
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
        toolChoiceAllowedTypes: ['string', 'object'],
        toolChoiceStringValues: ['auto', 'none', 'required'],
        toolChoiceRequiredNeedsTools: true,
        toolChoiceObjectMustUseFunctionType: true,
        toolChoiceObjectRequiresFunctionName: true,
        toolChoiceObjectRequiresTools: true,
        toolChoiceObjectMustReferenceDeclaredTool: true,
        emptyBodyUsesResponsesErrorShape: true,
        invalidJsonUsesResponsesErrorShape: true,
        errorHasCodeField: true,
        goldenCasesImplemented: true,
        responseRequiredFields: ['id', 'object', 'created_at', 'model', 'status', 'output', 'usage', 'error', 'incomplete_details'],
        errorRequiredFields: ['code', 'type', 'message'],
        streamEventOrder: ['response.created', 'response.output_text.delta', 'response.completed'],
        rootCapabilitiesPath: '/',
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
function loadProtocolContract() {
    const contractPath = path.resolve(__dirname, '..', '..', 'contracts', 'editor-capability-matrix.json');
    try {
        const raw = fs.readFileSync(contractPath, 'utf8');
        return JSON.parse(raw);
    }
    catch {
        return fallbackContract;
    }
}
exports.protocolContract = loadProtocolContract();
exports.workflowControlModes = exports.protocolContract.protocol.workflowControlModes ?? ['manual', 'assisted', 'autonomous'];
exports.defaultWorkflowControlMode = exports.protocolContract.protocol.defaultWorkflowControlMode ?? 'assisted';
exports.platformModes = exports.protocolContract.protocol.platformModes ?? ['universal', 'phase_compat'];
exports.defaultPlatformMode = exports.protocolContract.protocol.defaultPlatformMode ?? 'phase_compat';
const protocolModeAliases = {
    adaptive: 'adaptive',
    auto: 'adaptive',
    acp_stdio: 'acp_stdio',
    'acp+stdio': 'acp_stdio',
    'acp-stdio': 'acp_stdio',
    acp: 'acp_stdio',
    acp_http: 'acp_http',
    'acp+http': 'acp_http',
    'acp-http': 'acp_http',
    mcp_stdio: 'mcp_stdio',
    'mcp+stdio': 'mcp_stdio',
    'mcp-stdio': 'mcp_stdio',
    mcp: 'mcp_stdio',
    mcp_http: 'mcp_http',
    'mcp+http': 'mcp_http',
    'mcp-http': 'mcp_http',
    from_config: 'from_config',
};
function normalizeProtocolMode(mode) {
    const normalized = protocolModeAliases[mode.trim().toLowerCase()];
    return normalized ?? mode.trim().toLowerCase();
}
exports.normalizeProtocolMode = normalizeProtocolMode;
function isAllowedProtocolMode(mode) {
    if (mode === 'from_config') {
        return true;
    }
    return exports.protocolContract.protocol.supportedModes.includes(mode);
}
exports.isAllowedProtocolMode = isAllowedProtocolMode;
//# sourceMappingURL=protocolContract.js.map