use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Hash, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhCn,
    ZhTw,
}

pub struct I18n {
    lang: Lang,
    strings: HashMap<&'static str, HashMap<Lang, &'static str>>,
}

impl I18n {
    pub fn new(lang: Lang) -> Self {
        let mut strings = HashMap::new();
        Self::load_all(&mut strings);
        I18n { lang, strings }
    }

    pub fn switch(&mut self, lang: Lang) {
        self.lang = lang;
    }

    pub fn t(&self, key: &str) -> Cow<'_, str> {
        self.strings
            .get(key)
            .and_then(|m| m.get(&self.lang))
            .copied()
            .map_or_else(|| Cow::Owned(key.to_string()), Cow::Borrowed)
    }

    fn load_all(m: &mut HashMap<&'static str, HashMap<Lang, &'static str>>) {
        // App
        m.insert(
            "app.title",
            tr!(en, "Go-On GUI", cn, "Go-On 图形界面", tw, "Go-On 圖形界面"),
        );
        m.insert("app.start", tr!(en, "Start", cn, "启动", tw, "啟動"));
        m.insert("app.stop", tr!(en, "Stop", cn, "停止", tw, "停止"));
        m.insert("app.restart", tr!(en, "Restart", cn, "重启", tw, "重啟"));
        m.insert("app.refresh", tr!(en, "Refresh", cn, "刷新", tw, "刷新"));
        m.insert(
            "app.running",
            tr!(en, "Running", cn, "运行中", tw, "運行中"),
        );
        m.insert(
            "app.stopped",
            tr!(en, "Stopped", cn, "已停止", tw, "已停止"),
        );
        m.insert(
            "app.connecting",
            tr!(en, "Connecting...", cn, "连接中...", tw, "連接中..."),
        );

        m.insert(
            "chat.exportTitle",
            tr!(en, "Chat Export", cn, "聊天导出", tw, "聊天匯出"),
        );
        m.insert(
            "chat.exportedAt",
            tr!(
                en,
                "Exported: {time}",
                cn,
                "导出时间: {time}",
                tw,
                "匯出時間: {time}"
            ),
        );
        m.insert("chat.exportRoleYou", tr!(en, "You", cn, "你", tw, "你"));
        m.insert(
            "chat.exportRoleAssistant",
            tr!(en, "Assistant", cn, "助手", tw, "助手"),
        );
        m.insert(
            "chat.exportModel",
            tr!(
                en,
                "Model: {model}",
                cn,
                "模型: {model}",
                tw,
                "模型: {model}"
            ),
        );
        m.insert(
            "chat.exportThinking",
            tr!(
                en,
                "Thinking: {thinking}",
                cn,
                "思考: {thinking}",
                tw,
                "思考: {thinking}"
            ),
        );
        m.insert(
            "chat.copyCode",
            tr!(en, "Copy code", cn, "复制代码", tw, "複製程式碼"),
        );
        m.insert(
            "chat.tokenSummary",
            tr!(
                en,
                "Tokens in:{input} out:{output} total:{total}",
                cn,
                "Token 输入:{input} 输出:{output} 总计:{total}",
                tw,
                "Token 輸入:{input} 輸出:{output} 總計:{total}"
            ),
        );
        m.insert(
            "chat.chatError",
            tr!(
                en,
                "Chat error: {message}",
                cn,
                "聊天错误: {message}",
                tw,
                "聊天錯誤: {message}"
            ),
        );
        // Tabs
        m.insert("tab.monitor", tr!(en, "Monitor", cn, "监控", tw, "監控"));
        m.insert("tab.chat", tr!(en, "Chat", cn, "对话", tw, "對話"));
        m.insert("tab.settings", tr!(en, "Settings", cn, "设置", tw, "設置"));
        m.insert("tab.skills", tr!(en, "Skills", cn, "技能", tw, "技能"));
        m.insert(
            "tab.workflow",
            tr!(en, "Workflow", cn, "工作流", tw, "工作流"),
        );
        m.insert(
            "tab.autotune",
            tr!(en, "AutoTune", cn, "自动调优", tw, "自動調優"),
        );
        m.insert("tab.security", tr!(en, "Security", cn, "安全", tw, "安全"));
        m.insert("tab.config", tr!(en, "Config", cn, "配置", tw, "配置"));
        m.insert(
            "tab.providers",
            tr!(en, "Providers", cn, "供应商", tw, "供應商"),
        );

        // Monitor
        m.insert(
            "monitor.health",
            tr!(en, "Backend Health", cn, "后端状态", tw, "後端狀態"),
        );
        m.insert(
            "monitor.rpm",
            tr!(en, "Requests/min", cn, "请求/分钟", tw, "請求/分鐘"),
        );
        m.insert(
            "monitor.success",
            tr!(en, "Success Rate", cn, "成功率", tw, "成功率"),
        );
        m.insert(
            "monitor.latency",
            tr!(en, "Avg Latency", cn, "平均延迟", tw, "平均延遲"),
        );
        m.insert(
            "monitor.healthy",
            tr!(en, "Healthy", cn, "健康", tw, "健康"),
        );
        m.insert(
            "monitor.unhealthy",
            tr!(en, "Unhealthy", cn, "不健康", tw, "不健康"),
        );
        m.insert(
            "monitor.offline",
            tr!(en, "Offline", cn, "离线", tw, "離線"),
        );
        m.insert(
            "monitor.providers",
            tr!(en, "AI Providers", cn, "AI 供应商", tw, "AI 供應商"),
        );
        m.insert("monitor.ready", tr!(en, "Ready", cn, "就绪", tw, "就緒"));
        m.insert(
            "monitor.notReady",
            tr!(en, "Not Ready", cn, "未就绪", tw, "未就緒"),
        );
        m.insert(
            "monitor.offlineHint",
            tr!(
                en,
                "⚠ Providers configured – verify the backend is running",
                cn,
                "⚠ 已配置 AI 供应商，请确认后端正在运行",
                tw,
                "⚠ 已配置 AI 供應商，請確認後端正在運行"
            ),
        );
        m.insert(
            "monitor.restart",
            tr!(en, "Restart Backend", cn, "重启后端", tw, "重啟後端"),
        );
        m.insert(
            "monitor.restarting",
            tr!(en, "Restarting...", cn, "重启中...", tw, "重啟中..."),
        );
        m.insert(
            "monitor.restartHint",
            tr!(
                en,
                "(backend will come back online shortly)",
                cn,
                "(后端即将重新上线)",
                tw,
                "(後端即將重新上線)"
            ),
        );

        // Chat
        m.insert("chat.title", tr!(en, "Chat", cn, "对话", tw, "對話"));
        m.insert("chat.phase", tr!(en, "Phase", cn, "阶段", tw, "階段"));
        m.insert("chat.mode", tr!(en, "Mode", cn, "模式", tw, "模式"));
        m.insert("chat.model", tr!(en, "Model", cn, "模型", tw, "模型"));
        m.insert("chat.search", tr!(en, "Search", cn, "搜索", tw, "搜尋"));
        m.insert(
            "chat.searchMessages",
            tr!(
                en,
                "Search messages...",
                cn,
                "搜索消息...",
                tw,
                "搜尋訊息..."
            ),
        );
        m.insert(
            "chat.searchSessions",
            tr!(
                en,
                "Search sessions...",
                cn,
                "搜索会话...",
                tw,
                "搜尋會話..."
            ),
        );
        m.insert(
            "chat.searchTemplates",
            tr!(
                en,
                "Search templates...",
                cn,
                "搜索模板...",
                tw,
                "搜尋模板..."
            ),
        );
        m.insert(
            "chat.chooseModels",
            tr!(en, "Choose Models", cn, "选择模型", tw, "選擇模型"),
        );
        m.insert(
            "chat.multiModelHint",
            tr!(
                en,
                "Select one or more models to run the same prompt in parallel.",
                cn,
                "选择一个或多个模型，并行运行同一条消息。",
                tw,
                "選擇一個或多個模型，並行執行同一條訊息。"
            ),
        );
        m.insert(
            "chat.multiModelEnabled",
            tr!(en, "Multi-model", cn, "多模型", tw, "多模型"),
        );
        m.insert(
            "chat.modelAutoOnly",
            tr!(en, "Auto Only", cn, "仅自动", tw, "僅自動"),
        );
        m.insert(
            "chat.promptTemplates",
            tr!(en, "Prompt Templates", cn, "提示模板", tw, "提示模板"),
        );
        m.insert(
            "chat.templateName",
            tr!(en, "Template name", cn, "模板名称", tw, "模板名稱"),
        );
        m.insert(
            "chat.templateCommand",
            tr!(en, "Slash command", cn, "快捷指令", tw, "快捷指令"),
        );
        m.insert(
            "chat.templateBody",
            tr!(en, "Template body", cn, "模板内容", tw, "模板內容"),
        );
        m.insert(
            "chat.templatePlaceholderHint",
            tr!(
                en,
                "Use {{input}} to inject text after the slash command.",
                cn,
                "使用 {{input}} 注入快捷指令后的附加文本。",
                tw,
                "使用 {{input}} 注入快捷指令後的附加文字。"
            ),
        );
        m.insert(
            "chat.templateInsert",
            tr!(en, "Insert", cn, "插入", tw, "插入"),
        );
        m.insert(
            "chat.templateSave",
            tr!(en, "Save Template", cn, "保存模板", tw, "儲存模板"),
        );
        m.insert(
            "chat.templateDelete",
            tr!(en, "Delete Template", cn, "删除模板", tw, "刪除模板"),
        );
        m.insert(
            "chat.templateNew",
            tr!(en, "New Template", cn, "新建模板", tw, "新增模板"),
        );
        m.insert(
            "chat.templateValidation",
            tr!(
                en,
                "Template name, slash command, and body are required.",
                cn,
                "模板名称、快捷指令和模板内容均为必填。",
                tw,
                "模板名稱、快捷指令和模板內容均為必填。"
            ),
        );
        m.insert(
            "chat.templateDuplicate",
            tr!(
                en,
                "Slash command already exists.",
                cn,
                "快捷指令已存在。",
                tw,
                "快捷指令已存在。"
            ),
        );
        m.insert("chat.close", tr!(en, "Close", cn, "关闭", tw, "關閉"));
        m.insert("chat.stop", tr!(en, "Stop", cn, "停止", tw, "停止"));
        m.insert("chat.retry", tr!(en, "Retry", cn, "重试", tw, "重試"));
        m.insert("chat.edit", tr!(en, "Edit", cn, "编辑", tw, "編輯"));
        m.insert("chat.delete", tr!(en, "Delete", cn, "删除", tw, "刪除"));
        m.insert("chat.save", tr!(en, "Save", cn, "保存", tw, "儲存"));
        m.insert("chat.cancel", tr!(en, "Cancel", cn, "取消", tw, "取消"));
        m.insert(
            "chat.tokens",
            tr!(en, "Tokens", cn, "Token 用量", tw, "Token 用量"),
        );
        m.insert(
            "chat.exportSuccess",
            tr!(
                en,
                "Exported session to {path}",
                cn,
                "会话已导出到 {path}",
                tw,
                "會話已匯出到 {path}"
            ),
        );
        m.insert(
            "chat.exportFailed",
            tr!(
                en,
                "Export failed: {error}",
                cn,
                "导出失败: {error}",
                tw,
                "匯出失敗: {error}"
            ),
        );
        m.insert(
            "chat.template.explain",
            tr!(en, "Explain Code", cn, "解释代码", tw, "解釋程式碼"),
        );
        m.insert("chat.template.explain.body", tr!(en, "Explain the following code in detail. Cover the control flow, key data structures, and any risks or tradeoffs.\n\n{{input}}", cn, "请详细解释下面的代码，说明控制流、关键数据结构，以及潜在风险或权衡。\n\n{{input}}", tw, "請詳細解釋下面的程式碼，說明控制流程、關鍵資料結構，以及潛在風險或權衡。\n\n{{input}}"));
        m.insert(
            "chat.template.test",
            tr!(en, "Write Test", cn, "编写测试", tw, "編寫測試"),
        );
        m.insert("chat.template.test.body", tr!(en, "Write focused automated tests for the following code or behavior. Prefer edge cases and failure paths.\n\n{{input}}", cn, "请为下面的代码或行为编写聚焦的自动化测试，优先覆盖边界条件和失败路径。\n\n{{input}}", tw, "請為下面的程式碼或行為編寫聚焦的自動化測試，優先覆蓋邊界條件和失敗路徑。\n\n{{input}}"));
        m.insert(
            "chat.template.debug",
            tr!(en, "Debug Issue", cn, "调试问题", tw, "除錯問題"),
        );
        m.insert("chat.template.debug.body", tr!(en, "Analyze the issue below. Identify the most likely root cause, the cheapest discriminating check, and the concrete fix.\n\n{{input}}", cn, "请分析下面的问题，给出最可能的根因、最便宜的鉴别检查，以及具体修复方案。\n\n{{input}}", tw, "請分析下面的問題，給出最可能的根因、最低成本的鑑別檢查，以及具體修復方案。\n\n{{input}}"));
        m.insert(
            "chat.template.refactor",
            tr!(en, "Refactor", cn, "重构", tw, "重構"),
        );
        m.insert("chat.template.refactor.body", tr!(en, "Refactor the following code while preserving behavior. Explain the before/after structure and why the new version is better.\n\n{{input}}", cn, "请在保持行为一致的前提下重构下面的代码，并说明重构前后结构及收益。\n\n{{input}}", tw, "請在保持行為一致的前提下重構下面的程式碼，並說明重構前後的結構與收益。\n\n{{input}}"));
        m.insert(
            "chat.template.summary",
            tr!(en, "Summarize", cn, "总结", tw, "總結"),
        );
        m.insert("chat.template.summary.body", tr!(en, "Summarize the current conversation into decisions, open questions, risks, and next steps.\n\n{{input}}", cn, "请将当前对话总结为决策、待确认问题、风险和下一步。\n\n{{input}}", tw, "請將目前對話總結為決策、待確認問題、風險與下一步。\n\n{{input}}"));
        m.insert(
            "chat.template.docs",
            tr!(en, "Write Docs", cn, "编写文档", tw, "撰寫文件"),
        );
        m.insert("chat.template.docs.body", tr!(en, "Write concise developer-facing documentation for the following code or feature. Include purpose, usage, constraints, and examples when helpful.\n\n{{input}}", cn, "请为下面的代码或功能编写简洁的开发者文档，包含用途、用法、约束和必要示例。\n\n{{input}}", tw, "請為下面的程式碼或功能撰寫簡潔的開發者文件，包含用途、用法、限制與必要範例。\n\n{{input}}"));

        // Phase options
        m.insert("phase.coding", tr!(en, "Coding", cn, "编码", tw, "編碼"));
        m.insert("phase.review", tr!(en, "Review", cn, "审查", tw, "審查"));
        m.insert("phase.debug", tr!(en, "Debug", cn, "调试", tw, "調試"));
        m.insert("phase.test", tr!(en, "Test", cn, "测试", tw, "測試"));
        m.insert("phase.deploy", tr!(en, "Deploy", cn, "部署", tw, "部署"));

        // Mode options
        m.insert("mode.ask", tr!(en, "💬 Ask", cn, "💬 提问", tw, "💬 提問"));
        m.insert(
            "mode.plan",
            tr!(en, "📋 Plan", cn, "📋 计划", tw, "📋 計劃"),
        );
        m.insert(
            "mode.edit",
            tr!(en, "✏️ Edit", cn, "✏️ 编辑", tw, "✏️ 編輯"),
        );
        m.insert(
            "mode.safeguard",
            tr!(en, "🛡️ Safeguard", cn, "🛡️ 保护", tw, "🛡️ 保護"),
        );
        m.insert(
            "mode.full_auto",
            tr!(en, "🤖 Full Auto", cn, "🤖 全自动", tw, "🤖 全自動"),
        );

        m.insert(
            "chat.input",
            tr!(
                en,
                "Type a message...",
                cn,
                "输入消息...",
                tw,
                "輸入消息..."
            ),
        );
        m.insert("chat.send", tr!(en, "Send", cn, "发送", tw, "發送"));
        m.insert(
            "chat.attach",
            tr!(en, "Attach File", cn, "附件", tw, "附件"),
        );
        m.insert(
            "chat.clear",
            tr!(en, "Clear Chat", cn, "清空对话", tw, "清空對話"),
        );
        m.insert("chat.export", tr!(en, "Export", cn, "导出", tw, "導出"));
        m.insert("chat.you", tr!(en, "You", cn, "你", tw, "你"));
        m.insert(
            "chat.assistant",
            tr!(en, "Assistant", cn, "助手", tw, "助手"),
        );
        m.insert("chat.system", tr!(en, "System", cn, "系统", tw, "系統"));
        m.insert(
            "chat.noMessages",
            tr!(
                en,
                "No messages yet. Start a conversation!",
                cn,
                "暂无消息，开始对话吧！",
                tw,
                "暫無消息，開始對話吧！"
            ),
        );
        m.insert(
            "chat.hint",
            tr!(
                en,
                "Type a message below to start chatting",
                cn,
                "在下方输入消息开始对话",
                tw,
                "在下方輸入消息開始對話"
            ),
        );
        m.insert("chat.copy", tr!(en, "Copy", cn, "复制", tw, "複製"));
        m.insert(
            "chat.thinkingLabel",
            tr!(
                en,
                "\u{1f9e0} Thinking",
                cn,
                "\u{1f9e0} 思考",
                tw,
                "\u{1f9e0} 思考"
            ),
        );
        m.insert(
            "chat.thinking",
            tr!(en, "AI is thinking", cn, "AI 思考中", tw, "AI 思考中"),
        );
        m.insert(
            "chat.newSession",
            tr!(en, "New session", cn, "新对话", tw, "新對話"),
        );
        m.insert(
            "chat.clearAttachments",
            tr!(en, "Clear attachments", cn, "清除附件", tw, "清除附件"),
        );
        m.insert(
            "chat.modelName",
            tr!(
                en,
                "DeepSeek Chat",
                cn,
                "DeepSeek 对话",
                tw,
                "DeepSeek 對話"
            ),
        );
        m.insert(
            "chat.imeHint",
            tr!(
                en,
                "IME issue? Use external editor",
                cn,
                "输入法有问题？点击 外部编辑器",
                tw,
                "輸入法有問題？點擊 外部編輯器"
            ),
        );
        m.insert(
            "chat.externalEditor",
            tr!(
                en,
                "Open external editor",
                cn,
                "打开外部编辑器",
                tw,
                "打開外部編輯器"
            ),
        );

        // Setup
        m.insert(
            "setup.title",
            tr!(
                en,
                "AI Provider Setup",
                cn,
                "AI 供应商配置",
                tw,
                "AI 供應商配置"
            ),
        );
        m.insert(
            "setup.hint",
            tr!(
                en,
                "No valid AI provider found. Configure API keys to continue.",
                cn,
                "未找到有效的 AI 供应商。请配置 API 密钥以继续。",
                tw,
                "未找到有效的 AI 供應商。請配置 API 密鑰以繼續。"
            ),
        );
        m.insert(
            "setup.provider",
            tr!(en, "Provider", cn, "供应商", tw, "供應商"),
        );
        m.insert(
            "setup.apiKey",
            tr!(en, "API Key", cn, "API 密钥", tw, "API 密鑰"),
        );
        m.insert("setup.model", tr!(en, "Model", cn, "模型", tw, "模型"));
        m.insert(
            "setup.save",
            tr!(en, "Save & Activate", cn, "保存并激活", tw, "保存並激活"),
        );
        m.insert("setup.skip", tr!(en, "Skip", cn, "跳过", tw, "跳過"));
        m.insert(
            "setup.validating",
            tr!(en, "Validating...", cn, "验证中...", tw, "驗證中..."),
        );
        m.insert(
            "setup.success",
            tr!(
                en,
                "Provider configured successfully!",
                cn,
                "供应商配置成功！",
                tw,
                "供應商配置成功！"
            ),
        );
        m.insert(
            "setup.error",
            tr!(
                en,
                "Validation failed. Check your API key.",
                cn,
                "验证失败，请检查 API 密钥。",
                tw,
                "驗證失敗，請檢查 API 密鑰。"
            ),
        );
        m.insert(
            "setup.noConfig",
            tr!(
                en,
                "No config file found. Creating default...",
                cn,
                "未找到配置文件，正在创建默认配置...",
                tw,
                "未找到配置文件，正在創建默認配置..."
            ),
        );

        // Skills validation errors
        m.insert(
            "skills.create.errorName",
            tr!(
                en,
                "Name is required.",
                cn,
                "名称是必填项。",
                tw,
                "名稱是必填項。"
            ),
        );
        m.insert(
            "skills.create.errorPrompt",
            tr!(
                en,
                "Prompt is required.",
                cn,
                "提示是必填项。",
                tw,
                "提示是必填項。"
            ),
        );
        m.insert(
            "skills.import.errorUrl",
            tr!(
                en,
                "URL is required.",
                cn,
                "URL 是必填项。",
                tw,
                "URL 是必填項。"
            ),
        );
        m.insert(
            "skills.import.unnamed",
            tr!(en, "imported", cn, "已导入", tw, "已導入"),
        );
        m.insert(
            "skills.import.importedFrom",
            tr!(en, "Imported from {}", cn, "从 {} 导入", tw, "從 {} 導入"),
        );

        // Settings (feature toggles)
        m.insert(
            "settings.title",
            tr!(en, "Feature Settings", cn, "功能设置", tw, "功能設置"),
        );
        m.insert("settings.hint",       tr!(en,"Enable or disable feature modules. Disabled modules will be hidden from the main interface.",cn,"启用或禁用功能模块。禁用的模块将从主界面隐藏。",tw,"啟用或禁用功能模塊。禁用的模塊將從主界面隱藏。"));
        m.insert(
            "settings.language",
            tr!(en, "Language", cn, "语言", tw, "語言"),
        );

        // Status bar
        m.insert(
            "status.connected",
            tr!(en, "Connected", cn, "已连接", tw, "已連接"),
        );
        m.insert(
            "status.disconnected",
            tr!(en, "Disconnected", cn, "已断开", tw, "已斷開"),
        );
        m.insert("status.error", tr!(en, "Error", cn, "错误", tw, "錯誤"));

        // Skills
        m.insert(
            "skills.none",
            tr!(
                en,
                "No skills configured yet. Create or import one to get started.",
                cn,
                "暂无技能。创建或导入一个技能以开始使用。",
                tw,
                "暫無技能。創建或導入一個技能以開始使用。"
            ),
        );
        m.insert(
            "skills.create.title",
            tr!(en, "Create New Skill", cn, "创建新技能", tw, "創建新技能"),
        );
        m.insert(
            "skills.create.name",
            tr!(en, "Name", cn, "名称", tw, "名稱"),
        );
        m.insert(
            "skills.create.desc",
            tr!(en, "Description", cn, "描述", tw, "描述"),
        );
        m.insert(
            "skills.create.prompt",
            tr!(en, "Prompt Template", cn, "提示模板", tw, "提示模板"),
        );
        m.insert(
            "skills.create.schema",
            tr!(
                en,
                "Input Schema (JSON)",
                cn,
                "输入模式 (JSON)",
                tw,
                "輸入模式 (JSON)"
            ),
        );
        m.insert(
            "skills.create.save",
            tr!(en, "Save Skill", cn, "保存技能", tw, "保存技能"),
        );
        m.insert(
            "skills.create.success",
            tr!(
                en,
                "Skill created successfully!",
                cn,
                "技能创建成功！",
                tw,
                "技能創建成功！"
            ),
        );
        m.insert(
            "skills.create.error",
            tr!(
                en,
                "Failed to create skill.",
                cn,
                "创建技能失败。",
                tw,
                "創建技能失敗。"
            ),
        );
        m.insert(
            "skills.import.title",
            tr!(
                en,
                "Import Skill from URL",
                cn,
                "从 URL 导入技能",
                tw,
                "從 URL 導入技能"
            ),
        );
        m.insert(
            "skills.import.placeholder",
            tr!(
                en,
                "Enter skill URL...",
                cn,
                "输入技能 URL...",
                tw,
                "輸入技能 URL..."
            ),
        );
        m.insert(
            "skills.import.btn",
            tr!(en, "Import", cn, "导入", tw, "導入"),
        );
        m.insert(
            "skills.import.success",
            tr!(
                en,
                "Skill imported successfully!",
                cn,
                "技能导入成功！",
                tw,
                "技能導入成功！"
            ),
        );
        m.insert(
            "skills.import.error",
            tr!(
                en,
                "Failed to import skill.",
                cn,
                "导入技能失败。",
                tw,
                "導入技能失敗。"
            ),
        );
        m.insert(
            "skills.loading",
            tr!(
                en,
                "Loading skills...",
                cn,
                "加载技能中...",
                tw,
                "加載技能中..."
            ),
        );

        // Providers page
        m.insert(
            "providers.title",
            tr!(en, "Providers", cn, "提供商", tw, "提供商"),
        );
        m.insert(
            "providers.add_new",
            tr!(en, "Add Provider", cn, "添加提供商", tw, "添加提供商"),
        );
        m.insert(
            "providers.saved",
            tr!(
                en,
                "Saved Providers",
                cn,
                "已保存的提供商",
                tw,
                "已保存的提供商"
            ),
        );
        m.insert("providers.name", tr!(en, "Name", cn, "名称", tw, "名稱"));
        m.insert(
            "providers.api_key",
            tr!(en, "API Key", cn, "API 密钥", tw, "API 密鑰"),
        );
        m.insert("providers.model", tr!(en, "Model", cn, "模型", tw, "模型"));
        m.insert("providers.add", tr!(en, "Add", cn, "添加", tw, "添加"));
        m.insert(
            "providers.update_key",
            tr!(en, "Update Key", cn, "更新密钥", tw, "更新密鑰"),
        );
        m.insert(
            "providers.save_key",
            tr!(en, "Save", cn, "保存", tw, "保存"),
        );
        m.insert(
            "providers.cancel",
            tr!(en, "Cancel", cn, "取消", tw, "取消"),
        );
        m.insert(
            "providers.delete",
            tr!(en, "Delete", cn, "删除", tw, "刪除"),
        );
        m.insert(
            "providers.confirm_delete",
            tr!(en, "Confirm Delete", cn, "确认删除", tw, "確認刪除"),
        );
        m.insert(
            "providers.push",
            tr!(en, "Push to Backend", cn, "推送到后端", tw, "推送到後端"),
        );
        m.insert(
            "providers.validated",
            tr!(en, "Validated", cn, "已验证", tw, "已驗證"),
        );
        m.insert(
            "providers.key_preview",
            tr!(en, "Key:", cn, "密钥:", tw, "密鑰:"),
        );
        m.insert(
            "providers.already_exists",
            tr!(
                en,
                "already exists, use Update",
                cn,
                "已存在，请使用更新",
                tw,
                "已存在，請使用更新"
            ),
        );
        m.insert(
            "providers.save_failed",
            tr!(
                en,
                "Failed to save to system keyring:",
                cn,
                "保存到系统密钥环失败:",
                tw,
                "保存到系統密鑰環失敗:"
            ),
        );
        m.insert(
            "providers.keyring_ok",
            tr!(
                en,
                "saved to system keyring",
                cn,
                "已保存到系统密钥环",
                tw,
                "已保存到系統密鑰環"
            ),
        );
        m.insert(
            "providers.copilot_hint",
            tr!(
                en,
                "(Copilot uses GitHub token, not API key)",
                cn,
                "(Copilot 使用 GitHub Token，不是 API 密钥)",
                tw,
                "(Copilot 使用 GitHub Token，不是 API 密鑰)"
            ),
        );
        m.insert(
            "providers.enter_new_key",
            tr!(
                en,
                "Enter new key for",
                cn,
                "输入新的密钥",
                tw,
                "輸入新的密鑰"
            ),
        );
        m.insert(
            "providers.provider",
            tr!(en, "Provider:", cn, "提供商:", tw, "提供商:"),
        );
        m.insert("providers.auto", tr!(en, "Auto", cn, "自动", tw, "自動"));
        m.insert(
            "providers.added",
            tr!(en, "added.", cn, "已添加。", tw, "已添加。"),
        );
        m.insert(
            "providers.updated",
            tr!(en, "updated.", cn, "已更新。", tw, "已更新。"),
        );
        m.insert(
            "providers.configured",
            tr!(
                en,
                "configured on backend.",
                cn,
                "已在后端配置。",
                tw,
                "已在後端配置。"
            ),
        );
        m.insert(
            "providers.push_failed",
            tr!(
                en,
                "Provider push failed:",
                cn,
                "推送提供商失败:",
                tw,
                "推送提供商失敗:"
            ),
        );
        m.insert(
            "providers.click_delete_again",
            tr!(
                en,
                "Click delete again to remove",
                cn,
                "再次点击删除以移除",
                tw,
                "再次點擊刪除以移除"
            ),
        );
        m.insert(
            "providers.removed",
            tr!(
                en,
                "Provider removed.",
                cn,
                "提供商已移除。",
                tw,
                "提供商已移除。"
            ),
        );

        // Theme
        m.insert("theme.title", tr!(en, "Theme", cn, "主题", tw, "主題"));
        m.insert("theme.minimal", tr!(en, "Minimal", cn, "简约", tw, "簡約"));
        m.insert("theme.guofeng", tr!(en, "GuoFeng", cn, "国风", tw, "國風"));
        m.insert("theme.wuxia", tr!(en, "Wuxia", cn, "武侠", tw, "武俠"));
        m.insert(
            "theme.shanshui",
            tr!(en, "ShanShui", cn, "山水", tw, "山水"),
        );
        m.insert(
            "theme.hellokitty",
            tr!(en, "Hello Kitty", cn, "Hello Kitty", tw, "Hello Kitty"),
        );

        // Time format
        m.insert(
            "time.secondsAgo",
            tr!(en, "{}s ago", cn, "{}秒前", tw, "{}秒前"),
        );
        m.insert(
            "time.minutesAgo",
            tr!(en, "{}m ago", cn, "{}分钟前", tw, "{}分鐘前"),
        );
        m.insert(
            "time.hoursAgo",
            tr!(en, "{}h ago", cn, "{}小时前", tw, "{}小時前"),
        );
        m.insert(
            "time.daysAgo",
            tr!(en, "{}d ago", cn, "{}天前", tw, "{}天前"),
        );

        // Settings: Backend URL
        m.insert(
            "settings.backendUrl",
            tr!(en, "Backend URL", cn, "后端地址", tw, "後端地址"),
        );
        m.insert(
            "settings.backendUrlHint",
            tr!(
                en,
                "URL of the Go-On backend server",
                cn,
                "Go-On 后端服务器地址",
                tw,
                "Go-On 後端服務器地址"
            ),
        );

        // Language names (for settings display)
        m.insert("lang.en", tr!(en, "English", cn, "English", tw, "English"));
        m.insert(
            "lang.zhCn",
            tr!(en, "简体中文", cn, "简体中文", tw, "简体中文"),
        );
        m.insert(
            "lang.zhTw",
            tr!(en, "繁體中文", cn, "繁體中文", tw, "繁體中文"),
        );

        // Toast
        m.insert(
            "toast.serviceRestarted",
            tr!(
                en,
                "Service will restart to apply changes.",
                cn,
                "服务将重启以应用更改。",
                tw,
                "服務將重啟以應用更改。"
            ),
        );

        // Feature descriptions
        m.insert(
            "feature.workflow.desc",
            tr!(
                en,
                "Multi-step workflow orchestration for creating, managing, and running\nAI task pipelines with reusable steps.",
                cn,
                "多步骤工作流编排，可用于创建、管理和执行\n可复用的 AI 任务流水线。",
                tw,
                "多步驟工作流編排，可用於創建、管理和執行\n可復用的 AI 任務流水線。"
            ),
        );
        m.insert(
            "feature.autotune.desc",
            tr!(
                en,
                "Automatic model tuning controls for prompt generation behavior\nand output quality/cost trade-offs.",
                cn,
                "自动模型调优控制，用于调节生成行为以及\n质量/成本平衡。",
                tw,
                "自動模型調優控制，用於調節生成行為以及\n品質/成本平衡。"
            ),
        );
        m.insert(
            "feature.security.desc",
            tr!(
                en,
                "Security controls for confirmation gates, UI redaction,\nand runtime safety behavior.",
                cn,
                "安全控制项：确认门禁、界面脱敏、\n运行时安全行为。",
                tw,
                "安全控制項：確認門禁、介面脫敏、\n運行時安全行為。"
            ),
        );
        // Provider display names
        m.insert(
            "provider.openai",
            tr!(en, "OpenAI", cn, "OpenAI", tw, "OpenAI"),
        );
        m.insert(
            "provider.openai_compatible",
            tr!(
                en,
                "OpenAI Compatible",
                cn,
                "OpenAI 兼容",
                tw,
                "OpenAI 兼容"
            ),
        );
        m.insert(
            "provider.anthropic",
            tr!(en, "Anthropic", cn, "Anthropic", tw, "Anthropic"),
        );
        m.insert(
            "provider.cohere",
            tr!(en, "Cohere", cn, "Cohere", tw, "Cohere"),
        );
        m.insert(
            "provider.deepseek",
            tr!(en, "DeepSeek", cn, "深度求索", tw, "深度求索"),
        );
        m.insert(
            "provider.wenxin",
            tr!(en, "文心一言", cn, "文心一言", tw, "文心一言"),
        );
        m.insert("provider.qianfan", tr!(en, "千帆", cn, "千帆", tw, "千帆"));
        m.insert(
            "provider.qwen",
            tr!(en, "通义千问", cn, "通义千问", tw, "通義千問"),
        );
        m.insert(
            "provider.glm",
            tr!(en, "智谱 GLM", cn, "智谱 GLM", tw, "智譜 GLM"),
        );
        m.insert(
            "provider.yi",
            tr!(en, "零一万物", cn, "零一万物", tw, "零一萬物"),
        );
        m.insert(
            "provider.hunyuan",
            tr!(en, "腾讯混元", cn, "腾讯混元", tw, "騰訊混元"),
        );
        m.insert("provider.doubao", tr!(en, "豆包", cn, "豆包", tw, "豆包"));
        m.insert("provider.facewall", tr!(en, "面壁", cn, "面壁", tw, "面壁"));
        m.insert(
            "provider.langboat",
            tr!(en, "出门问问", cn, "出门问问", tw, "出門問問"),
        );
        m.insert("provider.skywork", tr!(en, "天工", cn, "天工", tw, "天工"));
        m.insert(
            "provider.stepfun",
            tr!(en, "阶跃星辰", cn, "阶跃星辰", tw, "階躍星辰"),
        );
        m.insert("provider.xihu", tr!(en, "西湖", cn, "西湖", tw, "西湖"));
        m.insert(
            "provider.moonshot",
            tr!(en, "Moonshot", cn, "月之暗面", tw, "月之暗面"),
        );
        m.insert(
            "provider.minimax",
            tr!(en, "MiniMax", cn, "MiniMax", tw, "MiniMax"),
        );
        m.insert(
            "provider.ai21",
            tr!(en, "AI21 Labs", cn, "AI21 Labs", tw, "AI21 Labs"),
        );
        m.insert(
            "provider.aleph",
            tr!(en, "Aleph Alpha", cn, "Aleph Alpha", tw, "Aleph Alpha"),
        );
        m.insert(
            "provider.copilot",
            tr!(
                en,
                "GitHub Copilot",
                cn,
                "GitHub Copilot",
                tw,
                "GitHub Copilot"
            ),
        );
        m.insert(
            "provider.deepquest",
            tr!(en, "DeepQuest", cn, "DeepQuest", tw, "DeepQuest"),
        );
        m.insert(
            "provider.fireworks",
            tr!(en, "Fireworks", cn, "Fireworks", tw, "Fireworks"),
        );
        m.insert(
            "provider.gemini",
            tr!(en, "Gemini", cn, "Gemini", tw, "Gemini"),
        );
        m.insert("provider.groq", tr!(en, "Groq", cn, "Groq", tw, "Groq"));
        m.insert("provider.llama", tr!(en, "Llama", cn, "Llama", tw, "Llama"));
        m.insert(
            "provider.loopai",
            tr!(en, "Loop AI", cn, "Loop AI", tw, "Loop AI"),
        );
        m.insert(
            "provider.mistral",
            tr!(en, "Mistral", cn, "Mistral", tw, "Mistral"),
        );
        m.insert(
            "provider.nim",
            tr!(en, "NVIDIA NIM", cn, "NVIDIA NIM", tw, "NVIDIA NIM"),
        );
        m.insert(
            "provider.perplexity",
            tr!(en, "Perplexity", cn, "Perplexity", tw, "Perplexity"),
        );
        m.insert(
            "provider.replicate",
            tr!(en, "Replicate", cn, "Replicate", tw, "Replicate"),
        );
        m.insert(
            "provider.titan",
            tr!(en, "Amazon Titan", cn, "Amazon Titan", tw, "Amazon Titan"),
        );
        m.insert(
            "provider.together",
            tr!(en, "Together", cn, "Together", tw, "Together"),
        );

        m.insert(
            "feature.config.desc",
            tr!(
                en,
                "Advanced configuration management with live JSON editing\nand immediate persistence.",
                cn,
                "高级配置管理：支持 JSON 实时编辑\n并立即持久化。",
                tw,
                "高級配置管理：支持 JSON 即時編輯\n並立即持久化。"
            ),
        );
        m.insert(
            "feature.providers.desc",
            tr!(
                en,
                "Provider management for adding/updating credentials,\nmodel selection, and backend push.",
                cn,
                "供应商管理：支持新增/更新凭据、\n模型选择与推送到后端。",
                tw,
                "供應商管理：支持新增/更新憑據、\n模型選擇與推送到後端。"
            ),
        );
    }
}

macro_rules! tr {
    ($_en:expr, $en_val:expr, $_cn:expr, $cn_val:expr, $_tw:expr, $tw_val:expr) => {{
        let mut m = std::collections::HashMap::new();
        m.insert($crate::i18n::Lang::En, $en_val);
        m.insert($crate::i18n::Lang::ZhCn, $cn_val);
        m.insert($crate::i18n::Lang::ZhTw, $tw_val);
        m
    }};
}
pub(crate) use tr;
