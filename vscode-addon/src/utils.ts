import * as crypto from "crypto";

/**
 * Generate a cryptographic nonce string for use in CSP headers.
 * Uses Node.js crypto.randomBytes for cryptographically secure nonces.
 */
export function getNonce(): string {
  return crypto.randomBytes(32).toString("base64url");
}

/**
 * Type guard that checks whether a value is a non-null object (Record).
 */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/**
 * Safely cast a value to Record<string, unknown>.
 * Returns an empty object if the value is not a non-null object.
 */
export function asRecord(value: unknown): Record<string, unknown> {
  return isRecord(value) ? value : {};
}

/**
 * Safely convert an unknown error to a string message.
 */
export function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Unified exponential backoff with ±30% jitter
 * (contracts/cross-client-sync.md):
 * `delay = min(1000 × 2^attempt, 30000) × (0.7 + random() × 0.3)`.
 *
 * Shared by the state-sync listener (`stateSync.ts`) and the reconnection
 * manager (`runtime/reconnect.ts`) so both stay on the same formula.
 *
 * @param attempt - zero-based retry attempt number
 * @returns delay in milliseconds
 */
export function backoffDelayMs(attempt: number): number {
  const capped = Math.min(1000 * Math.pow(2, attempt), 30_000);
  // 30% jitter: keep at least 70% of the base delay
  const jitter = 0.7 + Math.random() * 0.3;
  return Math.round(capped * jitter);
}
