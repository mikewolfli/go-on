import * as vscode from 'vscode';
import * as fs from 'fs';
import * as fsPromises from 'fs/promises';
import * as path from 'path';
import * as https from 'https';
import * as os from 'os';
import * as tar from 'tar';
import AdmZip = require('adm-zip');

export interface RuntimeResolution {
    executablePath: string;
    runtimeDir: string;
}

function isSupportedExecutablePath(filePath: string): boolean {
    if (os.platform() === 'win32') {
        const ext = path.extname(filePath).toLowerCase();
        return ext === '.exe' || ext === '.bat' || ext === '.sh';
    }

    try {
        fs.accessSync(filePath, fs.constants.X_OK);
        return true;
    } catch {
        return false;
    }
}

async function openExecutablePathSettings(): Promise<void> {
    await vscode.commands.executeCommand('go-on.openSettings');
    await vscode.commands.executeCommand('workbench.action.openSettings', '@ext:go-on-vscode go-on.executablePath');
}

export async function pathExists(filePath: string): Promise<boolean> {
    try {
        await fsPromises.access(filePath, fs.constants.F_OK);
        return true;
    } catch {
        return false;
    }
}

function platformAssetInfo(): { assetName: string; executableName: string } {
    switch (os.platform()) {
        case 'darwin':
            return { assetName: 'go-on-macos.tar.gz', executableName: 'go-on' };
        case 'linux':
            return { assetName: 'go-on-linux.tar.gz', executableName: 'go-on' };
        case 'win32':
            return { assetName: 'go-on-windows.zip', executableName: 'go-on.exe' };
        default:
            throw new Error(`Unsupported platform: ${os.platform()}`);
    }
}

function buildReleaseAssetUrl(repository: string, tag: string, assetName: string): string {
    if (tag === 'latest') {
        return `https://github.com/${repository}/releases/latest/download/${assetName}`;
    }
    return `https://github.com/${repository}/releases/download/${tag}/${assetName}`;
}

async function downloadFile(url: string, destinationPath: string, maxRedirects: number = 5): Promise<void> {
    if (maxRedirects <= 0) {
        throw new Error('Too many redirects while downloading file');
    }

    await fsPromises.mkdir(path.dirname(destinationPath), { recursive: true });

    await new Promise<void>((resolve, reject) => {
        const request = https.get(url, (response) => {
            const statusCode = response.statusCode ?? 0;

            if (statusCode >= 300 && statusCode < 400 && response.headers.location) {
                response.resume();
                downloadFile(response.headers.location, destinationPath, maxRedirects - 1).then(resolve).catch(reject);
                return;
            }

            if (statusCode < 200 || statusCode >= 300) {
                response.resume();
                reject(new Error(`Download failed with HTTP ${statusCode}`));
                return;
            }

            const fileStream = fs.createWriteStream(destinationPath);
            response.pipe(fileStream);
            fileStream.on('finish', () => {
                fileStream.close();
                resolve();
            });
            fileStream.on('error', reject);
        });

        request.on('error', reject);
    });
}

async function extractArchive(archivePath: string, destinationDir: string): Promise<void> {
    if (archivePath.endsWith('.tar.gz')) {
        await tar.x({
            file: archivePath,
            cwd: destinationDir,
            strip: 1
        });
        return;
    }

    if (archivePath.endsWith('.zip')) {
        const zip = new AdmZip(archivePath);
        zip.extractAllTo(destinationDir, true);
        return;
    }

    throw new Error(`Unsupported archive format: ${archivePath}`);
}

export async function ensureProvidersTomlForConfig(
    workspaceRoot: string,
    runtimeDir: string,
    configPath: string
): Promise<void> {
    const targetDir = path.dirname(configPath);
    const targetProvidersPath = path.join(targetDir, 'providers.toml');
    if (await pathExists(targetProvidersPath)) {
        return;
    }

    const sourceCandidates = [
        path.join(workspaceRoot, 'providers.toml'),
        path.join(runtimeDir, 'providers.toml')
    ];

    for (const candidate of sourceCandidates) {
        if (!(await pathExists(candidate))) {
            continue;
        }
        await fsPromises.mkdir(targetDir, { recursive: true });
        await fsPromises.copyFile(candidate, targetProvidersPath);
        vscode.window.showInformationMessage(`Go-On providers catalog synced: ${targetProvidersPath}`);
        return;
    }
}

