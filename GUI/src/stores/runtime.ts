import { defineStore } from "pinia";
import {
  checkHealthWithMeta,
  getEditorIntegrationStatusWithMeta,
  getAiUsageSnapshotWithMeta,
  getEndpointHealthStatsWithMeta,
  getUsageHeatmapWithMeta,
  getRecentLogs,
  fetchRuntimeFeatures,
  type EndpointHealthStat,
  type EditorIntegrationStatus,
  type UsageHeatmap,
  serviceStatus,
  type AiUsageSnapshot,
  type HealthSnapshot,
  type LogChunk,
  type ServiceStatus,
  type RuntimeFeatures,
} from "../services/bridge";
import { defaultRuntimeBaseUrl } from "../services/protocolContract";

const defaultHealthEndpoint = `${defaultRuntimeBaseUrl.replace(/\/$/, "")}/health`;

function safeGetItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function safeSetItem(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}

const STATUS_KEY = "goon.gui.statusPollingMs";
const LOGS_KEY = "goon.gui.logsPollingMs";

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
    statusPollingMs: Number(safeGetItem("goon.gui.statusPollingMs") || "2000"),
    logsPollingMs: Number(safeGetItem("goon.gui.logsPollingMs") || "1000"),
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
    refreshAllInProgress: false,
    loading: false,
    activeFeatures: {} as Partial<RuntimeFeatures>,
    lastError: "",
    offline: false,
    lastKnownStatus: { running: false } as ServiceStatus,
    lastKnownHealth: {
      ok: false,
      endpoint: defaultHealthEndpoint,
    } as HealthSnapshot,
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
      return (
        state.healthStale ||
        state.aiUsageStale ||
        state.logsStale ||
        state.endpointHealthStatsStale ||
        state.editorIntegrationsStale ||
        state.usageHeatmapStale
      );
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
      safeSetItem("goon.gui.statusPollingMs", String(this.statusPollingMs));
      this.startStatusPolling();
    },
    setLogsPollingInterval(ms: number) {
      this.logsPollingMs = Math.min(10000, Math.max(500, Number(ms)));
      safeSetItem("goon.gui.logsPollingMs", String(this.logsPollingMs));
    },
    async refreshFeatures() {
      if (!this.status.running) {
        return;
      }
      try {
        this.activeFeatures = await fetchRuntimeFeatures();
      } catch {
        // keep previous features on error
      }
    },
    async refreshAll() {
      if (this.refreshAllInProgress) {
        return;
      }
      this.refreshAllInProgress = true;
      this.loading = true;
      try {
        await Promise.all([
          this.refreshStatus(),
          this.refreshHealth(),
          this.refreshAiUsage(),
          this.refreshUsageHeatmap(),
          this.refreshEditorIntegrations(),
          this.refreshEndpointHealthStats(),
          this.refreshFeatures(),
        ]);
      } finally {
        this.refreshAllInProgress = false;
        this.loading = false;
      }
    },
    startStatusPolling() {
      this.stopStatusPolling();
      const generation = ++this.statusPollingGeneration;

      // Set up the interval first to prevent orphaned timer if
      // stopStatusPolling() is called during the initial refresh.
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

      // Initial refresh (non-blocking)
      this.statusPollingInFlight = true;
      this.refreshAll().finally(() => {
        this.statusPollingInFlight = false;
      });
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

      // Set up the interval first to prevent orphaned timer if
      // stopLogsPolling() is called during the initial refresh.
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

      // Initial refresh (non-blocking)
      this.logsPollingInFlight = true;
      this.refreshLogs(lines).finally(() => {
        this.logsPollingInFlight = false;
      });
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
