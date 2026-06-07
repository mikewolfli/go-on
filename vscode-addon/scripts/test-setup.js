/**
 * Test setup script — mocks native VSCode modules for unit tests
 * outside the extension host.
 *
 * Usage: mocha --require ./scripts/test-setup.js
 */
const Module = require("module");
const path = require("path");

const originalResolve = Module._resolveFilename;

Module._resolveFilename = function (request, parent, ...args) {
  // Intercept the 'vscode' module and resolve to our mock
  if (request === "vscode") {
    // Reuse this loaded setup module as the vscode shim source.
    return __filename;
  }
  return originalResolve.call(this, request, parent, ...args);
};

// Pre-populate the vscode mock so proxyquire doesn't fail
const vscodeMock = {
  window: {
    showErrorMessage: () => Promise.resolve(undefined),
    showInformationMessage: () => Promise.resolve(undefined),
    showWarningMessage: () => Promise.resolve(undefined),
  },
  workspace: {
    getConfiguration: () => ({ get: () => undefined }),
    workspaceFolders: [],
    onDidChangeConfiguration: () => ({ dispose: () => {} }),
  },
  commands: {
    registerCommand: () => ({ dispose: () => {} }),
    executeCommand: () => Promise.resolve(undefined),
  },
  EventEmitter: function () {
    this.event = () => ({ dispose: () => {} });
    this.fire = () => {};
  },
  extensions: {
    getExtension: () => undefined,
  },
  env: {
    language: "en",
    machineId: "test",
    appName: "VS Code",
  },
  Uri: {
    file: (f) => ({ fsPath: f, scheme: "file", path: f }),
    parse: (u) => ({ toString: () => u, fsPath: u, scheme: "https", path: u }),
  },
};

module.exports = vscodeMock;
