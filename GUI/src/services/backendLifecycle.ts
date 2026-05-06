import { ElMessage, ElMessageBox } from "element-plus";
import {
  autoConfigureBackendPath,
  backendExecutableExists,
  checkHealth,
  checkProvidersConfigured,
  configureServiceByDirectory,
  serviceStatus,
  startService,
} from "./bridge";
import { openDialog } from "./dialog";
import { normalizeErrorMessage } from "../utils/errors";
import { i18n } from "../locales";

export const MAX_BACKEND_CONFIGURE_ATTEMPTS = 5;

/** Guard to prevent rapid double-clicks from spawning multiple backend processes. */
let starting = false;

export function classifyStartupError(error: unknown): string {
  const message = normalizeErrorMessage(error);
  const raw = message.toLowerCase();
  if (raw.includes("startup_error:file_missing")) {
    return i18n.global.t("backendStartup.fileMissing");
  }
  if (raw.includes("startup_error:not_a_file")) {
    return i18n.global.t("backendStartup.notAFile");
  }
  if (raw.includes("startup_error:permission_denied")) {
    return i18n.global.t("backendStartup.permissionDenied");
  }
  if (raw.includes("startup_error:exited_early")) {
    return i18n.global.t("backendStartup.exitedEarly");
  }
  if (raw.includes("startup_error:spawn_failed")) {
    return i18n.global.t("backendStartup.spawnFailed");
  }
  return `${i18n.global.t("backendStartup.startupPrefix")}: ${message}`;
}

export async function waitForBackendHealthy(
  timeoutMs = 12000,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const status = await serviceStatus();
      if (status.running) {
        const health = await checkHealth(undefined, { bypassCache: true });
        if (health.ok) {
          return true;
        }
      }
    } catch {
      // Continue polling until timeout.
    }
    await new Promise((resolve) => window.setTimeout(resolve, 800));
  }
  return false;
}

/**
 * Start the backend service and wait for it to become healthy.
 */
export async function startBackendWithChecks() {
  try {
    await startService();
  } catch (error) {
    throw new Error(classifyStartupError(error));
  }

  const healthy = await waitForBackendHealthy();
  if (!healthy) {
    throw new Error(i18n.global.t("backendStartup.startupTimeout"));
  }
}

/**
 * Ensure the backend executable path is configured.
 * Returns true if a valid backend was found and linked.
 * Does NOT start the backend process.
 */
export async function ensureBackendConfigured(): Promise<boolean> {
  if (starting) {
    return false;
  }
  starting = true;
  try {
    // Step 1: Try auto-discover
    const localAuto = await autoConfigureBackendPath();
    if (localAuto.linked) {
      return true;
    }

    // Step 2: Retry loop with user directory picker
    for (
      let attempt = 1;
      attempt <= MAX_BACKEND_CONFIGURE_ATTEMPTS;
      attempt++
    ) {
      const exists = await backendExecutableExists();
      if (exists) {
        return true;
      }

      await ElMessageBox.alert(
        i18n.global.t("backend.executableNotFound", {
          attempt: String(attempt),
          max: String(MAX_BACKEND_CONFIGURE_ATTEMPTS),
        }),
        i18n.global.t("backend.configureBackendPath"),
        {
          confirmButtonText: "Select Directory",
          closeOnClickModal: false,
          closeOnPressEscape: false,
        },
      );

      const picked = await openDialog({
        multiple: false,
        directory: true,
        title: "Select directory containing go-on",
      });

      if (!picked) {
        ElMessage.warning(
          i18n.global.t("backend.noDirectorySelected", {
            attempt: String(attempt),
            max: String(MAX_BACKEND_CONFIGURE_ATTEMPTS),
          }),
        );
        continue;
      }

      const inputPath = Array.isArray(picked) ? picked[0] : picked;
      if (!inputPath || !String(inputPath).trim()) {
        ElMessage.warning(i18n.global.t("backend.pathCannotBeEmpty"));
        continue;
      }

      try {
        await configureServiceByDirectory(String(inputPath));
      } catch (error) {
        ElMessage.error(
          i18n.global.t("backend.failedToResolve", {
            error: normalizeErrorMessage(error),
          }),
        );
        continue;
      }

      const configuredExists = await backendExecutableExists();
      if (!configuredExists) {
        ElMessage.error(i18n.global.t("backend.pathInvalid"));
        continue;
      }

      // Found and configured successfully
      return true;
    }

    ElMessage.error(i18n.global.t("backendStartup.maxRetriesReached"));
    return false;
  } finally {
    starting = false;
  }
}

/**
 * Check whether AI providers are already configured in the backend config.
 * Runs as a Tauri command in the GUI process — no backend needed.
 */
export async function providersConfigured(): Promise<boolean> {
  try {
    return await checkProvidersConfigured();
  } catch {
    return false;
  }
}

/**
 * Bootstrap: find the backend executable and configure its path.
 * Separated from starting the backend — the caller decides when to start.
 *
 * Returns:
 *   "configured"   → backend path found and saved, caller can start it
 *   "no-backend"   → could not find or configure backend path
 *   "no-providers" → backend path found, but no AI providers configured yet
 *   "monitor-only" → monitor-only mode, user chose not to start
 */
export type BootstrapResult =
  | { status: "configured" }
  | { status: "no-backend"; reason: string }
  | { status: "no-providers" }
  | { status: "monitor-only" };

export async function bootstrapBackend(
  monitorOnly: boolean,
): Promise<BootstrapResult> {
  // Step 1: Check if backend path is already configured
  const hasConfiguredPath = await backendExecutableExists();

  if (!hasConfiguredPath) {
    if (monitorOnly) {
      ElMessage.warning(i18n.global.t("backend.monitorOnlyMode"));
      return { status: "monitor-only" };
    }

    // Step 2: Auto-discover backend executable
    const configured = await ensureBackendConfigured();
    if (!configured) {
      return {
        status: "no-backend",
        reason: "Failed to locate or configure backend executable",
      };
    }
  }

  // Step 4: Backend path is configured — check if providers exist
  const hasProviders = await providersConfigured();
  if (!hasProviders) {
    return { status: "no-providers" };
  }

  // Step 5: Everything ready — providers configured, backend path set
  return { status: "configured" };
}
