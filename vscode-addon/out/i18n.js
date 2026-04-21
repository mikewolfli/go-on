"use strict";
/**
 * Internationalization (i18n) System for Go-On VS Code Plugin
 *
 * Supports: Simplified Chinese (zh_CN), Traditional Chinese (zh_TW), English (en_US)
 * Auto-detects VS Code language and provides translations for the UI
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.i18n = exports.MessageKeys = void 0;
// Message keys enumeration
exports.MessageKeys = {
    // General
    goOn: 'general.goOn',
    settings: 'general.settings',
    start: 'general.start',
    stop: 'general.stop',
    status: 'general.status',
    running: 'general.running',
    stopped: 'general.stopped',
    // Runtime
    runtime: 'runtime.runtime',
    runtimeSettings: 'runtime.runtimeSettings',
    maintenanceInterval: 'runtime.maintenanceInterval',
    healthInterval: 'runtime.healthInterval',
    shutdownDrain: 'runtime.shutdownDrain',
    cacheSettings: 'runtime.cacheSettings',
    vectorSettings: 'runtime.vectorSettings',
    autotuneSettings: 'runtime.autotuneSettings',
    // Execution
    executionSettings: 'execution.executionSettings',
    startGoOn: 'execution.startGoOn',
    stopGoOn: 'execution.stopGoOn',
    healthCheck: 'execution.healthCheck',
    clearCache: 'execution.clearCache',
    // Workflow
    workflow: 'workflow.workflow',
    phases: 'workflow.phases',
    agents: 'workflow.agents',
    addPhase: 'workflow.addPhase',
    editPhase: 'workflow.editPhase',
    deletePhase: 'workflow.deletePhase',
    // Configuration
    configuration: 'configuration.configuration',
    configPath: 'configuration.configPath',
    executablePath: 'configuration.executablePath',
    autoDownloadBinary: 'configuration.autoDownloadBinary',
    releaseRepository: 'configuration.releaseRepository',
    releaseTag: 'configuration.releaseTag',
    autoStart: 'configuration.autoStart',
    // Chat
    chat: 'chat.chat',
    chatMaxHistory: 'chat.chatMaxHistory',
    chatModel: 'chat.chatModel',
    chatTemperature: 'chat.chatTemperature',
    chatTimeout: 'chat.chatTimeout',
    // Buttons
    save: 'buttons.save',
    cancel: 'buttons.cancel',
    reset: 'buttons.reset',
    apply: 'buttons.apply',
    delete: 'buttons.delete',
    edit: 'buttons.edit',
    add: 'buttons.add',
    // Messages
    successfullySaved: 'messages.successfullySaved',
    errorSaving: 'messages.errorSaving',
    unsavedChanges: 'messages.unsavedChanges',
    // Language
    language: 'language.language',
    simplifiedChinese: 'language.simplifiedChinese',
    traditionalChinese: 'language.traditionalChinese',
    english: 'language.english',
    // Credentials
    credentials: 'credentials.credentials',
    apiKey: 'credentials.apiKey',
    secretKey: 'credentials.secretKey',
    keyringSecrets: 'credentials.keyringSecrets',
    setSecret: 'credentials.setSecret',
    getSecret: 'credentials.getSecret',
    deleteSecret: 'credentials.deleteSecret',
    // Help
    help: 'help.help',
    documentation: 'help.documentation',
    about: 'help.about',
    version: 'help.version',
};
class I18nManager {
    constructor() {
        this.currentLanguage = 'en_US';
        this.messages = {};
        this.detectLanguage();
    }
    static getInstance() {
        if (!I18nManager.instance) {
            I18nManager.instance = new I18nManager();
        }
        return I18nManager.instance;
    }
    /**
     * Detect language from VS Code environment
     */
    detectLanguage() {
        const env = process.env;
        const lang = env.VSCODE_NLS_CONFIG
            ? JSON.parse(env.VSCODE_NLS_CONFIG).locale
            : env.LANG || env.LANGUAGE || 'en';
        if (lang.includes('zh_CN') || lang.includes('zh-CN') || lang.includes('chinese-PRC')) {
            this.currentLanguage = 'zh_CN';
        }
        else if (lang.includes('zh_TW') || lang.includes('zh-TW') || lang.includes('chinese-Taiwan')) {
            this.currentLanguage = 'zh_TW';
        }
        else {
            this.currentLanguage = 'en_US';
        }
        this.loadMessages(this.currentLanguage);
    }
    /**
     * Load messages for the specified language
     */
    loadMessages(language) {
        switch (language) {
            case 'zh_CN':
                this.messages = zhCN_Messages;
                break;
            case 'zh_TW':
                this.messages = zhTW_Messages;
                break;
            case 'en_US':
            default:
                this.messages = enUS_Messages;
                break;
        }
    }
    /**
     * Get translated message
     */
    getMessage(key, ...params) {
        const keys = key.split('.');
        let value = this.messages;
        for (const k of keys) {
            if (typeof value === 'object' && value !== null && k in value) {
                value = value[k];
            }
            else {
                return key; // Return key if not found
            }
        }
        if (typeof value === 'string') {
            // Simple parameter substitution
            let result = value;
            params.forEach((param, index) => {
                result = result.replace(`{${index}}`, String(param));
            });
            return result;
        }
        return key;
    }
    /**
     * Set language (will trigger language change)
     */
    setLanguage(language) {
        this.currentLanguage = language;
        this.loadMessages(language);
    }
    /**
     * Get current language
     */
    getCurrentLanguage() {
        return this.currentLanguage;
    }
    /**
     * Get language code for app (to sync with Rust app)
     */
    getLanguageCodeForApp() {
        return this.currentLanguage;
    }
}
// Message definitions
const enUS_Messages = {
    general: {
        goOn: 'Go-On',
        settings: 'Settings',
        start: 'Start',
        stop: 'Stop',
        status: 'Status',
        running: 'Running',
        stopped: 'Stopped',
    },
    runtime: {
        runtime: 'Runtime',
        runtimeSettings: 'Runtime Settings',
        maintenanceInterval: 'Maintenance Interval (seconds)',
        healthInterval: 'Health Check Interval (seconds)',
        shutdownDrain: 'Shutdown Drain (seconds)',
        cacheSettings: 'Cache Settings',
        vectorSettings: 'Vector Database Settings',
        autotuneSettings: 'Auto-tune Settings',
    },
    execution: {
        executionSettings: 'Execution Settings',
        startGoOn: 'Start Go-On',
        stopGoOn: 'Stop Go-On',
        healthCheck: 'Health Check',
        clearCache: 'Clear Cache',
    },
    workflow: {
        workflow: 'Workflow',
        phases: 'Phases',
        agents: 'Agents',
        addPhase: 'Add Phase',
        editPhase: 'Edit Phase',
        deletePhase: 'Delete Phase',
    },
    configuration: {
        configuration: 'Configuration',
        configPath: 'Configuration File Path',
        executablePath: 'Executable Path',
        autoDownloadBinary: 'Auto Download Binary',
        releaseRepository: 'Release Repository',
        releaseTag: 'Release Tag',
        autoStart: 'Auto Start',
        wizard: {
            title: 'Go-On Configuration Wizard',
            subtitle: 'Choose a usage scenario and recommended protocol mode, then save the workspace runtime settings.',
            step1: 'Scenario',
            step2: 'Protocol',
            step3: 'Review',
            next: 'Next',
            previous: 'Previous',
            recommended: 'Recommended',
            protocolMode: 'Protocol Mode',
            localTitle: 'Local Dev',
            localDesc: 'Single-user daily development on one machine.',
            sharedTitle: 'Shared Server',
            sharedDesc: 'One long-running backend shared across users or tools.',
            editorTitle: 'Editor Plugin',
            editorDesc: 'Best when the editor launches go-on as a child process.',
            adaptiveDesc: 'Dual-stack capability with adaptive routing.',
            acpStdioDesc: 'ACP over stdio for child-process editor integrations.',
            acpHttpDesc: 'ACP over HTTP for one shared long-running backend.',
            mcpStdioDesc: 'MCP over stdio when the client explicitly expects it.',
            mcpHttpDesc: 'MCP over HTTP for OpenAI-compatible /v1 endpoints.'
        }
    },
    chat: {
        chat: 'Chat',
        chatMaxHistory: 'Max Chat History',
        chatModel: 'Chat Model',
        chatTemperature: 'Temperature',
        chatTimeout: 'Timeout (seconds)',
    },
    buttons: {
        save: 'Save',
        cancel: 'Cancel',
        reset: 'Reset',
        apply: 'Apply',
        delete: 'Delete',
        edit: 'Edit',
        add: 'Add',
    },
    messages: {
        successfullySaved: 'Successfully saved',
        errorSaving: 'Error saving',
        unsavedChanges: 'You have unsaved changes',
    },
    language: {
        language: 'Language',
        simplifiedChinese: 'Simplified Chinese',
        traditionalChinese: 'Traditional Chinese',
        english: 'English',
    },
    credentials: {
        credentials: 'Credentials',
        apiKey: 'API Key',
        secretKey: 'Secret Key',
        keyringSecrets: 'Keyring Secrets',
        setSecret: 'Set Secret',
        getSecret: 'Get Secret',
        deleteSecret: 'Delete Secret',
    },
    help: {
        help: 'Help',
        documentation: 'Documentation',
        about: 'About',
        version: 'Version',
    }
};
const zhCN_Messages = {
    general: {
        goOn: 'Go-On',
        settings: '设置',
        start: '启动',
        stop: '停止',
        status: '状态',
        running: '运行中',
        stopped: '已停止',
    },
    runtime: {
        runtime: '运行时',
        runtimeSettings: '运行时设置',
        maintenanceInterval: '维护间隔(秒)',
        healthInterval: '健康检查间隔(秒)',
        shutdownDrain: '关闭清空时间(秒)',
        cacheSettings: '缓存设置',
        vectorSettings: '向量数据库设置',
        autotuneSettings: '自动优化设置',
    },
    execution: {
        executionSettings: '执行设置',
        startGoOn: '启动 Go-On',
        stopGoOn: '停止 Go-On',
        healthCheck: '健康检查',
        clearCache: '清空缓存',
    },
    workflow: {
        workflow: '工作流',
        phases: '阶段',
        agents: '代理',
        addPhase: '添加阶段',
        editPhase: '编辑阶段',
        deletePhase: '删除阶段',
    },
    configuration: {
        configuration: '配置',
        configPath: '配置文件路径',
        executablePath: '可执行文件路径',
        autoDownloadBinary: '自动下载二进制',
        releaseRepository: '发布仓库',
        releaseTag: '发布标签',
        autoStart: '自动启动',
        wizard: {
            title: 'Go-On 配置向导',
            subtitle: '先选择使用场景与推荐协议，再保存当前工作区运行时设置。',
            step1: '场景',
            step2: '协议',
            step3: '确认',
            next: '下一步',
            previous: '上一步',
            recommended: '推荐',
            protocolMode: '协议模式',
            localTitle: '本地开发',
            localDesc: '适合单机单用户的日常开发。',
            sharedTitle: '共享服务',
            sharedDesc: '适合多用户或多工具共享一个常驻后端。',
            editorTitle: '编辑器插件',
            editorDesc: '适合由编辑器以子进程方式拉起 go-on。',
            adaptiveDesc: '双栈能力与自适应路由。',
            acpStdioDesc: '面向编辑器子进程集成的 ACP over stdio。',
            acpHttpDesc: '面向共享常驻后端的 ACP over HTTP。',
            mcpStdioDesc: '当客户端明确要求时使用 MCP over stdio。',
            mcpHttpDesc: '面向 OpenAI 兼容 /v1 端点的 MCP over HTTP。'
        }
    },
    chat: {
        chat: '聊天',
        chatMaxHistory: '最大聊天历史',
        chatModel: '聊天模型',
        chatTemperature: '温度',
        chatTimeout: '超时(秒)',
    },
    buttons: {
        save: '保存',
        cancel: '取消',
        reset: '重置',
        apply: '应用',
        delete: '删除',
        edit: '编辑',
        add: '添加',
    },
    messages: {
        successfullySaved: '保存成功',
        errorSaving: '保存出错',
        unsavedChanges: '您有未保存的更改',
    },
    language: {
        language: '语言',
        simplifiedChinese: '简体中文',
        traditionalChinese: '繁体中文',
        english: '英文',
    },
    credentials: {
        credentials: '凭证',
        apiKey: 'API 密钥',
        secretKey: '密钥',
        keyringSecrets: '密钥环机密',
        setSecret: '设置机密',
        getSecret: '获取机密',
        deleteSecret: '删除机密',
    },
    help: {
        help: '帮助',
        documentation: '文档',
        about: '关于',
        version: '版本',
    }
};
const zhTW_Messages = {
    general: {
        goOn: 'Go-On',
        settings: '設定',
        start: '啟動',
        stop: '停止',
        status: '狀態',
        running: '執行中',
        stopped: '已停止',
    },
    runtime: {
        runtime: '運行時',
        runtimeSettings: '運行時設定',
        maintenanceInterval: '維護間隔(秒)',
        healthInterval: '健康檢查間隔(秒)',
        shutdownDrain: '關閉清空時間(秒)',
        cacheSettings: '快取設定',
        vectorSettings: '向量資料庫設定',
        autotuneSettings: '自動最佳化設定',
    },
    execution: {
        executionSettings: '執行設定',
        startGoOn: '啟動 Go-On',
        stopGoOn: '停止 Go-On',
        healthCheck: '健康檢查',
        clearCache: '清空快取',
    },
    workflow: {
        workflow: '工作流程',
        phases: '階段',
        agents: '代理',
        addPhase: '新增階段',
        editPhase: '編輯階段',
        deletePhase: '刪除階段',
    },
    configuration: {
        configuration: '設定',
        configPath: '設定檔案路徑',
        executablePath: '執行檔路徑',
        autoDownloadBinary: '自動下載二進位',
        releaseRepository: '發行儲存庫',
        releaseTag: '發行標籤',
        autoStart: '自動啟動',
        wizard: {
            title: 'Go-On 設定精靈',
            subtitle: '先選擇使用情境與建議協議，再保存目前工作區的運行時設定。',
            step1: '情境',
            step2: '協議',
            step3: '確認',
            next: '下一步',
            previous: '上一步',
            recommended: '推薦',
            protocolMode: '協議模式',
            localTitle: '本機開發',
            localDesc: '適合單機單使用者的日常開發。',
            sharedTitle: '共享服務',
            sharedDesc: '適合多使用者或多工具共用同一常駐後端。',
            editorTitle: '編輯器外掛',
            editorDesc: '適合由編輯器以子程序方式啟動 go-on。',
            adaptiveDesc: '雙棧能力與自適應路由。',
            acpStdioDesc: '面向編輯器子程序整合的 ACP over stdio。',
            acpHttpDesc: '面向共享常駐後端的 ACP over HTTP。',
            mcpStdioDesc: '當客戶端明確要求時使用 MCP over stdio。',
            mcpHttpDesc: '面向 OpenAI 相容 /v1 端點的 MCP over HTTP。'
        }
    },
    chat: {
        chat: '聊天',
        chatMaxHistory: '最大聊天記錄',
        chatModel: '聊天模型',
        chatTemperature: '溫度',
        chatTimeout: '逾時(秒)',
    },
    buttons: {
        save: '保存',
        cancel: '取消',
        reset: '重置',
        apply: '套用',
        delete: '刪除',
        edit: '編輯',
        add: '新增',
    },
    messages: {
        successfullySaved: '保存成功',
        errorSaving: '保存出錯',
        unsavedChanges: '您有未保存的變更',
    },
    language: {
        language: '語言',
        simplifiedChinese: '簡體中文',
        traditionalChinese: '繁體中文',
        english: '英文',
    },
    credentials: {
        credentials: '認證項目',
        apiKey: 'API 金鑰',
        secretKey: '秘密金鑰',
        keyringSecrets: '金鑰環秘密',
        setSecret: '設定秘密',
        getSecret: '取得秘密',
        deleteSecret: '刪除秘密',
    },
    help: {
        help: '說明',
        documentation: '文件',
        about: '關於',
        version: '版本',
    }
};
exports.i18n = I18nManager.getInstance();
//# sourceMappingURL=i18n.js.map