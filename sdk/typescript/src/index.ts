//! go-on TypeScript SDK — Public API exports.
//!
//! An async JSON-RPC client for the go-on ACP agent orchestration runtime.
//! Supports streaming chat, governance, observability, workflow,
//! learning, reliability, and operations APIs.

export { GoOnClient } from "./client";
export type { GoOnClientOptions } from "./client";
export type {
  AcpSessionCloseRequest,
  AcpSessionInfo,
  AcpSessionListResponse,
  AcpSessionNewRequest,
  AcpSessionNewResponse,
  AcpSessionPromptRequest,
  AcpSessionResumeRequest,
  AgentInfo,
  ApiResponse,
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
  MultimodalInput,
  PromptContentBlock,
  SelectorStatusResponse,
  SessionConfigGroup,
  SessionConfigKind,
  SessionConfigOption,
  SessionConfigSelectOptions,
  SessionModeDescription,
  SessionModeState,
  StreamChunk,
  TaskPlanResponse,
  ToolCall,
  Usage,
} from "./types";
