/**
 * Internationalization (i18n) System for Go-On VS Code Plugin
 *
 * Supports: Simplified Chinese (zh_CN), Traditional Chinese (zh_TW), English (en_US)
 * Auto-detects VS Code language and provides translations for the UI
 *
 * Locale files are stored in:
 *   src/locales/en-US.json
 *   src/locales/zh-CN.json
 *   src/locales/zh-TW.json
 *
 * Usage:
 *   import { t } from './i18n';
 *   t('general.goOn')            // => "Go-On"
 *   t('commands.start.title')    // => "Start Go-On Proxy"
 *   t('messages.goOnStartFailed', 'some error') // => "Failed to start Go-On: some error"
 */

import * as fs from "fs";
import * as path from "path";

interface I18nMessages {
  [key: string]: string | I18nMessages;
}

type Language = "en_US" | "zh_CN" | "zh_TW";

// Message keys enumeration for autocomplete and type safety
export const MessageKeys = {
  // General
  goOn: "general.goOn",
  extensionName: "general.extensionName",
  extensionDescription: "general.extensionDescription",
  settings: "general.settings",
  start: "general.start",
  stop: "general.stop",
  status: "general.status",
  running: "general.running",
  stopped: "general.stopped",
  loading: "general.loading",
  error: "general.error",
  warning: "general.warning",
  info: "general.info",
  success: "general.success",
  enabled: "general.enabled",
  disabled: "general.disabled",
  yes: "general.yes",
  no: "general.no",
  ok: "general.ok",
  close: "general.close",
  refresh: "general.refresh",
  reload: "general.reload",
  retry: "general.retry",
  back: "general.back",
  forward: "general.forward",
  next: "general.next",
  previous: "general.previous",
  save: "general.save",
  cancel: "general.cancel",
  reset: "general.reset",
  apply: "general.apply",
  delete: "general.delete",
  edit: "general.edit",
  add: "general.add",
  confirm: "general.confirm",
  search: "general.search",
  filter: "general.filter",
  clear: "general.clear",
  select: "general.select",
  export: "general.export",
  import: "general.import",
  undo: "general.undo",
  redo: "general.redo",

  // Runtime
  runtime: "runtime.runtime",
  runtimeSettings: "runtime.runtimeSettings",
  maintenanceInterval: "runtime.maintenanceInterval",
  healthInterval: "runtime.healthInterval",
  shutdownDrain: "runtime.shutdownDrain",
  cacheSettings: "runtime.cacheSettings",
  vectorSettings: "runtime.vectorSettings",
  autotuneSettings: "runtime.autotuneSettings",
  providerConfig: "runtime.providerConfig",
  agentConfig: "runtime.agentConfig",
  phaseConfig: "runtime.phaseConfig",
  flowConfig: "runtime.flowConfig",

  // Execution
  executionSettings: "execution.executionSettings",
  startGoOn: "execution.startGoOn",
  stopGoOn: "execution.stopGoOn",
  healthCheck: "execution.healthCheck",
  clearCache: "execution.clearCache",
  clearVector: "execution.clearVector",
  reloadConfig: "execution.reloadConfig",
  diagnose: "execution.diagnose",

  // Workflow
  workflow: "workflow.workflow",
  phases: "workflow.phases",
  agents: "workflow.agents",
  addPhase: "workflow.addPhase",
  editPhase: "workflow.editPhase",
  deletePhase: "workflow.deletePhase",
  workflowName: "workflow.workflowName",
  workflowDescription: "workflow.workflowDescription",
  executeWorkflow: "workflow.executeWorkflow",
  createNewWorkflow: "workflow.createNewWorkflow",
  workflowRunning: "workflow.workflowRunning",
  workflowCompleted: "workflow.workflowCompleted",
  workflowFailed: "workflow.workflowFailed",
  selectWorkflow: "workflow.selectWorkflow",
  planTask: "workflow.planTask",
  executeTask: "workflow.executeTask",
  workflowCreatedSuccess: "workflow.createdSuccess",
  workflowNotFound: "workflow.notFound",
  workflowCompletedSuccess: "workflow.completedSuccess",
  workflowExecutionFailed: "workflow.executionFailed",
  workflowDeletedSuccess: "workflow.deletedSuccess",
  workflowDeleteFailed: "workflow.deleteFailed",
  workflowError: "workflow.error",
  workflowCodeNotSupported: "workflow.codeNotSupported",

  // Configuration
  configuration: "config.title",
  configPath: "config.configPath.description",
  executablePath: "config.executablePath.description",
  autoDownloadBinary: "config.autoDownloadBinary.description",
  releaseRepository: "config.releaseRepository.description",
  releaseTag: "config.releaseTag.description",
  autoStart: "config.autoStart.description",

  // Chat
  chat: "chat.chat",
  chatMaxHistory: "chat.chatMaxHistory",
  chatModel: "chat.chatModel",
  chatTemperature: "chat.chatTemperature",
  chatTimeout: "chat.chatTimeout",
  chatMaxTokens: "chat.chatMaxTokens",
  chatStreaming: "chat.chatStreaming",
  inputPlaceholder: "chat.inputPlaceholder",
  sendMessage: "chat.sendMessage",
  attachFiles: "chat.attachFiles",
  clearChat: "chat.clearChat",
  newSession: "chat.newSession",
  switchSession: "chat.switchSession",
  sessionName: "chat.sessionName",
  sessionNamePlaceholder: "chat.sessionNamePlaceholder",
  selectSession: "chat.selectSession",
  noMessages: "chat.noMessages",
  thinking: "chat.thinking",
  responseReceived: "chat.responseReceived",
  codeCopied: "messages.codeCopied",
  chatError: "messages.chatError",
  chatInitFailed: "messages.chatInitFailed",
  createSessionFailed: "messages.createSessionFailed",
  codeExecutionConfirm: "messages.codeExecutionConfirm",
  execute: "messages.execute",
  executionCanceled: "messages.executionCanceled",
  codeExecutionNotSupported: "messages.codeExecutionNotSupported",
  executionFailed: "messages.executionFailed",
  providersCatalogSynced: "messages.providersCatalogSynced",
  configFromWorkspaceTemplate: "messages.configFromWorkspaceTemplate",
  configFromRuntimeTemplate: "messages.configFromRuntimeTemplate",
  downloadFailed: "messages.downloadFailed",
  selectLocalBinary: "messages.selectLocalBinary",
  openGoOnSettings: "messages.openGoOnSettings",
  usingLocalBinary: "messages.usingLocalBinary",
  runtimeDownloading: "messages.runtimeDownloading",
  runtimeDownloadComplete: "messages.runtimeDownloadComplete",
  requestFailed: "chat.requestFailed",
  responseLabel: "chat.responseLabel",

  // Config Wizard
  configWizardTitle: "configWizard.title",
  configWizardSubtitle: "configWizard.subtitle",
  configWizardStep1: "configWizard.step1",
  configWizardStep2: "configWizard.step2",
  configWizardStep3: "configWizard.step3",
  configWizardNext: "configWizard.next",
  configWizardPrevious: "configWizard.previous",
  configWizardRecommended: "configWizard.recommended",
  configWizardProtocolMode: "configWizard.protocolMode",
  configWizardLocalTitle: "configWizard.localTitle",
  configWizardLocalDesc: "configWizard.localDesc",
  configWizardSharedTitle: "configWizard.sharedTitle",
  configWizardSharedDesc: "configWizard.sharedDesc",
  configWizardEditorTitle: "configWizard.editorTitle",
  configWizardEditorDesc: "configWizard.editorDesc",
  configWizardAdaptiveDesc: "configWizard.adaptiveDesc",
  configWizardAcpStdioDesc: "configWizard.acpStdioDesc",
  configWizardAcpHttpDesc: "configWizard.acpHttpDesc",
  configWizardMcpStdioDesc: "configWizard.mcpStdioDesc",
  configWizardMcpHttpDesc: "configWizard.mcpHttpDesc",

  // Messages
  successfullySaved: "messages.successfullySaved",
  errorSaving: "messages.errorSaving",
  unsavedChanges: "messages.unsavedChanges",
  goOnStarted: "messages.goOnStarted",
  goOnStopped: "messages.goOnStopped",
  goOnStartedWithoutKeys: "messages.goOnStartedWithoutKeys",
  goOnStartFailed: "messages.goOnStartFailed",
  goOnNotRunning: "messages.goOnNotRunning",
  cacheCleared: "messages.cacheCleared",
  cacheClearFailed: "messages.cacheClearFailed",
  vectorCleared: "messages.vectorCleared",
  vectorClearFailed: "messages.vectorClearFailed",
  configReloaded: "messages.configReloaded",
  configReloadFailed: "messages.configReloadFailed",
  goOnShutdown: "messages.goOnShutdown",
  goOnShutdownFailed: "messages.goOnShutdownFailed",
  healthCheckResult: "messages.healthCheckResult",
  healthCheckFailed: "messages.healthCheckFailed",
  breakerStatusResult: "messages.breakerStatusResult",
  breakerStatusFailed: "messages.breakerStatusFailed",
  diagnosisCompleted: "messages.diagnosisCompleted",
  diagnosisIssue: "messages.diagnosisIssue",
  noWorkspaceFolderOpen: "messages.noWorkspaceFolderOpen",
  executableNotReady: "messages.executableNotReady",
  chatViewNotAvailable: "messages.chatViewNotAvailable",
  settingsViewNotAvailable: "messages.settingsViewNotAvailable",
  workflowViewNotAvailable: "messages.workflowViewNotAvailable",
  processFlowViewNotAvailable: "messages.processFlowViewNotAvailable",
  chatClosedBackendStopped: "messages.chatClosedBackendStopped",
  chatClosedBackendAlreadyStopped: "messages.chatClosedBackendAlreadyStopped",
  healthCheckWarning: "messages.healthCheckWarning",
  backendNotReady: "messages.backendNotReady",
  goOnStartFailedMissingEnv: "messages.goOnStartFailedMissingEnv",
  reconnectMaxAttempts: "messages.reconnectMaxAttempts",
  runtimeInitFailed: "messages.runtimeInitFailed",
  templateRequired: "messages.templateRequired",
  workflowMappingRequired: "messages.workflowMappingRequired",
  rulesPayloadRequired: "messages.rulesPayloadRequired",

  // Language
  language: "language.language",
  simplifiedChinese: "language.simplifiedChinese",
  traditionalChinese: "language.traditionalChinese",
  english: "language.english",

  // Credentials
  credentials: "credentials.credentials",
  apiKey: "credentials.apiKey",
  secretKey: "credentials.secretKey",
  keyringSecrets: "credentials.keyringSecrets",
  setSecret: "credentials.setSecret",
  getSecret: "credentials.getSecret",
  deleteSecret: "credentials.deleteSecret",
  listSecrets: "credentials.listSecrets",

  // Help
  help: "help.help",
  documentation: "help.documentation",
  about: "help.about",
  version: "help.version",
  releaseNotes: "help.releaseNotes",
  reportIssue: "help.reportIssue",
  githubRepo: "help.githubRepo",

  // StatusBar
  statusBarText: "statusBar.text",
  statusBarRunningTooltip: "statusBar.runningTooltip",
  statusBarStoppedTooltip: "statusBar.stoppedTooltip",
  statusBarHealthCheckFailedTooltip: "statusBar.healthCheckFailedTooltip",
  statusBarHealthTooltip: "statusBar.healthTooltip",

  // Editing
  advancedEdit: "editing.advancedEdit",
  refactorCode: "editing.refactorCode",
  noCodeSelected: "editing.noCodeSelected",
  editPrompt: "editing.editPrompt",
  refactorPrompt: "editing.refactorPrompt",
  applyingChanges: "editing.applyingChanges",
  changesApplied: "editing.changesApplied",
  changesFailed: "editing.changesFailed",
  showDiff: "editing.showDiff",
  acceptChanges: "editing.acceptChanges",
  rejectChanges: "editing.rejectChanges",
  editingNoActiveEditor: "editing.noActiveEditor",
  editingSelectCodeToEdit: "editing.selectCodeToEdit",
  editingChooseActionPlaceholder: "editing.chooseActionPlaceholder",
  editingNoResponseFromAi: "editing.noResponseFromAi",
  editingChooseResultDisplayPlaceholder:
    "editing.chooseResultDisplayPlaceholder",
  editingResultApplied: "editing.resultApplied",
  editingOriginalRefactoredDiffTitle: "editing.originalRefactoredDiffTitle",
  editingResultReplaceSelection: "editing.resultReplaceSelection",
  editingResultShowInNewDocument: "editing.resultShowInNewDocument",
  editingResultCopyToClipboard: "editing.resultCopyToClipboard",
  editingSelectCodeToRefactor: "editing.selectCodeToRefactor",
  editingChooseRefactorTypePlaceholder: "editing.chooseRefactorTypePlaceholder",
  editingDescribeRefactorPrompt: "editing.describeRefactorPrompt",
  editingDescribeRefactorPlaceholder: "editing.describeRefactorPlaceholder",
  editingActionExplainCode: "editing.actionExplainCode",
  editingActionRefactorCode: "editing.actionRefactorCode",
  editingActionOptimizeCode: "editing.actionOptimizeCode",
  editingActionAddComments: "editing.actionAddComments",
  editingActionConvertToAsync: "editing.actionConvertToAsync",
  editingActionAddErrorHandling: "editing.actionAddErrorHandling",
  editingActionGenerateUnitTests: "editing.actionGenerateUnitTests",
  editingActionSecurityAudit: "editing.actionSecurityAudit",
  editingRefactorExtractFunction: "editing.refactorExtractFunction",
  editingRefactorRenameVariables: "editing.refactorRenameVariables",
  editingRefactorSimplifyLogic: "editing.refactorSimplifyLogic",
  editingRefactorImprovePerformance: "editing.refactorImprovePerformance",
  editingRefactorAddTypeHints: "editing.refactorAddTypeHints",
  editingRefactorCustom: "editing.refactorCustom",

  // Process Flow
  processFlow: "processFlow.processFlow",
  showProcessFlow: "processFlow.showProcessFlow",
  noActiveWorkflow: "processFlow.noActiveWorkflow",
  workflowInProgress: "processFlow.workflowInProgress",
  phaseCompleted: "processFlow.phaseCompleted",
  phaseFailed: "processFlow.phaseFailed",
  noProcessSelected: "processFlow.noProcessSelected",
  createProcess: "processFlow.createProcess",
  runProcess: "processFlow.runProcess",
  exportProcessJson: "processFlow.exportProcessJson",
  importProcessJson: "processFlow.importProcessJson",
  agentRunning: "processFlow.agentRunning",
  agentCompleted: "processFlow.agentCompleted",
  processFlowInvalidImportData: "processFlow.invalidImportData",
  processFlowImported: "processFlow.imported",
  processFlowProcessNotFound: "processFlow.processNotFound",
  processFlowInvalidProcessId: "processFlow.invalidProcessId",
  processFlowInvalidStagesFormat: "processFlow.invalidStagesFormat",
  processFlowCreatedSuccess: "processFlow.createdSuccess",
  processFlowCompletedSuccess: "processFlow.completedSuccess",
  processFlowCodeExecutionNotSupported: "processFlow.codeExecutionNotSupported",
  processFlowManualStagePrompt: "processFlow.manualStagePrompt",
  processFlowContinueButton: "processFlow.continueButton",

  // Advanced
  governancePlan: "advanced.governancePlan",
  governanceAudit: "advanced.governanceAudit",
  governanceStatus: "advanced.governanceStatus",
  skillManagement: "advanced.skillManagement",
  importSkill: "advanced.importSkill",
  toggleSkill: "advanced.toggleSkill",
  securityBaseline: "advanced.securityBaseline",
  releaseReadiness: "advanced.releaseReadiness",
  qualityBaseline: "advanced.qualityBaseline",
  observabilityAlerts: "advanced.observabilityAlerts",

  // RPC
  healthProbesFailed: "rpc.healthProbesFailed",
  lockStatusFailed: "rpc.lockStatusFailed",
  healthProbesLabel: "rpc.healthProbesLabel",
  lockStatusLabel: "rpc.lockStatusLabel",
  enterMessage: "rpc.enterMessage",
  messagePlaceholder: "rpc.messagePlaceholder",
  missingEnvVars: "rpc.missingEnvVars",
  noWorkspace: "rpc.noWorkspace",
  rpcCommandResult: "rpc.commandResult",
  rpcCommandCompleted: "rpc.commandCompleted",
  rpcCommandFailed: "rpc.commandFailed",
  maintenanceGcCompleted: "rpc.maintenanceGcCompleted",
  metricsResetCompleted: "rpc.metricsResetCompleted",
  checkpointCreated: "rpc.checkpointCreated",
  checkpointsResult: "rpc.checkpointsResult",
  rolledBack: "rpc.rolledBack",
  autotuneResetConfirm: "rpc.autotuneResetConfirm",
  metricsResetConfirm: "rpc.metricsResetConfirm",
  resetButton: "rpc.resetButton",
  promptWorkflowObjective: "rpc.promptWorkflowObjective",
  promptWorkflowObjectivePlaceholder: "rpc.promptWorkflowObjectivePlaceholder",
  promptTaskPlan: "rpc.promptTaskPlan",
  promptTaskPlanPlaceholder: "rpc.promptTaskPlanPlaceholder",
  promptTaskExecute: "rpc.promptTaskExecute",
  promptTaskExecutePlaceholder: "rpc.promptTaskExecutePlaceholder",
  promptHardnessTask: "rpc.promptHardnessTask",
  promptHardnessTaskPlaceholder: "rpc.promptHardnessTaskPlaceholder",
  promptCostTask: "rpc.promptCostTask",
  promptCostTaskPlaceholder: "rpc.promptCostTaskPlaceholder",
  promptAuditLimit: "rpc.promptAuditLimit",
  promptSkillManifestPath: "rpc.promptSkillManifestPath",
  promptSkillManifestPathPlaceholder: "rpc.promptSkillManifestPathPlaceholder",
  promptSkillSha256: "rpc.promptSkillSha256",
  promptSkillSha256Placeholder: "rpc.promptSkillSha256Placeholder",
  promptSkillName: "rpc.promptSkillName",
  promptSkillNamePlaceholder: "rpc.promptSkillNamePlaceholder",
  promptSkillAction: "rpc.promptSkillAction",
  promptBreakerAgent: "rpc.promptBreakerAgent",
  promptBreakerAgentPlaceholder: "rpc.promptBreakerAgentPlaceholder",
  promptRecoveryAgent: "rpc.promptRecoveryAgent",
  promptConversationId: "rpc.promptConversationId",
  promptConversationIdPlaceholder: "rpc.promptConversationIdPlaceholder",
  promptCheckpointMessage: "rpc.promptCheckpointMessage",
  promptCheckpointMessagePlaceholder: "rpc.promptCheckpointMessagePlaceholder",
  promptCheckpointId: "rpc.promptCheckpointId",
  promptCheckpointIdPlaceholder: "rpc.promptCheckpointIdPlaceholder",
  goOnNotRunningRpc: "messages.goOnNotRunningRpc",
  keyringSetFailed: "messages.keyringSetFailed",
  keyringGetFailed: "messages.keyringGetFailed",
  keyringDeleteFailed: "messages.keyringDeleteFailed",
  keyringListFailed: "messages.keyringListFailed",
  processFlowFailed: "messages.processFlowFailed",
  apiKeyMissing: "messages.apiKeyMissing",
  openSettings: "messages.openSettings",
  later: "messages.later",
  quickSetup: "messages.quickSetup",
  selectProvider: "messages.selectProvider",
  quickSetupStep1Title: "messages.quickSetupStep1Title",
  apiKeyConfigured: "messages.apiKeyConfigured",
  setupFailed: "messages.setupFailed",
} as const;

