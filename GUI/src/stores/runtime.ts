import { defineStore } from "pinia";
import {
    checkHealthWithMeta,
    getEditorIntegrationStatusWithMeta,
    getAiUsageSnapshotWithMeta,
    getEndpointHealthStatsWithMeta,
    getUsageHeatmapWithMeta,
    getRecentLogs,
    type EndpointHealthStat,
    type EditorIntegrationStatus,
    type UsageHeatmap,
    serviceStatus,
    type AiUsageSnapshot,
    type HealthSnapshot,
    type LogChunk,
    type ServiceStatus,
} from "../services/bridge";
import { defaultRuntimeBaseUrl } from "../services/protocolContract";

const defaultHealthEndpoint = `${defaultRuntimeBaseUrl.replace(/\/$/, "")}/health`;

export const useRuntimeStore = defineStore("runtime", {
    state: () => ({
        status: { running: false } as ServiceStatus,
        health: { ok: false, endpoint: defaultHealthEndpoint } as HealthSnapshot,
        aiUsage: {
            timestamp: new Date().toISOString(),
            requestsPerMinute: 0,
            successRate: 0,
            avgLatencyMs: 0,
            timeoutCount: 0,
            rateLimitCount: 0,
            breakerCount: 0,
            upstreamFailureCount: 0,
        } as AiUsageSnapshot,
        logs: { path: "", lines: [], totalLinesRead: 0 } as LogChunk,
        endpointHealthStats: [] as EndpointHealthStat[],
        editorIntegrations: [] as EditorIntegrationStatus[],
        heatmapWindowSeconds: 300,
        statusPollingMs: Number(localStorage.getItem("goon.gui.statusPollingMs") || "2000"),
        logsPollingMs: Number(localStorage.getItem("goon.gui.logsPollingMs") || "1000"),
        usageHeatmap: {
            windowSeconds: 300,
            phaseTop: [],
            agentTop: [],
            trend: [],
            confidence: "event-buffer",
        } as UsageHeatmap,
        statusTimer: undefined as number | undefined,
        logsTimer: undefined as number | undefined,
        statusPollingInFlight: false,
        logsPollingInFlight: false,
        loading: false,
        lastError: "",
        offline: false,
        lastKnownStatus: { running: false } as ServiceStatus,
        lastKnownHealth: { ok: false, endpoint: defaultHealthEndpoint } as HealthSnapshot,
        lastKnownAiUsage: {
            timestamp: new Date().toISOString(),
            requestsPerMinute: 0,
            successRate: 0,
            avgLatencyMs: 0,
            timeoutCount: 0,
            rateLimitCount: 0,
            breakerCount: 0,
            upstreamFailureCount: 0,
        } as AiUsageSnapshot,
        healthStale: false,
        aiUsageStale: false,
        logsStale: false,
        endpointHealthStatsStale: false,
        editorIntegrationsStale: false,
        usageHeatmapStale: false,
        statusPollingGeneration: 0,
        logsPollingGeneration: 0,
    }),
    getters: {
        hasStaleData(state) {
            return state.healthStale
                || state.aiUsageStale
                || state.logsStale
                || state.endpointHealthStatsStale
                || state.editorIntegrationsStale
                || state.usageHeatmapStale;
        },
    },
    actions: {
        async refreshStatus() {
            try {
                this.status = await serviceStatus();
                this.lastKnownStatus = this.status;
                this.offline = false;
            } catch (error) {
                this.lastError = String(error);
                this.offline = true;
                // Keep last known status
            }
        },
        async refreshHealth() {
            try {
                const result = await checkHealthWithMeta();
                this.health = result.value;
                this.lastKnownHealth = this.health;
                this.offline = false;
                this.healthStale = result.cache.cached;
            } catch (error) {
                this.lastError = String(error);
                this.offline = true;
                // Restore from last known
                this.health = this.lastKnownHealth;
                this.healthStale = true;
            }
        },
        async refreshAiUsage() {
            try {
                const result = await getAiUsageSnapshotWithMeta();
                this.aiUsage = result.value;
                this.lastKnownAiUsage = this.aiUsage;
                this.aiUsageStale = result.cache.cached;
            } catch (error) {
                this.lastError = String(error);
                // Restore from last known
                this.aiUsage = this.lastKnownAiUsage;
                this.aiUsageStale = true;
            }
        },
        async refreshLogs(lines = 200) {
            try {
                this.logs = await getRecentLogs(undefined, lines);
                this.logsStale = false;
            } catch (error) {
                this.lastError = String(error);
                // Logs are not critical for offline mode
                this.logsStale = true;
            }
        },
        async refreshUsageHeatmap() {
            try {
                const result = await getUsageHeatmapWithMeta(this.heatmapWindowSeconds);
                this.usageHeatmap = result.value;
                this.usageHeatmapStale = result.cache.cached;
            } catch (error) {
                this.lastError = String(error);
                this.usageHeatmapStale = true;
            }
        },
        async refreshEditorIntegrations() {
            try {
                const result = await getEditorIntegrationStatusWithMeta();
                this.editorIntegrations = result.value;
                this.editorIntegrationsStale = result.cache.cached;
            } catch (error) {
                this.lastError = String(error);
                this.editorIntegrationsStale = true;
            }
        },
        async refreshEndpointHealthStats() {
            try {
                const result = await getEndpointHealthStatsWithMeta();
                this.endpointHealthStats = result.value;
                this.endpointHealthStatsStale = result.cache.cached;
            } catch (error) {
                this.lastError = String(error);
                this.endpointHealthStatsStale = true;
            }
        },
        async setHeatmapWindow(seconds: number) {
            this.heatmapWindowSeconds = seconds;
            await this.refreshUsageHeatmap();
        },
        setStatusPollingInterval(ms: number) {
            this.statusPollingMs = Math.min(10000, Math.max(500, Number(ms)));
            localStorage.setItem("goon.gui.statusPollingMs", String(this.statusPollingMs));
            this.startStatusPolling();
        },
        setLogsPollingInterval(ms: number) {
            this.logsPollingMs = Math.min(10000, Math.max(500, Number(ms)));
            localStorage.setItem("goon.gui.logsPollingMs", String(this.logsPollingMs));
        },
        async refreshAll() {
            this.loading = true;
            await Promise.all([
                this.refreshStatus(),
                this.refreshHealth(),
                this.refreshAiUsage(),
                this.refreshUsageHeatmap(),
                this.refreshEditorIntegrations(),
                this.refreshEndpointHealthStats(),
            ]);
            this.loading = false;
        },
        startStatusPolling() {
            this.stopStatusPolling();
            const generation = ++this.statusPollingGeneration;
            void this.refreshAll();
            this.statusTimer = window.setInterval(async () => {
                if (generation !== this.statusPollingGeneration) {
                    return;
                }
                if (this.statusPollingInFlight) {
                    return;
                }
                this.statusPollingInFlight = true;
                try {
                    await this.refreshAll();
                } finally {
                    this.statusPollingInFlight = false;
                }
            }, this.statusPollingMs);
        },
        stopStatusPolling() {
            this.statusPollingGeneration += 1;
            if (this.statusTimer) {
                window.clearInterval(this.statusTimer);
                this.statusTimer = undefined;
            }
            this.statusPollingInFlight = false;
        },
        startLogsPolling(lines = 200) {
            this.stopLogsPolling();
            const generation = ++this.logsPollingGeneration;
            void this.refreshLogs(lines);
            this.logsTimer = window.setInterval(async () => {
                if (generation !== this.logsPollingGeneration) {
                    return;
                }
                if (this.logsPollingInFlight) {
                    return;
                }
                this.logsPollingInFlight = true;
                try {
                    await this.refreshLogs(lines);
                } finally {
                    this.logsPollingInFlight = false;
                }
            }, this.logsPollingMs);
        },
        stopLogsPolling() {
            this.logsPollingGeneration += 1;
            if (this.logsTimer) {
                window.clearInterval(this.logsTimer);
                this.logsTimer = undefined;
            }
            this.logsPollingInFlight = false;
        },
    },
});
