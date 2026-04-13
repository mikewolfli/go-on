import { defineStore } from "pinia";
import {
    checkHealth,
    getEditorIntegrationStatus,
    getAiUsageSnapshot,
    getEndpointHealthStats,
    getUsageHeatmap,
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

export const useRuntimeStore = defineStore("runtime", {
    state: () => ({
        status: { running: false } as ServiceStatus,
        health: { ok: false, endpoint: "http://127.0.0.1:8090/health" } as HealthSnapshot,
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
        loading: false,
        lastError: "",
        offline: false,
        lastKnownStatus: { running: false } as ServiceStatus,
        lastKnownHealth: { ok: false, endpoint: "http://127.0.0.1:8090/health" } as HealthSnapshot,
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
    }),
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
                this.health = await checkHealth();
                this.lastKnownHealth = this.health;
                this.offline = false;
            } catch (error) {
                this.lastError = String(error);
                this.offline = true;
                // Restore from last known
                this.health = this.lastKnownHealth;
            }
        },
        async refreshAiUsage() {
            try {
                this.aiUsage = await getAiUsageSnapshot();
                this.lastKnownAiUsage = this.aiUsage;
                this.offline = false;
            } catch (error) {
                this.lastError = String(error);
                this.offline = true;
                // Restore from last known
                this.aiUsage = this.lastKnownAiUsage;
            }
        },
        async refreshLogs(lines = 200) {
            try {
                this.logs = await getRecentLogs(undefined, lines);
                this.offline = false;
            } catch (error) {
                this.lastError = String(error);
                this.offline = true;
                // Logs are not critical for offline mode
            }
        },
        async refreshUsageHeatmap() {
            try {
                this.usageHeatmap = await getUsageHeatmap(this.heatmapWindowSeconds);
            } catch (error) {
                this.lastError = String(error);
            }
        },
        async refreshEditorIntegrations() {
            try {
                this.editorIntegrations = await getEditorIntegrationStatus();
            } catch (error) {
                this.lastError = String(error);
            }
        },
        async refreshEndpointHealthStats() {
            try {
                this.endpointHealthStats = await getEndpointHealthStats();
            } catch (error) {
                this.lastError = String(error);
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
            this.refreshAll();
            this.statusTimer = window.setInterval(() => {
                this.refreshAll();
            }, this.statusPollingMs);
        },
        stopStatusPolling() {
            if (this.statusTimer) {
                window.clearInterval(this.statusTimer);
                this.statusTimer = undefined;
            }
        },
        startLogsPolling(lines = 200) {
            this.stopLogsPolling();
            this.refreshLogs(lines);
            this.logsTimer = window.setInterval(() => {
                this.refreshLogs(lines);
            }, this.logsPollingMs);
        },
        stopLogsPolling() {
            if (this.logsTimer) {
                window.clearInterval(this.logsTimer);
                this.logsTimer = undefined;
            }
        },
    },
});
