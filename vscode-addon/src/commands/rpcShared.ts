import * as vscode from "vscode";
import { Logger } from "../logger";
import { i18n, MessageKeys } from "../i18n";

const log = Logger.forModule("rpcCommandRegistry");
import { asRecord as _asRecord, getErrorMessage } from "../utils";

export interface RpcCommandRegistryDeps {
  isRunning: () => boolean;
  sendRequest: (_method: string, _params?: unknown) => Promise<unknown>;
}

export { getErrorMessage };

/** Re-exported from utils.ts for convenience. */
export function asRecord(value: unknown): Record<string, unknown> {
  return _asRecord(value);
}

export function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

/**
 * Safely serialize a value to JSON, falling back to readable representation
 * if the value contains circular references or other serialization issues.
 */
export function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch (err) {
    log.warn("safeStringify failed:", err);
    try {
      // Use a replacer that tracks visited objects
      const seen = new WeakSet<object>();
      return JSON.stringify(value, (_key: string, val: unknown) => {
        if (typeof val === "object" && val !== null) {
          if (seen.has(val)) {
            return "[Circular]";
          }
          seen.add(val);
        }
        return val;
      });
    } catch {
      log.error("Failed to safe-stringify value", value);
      return String(value);
    }
  }
}

export async function ensureRunning(
  deps: RpcCommandRegistryDeps,
): Promise<boolean> {
  if (!deps.isRunning()) {
    await vscode.window.showErrorMessage(
      i18n.getMessage(MessageKeys.goOnNotRunningRpc),
    );
    return false;
  }
  return true;
}
