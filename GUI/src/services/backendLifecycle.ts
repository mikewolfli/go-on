import { ElMessage, ElMessageBox } from "element-plus";
import {
  autoConfigureBackendPath,
  backendExecutableExists,
  checkHealth,
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
    return "Startup failed: backend executable not found, please re-select the path.";
  }
  if (raw.includes("startup_error:not_a_file")) {
    return "Startup failed: configured path is not an executable file.";
  }
  if (raw.includes("startup_error:permission_denied")) {
    return "Startup failed: permission denied. Please check file permissions or run as administrator.";
  }
  if (raw.includes("startup_error:exited_early")) {
    return "Startup failed: backend process exited immediately. Please check logs and port usage.";
  }
  if (raw.includes("startup_error:spawn_failed")) {
    return "Startup failed: unable to spawn backend process. Please check dependencies and runtime environment.";
  }
  return `Startup failed: ${message}`;
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

export async function startBackendWithChecks() {
  try {
    await startService();
  } catch (error) {
    throw new Error(classifyStartupError(error));
  }

  const healthy = await waitForBackendHealthy();
  if (!healthy) {
    throw new Error(
      "Startup timeout: backend process did not become ready within 12 seconds. Please check port, config, or dependencies.",
    );
  }
}

export async function ensureBackendAndStart() {
  if (starting) {
    return;
  }
  starting = true;
  try {
    const localAuto = await autoConfigureBackendPath();
    if (localAuto.linked) {
      await startBackendWithChecks();
      ElMessage.success(i18n.global.t("backend.autoDetectedAndLinked"));
      return;
    }

    for (
      let attempt = 1;
      attempt <= MAX_BACKEND_CONFIGURE_ATTEMPTS;
      attempt++
    ) {
      const exists = await backendExecutableExists();
      if (exists) {
        await startBackendWithChecks();
        return;
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

      await startBackendWithChecks();
      ElMessage.success(i18n.global.t("backend.started"));
      return;
    }

    throw new Error(
      "Maximum retry attempts reached. Please manually set the backend path in the config page.",
    );
  } finally {
    starting = false;
  }
}

export async function bootstrapBackend(monitorOnly: boolean) {
  const hasConfiguredPath = await backendExecutableExists();
  if (hasConfiguredPath) {
    return;
  }

  try {
    const health = await checkHealth();
    if (health.ok) {
      const result = await autoConfigureBackendPath();
      if (result.linked) {
        ElMessage.success(i18n.global.t("backend.detectedRunning"));
        return;
      }
      ElMessage.warning(
        i18n.global.t("backend.detectedRunningAutoLinkFailed", {
          reason: result.reason,
        }),
      );
    }
  } catch {
    // Ignore health probe failures and continue to manual path flow.
  }

  if (monitorOnly) {
    ElMessage.warning(i18n.global.t("backend.monitorOnlyMode"));
    return;
  }

  await ensureBackendAndStart();
}
