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
            "app.shortcutHint",
            tr!(
                en,
                "⌨ Ctrl+1-9 tabs | Ctrl+N new chat | Ctrl+L clear",
                cn,
                "⌨ Ctrl+1-9 切换标签 | Ctrl+N 新建对话 | Ctrl+L 清空输入",
                tw,
                "⌨ Ctrl+1-9 切換標籤 | Ctrl+N 新建對話 | Ctrl+L 清空輸入"
            ),
        );
        m.insert(
            "app.connecting",
            tr!(en, "Connecting...", cn, "连接中...", tw, "連接中..."),
        );

        // Chat
        m.insert("chat.title", tr!(en, "Chat", cn, "对话", tw, "對話"));
        m.insert("chat.phase", tr!(en, "Phase", cn, "阶段", tw, "階段"));
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
        m.insert("about.unknown", tr!(en, "unknown", cn, "未知", tw, "未知"));
        m.insert(
            "about.external",
            tr!(en, "external", cn, "外部进程", tw, "外部進程"),
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
            "about.backendRelease",
            tr!(en, "release", cn, "正式版", tw, "正式版"),
        );
        m.insert(
            "about.projectTitle",
            tr!(
                en,
                "Go-On AI Orchestrator",
                cn,
                "Go-On AI 编排器",
                tw,
                "Go-On AI 編排器"
            ),
        );
        m.insert(
            "about.projectDescription",
            tr!(en, "An open-source ACP/MCP intelligent agent orchestration runtime with cross-platform GUI.", cn, "一个开源的 ACP/MCP 智能体编排运行时，附带跨平台图形界面。", tw, "一個開源的 ACP/MCP 智能體編排執行時，附帶跨平台圖形介面。"),
        );
        m.insert(
            "about.guiVersion",
            tr!(en, "GUI Version", cn, "GUI 版本", tw, "GUI 版本"),
        );

        // Status
        m.insert(
            "status.connected",
            tr!(en, "Connected", cn, "已连接", tw, "已連接"),
        );
        m.insert(
            "status.disconnected",
            tr!(en, "Disconnected", cn, "未连接", tw, "未連接"),
        );
        m.insert("status.idle", tr!(en, "Idle", cn, "空闲", tw, "空閒"));
        m.insert(
            "status.processing",
            tr!(en, "Processing", cn, "处理中", tw, "處理中"),
        );
        m.insert(
            "status.error",
            tr!(en, "Error", cn, "错误", tw, "錯誤"),
        );
        m.insert(
            "status.ready",
            tr!(en, "Ready", cn, "就绪", tw, "就緒"),
        );

        // Provider labels
        m.insert(
            "provider.openai",
            tr!(en, "OpenAI", cn, "OpenAI", tw, "OpenAI"),
        );
        m.insert(
            "provider.openai_compatible",
            tr!(en, "OpenAI Compatible", cn, "OpenAI 兼容", tw, "OpenAI 兼容"),
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
            tr!(en, "DeepSeek", cn, "DeepSeek", tw, "DeepSeek"),
        );
        m.insert(
            "provider.wenxin",
            tr!(en, "Wenxin (百度)", cn, "文心一言", tw, "文心一言"),
        );
        m.insert(
            "provider.qianfan",
            tr!(en, "Qianfan (百度)", cn, "千帆", tw, "千帆"),
        );
        m.insert("provider.qwen", tr!(en, "Qwen (通义)", cn, "通义千问", tw, "通義千問"));
        m.insert("provider.glm", tr!(en, "GLM (智谱)", cn, "智谱 GLM", tw, "智譜 GLM"));
        m.insert("provider.yi", tr!(en, "Yi (零一)", cn, "零一万物", tw, "零一萬物"));
        m.insert(
            "provider.hunyuan",
            tr!(en, "Hunyuan (腾讯)", cn, "腾讯混元", tw, "騰訊混元"),
        );
        m.insert(
            "provider.doubao",
            tr!(en, "Doubao (字节)", cn, "豆包", tw, "豆包"),
        );
        m.insert(
            "provider.gemini",
            tr!(en, "Gemini", cn, "Gemini", tw, "Gemini"),
        );
        m.insert(
            "provider.groq",
            tr!(en, "Groq", cn, "Groq", tw, "Groq"),
        );
        m.insert(
            "provider.mistral",
            tr!(en, "Mistral", cn, "Mistral", tw, "Mistral"),
        );
        m.insert(
            "provider.copilot",
            tr!(en, "GitHub Copilot", cn, "GitHub Copilot", tw, "GitHub Copilot"),
        );
        m.insert(
            "provider.facewall",
            tr!(en, "FaceWall", cn, "面壁智能", tw, "面壁智能"),
        );
        m.insert(
            "provider.langboat",
            tr!(en, "Langboat", cn, "澜舟科技", tw, "瀾舟科技"),
        );
        m.insert(
            "provider.skywork",
            tr!(en, "Skywork", cn, "天工", tw, "天工"),
        );
        m.insert(
            "provider.stepfun",
            tr!(en, "StepFun", cn, "阶跃星辰", tw, "階躍星辰"),
        );
        m.insert(
            "provider.xihu",
            tr!(en, "Xihu (西湖)", cn, "西湖大模型", tw, "西湖大模型"),
        );
        m.insert(
            "provider.moonshot",
            tr!(en, "Moonshot (月之)", cn, "月之暗面", tw, "月之暗面"),
        );
        m.insert(
            "provider.minimax",
            tr!(en, "MiniMax", cn, "MiniMax", tw, "MiniMax"),
        );
        m.insert(
            "provider.ai21",
            tr!(en, "AI21", cn, "AI21", tw, "AI21"),
        );
        m.insert(
            "provider.aleph",
            tr!(en, "Aleph Alpha", cn, "Aleph Alpha", tw, "Aleph Alpha"),
        );
        m.insert(
            "provider.deepquest",
            tr!(en, "DeepSeek(DeepQuest)", cn, "DeepSeek(DeepQuest)", tw, "DeepSeek(DeepQuest)"),
        );
        m.insert(
            "provider.fireworks",
            tr!(en, "Fireworks", cn, "Fireworks", tw, "Fireworks"),
        );
        m.insert(
            "provider.llama",
            tr!(en, "Llama.cpp", cn, "Llama.cpp", tw, "Llama.cpp"),
        );
        m.insert(
            "provider.loopai",
            tr!(en, "Loop AI", cn, "Loop AI", tw, "Loop AI"),
        );
        m.insert(
            "provider.nim",
            tr!(en, "Nvidia NIM", cn, "Nvidia NIM", tw, "Nvidia NIM"),
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
            tr!(en, "Titan", cn, "Titan", tw, "Titan"),
        );
        m.insert(
            "provider.together",
            tr!(en, "Together", cn, "Together", tw, "Together"),
        );

        // Phase labels
        m.insert(
            "phase.think",
            tr!(en, "Think", cn, "思考", tw, "思考"),
        );
        m.insert(
            "phase.coding",
            tr!(en, "Coding", cn, "编码", tw, "編碼"),
        );
        m.insert(
            "phase.review",
            tr!(en, "Review", cn, "审查", tw, "審查"),
        );
        m.insert(
            "phase.test",
            tr!(en, "Test", cn, "测试", tw, "測試"),
        );

        // Mode labels
        m.insert("mode.ask", tr!(en, "Ask", cn, "问答", tw, "問答"));
        m.insert("mode.plan", tr!(en, "Plan", cn, "规划", tw, "規劃"));
        m.insert("mode.edit", tr!(en, "Edit", cn, "编辑", tw, "編輯"));
        m.insert("mode.full_auto", tr!(en, "Full Auto", cn, "全自动", tw, "全自動"));
        m.insert(
            "mode.safeguard",
            tr!(en, "Safeguard", cn, "安全审查", tw, "安全審查"),
        );

        // AutoTune
        m.insert(
            "autotune.hint",
            tr!(
                en,
                "Adjust inference parameters for the active model.",
                cn,
                "调整当前模型的推理参数。",
                tw,
                "調整當前模型的推理參數。"
            ),
        );
        m.insert(
            "autotune.temperature",
            tr!(en, "Temperature", cn, "温度", tw, "溫度"),
        );
        m.insert(
            "autotune.topP",
            tr!(en, "Top P", cn, "Top P", tw, "Top P"),
        );
        m.insert(
            "autotune.maxTokens",
            tr!(en, "Max Tokens", cn, "最大 Token", tw, "最大 Token"),
        );
        m.insert("autotune.aggressive", tr!(en, "Aggressive", cn, "激进", tw, "激進"));
        m.insert(
            "autotune.resetDefaults",
            tr!(en, "Reset to Defaults", cn, "恢复默认", tw, "恢復默認"),
        );
        m.insert(
            "autotune.saved",
            tr!(en, "✓ Saved", cn, "✓ 已保存", tw, "✓ 已保存"),
        );

        // Security
        m.insert(
            "security.hint",
            tr!(
                en,
                "Configure security preferences to protect your data.",
                cn,
                "配置安全偏好设置以保护数据安全。",
                tw,
                "配置安全偏好設定以保護數據安全。"
            ),
        );
        m.insert(
            "security.confirmDangerousActions",
            tr!(
                en,
                "Confirm before dangerous actions",
                cn,
                "危险操作前确认",
                tw,
                "危險操作前確認"
            ),
        );
        m.insert(
            "security.redactApiKeys",
            tr!(
                en,
                "Redact API keys in UI",
                cn,
                "在界面中遮盖 API 密钥",
                tw,
                "在界面中遮蓋 API 密鑰"
            ),
        );
        m.insert(
            "security.blockExternalUrls",
            tr!(
                en,
                "Block external URLs",
                cn,
                "阻止外部 URL",
                tw,
                "阻止外部 URL"
            ),
        );
        m.insert(
            "security.saved",
            tr!(en, "Security settings saved.", cn, "安全设置已保存。", tw, "安全設置已保存。"),
        );
        m.insert(
            "security.restart",
            tr!(en, "Restart Backend", cn, "重启后端", tw, "重啟後端"),
        );
        m.insert(
            "security.restartRequested",
            tr!(
                en,
                "Backend restart requested.",
                cn,
                "已请求重启后端。",
                tw,
                "已請求重啟後端。"
            ),
        );
        m.insert(
            "security.restartFailed",
            tr!(
                en,
                "Backend restart failed",
                cn,
                "后端重启失败",
                tw,
                "後端重啟失敗"
            ),
        );
        m.insert(
            "security.confirmRestart",
            tr!(
                en,
                "Confirm Restart",
                cn,
                "确认重启",
                tw,
                "確認重啟"
            ),
        );

        // Provisioning
        m.insert(
            "setup.title",
            tr!(en, "Setup Go-On", cn, "设置 Go-On", tw, "設置 Go-On"),
        );
        m.insert(
            "setup.hint",
            tr!(
                en,
                "Add an AI provider to get started.",
                cn,
                "添加一个 AI 提供商以开始使用。",
                tw,
                "添加一個 AI 提供商以開始使用。"
            ),
        );
        m.insert("setup.provider", tr!(en, "Provider:", cn, "提供商:", tw, "提供商:"));
        m.insert(
            "setup.apiKey",
            tr!(en, "API Key:", cn, "API 密钥:", tw, "API 密鑰:"),
        );
        m.insert("setup.model", tr!(en, "Model:", cn, "模型:", tw, "模型:"));
        m.insert("setup.save", tr!(en, "Save", cn, "保存", tw, "保存"));
        m.insert("setup.skip", tr!(en, "Skip", cn, "跳过", tw, "跳過"));
        m.insert(
            "setup.success",
            tr!(en, "Configured successfully!", cn, "配置成功！", tw, "配置成功！"),
        );
        m.insert(
            "setup.saveError",
            tr!(
                en,
                "Failed to save configuration. Check disk space and permissions.",
                cn,
                "保存配置失败。请检查磁盘空间和权限。",
                tw,
                "保存配置失敗。請檢查磁碟空間和權限。"
            ),
        );
        m.insert(
            "setup.environment",
            tr!(en, "Environment:", cn, "环境:", tw, "環境:"),
        );
        m.insert(
            "setup.secretSource",
            tr!(en, "Secret source:", cn, "密钥来源:", tw, "密鑰來源:"),
        );

        // Chat strings
        m.insert(
            "chat.newSession",
            tr!(en, "New Chat", cn, "新对话", tw, "新對話"),
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
        m.insert(
            "chat.attach",
            tr!(en, "Attach file", cn, "附件", tw, "附件"),
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
        m.insert(
            "chat.generateWorkflow",
            tr!(en, "Generate Workflow", cn, "生成工作流", tw, "生成工作流"),
        );
        m.insert(
            "chat.generateWorkflowHint",
            tr!(
                en,
                "Auto-generate a reusable workflow from this conversation",
                cn,
                "从此对话自动生成可复用工作流",
                tw,
                "從此對話自動生成可復用工作流"
            ),
        );
        m.insert(
            "chat.noMessagesForWorkflow",
            tr!(
                en,
                "No user messages to analyze for workflow generation",
                cn,
                "没有可供分析生成工作流的用户消息",
                tw,
                "沒有可供分析生成工作流的用戶消息"
            ),
        );
        m.insert(
            "chat.workflowGenerated",
            tr!(
                en,
                "Workflow generated: {workflow}",
                cn,
                "工作流已生成: {workflow}",
                tw,
                "工作流已生成: {workflow}"
            ),
        );
        m.insert(
            "chat.workflowGenError",
            tr!(
                en,
                "Workflow generation failed: {error}",
                cn,
                "工作流生成失败: {error}",
                tw,
                "工作流生成失敗: {error}"
            ),
        );
        m.insert(
            "chat.modelAutoOnly",
            tr!(
                en,
                "(auto mode: backend selects model)",
                cn,
                "（自动模式：后端选择模型）",
                tw,
                "（自動模式：後端選擇模型）"
            ),
        );
        m.insert(
            "chat.multiModelEnabled",
            tr!(
                en,
                "(multi-model mode: selections enabled)",
                cn,
                "（多模型模式：可选择多个模型）",
                tw,
                "（多模型模式：可選擇多個模型）"
            ),
        );
        m.insert(
            "chat.chooseModels",
            tr!(
                en,
                "Choose models for this conversation",
                cn,
                "选择对话模型",
                tw,
                "選擇對話模型"
            ),
        );
        m.insert(
            "chat.searchSessions",
            tr!(
                en,
                "Search sessions...",
                cn,
                "搜索对话...",
                tw,
                "搜索對話..."
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
                "搜索模板..."
            ),
        );
        m.insert(
            "chat.rename",
            tr!(en, "Rename", cn, "重命名", tw, "重命名"),
        );
        m.insert(
            "chat.exportSuccess",
            tr!(en, "Export succeeded.", cn, "导出成功。", tw, "導出成功。"),
        );
        m.insert(
            "chat.exportFailed",
            tr!(en, "Export failed.", cn, "导出失败。", tw, "導出失敗。"),
        );
        m.insert(
            "chat.tokenStats",
            tr!(en, "Token Stats", cn, "Token 统计", tw, "Token 統計"),
        );
        m.insert(
            "chat.responseTime",
            tr!(en, "Response Time", cn, "响应时间", tw, "響應時間"),
        );
        m.insert(
            "chat.tokens",
            tr!(en, "Tokens", cn, "Token 数", tw, "Token 數"),
        );
        m.insert(
            "chat.successRate",
            tr!(en, "Success Rate", cn, "成功率", tw, "成功率"),
        );
        m.insert(
            "chat.tokensPerMinute",
            tr!(en, "Tokens/min", cn, "Token/分钟", tw, "Token/分鐘"),
        );
        m.insert(
            "chat.clear",
            tr!(en, "Clear", cn, "清空", tw, "清空"),
        );
        m.insert(
            "chat.clearAll",
            tr!(en, "Clear All", cn, "清空所有", tw, "清空所有"),
        );
        m.insert(
            "chat.cannotDeleteLastSession",
            tr!(
                en,
                "Cannot delete the last session.",
                cn,
                "不能删除最后一个对话。",
                tw,
                "不能刪除最後一個對話。"
            ),
        );

        // Risk Decision (used in render_risk_decision_summary)
        m.insert(
            "chat.riskDecisionTitle",
            tr!(en, "Risk Decision", cn, "风险决策", tw, "風險決策"),
        );
        m.insert(
            "chat.riskDecisionHigh",
            tr!(en, "High Risk", cn, "高风险", tw, "高風險"),
        );
        m.insert(
            "chat.riskDecisionNormal",
            tr!(en, "Normal", cn, "正常", tw, "正常"),
        );
        m.insert(
            "chat.riskDecisionReviewRequired",
            tr!(en, "Review Required", cn, "需要审查", tw, "需要審查"),
        );
        m.insert(
            "chat.riskDecisionNoReview",
            tr!(en, "No Review Needed", cn, "无需审查", tw, "無需審查"),
        );
        m.insert(
            "chat.riskDecisionState",
            tr!(en, "State", cn, "状态", tw, "狀態"),
        );
        m.insert(
            "chat.riskDecisionReview",
            tr!(en, "Review", cn, "审查", tw, "審查"),
        );
        m.insert(
            "chat.riskDecisionStrategy",
            tr!(en, "Strategy", cn, "策略", tw, "策略"),
        );
        m.insert(
            "chat.riskDecisionReasons",
            tr!(en, "Reasons", cn, "原因", tw, "原因"),
        );

        // Template editor
        m.insert(
            "chat.templateName",
            tr!(en, "Template Name", cn, "模板名称", tw, "模板名稱"),
        );
        m.insert(
            "chat.templateCommand",
            tr!(
                en,
                "Command (e.g., /explain)",
                cn,
                "命令（如 /explain）",
                tw,
                "命令（如 /explain）"
            ),
        );
        m.insert(
            "chat.templateBody",
            tr!(en, "Template Body", cn, "模板内容", tw, "模板內容"),
        );
        m.insert(
            "chat.templatePlaceholderHint",
            tr!(
                en,
                "Use {{input}} as placeholder for user arguments",
                cn,
                "使用 {{input}} 作为用户输入占位符",
                tw,
                "使用 {{input}} 作為用戶輸入佔位符"
            ),
        );
        m.insert(
            "chat.templateSave",
            tr!(en, "Save Template", cn, "保存模板", tw, "保存模板"),
        );
        m.insert(
            "chat.templateDelete",
            tr!(en, "Delete", cn, "删除", tw, "刪除"),
        );
        m.insert(
            "chat.templateValidation",
            tr!(
                en,
                "Name and command are required.",
                cn,
                "名称和命令必填。",
                tw,
                "名稱和命令必填。"
            ),
        );
        m.insert(
            "chat.templateNew",
            tr!(en, "New Template", cn, "新建模板", tw, "新建模板"),
        );
        m.insert(
            "chat.templateInsert",
            tr!(en, "Insert into message", cn, "插入消息", tw, "插入消息"),
        );
        m.insert(
            "chat.templateDuplicate",
            tr!(en, "Duplicate", cn, "复制", tw, "複製"),
        );
        m.insert(
            "chat.promptTemplates",
            tr!(en, "Prompt Templates", cn, "提示模板", tw, "提示模板"),
        );

        // Providers
        m.insert(
            "providers.copilot_authorize",
            tr!(en, "Authorize GitHub Copilot", cn, "授权 GitHub Copilot", tw, "授權 GitHub Copilot"),
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
        m.insert(
            "providers.noKey",
            tr!(
                en,
                "No API key configured for this provider.",
                cn,
                "未配置 API 密钥。",
                tw,
                "未配置 API 密鑰。"
            ),
        );
        m.insert(
            "providers.secret_key",
            tr!(en, "Secret Key", cn, "Secret Key", tw, "Secret Key"),
        );

        // ── Copilot OAuth Device Code ──
        m.insert(
            "providers.copilot_requesting",
            tr!(
                en,
                "Requesting device code from GitHub…",
                cn,
                "正在请求 GitHub 设备代码…",
                tw,
                "正在請求 GitHub 設備代碼…"
            ),
        );
        m.insert(
            "providers.copilot_open_url",
            tr!(
                en,
                "1. Open this URL in your browser:",
                cn,
                "1. 在浏览器中打开此链接:",
                tw,
                "1. 在瀏覽器中打開此連結:"
            ),
        );
        m.insert(
            "providers.copilot_enter_code",
            tr!(
                en,
                "2. Enter this code",
                cn,
                "2. 输入此代码",
                tw,
                "2. 輸入此代碼"
            ),
        );
        m.insert(
            "providers.copilot_waiting",
            tr!(
                en,
                "Waiting for authorization…",
                cn,
                "等待授权中…",
                tw,
                "等待授權中…"
            ),
        );
        m.insert(
            "providers.copilot_authorized",
            tr!(
                en,
                "✓ Authorized successfully!",
                cn,
                "✓ 授权成功！",
                tw,
                "✓ 授權成功！"
            ),
        );
        m.insert(
            "providers.copilot_expired",
            tr!(
                en,
                "✗ Device code expired. Please try again.",
                cn,
                "✗ 设备码已过期，请重试。",
                tw,
                "✗ 設備碼已過期，請重試。"
            ),
        );
        m.insert(
            "providers.copilot_denied",
            tr!(
                en,
                "✗ Authorization denied.",
                cn,
                "✗ 授权已拒绝。",
                tw,
                "✗ 授權已拒絕。"
            ),
        );
        m.insert(
            "providers.copilot_cancel",
            tr!(en, "Cancel", cn, "取消", tw, "取消"),
        );
        m.insert(
            "providers.copilot_clear",
            tr!(en, "Clear", cn, "清除", tw, "清除"),
        );
        m.insert(
            "providers.copilot_retry",
            tr!(en, "Retry", cn, "重试", tw, "重試"),
        );
        m.insert(
            "providers.ops.testConn",
            tr!(en, "Test Connection", cn, "测试连接", tw, "測試連接"),
        );
        m.insert(
            "providers.ops.testCompletion",
            tr!(en, "Test Completion", cn, "测试补全", tw, "測試補全"),
        );
        m.insert(
            "providers.ops.connStatus",
            tr!(
                en,
                "Connection: {status}",
                cn,
                "连接: {status}",
                tw,
                "連接: {status}"
            ),
        );
        m.insert(
            "providers.ops.connStatusFailed",
            tr!(
                en,
                "Connection check failed: {error}",
                cn,
                "连接检查失败: {error}",
                tw,
                "連接檢查失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.completionStatus",
            tr!(
                en,
                "Completion: {status}",
                cn,
                "补全: {status}",
                tw,
                "補全: {status}"
            ),
        );
        m.insert(
            "providers.ops.completionStatusFailed",
            tr!(
                en,
                "Completion test failed: {error}",
                cn,
                "补全测试失败: {error}",
                tw,
                "補全測試失敗: {error}"
            ),
        );
        m.insert(
            "providers.ops.capabilitiesCount",
            tr!(
                en,
                "Capabilities: {count} models",
                cn,
                "能力: {count} 个模型",
                tw,
                "能力: {count} 個模型"
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
                "Capabilities encoding error: {error}",
                cn,
                "能力编码错误: {error}",
                tw,
                "能力編碼錯誤: {error}"
            ),
        );

        // ── UI Stability keys ──────────────────────────────
        m.insert(
            "settings.uiStability.title",
            tr!(
                en,
                "UI Stability",
                cn,
                "界面稳定性",
                tw,
                "界面穩定性"
            ),
        );
        m.insert(
            "settings.uiStability.balanced",
            tr!(en, "Balanced", cn, "均衡", tw, "均衡"),
        );
        m.insert(
            "settings.uiStability.stable",
            tr!(en, "Stable", cn, "稳定", tw, "穩定"),
        );
        m.insert(
            "settings.uiStability.low_end",
            tr!(en, "Low-end Device", cn, "低端设备", tw, "低端設備"),
        );
        m.insert(
            "settings.uiStability.low_latency",
            tr!(
                en,
                "Low Latency",
                cn,
                "低延迟",
                tw,
                "低延遲"
            ),
        );
        m.insert(
            "settings.uiStability.custom",
            tr!(en, "Custom", cn, "自定义", tw, "自定義"),
        );
        m.insert(
            "settings.uiStability.preset",
            tr!(
                en,
                "UI Stability Preset",
                cn,
                "界面稳定性预设",
                tw,
                "界面穩定性預設"
            ),
        );
        m.insert(
            "settings.uiStability.hint",
            tr!(
                en,
                "Balanced = smooth UI, Stable = lower repaint, Low-end = minimal CPU, Low Latency = fast streaming",
                cn,
                "均衡=流畅界面, 稳定=降低刷新, 低端=最小CPU, 低延迟=快速流式",
                tw,
                "均衡=流暢界面, 穩定=降低刷新, 低端=最小CPU, 低延遲=快速流式"
            ),
        );
        m.insert(
            "settings.uiStability.refreshInterval",
            tr!(
                en,
                "Backend Refresh Interval (sec)",
                cn,
                "后端刷新间隔（秒）",
                tw,
                "後端刷新間隔（秒）"
            ),
        );
        m.insert(
            "settings.uiStability.commitDebounce",
            tr!(
                en,
                "UI Commit Debounce (ms)",
                cn,
                "UI 提交防抖（毫秒）",
                tw,
                "UI 提交防抖（毫秒）"
            ),
        );
        m.insert(
            "settings.uiStability.disconnectDebounce",
            tr!(
                en,
                "Disconnect Debounce Count",
                cn,
                "断开防抖次数",
                tw,
                "斷開防抖次數"
            ),
        );
        m.insert(
            "settings.uiStability.chunkFlush",
            tr!(
                en,
                "Stream Chunk Flush (ms)",
                cn,
                "流式块刷新（毫秒）",
                tw,
                "流式塊刷新（毫秒）"
            ),
        );
        m.insert(
            "settings.uiStability.repaintInterval",
            tr!(
                en,
                "Chat Repaint Interval (ms)",
                cn,
                "聊天重绘间隔（毫秒）",
                tw,
                "聊天重繪間隔（毫秒）"
            ),
        );
        m.insert(
            "settings.uiStability.maxPending",
            tr!(
                en,
                "Max Pending Events / Frame",
                cn,
                "每帧最大待处理事件",
                tw,
                "每幀最大待處理事件"
            ),
        );

        // Settings
        m.insert(
            "settings.title",
            tr!(en, "Settings", cn, "设置", tw, "設置"),
        );
        m.insert(
            "settings.hint",
            tr!(
                en,
                "Configure application preferences.",
                cn,
                "配置应用偏好。",
                tw,
                "配置應用偏好。"
            ),
        );
        m.insert(
            "settings.backendUrl",
            tr!(en, "Backend URL", cn, "后端 URL", tw, "後端 URL"),
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
            "settings.backendUrlHint",
            tr!(
                en,
                "Press Enter or click Restart to apply changes.",
                cn,
                "按 Enter 或点击重启以应用更改。",
                tw,
                "按 Enter 或點擊重啟以應用更改。"
            ),
        );
        m.insert(
            "settings.theme",
            tr!(en, "Theme", cn, "主题", tw, "主題"),
        );
        m.insert(
            "settings.language",
            tr!(en, "Language", cn, "语言", tw, "語言"),
        );
        m.insert(
            "settings.resetDefaults",
            tr!(
                en,
                "Reset to Defaults",
                cn,
                "恢复默认设置",
                tw,
                "恢復默認設置"
            ),
        );
        m.insert(
            "settings.section.core",
            tr!(en, "Core", cn, "核心", tw, "核心"),
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
            "settings.section.advanced",
            tr!(en, "Advanced", cn, "高级", tw, "高級"),
        );
        m.insert(
            "settings.section.backend",
            tr!(en, "Backend", cn, "后端", tw, "後端"),
        );
        m.insert(
            "settings.section.system",
            tr!(en, "System", cn, "系统", tw, "系統"),
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
                "自动调优链注入",
                tw,
                "自動調優鏈注入"
            ),
        );
        m.insert(
            "settings.feature.skillsLifecycle",
            tr!(
                en,
                "Skills Version Lifecycle",
                cn,
                "技能版本生命周期",
                tw,
                "技能版本生命週期"
            ),
        );
        m.insert(
            "settings.feature.providersOps",
            tr!(
                en,
                "Providers Ops Panel",
                cn,
                "提供商运维面板",
                tw,
                "提供商運維面板"
            ),
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
            tr!(
                en,
                "Enterprise Setup",
                cn,
                "企业级设置",
                tw,
                "企業級設置"
            ),
        );
        m.insert(
            "settings.enterprise.secretSource",
            tr!(
                en,
                "Secret source:",
                cn,
                "密钥来源:",
                tw,
                "密鑰來源:"
            ),
        );
        m.insert(
            "settings.enterprise.importPath",
            tr!(
                en,
                "Import path:",
                cn,
                "导入路径:",
                tw,
                "導入路徑:"
            ),
        );
        m.insert(
            "settings.enterprise.exportPath",
            tr!(
                en,
                "Export path:",
                cn,
                "导出路径:",
                tw,
                "導出路徑:"
            ),
        );
        m.insert(
            "settings.enterprise.exportMasked",
            tr!(
                en,
                "Export (masked keys)",
                cn,
                "导出（遮盖密钥）",
                tw,
                "導出（遮蓋密鑰）"
            ),
        );
        m.insert(
            "settings.enterprise.exportFull",
            tr!(
                en,
                "Export (full keys)",
                cn,
                "导出（完整密钥）",
                tw,
                "導出（完整密鑰）"
            ),
        );
        m.insert(
            "settings.enterprise.importConfig",
            tr!(
                en,
                "Import Config",
                cn,
                "导入配置",
                tw,
                "導入配置"
            ),
        );
        m.insert(
            "settings.enterprise.syncCurrent",
            tr!(
                en,
                "Sync Current",
                cn,
                "同步当前",
                tw,
                "同步當前"
            ),
        );
        m.insert(
            "settings.enterprise.environmentUrl",
            tr!(
                en,
                "Environment URL:",
                cn,
                "环境 URL:",
                tw,
                "環境 URL:"
            ),
        );

        // Workflow
        m.insert(
            "workflow.hint",
            tr!(
                en,
                "Configure and manage workflow automation.",
                cn,
                "配置和管理工作流自动化。",
                tw,
                "配置和管理工作流自動化。"
            ),
        );
        m.insert(
            "workflow.confirmDelete",
            tr!(
                en,
                "Delete this step?",
                cn,
                "删除此步骤？",
                tw,
                "刪除此步驟？"
            ),
        );
        m.insert(
            "workflow.noSteps",
            tr!(
                en,
                "No workflow steps defined.",
                cn,
                "未定义工作流步骤。",
                tw,
                "未定義工作流步驟。"
            ),
        );
        m.insert(
            "workflow.confirmRun",
            tr!(
                en,
                "Run this workflow?",
                cn,
                "运行此工作流？",
                tw,
                "運行此工作流？"
            ),
        );
        m.insert(
            "workflow.deleteConfirmAgain",
            tr!(
                en,
                "Click delete again to confirm.",
                cn,
                "再次点击删除以确认。",
                tw,
                "再次點擊刪除以確認。"
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
        m.insert(
            "workflow.runActionRequested",
            tr!(
                en,
                "Run action sent.",
                cn,
                "运行指令已发送。",
                tw,
                "運行指令已發送。"
            ),
        );
        m.insert(
            "workflow.runActionFailed",
            tr!(
                en,
                "Run action failed: {error}",
                cn,
                "运行失败: {error}",
                tw,
                "運行失敗: {error}"
            ),
        );
        m.insert(
            "workflow.executionError",
            tr!(
                en,
                "Execution error: {error}",
                cn,
                "执行错误: {error}",
                tw,
                "執行錯誤: {error}"
            ),
        );
        m.insert(
            "workflow.stepFailure",
            tr!(
                en,
                "Step failed: {error}",
                cn,
                "步骤失败: {error}",
                tw,
                "步驟失敗: {error}"
            ),
        );
        m.insert(
            "workflow.stepTimeout",
            tr!(
                en,
                "Step timed out",
                cn,
                "步骤超时",
                tw,
                "步驟超時"
            ),
        );
        m.insert(
            "workflow.noOutput",
            tr!(en, "No output", cn, "无输出", tw, "無輸出"),
        );
        m.insert(
            "workflow.noEnabledSteps",
            tr!(
                en,
                "No enabled steps to run.",
                cn,
                "没有启用的工作流步骤。",
                tw,
                "沒有啟用的工作流步驟。"
            ),
        );

        // Workflow Run Center
        m.insert(
            "workflow.runCenter.title",
            tr!(en, "Workflow Run Center", cn, "工作流运行中心", tw, "工作流運行中心"),
        );
        m.insert(
            "workflow.runCenter.hidden",
            tr!(
                en,
                "Workflow Run Center is hidden (enable in Settings > Features).",
                cn,
                "工作流运行中心已隐藏（在设置 > 功能中启用）。",
                tw,
                "工作流運行中心已隱藏（在設置 > 功能中啟用）。"
            ),
        );
        m.insert(
            "workflow.runCenter.refresh",
            tr!(en, "Refresh", cn, "刷新", tw, "刷新"),
        );
        m.insert(
            "workflow.runCenter.decodeFailed",
            tr!(
                en,
                "Failed to decode run detail response.",
                cn,
                "解码运行详情响应失败。",
                tw,
                "解碼運行詳情響應失敗。"
            ),
        );

        // Workflow run statuses
        m.insert(
            "workflow.runStatus.all",
            tr!(en, "All", cn, "全部", tw, "全部"),
        );
        m.insert(
            "workflow.runStatus.running",
            tr!(en, "Running", cn, "运行中", tw, "運行中"),
        );
        m.insert(
            "workflow.runStatus.succeeded",
            tr!(en, "Succeeded", cn, "成功", tw, "成功"),
        );
        m.insert(
            "workflow.runStatus.failed",
            tr!(en, "Failed", cn, "失败", tw, "失敗"),
        );
        m.insert(
            "workflow.runStatus.cancelled",
            tr!(en, "Cancelled", cn, "已取消", tw, "已取消"),
        );
        m.insert(
            "workflow.runStatus.queued",
            tr!(en, "Queued", cn, "排队中", tw, "排隊中"),
        );
        m.insert(
            "workflow.runStatus.paused",
            tr!(en, "Paused", cn, "已暂停", tw, "已暫停"),
        );

        // Workflow details
        m.insert(
            "workflow.status",
            tr!(en, "Status", cn, "状态", tw, "狀態"),
        );
        m.insert(
            "workflow.createdAt",
            tr!(en, "Created At", cn, "创建时间", tw, "創建時間"),
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
            "workflow.activeSummary",
            tr!(
                en,
                "Active Summary: {count} steps",
                cn,
                "活动摘要: {count} 步骤",
                tw,
                "活動摘要: {count} 步驟"
            ),
        );
        m.insert(
            "workflow.estimatedRemaining",
            tr!(
                en,
                "Estimated {secs}s remaining",
                cn,
                "预计剩余 {secs} 秒",
                tw,
                "預計剩餘 {secs} 秒"
            ),
        );

        // Skills
        m.insert(
            "skills.fetchFailed",
            tr!(
                en,
                "Failed to fetch skills: {error}",
                cn,
                "获取技能失败: {error}",
                tw,
                "獲取技能失敗: {error}"
            ),
        );
        m.insert(
            "skills.defaultCreator.title",
            tr!(
                en,
                "Built-in Prompt Catalog",
                cn,
                "内置提示目录",
                tw,
                "內置提示目錄"
            ),
        );
        m.insert(
            "skills.defaultCreator.loaded",
            tr!(
                en,
                "System built-in skill loaded.",
                cn,
                "系统内置技能已加载。",
                tw,
                "系統內置技能已加載。"
            ),
        );
        m.insert(
            "skills.defaultCreator.button",
            tr!(
                en,
                "Load Built-in Catalog",
                cn,
                "加载内置目录",
                tw,
                "加載內置目錄"
            ),
        );
        m.insert(
            "skills.defaultCreator.description",
            tr!(
                en,
                "Load the built-in prompt catalog as skills.",
                cn,
                "将内置提示目录加载为技能。",
                tw,
                "將內置提示目錄加載為技能。"
            ),
        );
        m.insert(
            "skills.error.invalidSchema",
            tr!(
                en,
                "Invalid input schema JSON: {error}",
                cn,
                "无效的输入 schema JSON: {error}",
                tw,
                "無效的輸入 schema JSON: {error}"
            ),
        );
        m.insert(
            "skills.error.invalidSchemaObject",
            tr!(
                en,
                "Input schema must be a JSON object.",
                cn,
                "input_schema 必须是一个 JSON 对象。",
                tw,
                "input_schema 必須是一個 JSON 對象。"
            ),
        );
        m.insert(
            "skills.error.invalidTestInput",
            tr!(
                en,
                "Invalid test input JSON: {error}",
                cn,
                "无效的测试输入 JSON: {error}",
                tw,
                "無效的測試輸入 JSON: {error}"
            ),
        );
        m.insert(
            "skills.create.errorName",
            tr!(
                en,
                "Skill name is required.",
                cn,
                "技能名称必填。",
                tw,
                "技能名稱必填。"
            ),
        );
        m.insert(
            "skills.create.errorPrompt",
            tr!(
                en,
                "Prompt template is required.",
                cn,
                "提示模板必填。",
                tw,
                "提示模板必填。"
            ),
        );

        // Skills import
        m.insert(
            "skills.import.invalidUrl",
            tr!(
                en,
                "Invalid import URL: {url}",
                cn,
                "无效的导入 URL: {url}",
                tw,
                "無效的導入 URL: {url}"
            ),
        );
        m.insert(
            "skills.import.httpClientError",
            tr!(
                en,
                "HTTP client error: {error}",
                cn,
                "HTTP 客户端错误: {error}",
                tw,
                "HTTP 客戶端錯誤: {error}"
            ),
        );
        m.insert(
            "skills.import.httpStatusError",
            tr!(
                en,
                "HTTP {status} error importing from {url}",
                cn,
                "从 {url} 导入时 HTTP {status} 错误",
                tw,
                "從 {url} 導入時 HTTP {status} 錯誤"
            ),
        );
        m.insert(
            "skills.import.fetchError",
            tr!(
                en,
                "Failed to fetch manifest from {url}: {error}",
                cn,
                "从 {url} 获取清单失败: {error}",
                tw,
                "從 {url} 獲取清單失敗: {error}"
            ),
        );
        m.insert(
            "skills.import.invalidManifest",
            tr!(
                en,
                "Invalid manifest from {url}",
                cn,
                "来自 {url} 的清单无效",
                tw,
                "來自 {url} 的清單無效"
            ),
        );
        m.insert(
            "skills.import.missingPromptTemplate",
            tr!(
                en,
                "Manifest missing prompt_template field.",
                cn,
                "清单缺少 prompt_template 字段。",
                tw,
                "清單缺少 prompt_template 字段。"
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

        // Skills lifecycle
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
            "skills.lifecycle.versions",
            tr!(en, "Versions", cn, "版本", tw, "版本"),
        );

        // ── Feature toggle labels ──────────────────────────
        m.insert(
            "feature.monitor.desc",
            tr!(
                en,
                "Real-time system monitoring dashboard with health status,\nperformance metrics, and provider readiness.",
                cn,
                "实时系统监控面板：健康状态、\n性能指标和提供商就绪状态。",
                tw,
                "實時系統監控面板：健康狀態、\n性能指標和提供商就緒狀態。"
            ),
        );
        m.insert(
            "feature.chat.desc",
            tr!(
                en,
                "AI-powered chat interface with multi-turn conversations,\nmode/phase selection, and streaming responses.",
                cn,
                "AI 驱动聊天界面：多轮对话、\n模式/阶段选择和流式响应。",
                tw,
                "AI 驅動聊天界面：多輪對話、\n模式/階段選擇和流式響應。"
            ),
        );
        m.insert(
            "feature.skills.desc",
            tr!(
                en,
                "Skill management system for creating, importing, and\nversioning reusable prompt templates.",
                cn,
                "技能管理系统：创建、导入和\n版本管理可复用的提示模板。",
                tw,
                "技能管理系統：創建、導入和\n版本管理可復用的提示模板。"
            ),
        );
        m.insert(
            "feature.workflow.desc",
            tr!(
                en,
                "Automated workflow runner with configurable steps and\nreal-time run tracking.",
                cn,
                "自动化工作流运行器：可配置步骤和\n实时运行追踪。",
                tw,
                "自動化工作流運行器：可配置步驟和\n實時運行追蹤。"
            ),
        );
        m.insert(
            "feature.autotune.desc",
            tr!(
                en,
                "Inference parameter tuning UI for temperature, top_p,\nmax_tokens, and aggressive mode.",
                cn,
                "推理参数调优界面：温度、Top P、\n最大 Token 和激进模式。",
                tw,
                "推理參數調優界面：溫度、Top P、\n最大 Token 和激進模式。"
            ),
        );
        m.insert(
            "feature.security.desc",
            tr!(
                en,
                "Security preference controls for dangerous action\nconfirmations, API key redaction, and URL blocking.",
                cn,
                "安全偏好控制：危险操作确认、\nAPI 密钥遮盖和 URL 阻止。",
                tw,
                "安全偏好控制：危險操作確認、\nAPI 密鑰遮蓋和 URL 阻止。"
            ),
        );
        m.insert(
            "feature.config.desc",
            tr!(
                en,
                "Live JSON config editor with validation, search, and\nsafe mode with snapshots/rollback.",
                cn,
                "实时 JSON 配置编辑器：验证、搜索、\n安全模式（快照/回滚）。",
                tw,
                "實時 JSON 配置編輯器：驗證、搜索、\n安全模式（快照/回滾）。"
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

        // ── Config keys ──────────────────────────────────
        m.insert(
            "config.hint",
            tr!(
                en,
                "Edit the full application configuration as raw JSON.",
                cn,
                "以原始 JSON 编辑完整应用配置。",
                tw,
                "以原始 JSON 編輯完整應用配置。"
            ),
        );
        m.insert(
            "config.reloadCurrent",
            tr!(
                en,
                "Reload Current",
                cn,
                "重新加载",
                tw,
                "重新加載"
            ),
        );
        m.insert(
            "config.createSnapshot",
            tr!(
                en,
                "Create Snapshot (save)",
                cn,
                "创建快照（保存）",
                tw,
                "創建快照（保存）"
            ),
        );
        m.insert(
            "config.applyJson",
            tr!(
                en,
                "Apply JSON",
                cn,
                "应用 JSON",
                tw,
                "應用 JSON"
            ),
        );
        m.insert(
            "config.snapshotSaved",
            tr!(
                en,
                "Snapshot saved.",
                cn,
                "快照已保存。",
                tw,
                "快照已保存。"
            ),
        );
        m.insert(
            "config.reloaded",
            tr!(
                en,
                "Reloaded current config.",
                cn,
                "已重新加载当前配置。",
                tw,
                "已重新加載當前配置。"
            ),
        );
        m.insert(
            "config.applied",
            tr!(
                en,
                "Config applied successfully.",
                cn,
                "配置已成功应用。",
                tw,
                "配置已成功應用。"
            ),
        );
        m.insert(
            "config.invalidJson",
            tr!(
                en,
                "Invalid JSON: {error}",
                cn,
                "无效 JSON: {error}",
                tw,
                "無效 JSON: {error}"
            ),
        );
        m.insert(
            "config.snapshots",
            tr!(en, "Snapshots:", cn, "快照:", tw, "快照:"),
        );
        m.insert(
            "config.rollbackSnapshot",
            tr!(
                en,
                "Rollback to Last Snapshot",
                cn,
                "回滚到上一个快照",
                tw,
                "回滾到上一個快照"
            ),
        );
        m.insert(
            "config.rolledBack",
            tr!(
                en,
                "Rolled back to snapshot.",
                cn,
                "已回滚到上一个快照。",
                tw,
                "已回滾到上一個快照。"
            ),
        );
        m.insert(
            "config.search",
            tr!(en, "🔍 Search...", cn, "🔍 搜索...", tw, "🔍 搜索..."),
        );
        m.insert(
            "config.validJson",
            tr!(
                en,
                "✓ Valid JSON",
                cn,
                "✓ 有效的 JSON",
                tw,
                "✓ 有效的 JSON"
            ),
        );
        m.insert(
            "config.safeModeHidden",
            tr!(
                en,
                "Safe mode is disabled. Edit with caution.",
                cn,
                "安全模式已关闭，请谨慎编辑。",
                tw,
                "安全模式已關閉，請謹慎編輯。"
            ),
        );

        // ── Monitor keys ─────────────────────────────────
        m.insert(
            "monitor.health",
            tr!(en, "Health", cn, "健康", tw, "健康"),
        );
        m.insert(
            "monitor.rpm",
            tr!(en, "RPM", cn, "请求/分钟", tw, "請求/分鐘"),
        );
        m.insert(
            "monitor.latency",
            tr!(en, "Latency", cn, "延迟", tw, "延遲"),
        );
        m.insert(
            "monitor.success",
            tr!(en, "Success", cn, "成功", tw, "成功"),
        );
        m.insert(
            "monitor.unhealthy",
            tr!(en, "Unhealthy", cn, "异常", tw, "異常"),
        );
        m.insert(
            "monitor.offline",
            tr!(en, "Offline", cn, "离线", tw, "離線"),
        );
        m.insert(
            "monitor.offlineHint",
            tr!(
                en,
                "Check the backend process or Settings to configure.",
                cn,
                "请检查后端进程或设置页面。",
                tw,
                "請檢查後端進程或設置頁面。"
            ),
        );
        m.insert(
            "monitor.providers",
            tr!(en, "Providers", cn, "提供商", tw, "提供商"),
        );
        m.insert(
            "monitor.filterProviders",
            tr!(
                en,
                "Filter providers...",
                cn,
                "筛选提供商...",
                tw,
                "篩選提供商..."
            ),
        );
        m.insert(
            "monitor.notReady",
            tr!(en, "No providers configured", cn, "未配置提供商", tw, "未配置提供商"),
        );
        m.insert(
            "monitor.healthy",
            tr!(en, "Healthy", cn, "健康", tw, "健康"),
        );
        m.insert(
            "monitor.loadErrors",
            tr!(
                en,
                "Failed to load errors: {error}",
                cn,
                "加载错误失败: {error}",
                tw,
                "加載錯誤失敗: {error}"
            ),
        );
        m.insert(
            "monitor.loadTrends",
            tr!(
                en,
                "Failed to load trends: {error}",
                cn,
                "加载趋势失败: {error}",
                tw,
                "加載趨勢失敗: {error}"
            ),
        );
        m.insert(
            "monitor.errorRate",
            tr!(en, "Error Rate", cn, "错误率", tw, "錯誤率"),
        );
        m.insert(
            "monitor.successRate",
            tr!(en, "Success Rate", cn, "成功率", tw, "成功率"),
        );
        m.insert(
            "monitor.trendSummary",
            tr!(
                en,
                "Trend Summary (last {window})",
                cn,
                "趋势摘要（最近 {window}）",
                tw,
                "趨勢摘要（最近 {window}）"
            ),
        );
        m.insert(
            "monitor.errorTopGroups",
            tr!(
                en,
                "Top Error Groups (last {window})",
                cn,
                "错误分组 TOP（最近 {window}）",
                tw,
                "錯誤分組 TOP（最近 {window}）"
            ),
        );
        m.insert(
            "monitor.refreshNow",
            tr!(en, "Refresh Now", cn, "立即刷新", tw, "立即刷新"),
        );
        m.insert(
            "monitor.sampleFailures",
            tr!(
                en,
                "Sample failures: {count}",
                cn,
                "采样失败: {count}",
                tw,
                "採樣失敗: {count}"
            ),
        );
        m.insert(
            "common.close",
            tr!(en, "Close", cn, "关闭", tw, "關閉"),
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