export async function resolveConfigPath(
    workspaceRoot: string,
    configuredConfigPath: string,
    runtimeDir: string
): Promise<string> {
    const workspaceConfigPath = path.resolve(workspaceRoot, configuredConfigPath);
    if (await pathExists(workspaceConfigPath)) {
        await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, workspaceConfigPath);
        return workspaceConfigPath;
    }

    const bundledConfigPath = path.join(runtimeDir, 'config.toml');
    if (await pathExists(bundledConfigPath)) {
        return bundledConfigPath;
    }

    const workspaceConfigTemplatePath = path.join(workspaceRoot, 'config.toml.autopilot-adaptive');
    if (await pathExists(workspaceConfigTemplatePath)) {
        await fsPromises.mkdir(path.dirname(workspaceConfigPath), { recursive: true });
        await fsPromises.copyFile(workspaceConfigTemplatePath, workspaceConfigPath);
        await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, workspaceConfigPath);
        vscode.window.showInformationMessage(`Go-On config created from workspace template: ${workspaceConfigPath}`);
        return workspaceConfigPath;
    }

    const bundledConfigTemplatePath = path.join(runtimeDir, 'config.toml.autopilot-adaptive');
    if (await pathExists(bundledConfigTemplatePath)) {
        await fsPromises.mkdir(path.dirname(workspaceConfigPath), { recursive: true });
        await fsPromises.copyFile(bundledConfigTemplatePath, workspaceConfigPath);
        await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, workspaceConfigPath);
        vscode.window.showInformationMessage(`Go-On config created from runtime template: ${workspaceConfigPath}`);
        return workspaceConfigPath;
    }

    throw new Error(
        `Config not found. Checked workspace path '${workspaceConfigPath}' and bundled path '${bundledConfigPath}'.`
    );
}

async function promptForManualBinaryPath(
    config: vscode.WorkspaceConfiguration,
    workspaceRoot: string | undefined,
    reason: string
): Promise<RuntimeResolution> {
    const selectOption = 'Select Local Binary';
    const openSettingsOption = 'Open Go-On Settings';
    const cancelOption = 'Cancel';
    const choice = await vscode.window.showErrorMessage(
        `Failed to download Go-On runtime: ${reason}`,
        selectOption,
        openSettingsOption,
        cancelOption
    );

    if (choice === openSettingsOption) {
        await openExecutablePathSettings();
        throw new Error('Runtime download failed. Set go-on.executablePath and try again.');
    }

    if (choice !== selectOption) {
        throw new Error('Runtime download was canceled. You can set go-on.executablePath in settings.');
    }

    const fileSelection = await vscode.window.showOpenDialog({
        canSelectFiles: true,
        canSelectFolders: false,
        canSelectMany: false,
        title: 'Select Go-On executable',
        openLabel: 'Use This Binary'
    });

    if (!fileSelection || fileSelection.length === 0) {
        await openExecutablePathSettings();
        throw new Error('No local binary selected. Set go-on.executablePath in settings and try again.');
    }

    const selectedPath = fileSelection[0].fsPath;
    if (!(await pathExists(selectedPath))) {
        await openExecutablePathSettings();
        throw new Error(`Selected executable does not exist: ${selectedPath}`);
    }

    if (!isSupportedExecutablePath(selectedPath)) {
        await openExecutablePathSettings();
        if (os.platform() === 'win32') {
            throw new Error(`Selected file is not supported: ${selectedPath}. Please select an .exe, .bat, or .sh file.`);
        }
        throw new Error(`Selected file is not executable: ${selectedPath}. Please select a binary with execute permission.`);
    }

    if (os.platform() !== 'win32') {
        try {
            await fsPromises.chmod(selectedPath, 0o755);
        } catch {
            // Ignore chmod failures for user-managed binaries.
        }
    }

    await config.update(
        'executablePath',
        selectedPath,
        workspaceRoot ? vscode.ConfigurationTarget.Workspace : vscode.ConfigurationTarget.Global
    );

    vscode.window.showInformationMessage(`Using local Go-On binary: ${selectedPath}`);
    return {
        executablePath: selectedPath,
        runtimeDir: path.dirname(selectedPath)
    };
}