class I18nManager {
  private currentLanguage: Language = "en_US";
  private messages: I18nMessages = {};
  private static instance: I18nManager;
  private loadedLocales = new Set<string>();

  private constructor() {
    this.detectLanguage();
  }

  static getInstance(): I18nManager {
    if (!I18nManager.instance) {
      I18nManager.instance = new I18nManager();
    }
    return I18nManager.instance;
  }

  /**
   * Detect language from VS Code environment
   */
  private detectLanguage(): void {
    const env = process.env;
    let lang: string;
    if (env.VSCODE_NLS_CONFIG) {
      try {
        const parsed = JSON.parse(env.VSCODE_NLS_CONFIG);
        lang = parsed.locale || "en";
      } catch (err) {
        console.warn("[i18n] detectLanguage parse failed:", err);
        lang = env.LANG || env.LANGUAGE || "en";
      }
    } else {
      lang = env.LANG || env.LANGUAGE || "en";
    }

    if (
      lang.includes("zh_CN") ||
      lang.includes("zh-CN") ||
      lang.includes("chinese-PRC")
    ) {
      this.currentLanguage = "zh_CN";
    } else if (
      lang.includes("zh_TW") ||
      lang.includes("zh-TW") ||
      lang.includes("chinese-Taiwan")
    ) {
      this.currentLanguage = "zh_TW";
    } else {
      this.currentLanguage = "en_US";
    }

    this.loadMessages(this.currentLanguage);
  }

