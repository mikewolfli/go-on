//! go-on Node.js SDK — Public API exports.
//!
//! An async JSON-RPC client for the go-on ACP agent orchestration runtime.
//! Supports streaming chat, governance, observability, workflow,
//! learning, reliability, and operations APIs.

export { GoOnClient } from "./client";
export type { GoOnClientOptions } from "./client";
export {
  GoOnClientError,
  GoOnJsonRpcError,
  GoOnHttpError,
} from "./errors";
export type {
  BreakerStatusResponse,
  ChatMessage,
  ChatRequest,
  CheckpointListResponse,
  ConfigBaselineResponse,
  CostStatusResponse,
  GovernanceStatusResponse,
  HarnessStatusResponse,
  HealthProbesResponse,
  HealthResponse,
  LearningSummaryResponse,
  MetricsResponse,
  SelectorStatusResponse,
  TaskPlanResponse,
} from "./types";