export async function ensureGoOnBinary(
    workspaceRoot: string | undefined,
    config: vscode.WorkspaceConfiguration,
    context: vscode.ExtensionContext
): Promise<RuntimeResolution> {
    const configuredExecutablePath = config.get<string>('executablePath', './target/release/go-on');

    const ensureSupportedPath = async (resolvedPath: string): Promise<RuntimeResolution> => {
        if (!isSupportedExecutablePath(resolvedPath)) {
            await openExecutablePathSettings();
            if (os.platform() === 'win32') {
                throw new Error(
                    `Configured executable must be an .exe, .bat, or .sh file: ${resolvedPath}`
                );
            }
            throw new Error(
                `Configured executable is missing execute permission: ${resolvedPath}`
            );
        }
        return {
            executablePath: resolvedPath,
            runtimeDir: path.dirname(resolvedPath)
        };
    };

    if (workspaceRoot) {
        const resolvedWorkspaceExecutable = path.isAbsolute(configuredExecutablePath)
            ? configuredExecutablePath
            : path.resolve(workspaceRoot, configuredExecutablePath);
        if (await pathExists(resolvedWorkspaceExecutable)) {
            return await ensureSupportedPath(resolvedWorkspaceExecutable);
        }
    } else if (path.isAbsolute(configuredExecutablePath) && await pathExists(configuredExecutablePath)) {
        return await ensureSupportedPath(configuredExecutablePath);
    }

    const autoDownloadEnabled = config.get<boolean>('autoDownloadBinary', false);
    if (!autoDownloadEnabled) {
        await openExecutablePathSettings();
        throw new Error(
            `Configured executable does not exist: ${configuredExecutablePath}. Set go-on.executablePath to a valid local runtime path.`
        );
    }

    const { assetName, executableName } = platformAssetInfo();
    const releaseRepository = config.get<string>('releaseRepository', 'mikewolfli/go-on');
    const releaseTag = config.get<string>('releaseTag', 'latest');
    const runtimeDir = path.join(context.globalStorageUri.fsPath, 'runtime');
    const executablePath = path.join(runtimeDir, executableName);

    if (await pathExists(executablePath)) {
        return { executablePath, runtimeDir };
    }

    await fsPromises.mkdir(runtimeDir, { recursive: true });

    const archivePath = path.join(context.globalStorageUri.fsPath, assetName);
    const downloadUrl = buildReleaseAssetUrl(releaseRepository, releaseTag, assetName);

    vscode.window.showInformationMessage(`Go-On runtime not found. Downloading ${assetName} from ${releaseRepository} (${releaseTag})...`);
    try {
        await downloadFile(downloadUrl, archivePath);
        await extractArchive(archivePath, runtimeDir);
    } catch (error: unknown) {
        return await promptForManualBinaryPath(
            config,
            workspaceRoot,
            error instanceof Error ? error.message : String(error)
        );
    }

    if (os.platform() !== 'win32') {
        await fsPromises.chmod(executablePath, 0o755);
    }

    if (!(await pathExists(executablePath))) {
        throw new Error(`Downloaded archive did not contain executable: ${executableName}`);
    }

    if (!isSupportedExecutablePath(executablePath)) {
        await openExecutablePathSettings();
        if (os.platform() === 'win32') {
            throw new Error(`Resolved runtime is not supported: ${executablePath}. Expected .exe, .bat, or .sh.`);
        }
        throw new Error(`Resolved runtime is not executable: ${executablePath}.`);
    }

    vscode.window.showInformationMessage('Go-On runtime download complete. Chat is ready to use.');

    return { executablePath, runtimeDir };
}
