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

        // Chat
        m.insert("chat.title", tr!(en, "Chat", cn, "对话", tw, "對話"));
        m.insert("chat.phase", tr!(en, "Phase", cn, "阶段", tw, "階段"));
        m.insert("chat.mode", tr!(en, "Mode", cn, "模式", tw, "模式"));

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
