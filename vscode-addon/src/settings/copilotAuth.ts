import * as http from "http";
import * as https from "https";

// ── Copilot OAuth types ──

export interface PersistedCopilotState {
  authMode?: string;
  accountLabel?: string;
  oauthClientId?: string;
  lastError?: string;
  lastStatus?: string;
}

export interface CopilotAuthState {
  isAuthorized: boolean;
  authMode: string;
  accountLabel: string;
  oauthClientId: string;
  pending: boolean;
  statusMessage: string;
  lastError: string;
  userCode?: string;
  verificationUri?: string;
  expiresAt?: number;
  modelSource?: string;
  modelCount?: number;
}

export interface ProviderModelResolution {
  modelOptions: string[];
  copilotAuth?: CopilotAuthState;
}

export interface CopilotTokenExchange {
  token: string;
  expiresAt: number;
}

export interface CopilotModelCache {
  models: string[];
  fetchedAt: number;
}

export interface PendingCopilotDeviceAuth {
  cancelRequested: boolean;
  userCode: string;
  verificationUri: string;
  expiresAt: number;
}

export interface DeviceCodeResponse {
  device_code?: string;
  user_code?: string;
  verification_uri?: string;
  expires_in?: number;
  interval?: number;
}

export interface HttpJsonResponse {
  status: number;
  bodyText: string;
  body: unknown;
}

// ── Constants ──

export const COPILOT_ENV_VAR = "GITHUB_COPILOT_TOKEN";
export const COPILOT_SECRET_NAME = "github_copilot_token";
export const COPILOT_TOKEN_URL =
  "https://api.github.com/copilot_internal/v2/token";
export const COPILOT_MODELS_URL = "https://api.githubcopilot.com/models";
export const GITHUB_DEVICE_CODE_URL = "https://github.com/login/device/code";
export const GITHUB_ACCESS_TOKEN_URL =
  "https://github.com/login/oauth/access_token";
export const COPILOT_MODEL_CACHE_KEY = "go-on.copilot.modelsCache.v1";
export const COPILOT_STATE_KEY = "go-on.copilot.authState.v1";

// ── Utility functions ──

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function createTransport(urlValue: URL): typeof http | typeof https {
  return urlValue.protocol === "http:" ? http : https;
}

/**
 * Make an HTTP(S) JSON request and return the parsed response.
 */
export async function requestJson(
  urlString: string,
  options: {
    method?: string;
    headers?: Record<string, string>;
    body?: string;
  } = {},
): Promise<HttpJsonResponse> {
  const target = new URL(urlString);
  const body = options.body ?? "";
  const headers: Record<string, string> = {
    Accept: "application/json",
    ...(options.headers || {}),
  };

  if (body && headers["Content-Length"] === undefined) {
    headers["Content-Length"] = Buffer.byteLength(body).toString();
  }

  return new Promise<HttpJsonResponse>((resolve, reject) => {
    const req = createTransport(target).request(
      target,
      {
        method: options.method || "GET",
        headers,
      },
      (res) => {
        let chunks = "";
        res.setEncoding("utf8");
        res.on("data", (chunk: string) => {
          chunks += chunk;
        });
        res.on("end", () => {
          let parsed: unknown = undefined;
          if (chunks.trim()) {
            try {
              parsed = JSON.parse(chunks);
            } catch (err) {
              console.warn("[copilotAuth] requestJson parse failed:", err);
              parsed = undefined;
            }
          }
          resolve({
            status: res.statusCode || 0,
            bodyText: chunks,
            body: parsed,
          });
        });
      },
    );

    req.on("error", reject);
    if (body) {
      req.write(body);
    }
    req.end();
  });
}

export function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
