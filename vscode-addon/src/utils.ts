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
export function isRecord(value: unknown): value is Record<string, unknown> {
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
