import { invoke } from "@tauri-apps/api/core";
import { rpcCache } from "../utils/cache";

export interface CacheMetadata {
    cached: boolean;
    cachedAt: number;
    ageMs: number;
    ttl: number;
}

export interface CachedResponse<T> {
    value: T;
    cache: CacheMetadata;
}

const DEFAULT_INVOKE_TIMEOUT_MS = 15000;
const STARTUP_INVOKE_TIMEOUT_MS = 20000;
const RUNTIME_RPC_TIMEOUT_MS = 30000;

function buildCacheMetadata(cached: boolean, cachedAt: number, ttl: number): CacheMetadata {
    return {
        cached,
        cachedAt,
        ageMs: Math.max(0, Date.now() - cachedAt),
        ttl,
    };
}

async function invokeWithTimeout<T>(
    command: string,
    args?: Record<string, unknown>,
    timeoutMs: number = DEFAULT_INVOKE_TIMEOUT_MS,
): Promise<T> {
    return await new Promise<T>((resolve, reject) => {
        const timeoutId = window.setTimeout(() => {
            reject(new Error(`Tauri command timed out after ${timeoutMs}ms: ${command}`));
        }, timeoutMs);

        invoke<T>(command, args)
            .then((result) => {
                window.clearTimeout(timeoutId);
                resolve(result);
            })
            .catch((error) => {
                window.clearTimeout(timeoutId);
                reject(error);
            });
    });
}

async function withCacheMeta<T>(
    key: string,
    fn: () => Promise<T>,
    ttl: number = 5000
): Promise<CachedResponse<T>> {
    const cachedEntry = rpcCache.getEntry<T>(key);
    if (cachedEntry !== null) {
        return {
            value: cachedEntry.value,
            cache: buildCacheMetadata(true, cachedEntry.timestamp, cachedEntry.ttl),
        };
    }

    const fetchedAt = Date.now();
    const value = await fn();
    rpcCache.set(key, value, ttl);
    return {
        value,
        cache: buildCacheMetadata(false, fetchedAt, ttl),
    };
}

export interface ServiceStatus {
    running: boolean;
    pid?: number;
    executablePath?: string;
    workingDir?: string;
    uptimeSeconds?: number;
    startedAt?: string;
    lastError?: string;
}

export interface HealthSnapshot {
    ok: boolean;
    endpoint: string;
    responseCode?: number;
    responseBody?: string;
    message?: string;
}

export interface AiUsageSnapshot {
    timestamp: string;
    requestsPerMinute: number;
    successRate: number;
    avgLatencyMs: number;
    timeoutCount: number;
    rateLimitCount: number;
    breakerCount: number;
    upstreamFailureCount: number;
}

export interface LogChunk {
    path: string;
    lines: string[];
    totalLinesRead: number;
}

export interface NameCount {
    name: string;
    count: number;
}

export interface TrendPoint {
    secondBucket: number;
    count: number;
}

export interface UsageHeatmap {
    windowSeconds: number;
    phaseTop: NameCount[];
    agentTop: NameCount[];
    trend: TrendPoint[];
    confidence: string;
}

export interface EndpointHealthStat {
    endpoint: string;
    total: number;
    success: number;
    failure: number;
    successRate: number;
    avgLatencyMs: number;
}

export interface EditorIntegrationStatus {
    editor: string;
    interfaceName: string;
    protocolMode: string;
    processRunning: boolean;
    processCount: number;
    transport: string;
    endpoint?: string;
    endpointOk: boolean;
    endpointCode?: number;
    addonPresent: boolean;
    note: string;
}

export interface CopilotTokenResult {
    found: boolean;
    source: string;
    tokenMasked?: string;
    tokenPlain?: string;
    verificationUri?: string;
    userCode?: string;
    expiresInSeconds?: number;
    pollIntervalSeconds?: number;
    note: string;
}

export interface AutoConfigureResult {
    linked: boolean;
    executablePath?: string;
    reason: string;
}

export interface ProviderCatalogEntry {
    name: string;
    agentType: string;
    defaultModel?: string;
    apiKeyEnv?: string;
    secretKeyEnv?: string;
    url?: string;
    chatPath?: string;
    supportsSystem?: boolean;
    configuredModel?: string;
    configuredEnvVar?: string;
}

export interface ProviderSelectionSaveResult {
    provider: string;
    model: string;
    configPath: string;
    note: string;
}

export async function configureService(executablePath: string, workingDir: string, protocolMode?: string) {
    return invokeWithTimeout<void>(
        "configure_service",
        { executablePath, workingDir, protocolMode },
        STARTUP_INVOKE_TIMEOUT_MS
    );
}

