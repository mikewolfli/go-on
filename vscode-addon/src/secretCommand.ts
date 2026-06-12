import * as vscode from "vscode";
import { spawn } from "child_process";
import { ensureGoOnBinary } from "./runtimeBinaryService";

export type SecretAction = "set" | "get" | "delete" | "list";

export interface SecretCommandOptions {
  /** VS Code extension context, used to locate the runtime binary. */
  context: vscode.ExtensionContext;
  /** The secret command action to execute. */
  action: SecretAction;
  /** The secret name (required for "set", "get", "delete"; optional for "list"). */
  secretName?: string;
  /** The secret value to write via stdin (only used with "set"). */
  secretValue?: string;
  /** Optional logger for non-fatal warnings (e.g. forceKill failures). */
  warnLogger?: (message: string) => void;
}

/**
 * Run a go-on --secret command, piping secret values through stdin for security.
 *
 * The command supports four actions:
 * - "list": list all secret names (secretName is optional)
 * - "get": read a secret (secretName required)
 * - "set": write a secret (secretName and secretValue required)
 * - "delete": remove a secret (secretName required)
 *
 * Secrets are written to stdin (not passed as CLI arguments) so they do not
 * appear in /proc/PID/cmdline or ps output.
 *
 * The spawned process is given a 10-second timeout: SIGTERM first, then
 * SIGKILL after a 1-second grace period.
 */
export async function runSecretCommand(
  options: SecretCommandOptions,
): Promise<string> {
  const { context, action, secretName, secretValue, warnLogger } = options;

  if (action !== "list" && !secretName) {
    throw new Error(`secret name is required for action "${action}"`);
  }

  const config = vscode.workspace.getConfiguration("go-on");
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const runtime = await ensureGoOnBinary(workspaceRoot, config, context);

  const args: string[] = ["--secret", action];
  if (secretName) {
    args.push("--secret-name", secretName);
  }
  const hasSecretValue = secretValue !== undefined;

  return new Promise<string>((resolve, reject) => {
    const proc = spawn(runtime.executablePath, args, {
      cwd: workspaceRoot || runtime.runtimeDir,
      stdio: [hasSecretValue ? "pipe" : "ignore", "pipe", "pipe"],
    });

    // Pipe secret through stdin so it doesn't leak in process listings
    if (hasSecretValue && proc.stdin) {
      proc.stdin.write(secretValue!);
      proc.stdin.end();
    } else if (proc.stdin) {
      proc.stdin.end();
    }

    let stdout = "";
    let stderr = "";
    let timedOut = false;

    const timeoutHandle = setTimeout(() => {
      timedOut = true;
      proc.kill("SIGTERM");
      // Give it a moment to terminate, then force kill
      setTimeout(() => {
        try {
          if (!proc.killed) {
            proc.kill("SIGKILL");
          }
        } catch (err) {
          if (warnLogger) {
            warnLogger(`forceKill failed: ${err}`);
          }
        }
      }, 1000);
    }, 10000);

    proc.stdout?.on("data", (chunk: Buffer) => {
      stdout += chunk.toString();
    });

    proc.stderr?.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });

    proc.on("error", (err) => {
      clearTimeout(timeoutHandle);
      reject(err);
    });
    proc.on("close", (code) => {
      clearTimeout(timeoutHandle);
      if (timedOut) {
        const details = (stderr || stdout || "process timed out").trim();
        reject(new Error(`go-on secret command timed out: ${details}`));
        return;
      }
      if (code === 0) {
        resolve(stdout.trim());
        return;
      }
      const details = (stderr || stdout || `exit code ${code}`).trim();
      reject(new Error(`go-on secret command failed: ${details}`));
    });
  });
}
