import { invoke } from "@tauri-apps/api/core";
import { rpcCache } from "../utils/cache";

// Cache decorator helper
async function withCache<T>(
    key: string,
    fn: () => Promise<T>,
    ttl: number = 5000
): Promise<T> {
    const cached = rpcCache.get<T>(key);
    if (cached !== null) {
        return cached;
    }
    const result = await fn();
    rpcCache.set(key, result, ttl);
    return result;
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

export async function configureService(executablePath: string, workingDir: string) {
    return invoke<void>("configure_service", { executablePath, workingDir });
}

export async function configureServiceByExecutable(executablePath: string) {
    return invoke<void>("configure_service_by_executable", { executablePath });
}

export async function backendExecutableExists() {
    return invoke<boolean>("backend_executable_exists");
}

export async function autoConfigureBackendPath() {
    return invoke<AutoConfigureResult>("auto_configure_backend_path");
}

export async function exitApp() {
    return invoke<void>("exit_app");
}

export async function resetDefaultSettings() {
    return invoke<string>("reset_default_settings");
}

export async function setProviderApiKey(provider: string, apiKey: string, envVar?: string) {
    return invoke<string>("set_provider_api_key", { provider, apiKey, envVar });
}

export async function clearProviderApiKey(provider: string, envVar?: string) {
    return invoke<string>("clear_provider_api_key", { provider, envVar });
}

export async function fetchGithubCopilotToken() {
    return invoke<CopilotTokenResult>("fetch_github_copilot_token");
}

export async function invokeRuntimeRpc(method: string, paramsJson?: string) {
    return invoke<string>("invoke_runtime_rpc", { method, paramsJson });
}

export async function startService() {
    return invoke<ServiceStatus>("start_service");
}

export async function stopService() {
    return invoke<ServiceStatus>("stop_service");
}

export async function restartService() {
    return invoke<ServiceStatus>("restart_service");
}

export async function serviceStatus() {
    return invoke<ServiceStatus>("service_status");
}

export async function checkHealth(endpoint?: string) {
    const cacheKey = `health:${endpoint || "default"}`;
    return withCache(cacheKey, () => invoke<HealthSnapshot>("check_health", { endpoint }), 3000);
}

export async function getAiUsageSnapshot() {
    return withCache("ai_usage", () => invoke<AiUsageSnapshot>("get_ai_usage_snapshot"), 5000);
}

export async function getUsageHeatmap(windowSeconds = 300) {
    const cacheKey = `heatmap:${windowSeconds}`;
    return withCache(cacheKey, () => invoke<UsageHeatmap>("get_usage_heatmap", { windowSeconds }), 5000);
}

export async function getEndpointHealthStats() {
    return withCache("endpoint_stats", () => invoke<EndpointHealthStat[]>("get_endpoint_health_stats"), 5000);
}

export async function getEditorIntegrationStatus() {
    return withCache("editor_status", () => invoke<EditorIntegrationStatus[]>("get_editor_integration_status"), 5000);
}

export async function getRecentLogs(logPath?: string, lines = 200) {
    // Logs are not cached to always get latest
    return invoke<LogChunk>("read_recent_logs", { logPath, lines, maskSensitive: true });
}

export async function runCliCommand(command: string) {
    return invoke<string>("run_cli_command", { command });
}

export async function showMiniConsole() {
    return invoke<void>("show_mini_console");
}

export async function hideMiniConsole() {
    return invoke<void>("hide_mini_console");
}
