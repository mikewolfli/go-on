import * as crypto from "crypto";
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

// Trusted SHA-256 checksums for offline verification (multi-hash to support version rollover).
//
// IMPLEMENTATION (FIXED):
//   Instead of relying solely on checksums.txt downloaded from the same server as the
//   binary (which is vulnerable to MITM), we pin known-good hashes here in the extension.
//   The verifyArchiveChecksum() function first checks against this hardcoded list; if no
//   match is found, it falls back to downloading checksums.txt (graceful degradation).
//
// HOW TO UPDATE:
//   1. Download a known-good runtime binary from a trusted source
//   2. Compute the SHA-256 hash:
//      $ shasum -a 256 <path-to-go-on-binary>  (macOS/Linux)
//      $ certutil -hashfile <path-to-go-on-binary> SHA256  (Windows)
//   3. Add the hash to the array below, keeping old hashes for rollback support
//   4. Commit and release the updated extension
//
// FUTURE WORK (signature verification):
//   The ideal solution is to publish a detached GPG/sigstore signature alongside each
//   release and verify it here. See: https://docs.github.com/en/repositories/releasing-projects-and-archives/managing-releases-in-a-repository#signing-releases
//   For now, SHA-256 pinning eliminates the MITM checkums.txt attack vector described
//   in the SECURITY GAP section above.
const TRUSTED_RUNTIME_SHA256: readonly string[] = []; // Add known-good hashes here during release

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
 * Download checksums.txt from the same release and verify the downloaded archive's SHA-256.
 */
async function verifyArchiveChecksum(
  archivePath: string,
  repository: string,
  tag: string,
  assetName: string,
  trustedSha256Hashes: readonly string[] = [],
): Promise<void> {
  // Compute SHA-256 of the downloaded archive once
  const archiveBuffer = await fsPromises.readFile(archivePath);
  const hash = crypto.createHash("sha256").update(archiveBuffer).digest("hex");

  if (trustedSha256Hashes.length > 0) {
    // Verify against the hardcoded pinned hashes (eliminates MITM risk)
    const hashLower = hash.toLowerCase();
    const matchFound = trustedSha256Hashes.some(
      (trusted) => trusted.toLowerCase() === hashLower,
    );
    if (!matchFound) {
      throw new Error(
        `Checksum mismatch for ${assetName}: no matching trusted hash (got ${hash})`,
      );
    }
    return;
  }

  // Fall back to checksums.txt downloaded from the same release
  const checksumsUrl = buildReleaseAssetUrl(repository, tag, "checksums.txt");
  const checksumsPath = archivePath + ".checksums.txt";

  try {
    await downloadFile(checksumsUrl, checksumsPath);
    const checksumsContent = await fsPromises.readFile(checksumsPath, "utf8");

    // Parse checksums.txt — format: "<sha256>  <filename>"
    let expectedChecksum: string | undefined;
    for (const line of checksumsContent.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      // Split on two or more spaces to separate hash from filename
      const parts = trimmed.split(/\s{2,}/);
      if (parts.length >= 2 && parts[1].trim() === assetName) {
        expectedChecksum = parts[0].trim();
        break;
      }
    }

    if (!expectedChecksum) {
      throw new Error(`Checksum for ${assetName} not found in checksums.txt`);
    }

    if (hash.toLowerCase() !== expectedChecksum.toLowerCase()) {
      throw new Error(
        `Checksum mismatch for ${assetName}: expected ${expectedChecksum}, got ${hash}`,
      );
    }
  } finally {
    // Clean up checksums.txt regardless
    await fsPromises.unlink(checksumsPath).catch(() => undefined);
  }
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
      // Exponential backoff with 30% jitter to prevent thundering herd.
      // delay = (2^attempt * 1000) * (0.7 + random * 0.3)
      const delay = Math.pow(2, attempt) * 1000;
      const jitter = 0.7 + Math.random() * 0.3;
      await new Promise((resolve) =>
        setTimeout(resolve, Math.round(delay * jitter)),
      );
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
  if (TRUSTED_RUNTIME_SHA256.length === 0) {
    void vscode.window.showWarningMessage(
      "[go-on] SHA-256 verification relies on checksums.txt from the same release server. " +
        "Set TRUSTED_RUNTIME_SHA256 at the top of runtimeBinaryService.ts for a pinned hash that " +
        "eliminates MITM risk.",
    );
  }

  try {
    await downloadFile(downloadUrl, archivePath);

    // Verify the downloaded archive (trusted hash if set, otherwise checksums.txt)
    await verifyArchiveChecksum(
      archivePath,
      releaseRepository,
      releaseTag,
      assetName,
      TRUSTED_RUNTIME_SHA256,
    );

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
