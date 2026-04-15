import { ChildProcess, spawn } from 'child_process';
import * as vscode from 'vscode';
import { protocolContract } from './protocolContract';

interface JsonRpcRequest {
    jsonrpc: '2.0';
    id: number;
    method: string;
    params?: unknown;
}

interface JsonRpcResponse {
    jsonrpc: '2.0';
    id: number;
    result?: unknown;
    error?: {
        code: number;
        message: string;
        data?: unknown;
    };
}

interface PendingRequest {
    resolve: (_value: unknown) => void;
    reject: (_reason?: unknown) => void;
}

function asRecord(value: unknown): Record<string, unknown> {
    return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
}

export class GoOnManager {
    private process: ChildProcess | null = null;
    private requestId = 0;
    private pendingRequests = new Map<number, PendingRequest>();
    private statusItems: vscode.TreeItem[] = [];
    private runtimeEnvOverrides: Record<string, string> = {};
    private providerReadyCache?: { checkedAt: number; ready: boolean };
    private lastWizardPromptAt = 0;

    private classifyRpcErrorKind(message: string, data?: unknown): string {
        const details = asRecord(data);
        const explicit = typeof details.kind === 'string' ? details.kind : undefined;
        if (explicit && explicit.trim().length > 0) {
            return explicit;
        }

        const lower = String(message || '').toLowerCase();
        if (lower.includes('pua')) return 'PuaViolation';
        if (lower.includes('budget')) return 'BudgetExceeded';
        if (lower.includes('hardening policy denied') || lower.includes('sandbox')) {
            return 'SandboxBlocked';
        }
        return 'GeneralError';
    }

    private formatRpcError(error: { code: number; message: string; data?: unknown }): string {
        const kind = this.classifyRpcErrorKind(error.message, error.data);
        const errorData = asRecord(error.data);
        const detail = typeof errorData.detail === 'string' ? errorData.detail : '';
        const context = detail.includes(protocolContract.errors.requestErrorContextPrefix)
            ? protocolContract.errors.requestErrorContextPrefix
            : 'none';
        return `rpc_error:${error.code}:${kind}:${error.message} (context=${context})`;
    }

    constructor() {
        this.updateStatus();
    }

    async start(
        configPath: string,
        executablePath: string,
        cwd: string,
        protocolMode: string
    ): Promise<void> {
        if (this.process) {
            throw new Error('Go-On is already running');
        }

        return new Promise((resolve, reject) => {
            let resolved = false;
            let stderrBuffer = '';

            const args = ['--config', configPath, '--verbose'];
            if (protocolMode && protocolMode !== 'from_config') {
                args.push('--protocol-mode', protocolMode);
            }

            this.process = spawn(executablePath, args, {
                cwd,
                env: {
                    ...process.env,
                    ...this.runtimeEnvOverrides
                },
                stdio: ['pipe', 'pipe', 'pipe']
            });

            let startupTimeout: NodeJS.Timeout | undefined = setTimeout(() => {
                this.process?.kill();
                reject(new Error('Go-On startup timeout'));
            }, 10000);

            this.process.stdout?.on('data', (data: Buffer) => {
                const output = data.toString();
                console.log(`Go-On stdout: ${output}`);

                try {
                    const lines = output.trim().split('\n');
                    for (const line of lines) {
                        if (!line.trim()) {
                            continue;
                        }
                        const response: JsonRpcResponse = JSON.parse(line);
                        const pending = this.pendingRequests.get(response.id);
                        if (pending) {
                            this.pendingRequests.delete(response.id);
                            if (response.error) {
                                pending.reject(new Error(this.formatRpcError(response.error)));
                            } else {
                                pending.resolve(response.result);
                            }
                        }
                    }
                } catch {
                    // Not a JSON-RPC response, ignore.
                }

                if (startupTimeout) {
                    clearTimeout(startupTimeout);
                    startupTimeout = undefined;
                    resolved = true;
                    resolve();
                }
            });

            this.process.stderr?.on('data', (data: Buffer) => {
                const text = data.toString();
                stderrBuffer += text;
                if (stderrBuffer.length > 4000) {
                    stderrBuffer = stderrBuffer.slice(-4000);
                }
                console.error(`Go-On stderr: ${text}`);
            });

            this.process.on('close', (code: number) => {
                console.log(`Go-On process exited with code ${code}`);
                const failedBeforeStartup = !resolved;
                this.process = null;
                this.updateStatus();

                if (startupTimeout) {
                    clearTimeout(startupTimeout);
                    startupTimeout = undefined;
                }

                if (failedBeforeStartup) {
                    const details = stderrBuffer.trim();
                    reject(new Error(`Go-On exited before startup (code ${code}). ${details || 'No stderr output.'}`));
                }
            });

            this.process.on('error', (error) => {
                console.error(`Go-On process error: ${error}`);
                this.process = null;
                reject(error);
            });
        });
    }

