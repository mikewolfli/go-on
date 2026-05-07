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

    pub fn t(&self, key: &'static str) -> &str {
        self.strings
            .get(key)
            .and_then(|m| m.get(&self.lang))
            .copied()
            .unwrap_or(key)
    }

    fn load_all(m: &mut HashMap<&'static str, HashMap<Lang, &'static str>>) {
        let _en = Lang::En;
        let _cn = Lang::ZhCn;
        let _tw = Lang::ZhTw;

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

        // Chat
        m.insert("chat.title", tr!(en, "Chat", cn, "对话", tw, "對話"));
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
