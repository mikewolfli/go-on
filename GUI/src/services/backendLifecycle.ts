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
    return "启动失败：未找到后台可执行文件，请重新选择路径。";
  }
  if (raw.includes("startup_error:not_a_file")) {
    return "启动失败：配置路径不是可执行文件。";
  }
  if (raw.includes("startup_error:permission_denied")) {
    return "启动失败：没有执行权限，请检查文件权限或以管理员身份运行。";
  }
  if (raw.includes("startup_error:exited_early")) {
    return "启动失败：后台进程启动后立即退出，请检查日志和端口占用。";
  }
  if (raw.includes("startup_error:spawn_failed")) {
    return "启动失败：无法拉起后台进程，请检查依赖与运行环境。";
  }
  return `启动失败：${message}`;
}

export async function waitForBackendHealthy(timeoutMs = 12000): Promise<boolean> {
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
    throw new Error("启动超时：后台进程未在 12 秒内就绪，请检查端口、配置或依赖。");
  }
}

export async function ensureBackendAndStart() {
  const localAuto = await autoConfigureBackendPath();
  if (localAuto.linked) {
    await startBackendWithChecks();
    ElMessage.success("已在本地目录自动探测并关联后台。");
    return;
  }

  for (let attempt = 1; attempt <= MAX_BACKEND_CONFIGURE_ATTEMPTS; attempt++) {
    const exists = await backendExecutableExists();
    if (exists) {
      await startBackendWithChecks();
      return;
    }

    await ElMessageBox.alert(
      `未找到后台程序 go-on，请选择包含 go-on 的目录（将自动查找 root/bin/exec/backend）。\n尝试 ${attempt}/${MAX_BACKEND_CONFIGURE_ATTEMPTS}`,
      "配置后台路径",
      {
        confirmButtonText: "选择目录",
        closeOnClickModal: false,
        closeOnPressEscape: false,
      },
    );

    const picked = await openDialog({
      multiple: false,
      directory: true,
      title: "选择包含 go-on 的目录",
    });

    if (!picked) {
      ElMessage.warning(`未选择目录（${attempt}/${MAX_BACKEND_CONFIGURE_ATTEMPTS}），请重试。`);
      continue;
    }

    const inputPath = Array.isArray(picked) ? picked[0] : picked;
    if (!inputPath || !String(inputPath).trim()) {
      ElMessage.warning("路径不能为空，请重新指定。");
      continue;
    }

    try {
      await configureServiceByDirectory(String(inputPath));
    } catch (error) {
      ElMessage.error(`目录解析后台失败：${normalizeErrorMessage(error)}`);
      continue;
    }

    const configuredExists = await backendExecutableExists();
    if (!configuredExists) {
      ElMessage.error("指定路径无效或文件不存在，请重新指定。");
      continue;
    }

    await startBackendWithChecks();
    ElMessage.success("后台已启动。");
    return;
  }

  throw new Error("已达到最大重试次数，请在配置页手动设置 backend 路径后重试。");
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
        ElMessage.success("检测到后台已运行，已自动关联并写入配置。");
        return;
      }
      ElMessage.warning(`检测到后台在运行，但自动关联失败：${result.reason}`);
    }
  } catch {
    // Ignore health probe failures and continue to manual path flow.
  }

  if (monitorOnly) {
    ElMessage.warning("当前为仅监控模式：不会自动启动后台，请先手动启动 go-on。");
    return;
  }

  await ensureBackendAndStart();
}
