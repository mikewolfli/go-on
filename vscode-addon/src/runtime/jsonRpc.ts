import { asRecord } from "../utils";
export { asRecord };

/**
 * JSON-RPC 2.0 request message.
 */
export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params?: unknown;
}

/**
 * JSON-RPC 2.0 response message.
 */
export interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: number;
  result?: unknown;
  error?: {
    code: number;
    message: string;
    data?: unknown;
  };
}

/**
 * A pending request awaiting a response from the runtime.
 */
export interface PendingRequest {
  resolve: (_value: unknown) => void;
  reject: (_reason?: unknown) => void;
}

/**
 * Extract the keyring account name from a provider's env-var / keyring URI.
 *
 * Handles two formats:
 * - `keyring://go-on/{account_name}` — extract account_name directly
 * - `OPENAI_API_KEY` — old-style env var name, convert to lowercase
 *
 * Returns the account name suitable for use with the system keyring
 * (e.g. `copilot_api_key`, `openai_api_key`).
 */
export function secretNameForEnvVar(envVar: string): string {
  const normalized = String(envVar || "").trim();
  if (!normalized) {
    return "";
  }
  // Handle keyring://go-on/{name} URIs
  const keyringPrefix = "keyring://go-on/";
  if (normalized.startsWith(keyringPrefix)) {
    return normalized.slice(keyringPrefix.length);
  }
  // Legacy env-var name: GITHUB_COPILOT_TOKEN → github_copilot_token
  if (normalized === "GITHUB_COPILOT_TOKEN") {
    return "github_copilot_token";
  }
  return normalized.toLowerCase();
}