export async function configureServiceByExecutable(executablePath: string) {
    return invokeWithTimeout<void>("configure_service_by_executable", { executablePath }, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function configureServiceByDirectory(directoryPath: string) {
    return invokeWithTimeout<void>("configure_service_by_directory", { directoryPath }, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function backendExecutableExists() {
    return invokeWithTimeout<boolean>("backend_executable_exists");
}

export async function autoConfigureBackendPath() {
    return invokeWithTimeout<AutoConfigureResult>("auto_configure_backend_path", undefined, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function exitApp() {
    return invokeWithTimeout<void>("exit_app");
}

export async function resetDefaultSettings() {
    return invokeWithTimeout<string>("reset_default_settings");
}

export async function setProviderApiKey(provider: string, apiKey: string, envVar?: string) {
    return invokeWithTimeout<string>("set_provider_api_key", { provider, apiKey, envVar }, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function clearProviderApiKey(provider: string, envVar?: string) {
    return invokeWithTimeout<string>("clear_provider_api_key", { provider, envVar }, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function listProviderCatalog() {
    return invokeWithTimeout<ProviderCatalogEntry[]>("list_provider_catalog");
}

export async function saveProviderSelection(provider: string, model: string, envVar?: string) {
    return invokeWithTimeout<ProviderSelectionSaveResult>("save_provider_selection", { provider, model, envVar }, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function fetchGithubCopilotToken() {
    return invokeWithTimeout<CopilotTokenResult>("fetch_github_copilot_token", undefined, RUNTIME_RPC_TIMEOUT_MS);
}

export async function invokeRuntimeRpc(method: string, paramsJson?: string) {
    return invokeWithTimeout<string>("invoke_runtime_rpc", { method, paramsJson }, RUNTIME_RPC_TIMEOUT_MS);
}

export async function startService() {
    return invokeWithTimeout<ServiceStatus>("start_service", undefined, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function stopService() {
    return invokeWithTimeout<ServiceStatus>("stop_service", undefined, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function restartService() {
    return invokeWithTimeout<ServiceStatus>("restart_service", undefined, STARTUP_INVOKE_TIMEOUT_MS);
}

export async function serviceStatus() {
    return invokeWithTimeout<ServiceStatus>("service_status");
}

export async function checkHealthWithMeta(endpoint?: string, options?: { bypassCache?: boolean }) {
    const cacheKey = `health:${endpoint || "default"}`;
    if (options?.bypassCache) {
        const fetchedAt = Date.now();
        const value = await invokeWithTimeout<HealthSnapshot>("check_health", { endpoint });
        return {
            value,
            cache: buildCacheMetadata(false, fetchedAt, 0),
        };
    }
    return withCacheMeta(cacheKey, () => invokeWithTimeout<HealthSnapshot>("check_health", { endpoint }), 3000);
}

export async function checkHealth(endpoint?: string, options?: { bypassCache?: boolean }) {
    return (await checkHealthWithMeta(endpoint, options)).value;
}

export async function getAiUsageSnapshotWithMeta() {
    return withCacheMeta("ai_usage", () => invokeWithTimeout<AiUsageSnapshot>("get_ai_usage_snapshot"), 5000);
}

export async function getAiUsageSnapshot() {
    return (await getAiUsageSnapshotWithMeta()).value;
}

export async function getUsageHeatmapWithMeta(windowSeconds = 300) {
    const cacheKey = `heatmap:${windowSeconds}`;
    return withCacheMeta(cacheKey, () => invokeWithTimeout<UsageHeatmap>("get_usage_heatmap", { windowSeconds }), 5000);
}

export async function getUsageHeatmap(windowSeconds = 300) {
    return (await getUsageHeatmapWithMeta(windowSeconds)).value;
}

export async function getEndpointHealthStatsWithMeta() {
    return withCacheMeta("endpoint_stats", () => invokeWithTimeout<EndpointHealthStat[]>("get_endpoint_health_stats"), 5000);
}

export async function getEndpointHealthStats() {
    return (await getEndpointHealthStatsWithMeta()).value;
}

export async function getEditorIntegrationStatusWithMeta() {
    return withCacheMeta("editor_status", () => invokeWithTimeout<EditorIntegrationStatus[]>("get_editor_integration_status"), 5000);
}

export async function getEditorIntegrationStatus() {
    return (await getEditorIntegrationStatusWithMeta()).value;
}

export async function getRecentLogs(logPath?: string, lines = 200) {
    // Logs are not cached to always get latest
    return invokeWithTimeout<LogChunk>("read_recent_logs", { logPath, lines, maskSensitive: true });
}

export async function runCliCommand(command: string) {
    return invokeWithTimeout<string>("run_cli_command", { command }, RUNTIME_RPC_TIMEOUT_MS);
}

export async function showMiniConsole() {
    return invokeWithTimeout<void>("show_mini_console");
}

export async function hideMiniConsole() {
    return invokeWithTimeout<void>("hide_mini_console");
}

export async function switchToMainWindow() {
    return invokeWithTimeout<void>("switch_to_main_window");
}

export async function switchToMiniWindow() {
    return invokeWithTimeout<void>("switch_to_mini_window");
}
