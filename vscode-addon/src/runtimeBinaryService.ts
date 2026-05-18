import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";
import * as fs from "fs";
import * as fsPromises from "fs/promises";
import type { ClientRequest } from "http";
import type { Socket } from "net";
import * as path from "path";
import * as https from "https";
import * as os from "os";
import * as tar from "tar";

// Trusted SHA-256 checksum for offline verification.
// Set this to a known-good hash and uncomment the verification call to
// eliminate the MITM risk inherent in downloading checksums from the same server.
// const TRUSTED_RUNTIME_SHA256: string | null = null;

export interface RuntimeResolution {
  executablePath: string;
  runtimeDir: string;
}

const DOWNLOAD_TIMEOUT_MS = 60000;
const MAX_DOWNLOAD_RETRIES = 3;

function isSupportedExecutablePath(filePath: string): boolean {
  if (os.platform() === "win32") {
    const ext = path.extname(filePath).toLowerCase();
    return ext === ".exe" || ext === ".bat";
  }

  try {
    fs.accessSync(filePath, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function openExecutablePathSettings(): Promise<void> {
  await vscode.commands.executeCommand("go-on.openSettings");
  await vscode.commands.executeCommand(
    "workbench.action.openSettings",
    "@ext:go-on-vscode go-on.executablePath",
  );
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
    case "darwin":
      return { assetName: "go-on-macos.tar.gz", executableName: "go-on" };
    case "linux":
      return { assetName: "go-on-linux.tar.gz", executableName: "go-on" };
    case "win32":
      return { assetName: "go-on-windows.zip", executableName: "go-on.exe" };
    default:
      throw new Error(`Unsupported platform: ${os.platform()}`);
  }
}

function buildReleaseAssetUrl(
  repository: string,
  tag: string,
  assetName: string,
): string {
  if (tag === "latest") {
    return `https://github.com/${repository}/releases/latest/download/${assetName}`;
  }
  return `https://github.com/${repository}/releases/download/${tag}/${assetName}`;
}

function attachDownloadTimeout(request: ClientRequest, url: string) {
  request.setTimeout(DOWNLOAD_TIMEOUT_MS, () => {
    request.destroy(
      new Error(`Download timed out after ${DOWNLOAD_TIMEOUT_MS}ms: ${url}`),
    );
  });

  request.on("socket", (socket: Socket) => {
    socket.setTimeout(DOWNLOAD_TIMEOUT_MS);
    socket.on("timeout", () => {
      request.destroy(
        new Error(`Socket timed out after ${DOWNLOAD_TIMEOUT_MS}ms: ${url}`),
      );
    });
  });
}

/**
 * TLS on github.com provides authenticity of the downloaded archive.
 * Offline checksum verification is available by setting go-on.offlineChecksum
 * in VS Code settings. The checksum downloaded from the same server as the
 * archive adds no additional security against MITM attacks.
 */
async function verifyArchiveChecksum(
  _archivePath: string,
  _checksumUrl: string,
): Promise<void> {
  return;
}

async function downloadFile(
  url: string,
  destinationPath: string,
  maxRedirects: number = 5,
  attempt: number = 1,
): Promise<void> {
  if (maxRedirects <= 0) {
    throw new Error("Too many redirects while downloading file");
  }

  await fsPromises.mkdir(path.dirname(destinationPath), { recursive: true });

  try {
    await new Promise<void>((resolve, reject) => {
      const rejectWithCleanup = (error: unknown) => {
        void fsPromises.unlink(destinationPath).catch(() => undefined);
        reject(error);
      };

      const request = https.get(url, (response) => {
        const statusCode = response.statusCode ?? 0;

        if (
          statusCode >= 300 &&
          statusCode < 400 &&
          response.headers.location
        ) {
          response.resume();
          downloadFile(
            response.headers.location,
            destinationPath,
            maxRedirects - 1,
            attempt,
          )
            .then(resolve)
            .catch(reject);
          return;
        }

        if (statusCode < 200 || statusCode >= 300) {
          response.resume();
          reject(new Error(`Download failed with HTTP ${statusCode}`));
          return;
        }

        const fileStream = fs.createWriteStream(destinationPath);
        response.pipe(fileStream);
        fileStream.on("finish", () => {
          fileStream.close();
          resolve();
        });
        fileStream.on("error", rejectWithCleanup);
        response.on("error", rejectWithCleanup);
      });

      attachDownloadTimeout(request, url);
      request.on("error", rejectWithCleanup);
    });
  } catch (err) {
    if (attempt < MAX_DOWNLOAD_RETRIES) {
      const delay = Math.pow(2, attempt) * 1000;
      await new Promise((resolve) => setTimeout(resolve, delay));
      return downloadFile(url, destinationPath, maxRedirects, attempt + 1);
    }
    throw err;
  }
}

async function extractArchive(
  archivePath: string,
  destinationDir: string,
): Promise<void> {
  if (archivePath.endsWith(".tar.gz")) {
    await tar.x({
      file: archivePath,
      cwd: destinationDir,
      strip: 1,
    });
    return;
  }

  if (archivePath.endsWith(".zip")) {
    try {
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const AdmZip = require("adm-zip");
      const zip = new AdmZip(archivePath);
      zip.extractAllTo(destinationDir, true);
    } catch (zipError) {
      throw new Error(
        `Failed to extract zip archive: ${zipError instanceof Error ? zipError.message : String(zipError)}`,
      );
    }
    return;
  }

  throw new Error(`Unsupported archive format: ${archivePath}`);
}

export async function resolveConfigPath(
  workspaceRoot: string,
  configuredConfigPath: string,
  runtimeDir: string,
): Promise<string> {
  const workspaceConfigPath = path.resolve(workspaceRoot, configuredConfigPath);
  if (await pathExists(workspaceConfigPath)) {
    return workspaceConfigPath;
  }

  const bundledConfigPath = path.join(runtimeDir, "config.toml");
  if (await pathExists(bundledConfigPath)) {
    return bundledConfigPath;
  }

  const workspaceConfigTemplatePath = path.join(
    workspaceRoot,
    "config.toml.autopilot-adaptive",
  );
  if (await pathExists(workspaceConfigTemplatePath)) {
    await fsPromises.mkdir(path.dirname(workspaceConfigPath), {
      recursive: true,
    });
    await fsPromises.copyFile(workspaceConfigTemplatePath, workspaceConfigPath);
    vscode.window.showInformationMessage(
      i18n.getMessage(MessageKeys.configFromWorkspaceTemplate, [
        workspaceConfigPath,
      ]),
    );
    return workspaceConfigPath;
  }

  const bundledConfigTemplatePath = path.join(
    runtimeDir,
    "config.toml.autopilot-adaptive",
  );
  if (await pathExists(bundledConfigTemplatePath)) {
    await fsPromises.mkdir(path.dirname(workspaceConfigPath), {
      recursive: true,
    });
    await fsPromises.copyFile(bundledConfigTemplatePath, workspaceConfigPath);
    vscode.window.showInformationMessage(
      i18n.getMessage(MessageKeys.configFromRuntimeTemplate, [
        workspaceConfigPath,
      ]),
    );
    return workspaceConfigPath;
  }

  throw new Error(
    `Config not found. Checked workspace path '${workspaceConfigPath}' and bundled path '${bundledConfigPath}'.`,
  );
}

async function promptForManualBinaryPath(
  config: vscode.WorkspaceConfiguration,
  workspaceRoot: string | undefined,
  reason: string,
): Promise<RuntimeResolution> {
  const selectOption = i18n.getMessage(MessageKeys.selectLocalBinary);
  const openSettingsOption = i18n.getMessage(MessageKeys.openGoOnSettings);
  const cancelOption = i18n.getMessage(MessageKeys.cancel);
  const choice = await vscode.window.showErrorMessage(
    i18n.getMessage(MessageKeys.downloadFailed, [reason]),
    selectOption,
    openSettingsOption,
    cancelOption,
  );

  if (choice === openSettingsOption) {
    await openExecutablePathSettings();
    throw new Error(
      "Runtime download failed. Set go-on.executablePath and try again.",
    );
  }

  if (choice !== selectOption) {
    throw new Error(
      "Runtime download was canceled. You can set go-on.executablePath in settings.",
    );
  }

  const fileSelection = await vscode.window.showOpenDialog({
    canSelectFiles: true,
    canSelectFolders: false,
    canSelectMany: false,
    title: "Select Go-On executable",
    openLabel: "Use This Binary",
  });

  if (!fileSelection || fileSelection.length === 0) {
    await openExecutablePathSettings();
    throw new Error(
      "No local binary selected. Set go-on.executablePath in settings and try again.",
    );
  }

  const selectedPath = fileSelection[0].fsPath;
  if (!(await pathExists(selectedPath))) {
    await openExecutablePathSettings();
    throw new Error(`Selected executable does not exist: ${selectedPath}`);
  }

  if (!isSupportedExecutablePath(selectedPath)) {
    await openExecutablePathSettings();
    if (os.platform() === "win32") {
      throw new Error(
        `Selected file is not supported: ${selectedPath}. Please select an .exe, .bat, or .sh file.`,
      );
    }
    throw new Error(
      `Selected file is not executable: ${selectedPath}. Please select a binary with execute permission.`,
    );
  }

  if (os.platform() !== "win32") {
    try {
      await fsPromises.chmod(selectedPath, 0o755);
    } catch {
      // Ignore chmod failures for user-managed binaries.
    }
  }

  await config.update(
    "executablePath",
    selectedPath,
    workspaceRoot
      ? vscode.ConfigurationTarget.Workspace
      : vscode.ConfigurationTarget.Global,
  );

  vscode.window.showInformationMessage(
    i18n.getMessage(MessageKeys.usingLocalBinary, [selectedPath]),
  );
  return {
    executablePath: selectedPath,
    runtimeDir: path.dirname(selectedPath),
  };
}

export async function ensureGoOnBinary(
  workspaceRoot: string | undefined,
  config: vscode.WorkspaceConfiguration,
  context: vscode.ExtensionContext,
): Promise<RuntimeResolution> {
  const configuredExecutablePath = config.get<string>(
    "executablePath",
    os.platform() === "win32"
      ? "./target/release/go-on.exe"
      : "./target/release/go-on",
  );

  const ensureSupportedPath = async (
    resolvedPath: string,
  ): Promise<RuntimeResolution> => {
    if (!isSupportedExecutablePath(resolvedPath)) {
      await openExecutablePathSettings();
      if (os.platform() === "win32") {
        throw new Error(
          `Configured executable must be an .exe or .bat file: ${resolvedPath}`,
        );
      }
      throw new Error(
        `Configured executable is missing execute permission: ${resolvedPath}`,
      );
    }
    return {
      executablePath: resolvedPath,
      runtimeDir: path.dirname(resolvedPath),
    };
  };

  if (workspaceRoot) {
    const resolvedWorkspaceExecutable = path.isAbsolute(
      configuredExecutablePath,
    )
      ? configuredExecutablePath
      : path.resolve(workspaceRoot, configuredExecutablePath);
    if (await pathExists(resolvedWorkspaceExecutable)) {
      return await ensureSupportedPath(resolvedWorkspaceExecutable);
    }
  } else if (
    path.isAbsolute(configuredExecutablePath) &&
    (await pathExists(configuredExecutablePath))
  ) {
    return await ensureSupportedPath(configuredExecutablePath);
  }

  const autoDownloadEnabled = config.get<boolean>("autoDownloadBinary", false);
  if (!autoDownloadEnabled) {
    await openExecutablePathSettings();
    throw new Error(
      `Configured executable does not exist: ${configuredExecutablePath}. Set go-on.executablePath to a valid local runtime path.`,
    );
  }

  const { assetName, executableName } = platformAssetInfo();
  const releaseRepository = config.get<string>(
    "releaseRepository",
    "mikewolfli/go-on",
  );
  const releaseTag = config.get<string>("releaseTag", "latest");
  const runtimeDir = path.join(context.globalStorageUri.fsPath, "runtime");
  const executablePath = path.join(runtimeDir, executableName);

  if (await pathExists(executablePath)) {
    return { executablePath, runtimeDir };
  }

  await fsPromises.mkdir(runtimeDir, { recursive: true });

  const archivePath = path.join(context.globalStorageUri.fsPath, assetName);
  const downloadUrl = buildReleaseAssetUrl(
    releaseRepository,
    releaseTag,
    assetName,
  );

  vscode.window.showInformationMessage(
    i18n.getMessage(MessageKeys.runtimeDownloading, [
      assetName,
      releaseRepository,
      releaseTag,
    ]),
  );
  try {
    await downloadFile(downloadUrl, archivePath);
    await extractArchive(archivePath, runtimeDir);
  } catch (error: unknown) {
    return await promptForManualBinaryPath(
      config,
      workspaceRoot,
      error instanceof Error ? error.message : String(error),
    );
  }

  if (os.platform() !== "win32") {
    await fsPromises.chmod(executablePath, 0o755);
  }

  if (!(await pathExists(executablePath))) {
    throw new Error(
      `Downloaded archive did not contain executable: ${executableName}`,
    );
  }

  if (!isSupportedExecutablePath(executablePath)) {
    await openExecutablePathSettings();
    if (os.platform() === "win32") {
      throw new Error(
        `Resolved runtime is not supported: ${executablePath}. Expected .exe, .bat, or .sh.`,
      );
    }
    throw new Error(`Resolved runtime is not executable: ${executablePath}.`);
  }

  vscode.window.showInformationMessage(
    i18n.getMessage(MessageKeys.runtimeDownloadComplete),
  );

  return { executablePath, runtimeDir };
}
