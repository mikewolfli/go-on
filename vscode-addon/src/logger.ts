import * as vscode from "vscode";

/**
 * A shared output channel for Go-On extension logging.
 * Created lazily on first use.
 */
let _outputChannel: vscode.OutputChannel | undefined;

function getOutputChannel(): vscode.OutputChannel {
  if (!_outputChannel) {
    _outputChannel = vscode.window.createOutputChannel("Go-On Logger");
  }
  return _outputChannel;
}

/**
 * Logger utility for the Go-On VSCode extension.
 *
 * All log output goes to:
 * 1. The "Go-On Logger" output channel (visible in VSCode's Output panel)
 * 2. The dev console (console.log/console.warn/console.error)
 *
 * Use `Logger.forModule(moduleName)` to get a scoped logger instance.
 */
export class Logger {
  // eslint-disable-next-line no-unused-vars
  private constructor(private readonly moduleName: string) {}

  /**
   * Create a logger scoped to a specific module.
   *
   * @example
   * const log = Logger.forModule("i18n");
   * log.warn("Failed to load locale", err);
   */
  static forModule(name: string): Logger {
    return new Logger(name);
  }

  info(message: string, ...args: unknown[]): void {
    const formatted = this.format("INFO", message, args);
    getOutputChannel().appendLine(formatted);
    // eslint-disable-next-line no-console
    console.info(formatted);
  }

  warn(message: string, ...args: unknown[]): void {
    const formatted = this.format("WARN", message, args);
    getOutputChannel().appendLine(formatted);
    // eslint-disable-next-line no-console
    console.warn(formatted);
  }

  error(message: string, ...args: unknown[]): void {
    const formatted = this.format("ERROR", message, args);
    getOutputChannel().appendLine(formatted);
    // eslint-disable-next-line no-console
    console.error(formatted);
  }

  private format(_level: string, message: string, args: unknown[]): string {
    const prefix = `[${this.moduleName}]`;
    if (args.length === 0) {
      return `${prefix} ${message}`;
    }
    const details = args
      .map((a) => (a instanceof Error ? a.message : String(a)))
      .join(", ");
    return `${prefix} ${message} — ${details}`;
  }
}

/**
 * Dispose the shared output channel (call on extension deactivation).
 */
export function disposeLogger(): void {
  if (_outputChannel) {
    _outputChannel.dispose();
    _outputChannel = undefined;
  }
}