  /**
   * Map internal language code to locale file name
   */
  private languageToLocale(language: Language): string {
    switch (language) {
      case "zh_CN":
        return "zh-CN";
      case "zh_TW":
        return "zh-TW";
      case "en_US":
      default:
        return "en-US";
    }
  }

  /**
   * Load messages from locale JSON file
   */
  private loadMessages(language: Language): void {
    const localeName = this.languageToLocale(language);

    if (
      this.loadedLocales.has(localeName) &&
      this.messages &&
      Object.keys(this.messages).length > 0
    ) {
      return; // Already loaded
    }

    // Try to load from the locales directory
    const localePath = path.resolve(__dirname, "locales", `${localeName}.json`);

    try {
      if (fs.existsSync(localePath)) {
        const content = fs.readFileSync(localePath, "utf-8");
        this.messages = JSON.parse(content) as I18nMessages;
        this.loadedLocales.add(localeName);
        return;
      }
    } catch (err) {
      console.warn("[i18n] loadMessages failed for locale path:", err);
    }

    // Fallback: try relative path for development
    try {
      const devPath = path.resolve(
        __dirname,
        "..",
        "src",
        "locales",
        `${localeName}.json`,
      );
      if (fs.existsSync(devPath)) {
        const content = fs.readFileSync(devPath, "utf-8");
        this.messages = JSON.parse(content) as I18nMessages;
        this.loadedLocales.add(localeName);
        return;
      }
    } catch (err) {
      console.warn("[i18n] loadMessages failed for dev path:", err);
    }

    // If all else fails, use built-in minimal fallback
    this.messages = this.getFallbackMessages(language);
  }

