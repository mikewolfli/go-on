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

export const MAX_BACKEND_CONFIGURE_ATTEMPTS = 5;

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
  const localAuto = await autoConfigureBackendPath();
  if (localAuto.linked) {
    await startBackendWithChecks();
    ElMessage.success(
      "Automatically detected and linked backend in the local directory.",
    );
    return;
  }

  for (let attempt = 1; attempt <= MAX_BACKEND_CONFIGURE_ATTEMPTS; attempt++) {
    const exists = await backendExecutableExists();
    if (exists) {
      await startBackendWithChecks();
      return;
    }

    await ElMessageBox.alert(
      `Backend executable "go-on" not found. Please select the directory containing go-on (it will look for root/bin/exec/backend automatically).\nAttempt ${attempt}/${MAX_BACKEND_CONFIGURE_ATTEMPTS}`,
      "Configure Backend Path",
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
        `No directory selected (${attempt}/${MAX_BACKEND_CONFIGURE_ATTEMPTS}), please retry.`,
      );
      continue;
    }

    const inputPath = Array.isArray(picked) ? picked[0] : picked;
    if (!inputPath || !String(inputPath).trim()) {
      ElMessage.warning("Path cannot be empty, please specify again.");
      continue;
    }

    try {
      await configureServiceByDirectory(String(inputPath));
    } catch (error) {
      ElMessage.error(
        `Failed to resolve backend from directory: ${normalizeErrorMessage(error)}`,
      );
      continue;
    }

    const configuredExists = await backendExecutableExists();
    if (!configuredExists) {
      ElMessage.error(
        "Specified path is invalid or file does not exist, please specify again.",
      );
      continue;
    }

    await startBackendWithChecks();
    ElMessage.success("Backend started.");
    return;
  }

  throw new Error(
    "Maximum retry attempts reached. Please manually set the backend path in the config page.",
  );
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
        ElMessage.success(
          "Backend detected as already running, automatically linked and saved to config.",
        );
        return;
      }
      ElMessage.warning(
        `Backend detected as running, but auto-link failed: ${result.reason}`,
      );
    }
  } catch {
    // Ignore health probe failures and continue to manual path flow.
  }

  if (monitorOnly) {
    ElMessage.warning(
      "Currently in monitor-only mode: will not auto-start backend. Please start go-on manually.",
    );
    return;
  }

  await ensureBackendAndStart();
}
