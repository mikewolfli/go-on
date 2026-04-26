"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.GoOnStatusProvider = exports.GoOnManager = void 0;
const child_process_1 = require("child_process");
const vscode = require("vscode");
const protocolContract_1 = require("./protocolContract");
function asRecord(value) {
    return typeof value === "object" && value !== null
        ? value
        : {};
}
class GoOnManager {
    /** Connect a VS Code OutputChannel so Go-On process output is visible to users. */
    setOutputChannel(channel) {
        this._outputChannel = channel;
    }
    classifyRpcErrorKind(message, data) {
        const details = asRecord(data);
        const explicit = typeof details.kind === "string" ? details.kind : undefined;
        if (explicit && explicit.trim().length > 0) {
            return explicit;
        }
        const lower = String(message || "").toLowerCase();
        if (lower.includes("pua"))
            return "PuaViolation";
        if (lower.includes("budget"))
            return "BudgetExceeded";
        if (lower.includes("hardening policy denied") ||
            lower.includes("sandbox")) {
            return "SandboxBlocked";
        }
        return "GeneralError";
    }
    formatRpcError(error) {
        const kind = this.classifyRpcErrorKind(error.message, error.data);
        const errorData = asRecord(error.data);
        const detail = typeof errorData.detail === "string" ? errorData.detail : "";
        const context = detail.includes(protocolContract_1.protocolContract.errors.requestErrorContextPrefix)
            ? protocolContract_1.protocolContract.errors.requestErrorContextPrefix
            : "none";
        return `rpc_error:${error.code}:${kind}:${error.message} (context=${context})`;
    }
    constructor() {
        this.process = null;
        this.requestId = 0;
        this.pendingRequests = new Map();
        this.statusItems = [];
        this.runtimeEnvOverrides = {};
        this.lastWizardPromptAt = 0;
        this.updateStatus();
    }
    async start(configPath, executablePath, cwd, protocolMode) {
        if (this.process) {
            throw new Error("Go-On is already running");
        }
        return new Promise((resolve, reject) => {
            let resolved = false;
            let stderrBuffer = "";
            const args = ["--config", configPath, "--verbose"];
            const normalizedProtocolMode = (0, protocolContract_1.normalizeProtocolMode)(protocolMode || "from_config");
            if (!(0, protocolContract_1.isAllowedProtocolMode)(normalizedProtocolMode)) {
                reject(new Error(`Invalid protocol mode '${protocolMode}'. Allowed values: from_config, ${protocolContract_1.protocolContract.protocol.supportedModes.join(", ")}`));
                return;
            }
            if (normalizedProtocolMode !== "from_config") {
                args.push("--protocol-mode", normalizedProtocolMode);
            }
            this.process = (0, child_process_1.spawn)(executablePath, args, {
                cwd,
                env: {
                    ...process.env,
                    ...this.runtimeEnvOverrides,
                },
                stdio: ["pipe", "pipe", "pipe"],
            });
            let startupTimeout = setTimeout(() => {
                this.process?.kill();
                reject(new Error("Go-On startup timeout"));
            }, 10000);
            this.process.stdout?.on("data", (data) => {
                const output = data.toString();
                this._outputChannel?.appendLine(output.trimEnd());
                try {
                    const lines = output.trim().split("\n");
                    for (const line of lines) {
                        if (!line.trim()) {
                            continue;
                        }
                        const response = JSON.parse(line);
                        const pending = this.pendingRequests.get(response.id);
                        if (pending) {
                            this.pendingRequests.delete(response.id);
                            if (response.error) {
                                pending.reject(new Error(this.formatRpcError(response.error)));
                            }
                            else {
                                pending.resolve(response.result);
                            }
                        }
                    }
                }
                catch {
                    // Not a JSON-RPC response, ignore.
                }
                if (startupTimeout) {
                    clearTimeout(startupTimeout);
                    startupTimeout = undefined;
                    resolved = true;
                    resolve();
                }
            });
            this.process.stderr?.on("data", (data) => {
                const text = data.toString();
                stderrBuffer += text;
                if (stderrBuffer.length > 4000) {
                    stderrBuffer = stderrBuffer.slice(-4000);
                }
                this._outputChannel?.appendLine("[stderr] " + text.trimEnd());
            });
            this.process.on("close", (code) => {
                this._outputChannel?.appendLine(`[exit] code ${code}`);
                const failedBeforeStartup = !resolved;
                this.process = null;
                this.updateStatus();
                if (startupTimeout) {
                    clearTimeout(startupTimeout);
                    startupTimeout = undefined;
                }
                if (failedBeforeStartup) {
                    const details = stderrBuffer.trim();
                    reject(new Error(`Go-On exited before startup (code ${code}). ${details || "No stderr output."}`));
                }
            });
            this.process.on("error", (error) => {
                this._outputChannel?.appendLine(`[error] ${error}`);
                this.process = null;
                reject(error);
            });
        });
    }
    stop() {
        if (this.process) {
            this.process.kill();
            this.process = null;
        }
        this.updateStatus();
    }
    isRunning() {
        return this.process !== null;
    }
    setRuntimeEnvOverrides(overrides) {
        this.runtimeEnvOverrides = {
            ...this.runtimeEnvOverrides,
            ...overrides,
        };
    }
    async sendRequest(method, params, options) {
        if (!this.process) {
            throw new Error("Go-On is not running");
        }
        if (!options?.skipProviderGuard && this.requiresAiProvider(method)) {
            const ready = await this.isAnyAiProviderReady();
            if (!ready) {
                await this.notifyAndOpenSetupWizard();
                throw new Error(`${protocolContract_1.protocolContract.errors.providerNotReady} ${protocolContract_1.protocolContract.errors.setupWizardOpened}`);
            }
        }
        const id = ++this.requestId;
        const request = {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        return new Promise((resolve, reject) => {
            this.pendingRequests.set(id, { resolve, reject });
            const requestStr = JSON.stringify(request) + "\n";
            if (!this.process || !this.process.stdin) {
                reject(new Error("Go-On process not available or stdin not connected"));
                this.pendingRequests.delete(id);
                return;
            }
            this.process.stdin.write(requestStr);
            setTimeout(() => {
                if (this.pendingRequests.has(id)) {
                    this.pendingRequests.delete(id);
                    reject(new Error("Request timeout"));
                }
            }, 30000);
        });
    }
    requiresAiProvider(method) {
        return new Set([
            "chat",
            "workflow.execute",
            "task.plan",
            "task.execute",
            "learning.summary",
            "primary_secondary.summary",
        ]).has(method);
    }
    async isAnyAiProviderReady() {
        const now = Date.now();
        if (this.providerReadyCache &&
            now - this.providerReadyCache.checkedAt < 5000) {
            return this.providerReadyCache.ready;
        }
        try {
            const report = asRecord(await this.sendRequest("runtime.health", undefined, {
                skipProviderGuard: true,
            }));
            const componentsValue = Array.isArray(report.components)
                ? report.components
                : Array.isArray(asRecord(report.report).components)
                    ? asRecord(report.report).components
                    : [];
            const providerComponent = componentsValue
                .map((component) => asRecord(component))
                .find((component) => component.name === "provider_dependencies");
            if (!providerComponent) {
                this.providerReadyCache = { checkedAt: now, ready: false };
                return false;
            }
            const details = asRecord(providerComponent.details);
            const ready = Number(details.ready ?? 0);
            const total = Number(details.total ?? 0);
            const isReady = total > 0 && ready > 0;
            this.providerReadyCache = { checkedAt: now, ready: isReady };
            return isReady;
        }
        catch {
            this.providerReadyCache = { checkedAt: now, ready: false };
            return false;
        }
    }
    async notifyAndOpenSetupWizard() {
        const now = Date.now();
        if (now - this.lastWizardPromptAt < 5000) {
            return;
        }
        this.lastWizardPromptAt = now;
        await vscode.window.showWarningMessage(protocolContract_1.protocolContract.errors.setupWizardPrompt);
        await vscode.commands.executeCommand("go-on.openSettings");
    }
    updateStatus() {
        this.statusItems = [
            new vscode.TreeItem(`Status: ${this.isRunning() ? "Running" : "Stopped"}`, vscode.TreeItemCollapsibleState.None),
        ];
        vscode.commands.executeCommand("go-on-status.refresh");
        vscode.commands.executeCommand("go-on.refreshStatusMonitor");
    }
    getStatusItems() {
        return this.statusItems;
    }
}
exports.GoOnManager = GoOnManager;
class GoOnStatusProvider {
    constructor(_manager) {
        this._onDidChangeTreeData = new vscode.EventEmitter();
        this.onDidChangeTreeData = this._onDidChangeTreeData.event;
        this.manager = _manager;
    }
    refresh() {
        this._onDidChangeTreeData.fire();
    }
    getTreeItem(element) {
        return element;
    }
    getChildren(element) {
        if (!element) {
            return Promise.resolve(this.manager.getStatusItems());
        }
        return Promise.resolve([]);
    }
}
exports.GoOnStatusProvider = GoOnStatusProvider;
//# sourceMappingURL=runtimeManager.js.map