  /**
   * Load a specific locale file by language code (useful for dynamic switching)
   */
  public loadLocale(language: Language): void {
    this.currentLanguage = language;
    this.loadedLocales.clear();
    this.loadMessages(language);
  }

  /**
   * Get translated message by dot-separated key path
   */
  getMessage(key: string, ...params: unknown[]): string {
    const keys = key.split(".");
    let value: unknown = this.messages;

    for (const k of keys) {
      if (
        typeof value === "object" &&
        value !== null &&
        k in (value as Record<string, unknown>)
      ) {
        value = (value as Record<string, unknown>)[k];
      } else {
        // Key not found, try fallback English
        return this.getFallbackValue(key, ...params);
      }
    }

    if (typeof value === "string") {
      let result = value;
      params.forEach((param, index) => {
        result = result.replace(`{${index}}`, String(param));
      });
      return result;
    }

    // If value is an object (not a leaf string), return the key
    if (typeof value === "object" && value !== null) {
      return key;
    }

    return key;
  }

  /**
   * Fallback to English locale if key not found in current language
   */
  private getFallbackValue(key: string, ...params: unknown[]): string {
    const keys = key.split(".");

    // Try hardcoded fallback messages first (always available for any language)
    const messages = this.getFallbackMessages(this.currentLanguage);
    let result: unknown = messages;
    for (const k of keys) {
      if (
        typeof result === "object" &&
        result !== null &&
        k in (result as Record<string, unknown>)
      ) {
        result = (result as Record<string, unknown>)[k];
      } else {
        result = undefined;
        break;
      }
    }
    if (typeof result === "string") {
      let output = result;
      params.forEach((param, index) => {
        output = output.replace(`{${index}}`, String(param));
      });
      return output;
    }

    // Fall back to English locale file if not already English
    if (this.currentLanguage !== "en_US") {
      const enPath = path.resolve(__dirname, "locales", "en-US.json");
      try {
        if (fs.existsSync(enPath)) {
          const content = fs.readFileSync(enPath, "utf-8");
          const enMessages = JSON.parse(content) as I18nMessages;
          let value: unknown = enMessages;
          for (const k of keys) {
            if (
              typeof value === "object" &&
              value !== null &&
              k in (value as Record<string, unknown>)
            ) {
              value = (value as Record<string, unknown>)[k];
            } else {
              return key;
            }
          }
          if (typeof value === "string") {
            let output = value as string;
            params.forEach((param, index) => {
              output = output.replace(`{${index}}`, String(param));
            });
            return output;
          }
        }
      } catch (err) {
        console.warn("[i18n] getFallbackValue failed:", err);
        return key;
      }
    }

    return key;
  }

