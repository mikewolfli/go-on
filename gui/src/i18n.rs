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
        m.insert("tab.about", tr!(en, "About", cn, "关于", tw, "關於"));
        m.insert(
            "app.unknownTab",
            tr!(
                en,
                "Unknown tab id.",
                cn,
                "未知标签页 ID。",
                tw,
                "未知分頁 ID。"
            ),
        );
        m.insert(
            "app.backendRequired",
            tr!(
                en,
                "Backend not connected",
                cn,
                "后端未连接",
                tw,
                "後端未連接"
            ),
        );
        m.insert(
            "app.backendRequiredHint",
            tr!(
                en,
                "Wait for backend to connect, or check Monitor / Settings to adjust.",
                cn,
                "请等待后端连接，或检查监控/设置页面进行调整。",
                tw,
                "請等待後端連接，或檢查監控/設置頁面進行調整。"
            ),
        );
        m.insert(
            "common.copyButton",
            tr!(en, "📋 Copy", cn, "📋 复制", tw, "📋 複製"),
        );
        m.insert(
            "common.apiKeyPlaceholder",
            tr!(en, "sk-...", cn, "sk-...", tw, "sk-..."),
        );

        // About
        m.insert(
            "about.title",
            tr!(en, "About This GUI", cn, "关于此 GUI", tw, "關於此 GUI"),
        );
        m.insert(
            "about.subtitle",
            tr!(
                en,
                "Current GUI and backend runtime information",
                cn,
                "当前 GUI 与后端运行信息",
                tw,
                "當前 GUI 與後端運行資訊"
            ),
        );
        m.insert(
            "about.guiVersion",
            tr!(en, "GUI Version", cn, "GUI 版本", tw, "GUI 版本"),
        );
        m.insert(
            "about.backendStatus",
            tr!(en, "Backend Status", cn, "后端状态", tw, "後端狀態"),
        );
        m.insert(
            "about.backendVersion",
            tr!(en, "Backend Version", cn, "后端版本", tw, "後端版本"),
        );
        m.insert(
            "about.backendBuild",
            tr!(en, "Backend Build", cn, "后端构建", tw, "後端構建"),
        );
        m.insert(
            "about.backendPid",
            tr!(en, "Backend PID", cn, "后端 PID", tw, "後端 PID"),
        );
        m.insert("about.unknown", tr!(en, "unknown", cn, "未知", tw, "未知"));
        m.insert(
            "about.external",
            tr!(en, "external", cn, "外部进程", tw, "外部進程"),
        );
        m.insert(
            "about.improvedTitle",
            tr!(en, "Improved", cn, "已改进", tw, "已改進"),
        );
        m.insert(
            "about.improved.monitor",
            tr!(
                en,
                "Monitor data path stability and structured summaries",
                cn,
                "监控数据链路稳定性与结构化摘要",
                tw,
                "監控資料鏈路穩定性與結構化摘要"
            ),
        );
        m.insert(
            "about.improved.workflow",
            tr!(
                en,
                "Workflow run center auto-polling and state-aware actions",
                cn,
                "工作流运行中心自动轮询与状态感知操作",
                tw,
                "工作流運行中心自動輪詢與狀態感知操作"
            ),
        );
        m.insert(
            "about.improved.providers",
            tr!(
                en,
                "Provider capability rendering and ops diagnostics",
                cn,
                "供应商能力渲染与运维诊断",
                tw,
                "供應商能力渲染與運維診斷"
            ),
        );
        m.insert(
            "about.improved.skills",
            tr!(
                en,
                "Skill version lifecycle and rollback consistency",
                cn,
                "技能版本生命周期与回滚一致性",
                tw,
                "技能版本生命週期與回滾一致性"
            ),
        );
        m.insert(
            "about.improved.i18n",
            tr!(
                en,
                "Localized operational texts for better usability",
                cn,
                "运维文案本地化提升易用性",
                tw,
                "運維文案本地化提升易用性"
            ),
        );
        m.insert(
            "about.backendStatus",
            tr!(en, "Backend Status", cn, "后端状态", tw, "後端狀態"),
        );
        m.insert(
            "about.backendVersion",
            tr!(en, "Backend Version", cn, "后端版本", tw, "後端版本"),
        );
        m.insert(
            "about.backendBuild",
            tr!(en, "Backend Build", cn, "后端构建", tw, "後端構建"),
        );
        m.insert(
            "about.backendPid",
            tr!(en, "Backend PID", cn, "后端进程 PID", tw, "後端進程 PID"),
        );
        m.insert(
            "about.guiVersion",
            tr!(en, "GUI Version", cn, "GUI 版本", tw, "GUI 版本"),
        );
        m.insert(
            "about.improvedTitle",
            tr!(
                en,
                "Improvements in this version",
                cn,
                "本版本改进",
                tw,
                "本版本改進"
            ),
        );
        m.insert(
            "app.unknownTab",
            tr!(en, "Unknown tab", cn, "未知标签页", tw, "未知標籤頁"),
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
        m.insert(
            "monitor.filterProviders",
            tr!(
                en,
                "Filter providers…",
                cn,
                "筛选供应商…",
                tw,
                "篩選供應商…"
            ),
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
            "chat.clearAll",
            tr!(
                en,
                "Clear all messages",
                cn,
                "清除全部消息",
                tw,
                "清除全部訊息"
            ),
        );
        m.insert(
            "chat.clear",
            tr!(en, "Clear Chat", cn, "清空对话", tw, "清空對話"),
        );
        m.insert(
            "chat.cannotDeleteLastSession",
            tr!(
                en,
                "Cannot delete the last session.",
                cn,
                "无法删除最后一个会话。",
                tw,
                "無法刪除最後一個會話。"
            ),
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
        m.insert("chat.showing", tr!(en, "Showing", cn, "显示", tw, "顯示"));
        m.insert(
            "chat.messages",
            tr!(en, "messages", cn, "条消息", tw, "條消息"),
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
            "chat.openWorkspace",
            tr!(en, "Open workspace", cn, "打开工作目录", tw, "打開工作目錄"),
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
        m.insert(
            "chat.sendShortcutHint",
            tr!(
                en,
                "Enter send, Shift+Enter newline",
                cn,
                "Enter 发送，Shift+Enter 换行",
                tw,
                "Enter 發送，Shift+Enter 換行"
            ),
        );
        m.insert(
            "chat.sendShortcutHintLinux",
            tr!(
                en,
                "Ctrl+Enter send (IME-safe), Enter newline",
                cn,
                "Ctrl+Enter 发送（输入法安全），Enter 换行",
                tw,
                "Ctrl+Enter 發送（輸入法安全），Enter 換行"
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
        m.insert("settings.theme", tr!(en, "Theme", cn, "主题", tw, "主題"));

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
            "skills.create.loading",
            tr!(en, "Creating...", cn, "创建中...", tw, "創建中..."),
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
            "skills.import.loading",
            tr!(en, "Importing...", cn, "导入中...", tw, "導入中..."),
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
            "providers.auto_push_hint",
            tr!(
                en,
                "💡 Key will be automatically pushed to backend after saving.",
                cn,
                "💡 保存后将自动推送到后端。",
                tw,
                "💡 保存後將自動推送到後端。"
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
            "providers.push_success",
            tr!(
                en,
                "pushed to backend successfully.",
                cn,
                "已成功推送到后端。",
                tw,
                "已成功推送到後端。"
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
        m.insert(
            "theme.serveThePeople",
            tr!(en, "Serve The People", cn, "为人民服务", tw, "為人民服務"),
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
        m.insert(
            "settings.backendUrlPlaceholder",
            tr!(
                en,
                "http://127.0.0.1:8090",
                cn,
                "http://127.0.0.1:8090",
                tw,
                "http://127.0.0.1:8090"
            ),
        );
        m.insert(
            "settings.section.core",
            tr!(en, "🔷 Core Features", cn, "🔷 核心功能", tw, "🔷 核心功能"),
        );
        m.insert(
            "settings.section.advanced",
            tr!(
                en,
                "⚡ Advanced Features",
                cn,
                "⚡ 高级功能",
                tw,
                "⚡ 進階功能"
            ),
        );
        m.insert(
            "settings.section.system",
            tr!(
                en,
                "⚙️ System Settings",
                cn,
                "⚙️ 系统设置",
                tw,
                "⚙️ 系統設定"
            ),
        );
        m.insert(
            "settings.section.enterprise",
            tr!(
                en,
                "🏢 Enterprise Settings",
                cn,
                "🏢 企业设置",
                tw,
                "🏢 企業設定"
            ),
        );
        m.insert(
            "settings.section.backend",
            tr!(en, "🔗 Backend URL", cn, "🔗 后端地址", tw, "🔗 後端位址"),
        );
        m.insert(
            "settings.section.language",
            tr!(en, "🌐 Language", cn, "🌐 语言", tw, "🌐 語言"),
        );
        m.insert(
            "settings.section.theme",
            tr!(en, "🎨 Theme", cn, "🎨 主题", tw, "🎨 主題"),
        );
        m.insert("common.close", tr!(en, "Close", cn, "关闭", tw, "關閉"));
        m.insert(
            "settings.feature.workflowRunCenter",
            tr!(
                en,
                "Workflow Run Center",
                cn,
                "工作流运行中心",
                tw,
                "工作流運行中心"
            ),
        );
        m.insert(
            "settings.feature.autotuneChainInjection",
            tr!(
                en,
                "AutoTune Chain Injection",
                cn,
                "AutoTune 链路注入",
                tw,
                "AutoTune 鏈路注入"
            ),
        );
        m.insert(
            "settings.feature.skillsLifecycle",
            tr!(
                en,
                "Skills Lifecycle",
                cn,
                "技能生命周期",
                tw,
                "技能生命週期"
            ),
        );
        m.insert(
            "settings.feature.providersOps",
            tr!(en, "Providers Ops", cn, "提供商运维", tw, "提供商運維"),
        );
        m.insert(
            "settings.feature.monitorHistoryAlerts",
            tr!(
                en,
                "Monitor History Alerts",
                cn,
                "监控历史告警",
                tw,
                "監控歷史告警"
            ),
        );
        m.insert(
            "settings.feature.configSafeMode",
            tr!(
                en,
                "Config Safe Mode",
                cn,
                "配置安全模式",
                tw,
                "配置安全模式"
            ),
        );
        m.insert(
            "settings.feature.setupEnterprise",
            tr!(en, "Setup Enterprise", cn, "企业化配置", tw, "企業化配置"),
        );
        m.insert(
            "settings.enterprise.title",
            tr!(en, "Enterprise Controls", cn, "企业控制", tw, "企業控制"),
        );
        m.insert(
            "settings.enterprise.environment",
            tr!(en, "Environment", cn, "环境", tw, "環境"),
        );
        m.insert(
            "settings.enterprise.environmentUrl",
            tr!(
                en,
                "Environment Backend URL",
                cn,
                "环境后端地址",
                tw,
                "環境後端地址"
            ),
        );
        m.insert(
            "settings.enterprise.secretSource",
            tr!(en, "Secret Source", cn, "密钥来源", tw, "密鑰來源"),
        );
        m.insert(
            "settings.enterprise.exportPath",
            tr!(en, "Export Path", cn, "导出路径", tw, "導出路徑"),
        );
        m.insert(
            "settings.enterprise.importPath",
            tr!(en, "Import Path", cn, "导入路径", tw, "導入路徑"),
        );
        m.insert(
            "settings.enterprise.exportMasked",
            tr!(en, "Export Masked", cn, "导出脱敏配置", tw, "導出脫敏配置"),
        );
        m.insert(
            "settings.enterprise.exportFull",
            tr!(en, "Export Full", cn, "导出完整配置", tw, "導出完整配置"),
        );
        m.insert(
            "settings.enterprise.importConfig",
            tr!(en, "Import Config", cn, "导入配置", tw, "導入配置"),
        );
        m.insert(
            "settings.enterprise.syncCurrent",
            tr!(
                en,
                "Sync Current URL",
                cn,
                "同步当前地址",
                tw,
                "同步當前地址"
            ),
        );
        m.insert(
            "settings.enterprise.hint",
            tr!(en, "Use named backend environments and import/export config packages for controlled rollout.", cn, "使用命名环境和配置包导入导出，便于受控发布与切换。", tw, "使用命名環境和配置包導入導出，便於受控發布與切換。"),
        );

        // ── UI Stability keys ──────────────────────────────
        m.insert(
            "settings.uiStability.title",
            tr!(
                en,
                "UI Stability Settings",
                cn,
                "界面防抖设置",
                tw,
                "界面防抖設置"
            ),
        );
        m.insert(
            "settings.uiStability.hint",
            tr!(
                en,
                "Adjust repaint batching and cadence to reduce periodic shaking.",
                cn,
                "调整重绘批处理与节奏以减少周期性抖动。",
                tw,
                "調整重繪批處理與節奏以減少週期性抖動。"
            ),
        );
        m.insert(
            "settings.uiStability.preset",
            tr!(en, "Preset", cn, "预设", tw, "預設"),
        );
        m.insert(
            "settings.uiStability.preset.balanced",
            tr!(en, "Balanced", cn, "平衡", tw, "平衡"),
        );
        m.insert(
            "settings.uiStability.preset.stable",
            tr!(en, "Stable", cn, "稳态优先", tw, "穩態優先"),
        );
        m.insert(
            "settings.uiStability.preset.lowend",
            tr!(en, "Low-end Machine", cn, "低性能机器", tw, "低性能機器"),
        );
        m.insert(
            "settings.uiStability.preset.lowlatency",
            tr!(en, "Low Latency", cn, "低延迟", tw, "低延遲"),
        );
        m.insert(
            "settings.uiStability.preset.custom",
            tr!(en, "Custom", cn, "自定义", tw, "自定義"),
        );
        m.insert(
            "settings.uiStability.backendRefreshInterval",
            tr!(
                en,
                "Backend refresh interval",
                cn,
                "后端刷新间隔",
                tw,
                "後端刷新間隔"
            ),
        );
        m.insert(
            "settings.uiStability.backendCommitDebounce",
            tr!(
                en,
                "Backend UI commit debounce",
                cn,
                "后端 UI 提交去抖",
                tw,
                "後端 UI 提交去抖"
            ),
        );
        m.insert(
            "settings.uiStability.disconnectDebounce",
            tr!(
                en,
                "Disconnect debounce samples",
                cn,
                "断连去抖采样数",
                tw,
                "斷連去抖採樣數"
            ),
        );
        m.insert(
            "settings.uiStability.chatStreamFlush",
            tr!(
                en,
                "Chat stream chunk flush",
                cn,
                "聊天流数据块刷新",
                tw,
                "聊天流數據塊刷新"
            ),
        );
        m.insert(
            "settings.uiStability.chatRepaintInterval",
            tr!(
                en,
                "Chat repaint interval",
                cn,
                "聊天重绘间隔",
                tw,
                "聊天重繪間隔"
            ),
        );
        m.insert(
            "settings.uiStability.chatMaxPendingEvents",
            tr!(
                en,
                "Chat max pending events/frame",
                cn,
                "聊天每帧最大待处理事件",
                tw,
                "聊天每幀最大待處理事件"
            ),
        );

        m.insert(
            "setup.environment",
            tr!(en, "Environment", cn, "环境", tw, "環境"),
        );
        m.insert(
            "setup.secretSource",
            tr!(en, "Secret Source", cn, "密钥来源", tw, "密鑰來源"),
        );
        m.insert(
            "setup.keyringError",
            tr!(
                en,
                "Failed to save to system keyring",
                cn,
                "保存到系统 keyring 失败",
                tw,
                "保存到系統 keyring 失敗"
            ),
        );
        m.insert(
            "workflow.hint",
            tr!(
                en,
                "Create reusable multi-step workflow presets and run enabled steps.",
                cn,
                "创建可复用的多步骤工作流预设，并执行已启用步骤。",
                tw,
                "創建可復用的多步驟工作流預設，並執行已啟用步驟。"
            ),
        );
        m.insert("workflow.step", tr!(en, "Step", cn, "步骤", tw, "步驟"));
        m.insert(
            "workflow.command",
            tr!(en, "Command", cn, "命令", tw, "命令"),
        );
        m.insert("workflow.add", tr!(en, "Add", cn, "添加", tw, "添加"));
        m.insert(
            "workflow.noSteps",
            tr!(en, "No steps yet.", cn, "暂无步骤。", tw, "暫無步驟。"),
        );
        m.insert("workflow.delete", tr!(en, "Delete", cn, "删除", tw, "刪除"));
        m.insert(
            "workflow.confirmDelete",
            tr!(en, "Confirm Delete", cn, "确认删除", tw, "確認刪除"),
        );
        m.insert(
            "workflow.run",
            tr!(
                en,
                "Run Enabled Steps",
                cn,
                "运行已启用步骤",
                tw,
                "運行已啟用步驟"
            ),
        );
        m.insert(
            "workflow.confirmRun",
            tr!(
                en,
                "Confirm Run Enabled Steps",
                cn,
                "确认运行已启用步骤",
                tw,
                "確認運行已啟用步驟"
            ),
        );
        m.insert(
            "workflow.runCenter.title",
            tr!(en, "Run Center", cn, "运行中心", tw, "運行中心"),
        );
        m.insert(
            "workflow.runCenter.refresh",
            tr!(en, "Refresh Runs", cn, "刷新运行记录", tw, "刷新運行記錄"),
        );
        m.insert(
            "workflow.runCenter.hidden",
            tr!(
                en,
                "Workflow run center is hidden. Enable 'Workflow Run Center' in Settings.",
                cn,
                "工作流运行中心已隐藏。请在设置中启用“工作流运行中心”。",
                tw,
                "工作流運行中心已隱藏。請在設置中啟用「工作流運行中心」。"
            ),
        );
        m.insert("workflow.phase", tr!(en, "Phase", cn, "阶段", tw, "階段"));
        m.insert(
            "workflow.createdAt",
            tr!(en, "Created At", cn, "创建时间", tw, "建立時間"),
        );
        m.insert(
            "workflow.startedAt",
            tr!(en, "Started At", cn, "开始时间", tw, "開始時間"),
        );
        m.insert(
            "workflow.endedAt",
            tr!(en, "Ended At", cn, "结束时间", tw, "結束時間"),
        );
        m.insert(
            "workflow.duration",
            tr!(en, "Duration", cn, "耗时", tw, "耗時"),
        );
        m.insert("workflow.error", tr!(en, "Error", cn, "错误", tw, "錯誤"));
        m.insert(
            "workflow.artifacts",
            tr!(en, "Artifacts:", cn, "产物:", tw, "產物:"),
        );
        m.insert(
            "workflow.runCenter.decodeFailed",
            tr!(
                en,
                "Failed to decode run detail.",
                cn,
                "运行详情解析失败。",
                tw,
                "運行詳情解析失敗。"
            ),
        );
        m.insert(
            "workflow.noEnabledSteps",
            tr!(
                en,
                "No enabled steps to run.",
                cn,
                "没有可运行的启用步骤。",
                tw,
                "沒有可運行的已啟用步驟。"
            ),
        );
        m.insert(
            "workflow.running",
            tr!(
                en,
                "Running workflow...",
                cn,
                "工作流运行中...",
                tw,
                "工作流運行中..."
            ),
        );
        m.insert(
            "workflow.deleteConfirmAgain",
            tr!(
                en,
                "Click delete again to remove step '{name}'.",
                cn,
                "再次点击删除以移除步骤“{name}”。",
                tw,
                "再次點擊刪除以移除步驟「{name}」。"
            ),
        );
        m.insert(
            "workflow.runConfirmAgain",
            tr!(
                en,
                "Click run again to confirm.",
                cn,
                "再次点击运行以确认。",
                tw,
                "再次點擊運行以確認。"
            ),
        );
        m.insert("workflow.pause", tr!(en, "Pause", cn, "暂停", tw, "暫停"));
        m.insert("workflow.resume", tr!(en, "Resume", cn, "恢复", tw, "恢復"));
        m.insert("workflow.cancel", tr!(en, "Cancel", cn, "取消", tw, "取消"));
        m.insert(
            "workflow.runActionRequested",
            tr!(
                en,
                "Run {run_id} {action} requested",
                cn,
                "已请求运行 {run_id} 执行 {action}",
                tw,
                "已請求運行 {run_id} 執行 {action}"
            ),
        );
        m.insert(
            "workflow.runActionFailed",
            tr!(
                en,
                "Run {run_id} {action} failed: {error}",
                cn,
                "运行 {run_id} 执行 {action} 失败: {error}",
                tw,
                "運行 {run_id} 執行 {action} 失敗: {error}"
            ),
        );
        m.insert(
            "workflow.executionError",
            tr!(
                en,
                "Workflow stopped due to execution error.",
                cn,
                "工作流因执行错误而停止。",
                tw,
                "工作流因執行錯誤而停止。"
            ),
        );
        m.insert(
            "workflow.stepFailure",
            tr!(
                en,
                "Workflow stopped due to step failure.",
                cn,
                "工作流因步骤失败而停止。",
                tw,
                "工作流因步驟失敗而停止。"
            ),
        );
        m.insert(
            "workflow.stepTimeout",
            tr!(
                en,
                "Workflow stopped due to step timeout.",
                cn,
                "工作流因步骤超时而停止。",
                tw,
                "工作流因步驟超時而停止。"
            ),
        );
        m.insert(
            "workflow.noOutput",
            tr!(en, "No output.", cn, "无输出。", tw, "無輸出。"),
        );
        m.insert(
            "workflow.runStatus.all",
            tr!(en, "All", cn, "全部", tw, "全部"),
        );
        m.insert(
            "workflow.runStatus.queued",
            tr!(en, "Queued", cn, "排队中", tw, "排隊中"),
        );
        m.insert(
            "workflow.runStatus.running",
            tr!(en, "Running", cn, "运行中", tw, "運行中"),
        );
        m.insert(
            "workflow.runStatus.paused",
            tr!(en, "Paused", cn, "已暂停", tw, "已暫停"),
        );
        m.insert(
            "workflow.runStatus.succeeded",
            tr!(en, "Succeeded", cn, "已成功", tw, "已成功"),
        );
        m.insert(
            "workflow.runStatus.failed",
            tr!(en, "Failed", cn, "已失败", tw, "已失敗"),
        );
        m.insert(
            "workflow.runStatus.cancelled",
            tr!(en, "Cancelled", cn, "已取消", tw, "已取消"),
        );
        m.insert(
            "skills.lifecycle.hidden",
            tr!(
                en,
                "Skills lifecycle actions are hidden. Enable 'Skills Lifecycle' in Settings.",
                cn,
                "技能生命周期操作已隐藏。请在设置中启用“技能生命周期”。",
                tw,
                "技能生命週期操作已隱藏。請在設置中啟用「技能生命週期」。"
            ),
        );
        m.insert(
            "skills.lifecycle.edit",
            tr!(en, "Edit", cn, "编辑", tw, "編輯"),
        );
        m.insert(
            "skills.lifecycle.enable",
            tr!(en, "Enable", cn, "启用", tw, "啟用"),
        );
        m.insert(
            "skills.lifecycle.disable",
            tr!(en, "Disable", cn, "停用", tw, "停用"),
        );
        m.insert(
            "skills.lifecycle.delete",
            tr!(en, "Delete", cn, "删除", tw, "刪除"),
        );
        m.insert(
            "skills.lifecycle.versions",
            tr!(en, "Versions", cn, "版本", tw, "版本"),
        );
        m.insert(
            "skills.lifecycle.editTitle",
            tr!(en, "Edit Skill", cn, "编辑技能", tw, "編輯技能"),
        );
        m.insert(
            "skills.lifecycle.promptOverride",
            tr!(
                en,
                "Prompt Template (optional override)",
                cn,
                "提示模板（可选覆盖）",
                tw,
                "提示模板（可選覆蓋）"
            ),
        );
        m.insert(
            "skills.lifecycle.inputSchema",
            tr!(
                en,
                "Input Schema JSON",
                cn,
                "输入 Schema JSON",
                tw,
                "輸入 Schema JSON"
            ),
        );
        m.insert(
            "skills.lifecycle.saveEdit",
            tr!(en, "Save Edit", cn, "保存编辑", tw, "保存編輯"),
        );
        m.insert(
            "skills.lifecycle.testInput",
            tr!(
                en,
                "Test Input JSON",
                cn,
                "测试输入 JSON",
                tw,
                "測試輸入 JSON"
            ),
        );
        m.insert(
            "skills.lifecycle.test",
            tr!(en, "Test Skill", cn, "测试技能", tw, "測試技能"),
        );
        m.insert(
            "skills.lifecycle.rollbackVersion",
            tr!(en, "Rollback Version", cn, "回滚版本", tw, "回滾版本"),
        );
        m.insert(
            "skills.lifecycle.rollback",
            tr!(en, "Rollback", cn, "回滚", tw, "回滾"),
        );
        m.insert(
            "skills.lifecycle.disabled",
            tr!(
                en,
                "Skill '{name}' disabled",
                cn,
                "技能“{name}”已停用",
                tw,
                "技能「{name}」已停用"
            ),
        );
        m.insert(
            "skills.lifecycle.enabled",
            tr!(
                en,
                "Skill '{name}' enabled",
                cn,
                "技能“{name}”已启用",
                tw,
                "技能「{name}」已啟用"
            ),
        );
        m.insert(
            "skills.lifecycle.toggleFailed",
            tr!(
                en,
                "Skill '{name}' toggle failed: {error}",
                cn,
                "技能“{name}”切换失败: {error}",
                tw,
                "技能「{name}」切換失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.removed",
            tr!(
                en,
                "Skill '{name}' removed",
                cn,
                "技能“{name}”已删除",
                tw,
                "技能「{name}」已刪除"
            ),
        );
        m.insert(
            "skills.lifecycle.removeFailed",
            tr!(
                en,
                "Skill '{name}' remove failed: {error}",
                cn,
                "技能“{name}”删除失败: {error}",
                tw,
                "技能「{name}」刪除失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.versionCount",
            tr!(
                en,
                "Skill '{name}' has {count} versions",
                cn,
                "技能“{name}”有 {count} 个版本",
                tw,
                "技能「{name}」有 {count} 個版本"
            ),
        );
        m.insert(
            "skills.lifecycle.versionsFailed",
            tr!(
                en,
                "Skill '{name}' versions failed: {error}",
                cn,
                "技能“{name}”版本查询失败: {error}",
                tw,
                "技能「{name}」版本查詢失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.updated",
            tr!(
                en,
                "Skill '{name}' updated",
                cn,
                "技能“{name}”已更新",
                tw,
                "技能「{name}」已更新"
            ),
        );
        m.insert(
            "skills.lifecycle.updateFailed",
            tr!(
                en,
                "Skill '{name}' update failed: {error}",
                cn,
                "技能“{name}”更新失败: {error}",
                tw,
                "技能「{name}」更新失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.testResult",
            tr!(
                en,
                "Skill '{name}' test result: {result}",
                cn,
                "技能“{name}”测试结果: {result}",
                tw,
                "技能「{name}」測試結果: {result}"
            ),
        );
        m.insert(
            "skills.lifecycle.testFailed",
            tr!(
                en,
                "Skill '{name}' test failed: {error}",
                cn,
                "技能“{name}”测试失败: {error}",
                tw,
                "技能「{name}」測試失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.rollbackRequired",
            tr!(
                en,
                "Rollback version is required.",
                cn,
                "必须填写回滚版本。",
                tw,
                "必須填寫回滾版本。"
            ),
        );
        m.insert(
            "skills.lifecycle.rolledBack",
            tr!(
                en,
                "Skill '{name}' rolled back to version {version}",
                cn,
                "技能“{name}”已回滚到版本 {version}",
                tw,
                "技能「{name}」已回滾到版本 {version}"
            ),
        );
        m.insert(
            "skills.lifecycle.rollbackFailed",
            tr!(
                en,
                "Skill '{name}' rollback failed: {error}",
                cn,
                "技能“{name}”回滚失败: {error}",
                tw,
                "技能「{name}」回滾失敗: {error}"
            ),
        );
        m.insert(
            "skills.defaultCreator.title",
            tr!(en, "Skill Creator", cn, "技能创建器", tw, "技能創建器"),
        );
        m.insert(
            "skills.defaultCreator.description",
            tr!(en, "Create and manage your own AI skills using natural language. Describe what you want, and this skill will help you build it.", cn, "使用自然语言创建和管理你的 AI 技能。描述你的目标，这个技能会帮你把它构建出来。", tw, "使用自然語言創建和管理你的 AI 技能。描述你的目標，這個技能會幫你把它構建出來。"),
        );
        m.insert(
            "skills.defaultCreator.button",
            tr!(
                en,
                "➕ Create Default Skill",
                cn,
                "➕ 创建默认技能",
                tw,
                "➕ 創建默認技能"
            ),
        );
        m.insert(
            "skills.defaultCreator.loaded",
            tr!(
                en,
                "Default skill is ready and loaded.",
                cn,
                "默认技能已就绪并加载。",
                tw,
                "默認技能已就緒並載入。"
            ),
        );
        m.insert(
            "skills.error.invalidSchemaObject",
            tr!(
                en,
                "Input schema must be a valid JSON object.",
                cn,
                "输入 schema 必须是合法的 JSON 对象。",
                tw,
                "輸入 schema 必須是合法的 JSON 對象。"
            ),
        );
        m.insert(
            "skills.error.invalidSchema",
            tr!(
                en,
                "Invalid input schema",
                cn,
                "输入 schema 无效",
                tw,
                "輸入 schema 無效"
            ),
        );
        m.insert(
            "skills.error.invalidTestInput",
            tr!(
                en,
                "Invalid test input",
                cn,
                "测试输入无效",
                tw,
                "測試輸入無效"
            ),
        );
        m.insert(
            "skills.error.rpc",
            tr!(en, "RPC error", cn, "RPC 错误", tw, "RPC 錯誤"),
        );
        m.insert(
            "skills.fetchFailed",
            tr!(
                en,
                "Failed to fetch skills",
                cn,
                "获取技能列表失败",
                tw,
                "獲取技能列表失敗"
            ),
        );
        m.insert(
            "skills.import.blockedBySecurity",
            tr!(
                en,
                "External URL import is blocked by security settings.",
                cn,
                "安全设置已阻止外部 URL 导入。",
                tw,
                "安全設置已阻止外部 URL 導入。"
            ),
        );
        m.insert(
            "skills.import.invalidUrl",
            tr!(
                en,
                "Invalid URL: must start with http:// or https://",
                cn,
                "URL 无效：必须以 http:// 或 https:// 开头",
                tw,
                "URL 無效：必須以 http:// 或 https:// 開頭"
            ),
        );
        m.insert(
            "skills.import.httpClientError",
            tr!(
                en,
                "Failed to build HTTP client",
                cn,
                "构建 HTTP 客户端失败",
                tw,
                "構建 HTTP 客戶端失敗"
            ),
        );
        m.insert(
            "skills.import.fetchError",
            tr!(
                en,
                "Failed to fetch URL",
                cn,
                "拉取 URL 失败",
                tw,
                "拉取 URL 失敗"
            ),
        );
        m.insert(
            "skills.import.httpStatusError",
            tr!(
                en,
                "HTTP status error",
                cn,
                "HTTP 状态错误",
                tw,
                "HTTP 狀態錯誤"
            ),
        );
        m.insert(
            "skills.import.invalidManifest",
            tr!(
                en,
                "Invalid JSON manifest",
                cn,
                "JSON manifest 无效",
                tw,
                "JSON manifest 無效"
            ),
        );
        m.insert(
            "skills.import.missingPromptTemplate",
            tr!(
                en,
                "Manifest missing required field: prompt_template",
                cn,
                "manifest 缺少必填字段: prompt_template",
                tw,
                "manifest 缺少必填字段: prompt_template"
            ),
        );
        m.insert(
            "skills.import.serializeSchemaError",
            tr!(
                en,
                "Failed to serialize input_schema",
                cn,
                "序列化 input_schema 失败",
                tw,
                "序列化 input_schema 失敗"
            ),
        );
        m.insert(
            "security.hint",
            tr!(
                en,
                "Manage client-side safety policies and runtime restart controls.",
                cn,
                "管理客户端安全策略与运行时重启控制。",
                tw,
                "管理客戶端安全策略與運行時重啟控制。"
            ),
        );
        m.insert(
            "security.confirmDangerousActions",
            tr!(
                en,
                "Require confirmation for dangerous actions",
                cn,
                "危险操作需要确认",
                tw,
                "危險操作需要確認"
            ),
        );
        m.insert(
            "security.redactApiKeys",
            tr!(
                en,
                "Redact API keys in UI",
                cn,
                "在界面中脱敏 API Key",
                tw,
                "在介面中脫敏 API Key"
            ),
        );
        m.insert(
            "security.blockExternalUrls",
            tr!(
                en,
                "Block external URL imports",
                cn,
                "阻止外部 URL 导入",
                tw,
                "阻止外部 URL 導入"
            ),
        );
        m.insert(
            "security.saved",
            tr!(
                en,
                "Security settings saved.",
                cn,
                "安全设置已保存。",
                tw,
                "安全設置已保存。"
            ),
        );
        m.insert(
            "security.confirmRestart",
            tr!(
                en,
                "Confirm Restart Runtime",
                cn,
                "确认重启运行时",
                tw,
                "確認重啟運行時"
            ),
        );
        m.insert(
            "security.restart",
            tr!(
                en,
                "Restart Backend Runtime",
                cn,
                "重启后端运行时",
                tw,
                "重啟後端運行時"
            ),
        );
        m.insert(
            "security.confirmAgain",
            tr!(
                en,
                "Click restart again to confirm.",
                cn,
                "再次点击重启以确认。",
                tw,
                "再次點擊重啟以確認。"
            ),
        );
        m.insert(
            "security.restartRequested",
            tr!(
                en,
                "Runtime restart requested.",
                cn,
                "已请求运行时重启。",
                tw,
                "已請求運行時重啟。"
            ),
        );
        m.insert(
            "security.restartFailed",
            tr!(en, "Restart failed", cn, "重启失败", tw, "重啟失敗"),
        );
        m.insert(
            "autotune.hint",
            tr!(
                en,
                "Tune generation defaults used by your workflows and prompts.",
                cn,
                "调整工作流与提示词默认生成参数。",
                tw,
                "調整工作流與提示詞默認生成參數。"
            ),
        );
        m.insert(
            "autotune.temperature",
            tr!(en, "Temperature", cn, "温度", tw, "溫度"),
        );
        m.insert("autotune.topP", tr!(en, "Top-p", cn, "Top-p", tw, "Top-p"));
        m.insert(
            "autotune.maxTokens",
            tr!(en, "Max tokens", cn, "最大 tokens", tw, "最大 tokens"),
        );
        m.insert(
            "autotune.aggressive",
            tr!(
                en,
                "Aggressive optimization",
                cn,
                "激进优化",
                tw,
                "激進優化"
            ),
        );
        m.insert(
            "autotune.resetDefaults",
            tr!(en, "Reset Defaults", cn, "重置默认值", tw, "重置默認值"),
        );
        m.insert(
            "autotune.saved",
            tr!(en, "✓ Saved", cn, "✓ 已保存", tw, "✓ 已儲存"),
        );
        m.insert(
            "config.hint",
            tr!(
                en,
                "Edit GUI config as JSON. Apply updates live and persist to disk.",
                cn,
                "以 JSON 编辑 GUI 配置，实时应用并持久化到磁盘。",
                tw,
                "以 JSON 編輯 GUI 配置，即時應用並持久化到磁碟。"
            ),
        );
        m.insert(
            "config.safeModeHidden",
            tr!(
                en,
                "Safe config controls are hidden. Enable 'Config Safe Mode' in Settings.",
                cn,
                "安全配置控制已隐藏。请在设置中启用“配置安全模式”。",
                tw,
                "安全配置控制已隱藏。請在設置中啟用「配置安全模式」。"
            ),
        );
        m.insert(
            "config.reloadCurrent",
            tr!(
                en,
                "Reload From Current",
                cn,
                "从当前配置重载",
                tw,
                "從當前配置重載"
            ),
        );
        m.insert(
            "config.reloaded",
            tr!(
                en,
                "Reloaded from in-memory config.",
                cn,
                "已从内存配置重载。",
                tw,
                "已從記憶體配置重載。"
            ),
        );
        m.insert(
            "config.createSnapshot",
            tr!(en, "Create Snapshot", cn, "创建快照", tw, "創建快照"),
        );
        m.insert(
            "config.snapshotSaved",
            tr!(en, "Snapshot saved", cn, "快照已保存", tw, "快照已保存"),
        );
        m.insert(
            "config.applyJson",
            tr!(en, "Apply JSON", cn, "应用 JSON", tw, "應用 JSON"),
        );
        m.insert(
            "config.applied",
            tr!(
                en,
                "Config applied and saved.",
                cn,
                "配置已应用并保存。",
                tw,
                "配置已應用並保存。"
            ),
        );
        m.insert(
            "config.invalidJson",
            tr!(en, "Invalid JSON", cn, "无效 JSON", tw, "無效 JSON"),
        );
        m.insert(
            "config.snapshots",
            tr!(en, "Snapshots", cn, "快照", tw, "快照"),
        );
        m.insert(
            "config.rollbackSnapshot",
            tr!(en, "Rollback Snapshot", cn, "回滚快照", tw, "回滾快照"),
        );
        m.insert(
            "config.rolledBack",
            tr!(
                en,
                "Rolled back draft to last snapshot.",
                cn,
                "已将草稿回滚到最近快照。",
                tw,
                "已將草稿回滾到最近快照。"
            ),
        );
        m.insert(
            "config.search",
            tr!(en, "Search JSON…", cn, "搜索 JSON…", tw, "搜索 JSON…"),
        );
        m.insert(
            "config.validJson",
            tr!(
                en,
                "✓ Valid JSON",
                cn,
                "✓ JSON 格式正确",
                tw,
                "✓ JSON 格式正確"
            ),
        );
        m.insert(
            "monitor.refreshNow",
            tr!(en, "Refresh now", cn, "立即刷新", tw, "立即重新整理"),
        );
        m.insert(
            "monitor.loadTrends",
            tr!(en, "Load Trends", cn, "加载趋势", tw, "載入趨勢"),
        );
        m.insert(
            "monitor.loadErrors",
            tr!(en, "Load Errors", cn, "加载错误", tw, "載入錯誤"),
        );
        m.insert(
            "monitor.trendSummary",
            tr!(en, "Trend Summary", cn, "趋势摘要", tw, "趨勢摘要"),
        );
        m.insert("monitor.qps", tr!(en, "QPS", cn, "QPS", tw, "QPS"));
        m.insert("monitor.p95", tr!(en, "P95", cn, "P95", tw, "P95"));
        m.insert(
            "monitor.errorRate",
            tr!(en, "Error Rate", cn, "错误率", tw, "錯誤率"),
        );
        m.insert(
            "monitor.successRate",
            tr!(en, "Success Rate", cn, "成功率", tw, "成功率"),
        );
        m.insert(
            "monitor.errorTopGroups",
            tr!(
                en,
                "Top Error Groups",
                cn,
                "错误分组 Top",
                tw,
                "錯誤分組 Top"
            ),
        );
        m.insert(
            "monitor.sampleFailures",
            tr!(
                en,
                "Sample failures: {count}",
                cn,
                "失败样本数: {count}",
                tw,
                "失敗樣本數: {count}"
            ),
        );
        m.insert(
            "providers.ops.hidden",
            tr!(
                en,
                "Provider operation controls are hidden. Enable 'Providers Ops' in Settings.",
                cn,
                "提供商运维控件已隐藏。请在设置中启用“提供商运维”。",
                tw,
                "提供商運維控件已隱藏。請在設置中啟用「提供商運維」。"
            ),
        );
        m.insert(
            "providers.ops.testConn",
            tr!(en, "Test Conn", cn, "测试连接", tw, "測試連接"),
        );
        m.insert(
            "providers.ops.testCompletion",
            tr!(en, "Test Completion", cn, "测试补全", tw, "測試補全"),
        );
        m.insert(
            "providers.ops.capabilities",
            tr!(en, "Capabilities", cn, "能力", tw, "能力"),
        );
        m.insert(
            "providers.ops.connStatus",
            tr!(
                en,
                "Conn ok={ok} latency={latency}ms",
                cn,
                "连接 ok={ok} 延迟={latency}ms",
                tw,
                "連接 ok={ok} 延遲={latency}ms"
            ),
        );
        m.insert(
            "providers.ops.connStatusFailed",
            tr!(
                en,
                "Conn failed: {error}",
                cn,
                "连接失败: {error}",
                tw,
                "連接失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.completionStatus",
            tr!(
                en,
                "Completion ok={ok} model={model}",
                cn,
                "补全 ok={ok} 模型={model}",
                tw,
                "補全 ok={ok} 模型={model}"
            ),
        );
        m.insert(
            "providers.ops.completionStatusFailed",
            tr!(
                en,
                "Completion failed: {error}",
                cn,
                "补全失败: {error}",
                tw,
                "補全失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.capabilitiesCount",
            tr!(
                en,
                "Capabilities models={count}",
                cn,
                "能力模型数={count}",
                tw,
                "能力模型數={count}"
            ),
        );
        m.insert(
            "providers.ops.capabilitiesFailed",
            tr!(
                en,
                "Capabilities failed: {error}",
                cn,
                "能力查询失败: {error}",
                tw,
                "能力查詢失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.capabilitiesEncodeFailed",
            tr!(
                en,
                "Capabilities encode failed: {error}",
                cn,
                "能力编码失败: {error}",
                tw,
                "能力編碼失敗: {error}"
            ),
        );
        m.insert(
            "providers.cap.context",
            tr!(en, "ctx", cn, "上下文", tw, "上下文"),
        );
        m.insert(
            "providers.cap.tool",
            tr!(en, "tool", cn, "工具调用", tw, "工具調用"),
        );
        m.insert(
            "providers.cap.vision",
            tr!(en, "vision", cn, "视觉", tw, "視覺"),
        );
        m.insert(
            "providers.cap.cost",
            tr!(en, "cost", cn, "成本层级", tw, "成本層級"),
        );
        m.insert(
            "providers.cap.moreModels",
            tr!(
                en,
                "+{count} more models",
                cn,
                "另有 {count} 个模型",
                tw,
                "另有 {count} 個模型"
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

        // ── Chat keys ────────────────────────────────────
        m.insert(
            "chat.newSession",
            tr!(en, "New session", cn, "新对话", tw, "新對話"),
        );
        m.insert(
            "chat.noMessages",
            tr!(en, "No messages yet", cn, "暂无消息", tw, "暫無消息"),
        );
        m.insert(
            "chat.hint",
            tr!(
                en,
                "Type a message and press Enter to begin",
                cn,
                "输入消息后按 Enter 发送",
                tw,
                "輸入訊息後按 Enter 發送"
            ),
        );
        m.insert(
            "chat.input",
            tr!(
                en,
                "Type your message...",
                cn,
                "输入消息...",
                tw,
                "輸入訊息..."
            ),
        );
        m.insert("chat.send", tr!(en, "Send", cn, "发送", tw, "發送"));
        m.insert("chat.stop", tr!(en, "Stop", cn, "停止", tw, "停止"));
        m.insert("chat.retry", tr!(en, "Retry", cn, "重试", tw, "重試"));
        m.insert(
            "chat.attach",
            tr!(en, "Attach file", cn, "附件", tw, "附件"),
        );
        m.insert("chat.export", tr!(en, "Export", cn, "导出", tw, "導出"));
        m.insert(
            "chat.exportTitle",
            tr!(en, "Chat Export", cn, "对话导出", tw, "對話導出"),
        );
        m.insert(
            "chat.exportedAt",
            tr!(
                en,
                "Exported at {time}",
                cn,
                "导出时间 {time}",
                tw,
                "導出時間 {time}"
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
                "模型：{model}",
                tw,
                "模型：{model}"
            ),
        );
        m.insert(
            "chat.exportThinking",
            tr!(
                en,
                "Thinking: {thinking}",
                cn,
                "思考：{thinking}",
                tw,
                "思考：{thinking}"
            ),
        );
        m.insert(
            "chat.exportSuccess",
            tr!(
                en,
                "Exported to {path}",
                cn,
                "已导出到 {path}",
                tw,
                "已導出到 {path}"
            ),
        );
        m.insert(
            "chat.exportFailed",
            tr!(
                en,
                "Export failed: {error}",
                cn,
                "导出失败：{error}",
                tw,
                "導出失敗：{error}"
            ),
        );
        m.insert(
            "chat.chatError",
            tr!(
                en,
                "Error: {message}",
                cn,
                "错误：{message}",
                tw,
                "錯誤：{message}"
            ),
        );
        m.insert("chat.clear", tr!(en, "Clear", cn, "清除", tw, "清除"));
        m.insert(
            "chat.clearAll",
            tr!(
                en,
                "Clear all messages",
                cn,
                "清除全部消息",
                tw,
                "清除全部訊息"
            ),
        );
        m.insert(
            "chat.rename",
            tr!(en, "Rename", cn, "重命名", tw, "重新命名"),
        );
        m.insert(
            "chat.copyCode",
            tr!(en, "Copy code", cn, "复制代码", tw, "複製代碼"),
        );
        m.insert(
            "chat.chooseModels",
            tr!(en, "Choose Models", cn, "选择模型", tw, "選擇模型"),
        );
        m.insert(
            "chat.modelAutoOnly",
            tr!(en, "Auto Only", cn, "仅自动", tw, "僅自動"),
        );
        m.insert(
            "chat.multiModelEnabled",
            tr!(en, "Multi-model", cn, "多模型", tw, "多模型"),
        );
        m.insert(
            "chat.tokenSummary",
            tr!(
                en,
                "Input: {input} | Output: {output} | Total: {total}",
                cn,
                "输入：{input} | 输出：{output} | 总计：{total}",
                tw,
                "輸入：{input} | 輸出：{output} | 總計：{total}"
            ),
        );
        m.insert(
            "chat.openWorkspace",
            tr!(en, "Open workspace", cn, "打开工作目录", tw, "打開工作目錄"),
        );
        m.insert(
            "chat.cannotDeleteLastSession",
            tr!(
                en,
                "Cannot delete the last session.",
                cn,
                "无法删除最后一个会话。",
                tw,
                "無法刪除最後一個會話。"
            ),
        );
        m.insert(
            "chat.thinking",
            tr!(en, "AI is thinking", cn, "AI 思考中", tw, "AI 思考中"),
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
        m.insert(
            "chat.promptTemplates",
            tr!(en, "Prompt Templates", cn, "提示模板", tw, "提示模板"),
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
            "chat.templateNew",
            tr!(en, "New Template", cn, "新建模板", tw, "新增模板"),
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
            "chat.templateValidation",
            tr!(
                en,
                "Name, command, and body cannot be empty.",
                cn,
                "名称、指令和内容不能为空。",
                tw,
                "名稱、指令和內容不能為空。"
            ),
        );
        m.insert(
            "chat.templateDuplicate",
            tr!(
                en,
                "Command already exists.",
                cn,
                "指令已存在。",
                tw,
                "指令已存在。"
            ),
        );
        m.insert("chat.close", tr!(en, "Close", cn, "关闭", tw, "關閉"));

        // ── Autotune keys ────────────────────────────────
        m.insert(
            "autotune.hint",
            tr!(
                en,
                "Tune generation defaults used by your workflows and prompts.",
                cn,
                "调整工作流与提示词默认生成参数。",
                tw,
                "調整工作流與提示詞默認生成參數。"
            ),
        );
        m.insert(
            "autotune.temperature",
            tr!(en, "Temperature", cn, "温度", tw, "溫度"),
        );
        m.insert("autotune.topP", tr!(en, "Top-p", cn, "Top-p", tw, "Top-p"));
        m.insert(
            "autotune.maxTokens",
            tr!(en, "Max tokens", cn, "最大 tokens", tw, "最大 tokens"),
        );
        m.insert(
            "autotune.aggressive",
            tr!(
                en,
                "Aggressive optimization",
                cn,
                "激进优化",
                tw,
                "激進優化"
            ),
        );
        m.insert(
            "autotune.resetDefaults",
            tr!(en, "Reset Defaults", cn, "重置默认值", tw, "重置默認值"),
        );
        m.insert(
            "autotune.saved",
            tr!(en, "✓ Saved", cn, "✓ 已保存", tw, "✓ 已儲存"),
        );

        // ── Common keys ──────────────────────────────────
        m.insert("common.copyButton", tr!(en, "Copy", cn, "复制", tw, "複製"));
        m.insert(
            "common.apiKeyPlaceholder",
            tr!(
                en,
                "Enter API key...",
                cn,
                "输入 API 密钥...",
                tw,
                "輸入 API 密鑰..."
            ),
        );
        m.insert("common.close", tr!(en, "Close", cn, "关闭", tw, "關閉"));

        // ── Config keys ──────────────────────────────────
        m.insert(
            "config.hint",
            tr!(
                en,
                "Edit GUI config as JSON. Apply updates live and persist to disk.",
                cn,
                "以 JSON 编辑 GUI 配置，实时应用并持久化到磁盘。",
                tw,
                "以 JSON 編輯 GUI 配置，即時應用並持久化到磁碟。"
            ),
        );
        m.insert(
            "config.safeModeHidden",
            tr!(
                en,
                "Safe config controls are hidden. Enable 'Config Safe Mode' in Settings.",
                cn,
                "安全配置控制已隐藏。请在设置中启用「配置安全模式」。",
                tw,
                "安全配置控制已隱藏。請在設置中啟用「配置安全模式」。"
            ),
        );
        m.insert(
            "config.reloadCurrent",
            tr!(
                en,
                "Reload From Current",
                cn,
                "从当前配置重载",
                tw,
                "從當前配置重載"
            ),
        );
        m.insert(
            "config.reloaded",
            tr!(
                en,
                "Reloaded from in-memory config.",
                cn,
                "已从内存配置重载。",
                tw,
                "已從記憶體配置重載。"
            ),
        );
        m.insert(
            "config.createSnapshot",
            tr!(en, "Create Snapshot", cn, "创建快照", tw, "創建快照"),
        );
        m.insert(
            "config.snapshotSaved",
            tr!(en, "Snapshot saved", cn, "快照已保存", tw, "快照已保存"),
        );
        m.insert(
            "config.applyJson",
            tr!(en, "Apply JSON", cn, "应用 JSON", tw, "應用 JSON"),
        );
        m.insert(
            "config.applied",
            tr!(
                en,
                "Config applied and saved.",
                cn,
                "配置已应用并保存。",
                tw,
                "配置已應用並保存。"
            ),
        );
        m.insert(
            "config.invalidJson",
            tr!(en, "Invalid JSON", cn, "JSON 格式错误", tw, "JSON 格式錯誤"),
        );
        m.insert(
            "config.snapshots",
            tr!(en, "Snapshots", cn, "快照", tw, "快照"),
        );
        m.insert(
            "config.rollbackSnapshot",
            tr!(en, "Rollback Snapshot", cn, "回滚快照", tw, "回滾快照"),
        );
        m.insert(
            "config.rolledBack",
            tr!(
                en,
                "Rolled back draft to last snapshot.",
                cn,
                "已将草稿回滚到最近快照。",
                tw,
                "已將草稿回滾到最近快照。"
            ),
        );
        m.insert(
            "config.search",
            tr!(en, "Search JSON…", cn, "搜索 JSON…", tw, "搜索 JSON…"),
        );
        m.insert(
            "config.validJson",
            tr!(
                en,
                "✓ Valid JSON",
                cn,
                "✓ JSON 格式正确",
                tw,
                "✓ JSON 格式正確"
            ),
        );

        // ── Monitor keys ─────────────────────────────────
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
        m.insert(
            "monitor.filterProviders",
            tr!(
                en,
                "Filter providers…",
                cn,
                "筛选供应商…",
                tw,
                "篩選供應商…"
            ),
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
            "monitor.loadTrends",
            tr!(en, "Load Trends", cn, "加载趋势", tw, "載入趨勢"),
        );
        m.insert(
            "monitor.loadErrors",
            tr!(en, "Load Errors", cn, "加载错误", tw, "載入錯誤"),
        );
        m.insert(
            "monitor.refreshNow",
            tr!(en, "Refresh now", cn, "立即刷新", tw, "立即重新整理"),
        );
        m.insert(
            "monitor.trendSummary",
            tr!(en, "Trend Summary", cn, "趋势摘要", tw, "趨勢摘要"),
        );
        m.insert("monitor.qps", tr!(en, "QPS", cn, "QPS", tw, "QPS"));
        m.insert("monitor.p95", tr!(en, "P95", cn, "P95", tw, "P95"));
        m.insert(
            "monitor.errorRate",
            tr!(en, "Error Rate", cn, "错误率", tw, "錯誤率"),
        );
        m.insert(
            "monitor.successRate",
            tr!(en, "Success Rate", cn, "成功率", tw, "成功率"),
        );
        m.insert(
            "monitor.errorTopGroups",
            tr!(
                en,
                "Top Error Groups",
                cn,
                "错误分组 Top",
                tw,
                "錯誤分組 Top"
            ),
        );
        m.insert(
            "monitor.sampleFailures",
            tr!(
                en,
                "Sample failures: {count}",
                cn,
                "失败样本数: {count}",
                tw,
                "失敗樣本數: {count}"
            ),
        );

        // ── Provider keys ────────────────────────────────
        m.insert(
            "providers.noKey",
            tr!(en, "No key", cn, "无密钥", tw, "無密鑰"),
        );
        m.insert(
            "providers.ops.testConn",
            tr!(en, "Test Conn", cn, "测试连接", tw, "測試連接"),
        );
        m.insert(
            "providers.ops.testCompletion",
            tr!(en, "Test Completion", cn, "测试补全", tw, "測試補全"),
        );
        m.insert(
            "providers.ops.connStatus",
            tr!(
                en,
                "Conn ok={ok} latency={latency}ms",
                cn,
                "连接 ok={ok} 延迟={latency}ms",
                tw,
                "連接 ok={ok} 延遲={latency}ms"
            ),
        );
        m.insert(
            "providers.ops.connStatusFailed",
            tr!(
                en,
                "Conn failed: {error}",
                cn,
                "连接失败: {error}",
                tw,
                "連接失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.completionStatus",
            tr!(
                en,
                "Completion ok={ok} model={model}",
                cn,
                "补全 ok={ok} 模型={model}",
                tw,
                "補全 ok={ok} 模型={model}"
            ),
        );
        m.insert(
            "providers.ops.completionStatusFailed",
            tr!(
                en,
                "Completion failed: {error}",
                cn,
                "补全失败: {error}",
                tw,
                "補全失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.capabilitiesCount",
            tr!(
                en,
                "Capabilities models={count}",
                cn,
                "能力模型数={count}",
                tw,
                "能力模型數={count}"
            ),
        );
        m.insert(
            "providers.ops.capabilitiesFailed",
            tr!(
                en,
                "Capabilities failed: {error}",
                cn,
                "能力查询失败: {error}",
                tw,
                "能力查詢失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.capabilitiesEncodeFailed",
            tr!(
                en,
                "Capabilities encode failed: {error}",
                cn,
                "能力编码失败: {error}",
                tw,
                "能力編碼失敗: {error}"
            ),
        );

        // ── Security keys ────────────────────────────────
        m.insert(
            "security.hint",
            tr!(
                en,
                "Manage client-side safety policies and runtime restart controls.",
                cn,
                "管理客户端安全策略与运行时重启控制。",
                tw,
                "管理客戶端安全策略與運行時重啟控制。"
            ),
        );
        m.insert(
            "security.confirmDangerousActions",
            tr!(
                en,
                "Require confirmation for dangerous actions",
                cn,
                "危险操作需要确认",
                tw,
                "危險操作需要確認"
            ),
        );
        m.insert(
            "security.redactApiKeys",
            tr!(
                en,
                "Redact API keys in UI",
                cn,
                "在界面中脱敏 API Key",
                tw,
                "在介面中脫敏 API Key"
            ),
        );
        m.insert(
            "security.blockExternalUrls",
            tr!(
                en,
                "Block external URL imports",
                cn,
                "阻止外部 URL 导入",
                tw,
                "阻止外部 URL 導入"
            ),
        );
        m.insert(
            "security.saved",
            tr!(
                en,
                "Security settings saved.",
                cn,
                "安全设置已保存。",
                tw,
                "安全設置已保存。"
            ),
        );
        m.insert(
            "security.confirmRestart",
            tr!(
                en,
                "Confirm Restart Runtime",
                cn,
                "确认重启运行时",
                tw,
                "確認重啟運行時"
            ),
        );
        m.insert(
            "security.restart",
            tr!(
                en,
                "Restart Backend Runtime",
                cn,
                "重启后端运行时",
                tw,
                "重啟後端運行時"
            ),
        );
        m.insert(
            "security.confirmAgain",
            tr!(
                en,
                "Click restart again to confirm.",
                cn,
                "再次点击重启以确认。",
                tw,
                "再次點擊重啟以確認。"
            ),
        );
        m.insert(
            "security.restartRequested",
            tr!(
                en,
                "Runtime restart requested.",
                cn,
                "已请求运行时重启。",
                tw,
                "已請求運行時重啟。"
            ),
        );
        m.insert(
            "security.restartFailed",
            tr!(en, "Restart failed", cn, "重启失败", tw, "重啟失敗"),
        );

        // ── Settings keys ────────────────────────────────
        m.insert(
            "settings.title",
            tr!(en, "Settings", cn, "设置", tw, "設置"),
        );
        m.insert(
            "settings.hint",
            tr!(
                en,
                "Toggle features and configure system settings.",
                cn,
                "切换功能和配置系统设置。",
                tw,
                "切換功能和配置系統設置。"
            ),
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
        m.insert(
            "settings.backendUrlPlaceholder",
            tr!(
                en,
                "http://127.0.0.1:8090",
                cn,
                "http://127.0.0.1:8090",
                tw,
                "http://127.0.0.1:8090"
            ),
        );
        m.insert(
            "settings.section.core",
            tr!(en, "Core Features", cn, "核心功能", tw, "核心功能"),
        );
        m.insert(
            "settings.section.advanced",
            tr!(en, "Advanced Features", cn, "高级功能", tw, "進階功能"),
        );
        m.insert(
            "settings.section.system",
            tr!(en, "System Settings", cn, "系统设置", tw, "系統設定"),
        );
        m.insert(
            "settings.section.backend",
            tr!(en, "Backend URL", cn, "后端地址", tw, "後端位址"),
        );
        m.insert(
            "settings.section.language",
            tr!(en, "Language", cn, "语言", tw, "語言"),
        );
        m.insert(
            "settings.section.theme",
            tr!(en, "Theme", cn, "主题", tw, "主題"),
        );
        m.insert(
            "settings.feature.workflowRunCenter",
            tr!(
                en,
                "Workflow Run Center",
                cn,
                "工作流运行中心",
                tw,
                "工作流運行中心"
            ),
        );
        m.insert(
            "settings.feature.autotuneChainInjection",
            tr!(
                en,
                "AutoTune Chain Injection",
                cn,
                "AutoTune 链路注入",
                tw,
                "AutoTune 鏈路注入"
            ),
        );
        m.insert(
            "settings.feature.skillsLifecycle",
            tr!(
                en,
                "Skills Lifecycle",
                cn,
                "技能生命周期",
                tw,
                "技能生命週期"
            ),
        );
        m.insert(
            "settings.feature.providersOps",
            tr!(en, "Providers Ops", cn, "提供商运维", tw, "提供商運維"),
        );
        m.insert(
            "settings.feature.monitorHistoryAlerts",
            tr!(
                en,
                "Monitor History Alerts",
                cn,
                "监控历史告警",
                tw,
                "監控歷史告警"
            ),
        );
        m.insert(
            "settings.feature.configSafeMode",
            tr!(
                en,
                "Config Safe Mode",
                cn,
                "配置安全模式",
                tw,
                "配置安全模式"
            ),
        );
        m.insert(
            "settings.feature.setupEnterprise",
            tr!(en, "Setup Enterprise", cn, "企业化配置", tw, "企業化配置"),
        );
        m.insert(
            "settings.enterprise.environmentUrl",
            tr!(
                en,
                "Environment Backend URL",
                cn,
                "环境后端地址",
                tw,
                "環境後端地址"
            ),
        );
        m.insert(
            "settings.enterprise.secretSource",
            tr!(en, "Secret Source", cn, "密钥来源", tw, "密鑰來源"),
        );
        m.insert(
            "settings.enterprise.exportPath",
            tr!(en, "Export Path", cn, "导出路径", tw, "導出路徑"),
        );
        m.insert(
            "settings.enterprise.importPath",
            tr!(en, "Import Path", cn, "导入路径", tw, "導入路徑"),
        );
        m.insert(
            "settings.enterprise.exportMasked",
            tr!(en, "Export Masked", cn, "导出脱敏配置", tw, "導出脫敏配置"),
        );
        m.insert(
            "settings.enterprise.exportFull",
            tr!(en, "Export Full", cn, "导出完整配置", tw, "導出完整配置"),
        );
        m.insert(
            "settings.enterprise.importConfig",
            tr!(en, "Import Config", cn, "导入配置", tw, "導入配置"),
        );
        m.insert(
            "settings.enterprise.syncCurrent",
            tr!(
                en,
                "Sync Current URL",
                cn,
                "同步当前地址",
                tw,
                "同步當前地址"
            ),
        );

        // ── Setup keys ───────────────────────────────────
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
            "setup.apiKey",
            tr!(en, "API Key", cn, "API 密钥", tw, "API 密鑰"),
        );
        m.insert(
            "setup.secretSource",
            tr!(en, "Secret Source", cn, "密钥来源", tw, "密鑰來源"),
        );

        // ── Theme keys ───────────────────────────────────
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
        m.insert(
            "theme.serveThePeople",
            tr!(en, "Serve The People", cn, "为人民服务", tw, "為人民服務"),
        );

        // ── Workflow keys ────────────────────────────────
        m.insert(
            "workflow.hint",
            tr!(
                en,
                "Create reusable multi-step workflow presets and run enabled steps.",
                cn,
                "创建可复用的多步骤工作流预设，并执行已启用步骤。",
                tw,
                "創建可復用的多步驟工作流預設，並執行已啟用步驟。"
            ),
        );
        m.insert(
            "workflow.noSteps",
            tr!(en, "No steps yet.", cn, "暂无步骤。", tw, "暫無步驟。"),
        );
        m.insert(
            "workflow.confirmDelete",
            tr!(en, "Confirm Delete", cn, "确认删除", tw, "確認刪除"),
        );
        m.insert(
            "workflow.confirmRun",
            tr!(en, "Confirm Run", cn, "确认运行", tw, "確認運行"),
        );
        m.insert(
            "workflow.runConfirmAgain",
            tr!(
                en,
                "Click run again to confirm.",
                cn,
                "再次点击运行以确认。",
                tw,
                "再次點擊運行以確認。"
            ),
        );
        m.insert(
            "workflow.deleteConfirmAgain",
            tr!(
                en,
                "Click delete again to remove step '{name}'.",
                cn,
                "再次点击删除以移除步骤「{name}」。",
                tw,
                "再次點擊刪除以移除步驟「{name}」。"
            ),
        );
        m.insert(
            "workflow.runCenter.title",
            tr!(en, "Run Center", cn, "运行中心", tw, "運行中心"),
        );
        m.insert(
            "workflow.runCenter.hidden",
            tr!(
                en,
                "Workflow run center is hidden. Enable 'Workflow Run Center' in Settings.",
                cn,
                "工作流运行中心已隐藏。请在设置中启用「工作流运行中心」。",
                tw,
                "工作流運行中心已隱藏。請在設置中啟用「工作流運行中心」。"
            ),
        );
        m.insert(
            "workflow.runCenter.refresh",
            tr!(en, "Refresh Runs", cn, "刷新运行记录", tw, "刷新運行記錄"),
        );
        m.insert(
            "workflow.runCenter.decodeFailed",
            tr!(
                en,
                "Failed to decode run detail.",
                cn,
                "运行详情解析失败。",
                tw,
                "運行詳情解析失敗。"
            ),
        );
        m.insert(
            "workflow.noEnabledSteps",
            tr!(
                en,
                "No enabled steps to run.",
                cn,
                "没有可运行的启用步骤。",
                tw,
                "沒有可運行的已啟用步驟。"
            ),
        );
        m.insert(
            "workflow.executionError",
            tr!(
                en,
                "Workflow stopped due to execution error.",
                cn,
                "工作流因执行错误而停止。",
                tw,
                "工作流因執行錯誤而停止。"
            ),
        );
        m.insert(
            "workflow.stepFailure",
            tr!(
                en,
                "Workflow stopped due to step failure.",
                cn,
                "工作流因步骤失败而停止。",
                tw,
                "工作流因步驟失敗而停止。"
            ),
        );
        m.insert(
            "workflow.stepTimeout",
            tr!(
                en,
                "Workflow stopped due to step timeout.",
                cn,
                "工作流因步骤超时而停止。",
                tw,
                "工作流因步驟超時而停止。"
            ),
        );
        m.insert(
            "workflow.noOutput",
            tr!(en, "No output.", cn, "无输出。", tw, "無輸出。"),
        );
        m.insert(
            "workflow.runStatus.all",
            tr!(en, "All", cn, "全部", tw, "全部"),
        );
        m.insert(
            "workflow.runStatus.queued",
            tr!(en, "Queued", cn, "排队中", tw, "排隊中"),
        );
        m.insert(
            "workflow.runStatus.running",
            tr!(en, "Running", cn, "运行中", tw, "運行中"),
        );
        m.insert(
            "workflow.runStatus.paused",
            tr!(en, "Paused", cn, "已暂停", tw, "已暫停"),
        );
        m.insert(
            "workflow.runStatus.succeeded",
            tr!(en, "Succeeded", cn, "已成功", tw, "已成功"),
        );
        m.insert(
            "workflow.runStatus.failed",
            tr!(en, "Failed", cn, "已失败", tw, "已失敗"),
        );
        m.insert(
            "workflow.runStatus.cancelled",
            tr!(en, "Cancelled", cn, "已取消", tw, "已取消"),
        );
        m.insert(
            "workflow.runActionRequested",
            tr!(
                en,
                "Run {run_id} {action} requested",
                cn,
                "已请求运行 {run_id} 执行 {action}",
                tw,
                "已請求運行 {run_id} 執行 {action}"
            ),
        );
        m.insert(
            "workflow.runActionFailed",
            tr!(
                en,
                "Run {run_id} {action} failed: {error}",
                cn,
                "运行 {run_id} 执行 {action} 失败: {error}",
                tw,
                "運行 {run_id} 執行 {action} 失敗: {error}"
            ),
        );
        m.insert(
            "workflow.createdAt",
            tr!(en, "Created At", cn, "创建时间", tw, "建立時間"),
        );
        m.insert(
            "workflow.startedAt",
            tr!(en, "Started At", cn, "开始时间", tw, "開始時間"),
        );
        m.insert(
            "workflow.endedAt",
            tr!(en, "Ended At", cn, "结束时间", tw, "結束時間"),
        );

        // ── Skills keys ──────────────────────────────────
        m.insert(
            "skills.fetchFailed",
            tr!(
                en,
                "Failed to fetch skills",
                cn,
                "获取技能列表失败",
                tw,
                "獲取技能列表失敗"
            ),
        );
        m.insert(
            "skills.defaultCreator.title",
            tr!(en, "Skill Creator", cn, "技能创建器", tw, "技能創建器"),
        );
        m.insert(
            "skills.defaultCreator.description",
            tr!(
                en,
                "Create and manage your own AI skills using natural language.",
                cn,
                "使用自然语言创建和管理你的 AI 技能。",
                tw,
                "使用自然語言創建和管理你的 AI 技能。"
            ),
        );
        m.insert(
            "skills.defaultCreator.button",
            tr!(
                en,
                "Create Default Skill",
                cn,
                "创建默认技能",
                tw,
                "創建默認技能"
            ),
        );
        m.insert(
            "skills.defaultCreator.loaded",
            tr!(
                en,
                "Default skill is ready and loaded.",
                cn,
                "默认技能已就绪并加载。",
                tw,
                "默認技能已就緒並載入。"
            ),
        );
        m.insert(
            "skills.error.invalidSchemaObject",
            tr!(
                en,
                "Input schema must be a valid JSON object.",
                cn,
                "输入 schema 必须是合法的 JSON 对象。",
                tw,
                "輸入 schema 必須是合法的 JSON 對象。"
            ),
        );
        m.insert(
            "skills.error.invalidSchema",
            tr!(
                en,
                "Invalid input schema",
                cn,
                "输入 schema 无效",
                tw,
                "輸入 schema 無效"
            ),
        );
        m.insert(
            "skills.error.invalidTestInput",
            tr!(
                en,
                "Invalid test input",
                cn,
                "测试输入无效",
                tw,
                "測試輸入無效"
            ),
        );
        m.insert(
            "skills.create.errorName",
            tr!(en, "Name is required", cn, "名称必填", tw, "名稱必填"),
        );
        m.insert(
            "skills.create.errorPrompt",
            tr!(
                en,
                "Prompt template is required",
                cn,
                "提示模板必填",
                tw,
                "提示模板必填"
            ),
        );
        m.insert(
            "skills.import.invalidUrl",
            tr!(
                en,
                "Invalid URL: must start with http:// or https://",
                cn,
                "URL 无效：必须以 http:// 或 https:// 开头",
                tw,
                "URL 無效：必須以 http:// 或 https:// 開頭"
            ),
        );
        m.insert(
            "skills.import.httpClientError",
            tr!(
                en,
                "Failed to build HTTP client",
                cn,
                "构建 HTTP 客户端失败",
                tw,
                "構建 HTTP 客戶端失敗"
            ),
        );
        m.insert(
            "skills.import.fetchError",
            tr!(
                en,
                "Failed to fetch URL",
                cn,
                "拉取 URL 失败",
                tw,
                "拉取 URL 失敗"
            ),
        );
        m.insert(
            "skills.import.httpStatusError",
            tr!(
                en,
                "HTTP status error",
                cn,
                "HTTP 状态错误",
                tw,
                "HTTP 狀態錯誤"
            ),
        );
        m.insert(
            "skills.import.invalidManifest",
            tr!(
                en,
                "Invalid JSON manifest",
                cn,
                "JSON manifest 无效",
                tw,
                "JSON manifest 無效"
            ),
        );
        m.insert(
            "skills.import.missingPromptTemplate",
            tr!(
                en,
                "Manifest missing required field: prompt_template",
                cn,
                "manifest 缺少必填字段: prompt_template",
                tw,
                "manifest 缺少必填字段: prompt_template"
            ),
        );
        m.insert(
            "skills.import.serializeSchemaError",
            tr!(
                en,
                "Failed to serialize input_schema",
                cn,
                "序列化 input_schema 失败",
                tw,
                "序列化 input_schema 失敗"
            ),
        );
        m.insert(
            "skills.import.blockedBySecurity",
            tr!(
                en,
                "External URL import is blocked by security settings.",
                cn,
                "安全设置已阻止外部 URL 导入。",
                tw,
                "安全設置已阻止外部 URL 導入。"
            ),
        );
        m.insert(
            "skills.import.importedFrom",
            tr!(
                en,
                "Imported from {url}",
                cn,
                "从 {url} 导入",
                tw,
                "從 {url} 導入"
            ),
        );
        m.insert(
            "skills.import.errorUrl",
            tr!(en, "URL is required", cn, "URL 必填", tw, "URL 必填"),
        );
        m.insert(
            "skills.lifecycle.editTitle",
            tr!(en, "Edit Skill", cn, "编辑技能", tw, "編輯技能"),
        );
        m.insert(
            "skills.lifecycle.promptOverride",
            tr!(
                en,
                "Prompt Template (optional)",
                cn,
                "提示模板（可选）",
                tw,
                "提示模板（可選）"
            ),
        );
        m.insert(
            "skills.lifecycle.inputSchema",
            tr!(
                en,
                "Input Schema JSON",
                cn,
                "输入 Schema JSON",
                tw,
                "輸入 Schema JSON"
            ),
        );
        m.insert(
            "skills.lifecycle.saveEdit",
            tr!(en, "Save Edit", cn, "保存编辑", tw, "保存編輯"),
        );
        m.insert(
            "skills.lifecycle.testInput",
            tr!(
                en,
                "Test Input JSON",
                cn,
                "测试输入 JSON",
                tw,
                "測試輸入 JSON"
            ),
        );
        m.insert(
            "skills.lifecycle.testResult",
            tr!(
                en,
                "Skill '{name}' test result: {result}",
                cn,
                "技能「{name}」测试结果: {result}",
                tw,
                "技能「{name}」測試結果: {result}"
            ),
        );
        m.insert(
            "skills.lifecycle.testFailed",
            tr!(
                en,
                "Skill '{name}' test failed: {error}",
                cn,
                "技能「{name}」测试失败: {error}",
                tw,
                "技能「{name}」測試失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.versionCount",
            tr!(
                en,
                "Skill '{name}' has {count} versions",
                cn,
                "技能「{name}」有 {count} 个版本",
                tw,
                "技能「{name}」有 {count} 個版本"
            ),
        );
        m.insert(
            "skills.lifecycle.versionsFailed",
            tr!(
                en,
                "Skill '{name}' versions failed: {error}",
                cn,
                "技能「{name}」版本查询失败: {error}",
                tw,
                "技能「{name}」版本查詢失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.updateFailed",
            tr!(
                en,
                "Skill '{name}' update failed: {error}",
                cn,
                "技能「{name}」更新失败: {error}",
                tw,
                "技能「{name}」更新失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.rollbackVersion",
            tr!(en, "Rollback Version", cn, "回滚版本", tw, "回滾版本"),
        );
        m.insert(
            "skills.lifecycle.rollbackRequired",
            tr!(
                en,
                "Rollback version is required.",
                cn,
                "必须填写回滚版本。",
                tw,
                "必須填寫回滾版本。"
            ),
        );
        m.insert(
            "skills.lifecycle.rolledBack",
            tr!(
                en,
                "Skill '{name}' rolled back to version {version}",
                cn,
                "技能「{name}」已回滚到版本 {version}",
                tw,
                "技能「{name}」已回滾到版本 {version}"
            ),
        );
        m.insert(
            "skills.lifecycle.rollbackFailed",
            tr!(
                en,
                "Skill '{name}' rollback failed: {error}",
                cn,
                "技能「{name}」回滚失败: {error}",
                tw,
                "技能「{name}」回滾失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.toggleFailed",
            tr!(
                en,
                "Skill '{name}' toggle failed: {error}",
                cn,
                "技能「{name}」切换失败: {error}",
                tw,
                "技能「{name}」切換失敗: {error}"
            ),
        );
        m.insert(
            "skills.lifecycle.removeFailed",
            tr!(
                en,
                "Skill '{name}' remove failed: {error}",
                cn,
                "技能「{name}」删除失败: {error}",
                tw,
                "技能「{name}」刪除失敗: {error}"
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
