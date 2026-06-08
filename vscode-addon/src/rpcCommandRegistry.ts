import * as vscode from "vscode";
import { registerAgentRpcCommands } from "./commands/agent";
import { registerWorkflowRpcCommands } from "./commands/workflow";
import { registerConfigRpcCommands } from "./commands/config";

// Re-export shared utilities for backward compatibility with existing imports.
export {
  asArray,
  asRecord,
  getErrorMessage,
  RpcCommandRegistryDeps,
} from "./commands/rpcShared";

/**
 * Register all RPC commands by delegating to domain-specific registries.
 *
 * Commands are split into three domains, each under 600 lines:
 * - agent: skills, learning, knowledge, RL alignment, provider status, self-model
 * - workflow: workflow/task execution, hardness/cost estimation, checkpoints, debugging
 * - config: baseline, build, governance, metrics, quality, security, breakers
 *
 * @returns Combined array of vscode.Disposable registrations from all domains.
 */
export function registerRpcCommands(
  deps: import("./commands/rpcShared").RpcCommandRegistryDeps,
): vscode.Disposable[] {
  return [
    ...registerAgentRpcCommands(deps),
    ...registerWorkflowRpcCommands(deps),
    ...registerConfigRpcCommands(deps),
  ];
}