    stop(): void {
        if (this.process) {
            this.process.kill();
            this.process = null;
        }
        this.updateStatus();
    }

    isRunning(): boolean {
        return this.process !== null;
    }

    setRuntimeEnvOverrides(overrides: Record<string, string>): void {
        this.runtimeEnvOverrides = {
            ...this.runtimeEnvOverrides,
            ...overrides
        };
    }

    async sendRequest(
        method: string,
        params?: unknown,
        options?: { skipProviderGuard?: boolean }
    ): Promise<unknown> {
        if (!this.process) {
            throw new Error('Go-On is not running');
        }

        if (!options?.skipProviderGuard && this.requiresAiProvider(method)) {
            const ready = await this.isAnyAiProviderReady();
            if (!ready) {
                await this.notifyAndOpenSetupWizard();
                throw new Error(`${protocolContract.errors.providerNotReady} ${protocolContract.errors.setupWizardOpened}`);
            }
        }

        const id = ++this.requestId;
        const request: JsonRpcRequest = {
            jsonrpc: '2.0',
            id,
            method,
            params
        };

        return new Promise((resolve, reject) => {
            this.pendingRequests.set(id, { resolve, reject });

            const requestStr = JSON.stringify(request) + '\n';
            this.process!.stdin!.write(requestStr);

            setTimeout(() => {
                if (this.pendingRequests.has(id)) {
                    this.pendingRequests.delete(id);
                    reject(new Error('Request timeout'));
                }
            }, 30000);
        });
    }

    private requiresAiProvider(method: string): boolean {
        return new Set([
            'chat',
            'workflow.execute',
            'task.plan',
            'task.execute',
            'learning.summary',
            'primary_secondary.summary'
        ]).has(method);
    }

    private async isAnyAiProviderReady(): Promise<boolean> {
        const now = Date.now();
        if (this.providerReadyCache && now - this.providerReadyCache.checkedAt < 5000) {
            return this.providerReadyCache.ready;
        }

        try {
            const report = asRecord(await this.sendRequest('runtime.health', undefined, {
                skipProviderGuard: true,
            }));

            const componentsValue = Array.isArray(report.components)
                ? report.components
                : Array.isArray(asRecord(report.report).components)
                    ? asRecord(report.report).components as unknown[]
                    : [];

            const providerComponent = componentsValue
                .map((component) => asRecord(component))
                .find((component) => component.name === 'provider_dependencies');

            if (!providerComponent) {
                this.providerReadyCache = { checkedAt: now, ready: true };
                return true;
            }

            const details = asRecord(providerComponent.details);
            const ready = Number(details.ready ?? 0);
            const total = Number(details.total ?? 0);
            const isReady = total > 0 && ready > 0;
            this.providerReadyCache = { checkedAt: now, ready: isReady };
            return isReady;
        } catch {
            this.providerReadyCache = { checkedAt: now, ready: true };
            return true;
        }
    }

    private async notifyAndOpenSetupWizard(): Promise<void> {
        const now = Date.now();
        if (now - this.lastWizardPromptAt < 5000) {
            return;
        }
        this.lastWizardPromptAt = now;

        await vscode.window.showWarningMessage(
            protocolContract.errors.setupWizardPrompt
        );
        await vscode.commands.executeCommand('go-on.openSettings');
    }

    private updateStatus(): void {
        this.statusItems = [
            new vscode.TreeItem(`Status: ${this.isRunning() ? 'Running' : 'Stopped'}`, vscode.TreeItemCollapsibleState.None)
        ];
        vscode.commands.executeCommand('go-on-status.refresh');
        vscode.commands.executeCommand('go-on.refreshStatusMonitor');
    }

    getStatusItems(): vscode.TreeItem[] {
        return this.statusItems;
    }
}

export class GoOnStatusProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
    private _onDidChangeTreeData: vscode.EventEmitter<vscode.TreeItem | undefined | null | void> = new vscode.EventEmitter<vscode.TreeItem | undefined | null | void>();
    readonly onDidChangeTreeData: vscode.Event<vscode.TreeItem | undefined | null | void> = this._onDidChangeTreeData.event;
    private manager: GoOnManager;

    constructor(_manager: GoOnManager) {
        this.manager = _manager;
    }

    refresh(): void {
        this._onDidChangeTreeData.fire();
    }

    getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
        return element;
    }

    getChildren(element?: vscode.TreeItem): Thenable<vscode.TreeItem[]> {
        if (!element) {
            return Promise.resolve(this.manager.getStatusItems());
        }
        return Promise.resolve([]);
    }
}