  /**
   * Minimal hardcoded fallback messages when locale files can't be loaded
   */
  private getFallbackMessages(language: Language): I18nMessages {
    const general: I18nMessages = {
      goOn: "Go-On",
      settings:
        language === "zh_CN"
          ? "设置"
          : language === "zh_TW"
            ? "設定"
            : "Settings",
      start:
        language === "zh_CN" ? "启动" : language === "zh_TW" ? "啟動" : "Start",
      stop:
        language === "zh_CN" ? "停止" : language === "zh_TW" ? "停止" : "Stop",
      status: "Status",
      running:
        language === "zh_CN"
          ? "运行中"
          : language === "zh_TW"
            ? "執行中"
            : "Running",
      stopped:
        language === "zh_CN"
          ? "已停止"
          : language === "zh_TW"
            ? "已停止"
            : "Stopped",
      save: "Save",
      cancel: "Cancel",
      delete: "Delete",
      edit: "Edit",
      add: "Add",
      ok: "OK",
      close: "Close",
      error: "Error",
      warning: "Warning",
      loading: "Loading...",
      success: "Success",
      enabled: "Enabled",
      disabled: "Disabled",
      yes: "Yes",
      no: "No",
      refresh: "Refresh",
      reload: "Reload",
      retry: "Retry",
      back: "Back",
      next: "Next",
      previous: "Previous",
      search: "Search",
      filter: "Filter",
      clear: "Clear",
      select: "Select",
      confirm: "Confirm",
      apply: "Apply",
      reset: "Reset",
      export: "Export",
      import: "Import",
      more: "More",
      less: "Less",
      default: "Default",
      custom: "Custom",
      advanced: "Advanced",
      basic: "Basic",
    };

    const commands: I18nMessages = {
      start: { title: "Start Go-On Proxy", category: "Go-On" },
      stop: { title: "Stop Go-On Proxy", category: "Go-On" },
      openChat: { title: "Open Go-On Chat", category: "Go-On" },
      openSettings: { title: "Open Go-On Settings", category: "Go-On" },
      healthCheck: { title: "Check Go-On Health", category: "Go-On" },
      diagnose: { title: "Diagnose Go-On Runtime", category: "Go-On" },
      cacheClear: { title: "Clear Cache", category: "Go-On" },
      vectorClear: { title: "Clear Vector Memory", category: "Go-On" },
      configReload: { title: "Reload Configuration", category: "Go-On" },
      shutdown: { title: "Shutdown Go-On", category: "Go-On" },
      clearChat: { title: "Clear Chat History", category: "Go-On" },
      exportChat: { title: "Export Chat History", category: "Go-On" },
      newSession: { title: "New Chat Session", category: "Go-On" },
      editCode: { title: "Advanced Code Edit", category: "Go-On" },
      refactorCode: { title: "Refactor Code", category: "Go-On" },
    };

    const messages: I18nMessages = {
      successfullySaved: "Successfully saved",
      errorSaving: "Error saving configuration",
      unsavedChanges: "You have unsaved changes",
      goOnStarted: "Go-On proxy started.",
      goOnStopped: "Go-On proxy stopped.",
      goOnNotRunning: "Go-On is not running. Start it first.",
      goOnShutdown: "Shutdown initiated.",
      cacheCleared: "Cache cleared.",
      vectorCleared: "Vector memory cleared.",
      configReloaded: "Configuration reloaded.",
      diagnosisCompleted: "Go-On diagnosis completed.",
    };

    return {
      general,
      commands,
      messages,
      runtime: {},
      execution: {},
      workflow: {
        createNewWorkflow: "Create New Workflow",
      },
      configuration: {},
      chat: {},
      configWizard: {},
      diagnosis: {},
      statusBar: {},
      rpc: {},
      credentials: {},
      help: {},
      language: {},
      editing: {},
      processFlow: {
        noProcessSelected: "No Process Selected",
        createProcess: "Create",
        runProcess: "Run",
        exportProcessJson: "Export JSON",
        importProcessJson: "Import JSON",
      },
      advanced: {},
      views: {},
      viewContainers: {},
      config: {},
    };
  }

  /**
   * Set language (will trigger language change)
   */
  setLanguage(language: Language): void {
    this.currentLanguage = language;
    this.loadedLocales.clear();
    this.loadMessages(language);
  }

  /**
   * Get current language
   */
  getCurrentLanguage(): Language {
    return this.currentLanguage;
  }

  /**
   * Get language code for app (to sync with Rust app)
   */
  getLanguageCodeForApp(): string {
    return this.currentLanguage;
  }

  /**
   * Reload messages (useful when locale files change)
   */
  reloadMessages(): void {
    this.loadedLocales.clear();
    this.loadMessages(this.currentLanguage);
  }
}

// Singleton instance
const i18nManager = I18nManager.getInstance();

/**
 * Shorthand translation function
 *
 * @param key - Dot-separated key path (e.g. "commands.start.title")
 * @param params - Optional parameters for {0}, {1}, etc. substitution
 * @returns Translated string
 *
 * @example
 *   t('general.goOn')                          // => "Go-On"
 *   t('messages.goOnStartFailed', 'timeout')   // => "Failed to start Go-On: timeout"
 */
export function t(key: string, ...params: unknown[]): string {
  return i18nManager.getMessage(key, ...params);
}

export const i18n = i18nManager;
