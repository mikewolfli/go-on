import { ElMessage, ElMessageBox } from "element-plus";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface CrashHandlerOptions {
  onRecover: () => Promise<void>;
  t: (key: string) => string;
  crashCooldownMs?: number;
}

export function useCrashHandler(options: CrashHandlerOptions) {
  let unlistenCrash: UnlistenFn | undefined;
  let lastCrashKey = "";
  let lastCrashAt = 0;
  const cooldownMs = options.crashCooldownMs ?? 60000;

  async function register() {
    // Unregister any previous listener to prevent leaks on multiple calls
    unregister();
    unlistenCrash = await listen<{ message: string; timestamp: string }>(
      "service-crash",
      async (event) => {
        const payload = event.payload;
        const now = Date.now();
        const crashKey = payload.message;
        if (crashKey === lastCrashKey && now - lastCrashAt < cooldownMs) {
          return;
        }
        lastCrashKey = crashKey;
        lastCrashAt = now;

        try {
          await ElMessageBox.confirm(
            `${payload.message}\n${options.t("toast.recoverPrompt")}`,
            options.t("toast.serviceCrashed"),
            {
              confirmButtonText: options.t("toast.recoverNow"),
              cancelButtonText: options.t("toast.later"),
              type: "error",
            },
          );
          await options.onRecover();
        } catch {
          ElMessage.warning(options.t("toast.recoverDeferred"));
        }
      },
    );
  }

  function unregister() {
    if (unlistenCrash) {
      unlistenCrash();
      unlistenCrash = undefined;
    }
  }

  return {
    register,
    unregister,
  };
}
