# go-on 提示词模板系统

## 概述

提示词模板系统（Prompts System）是 go-on 内置的提示词模板管理功能，提供**84+ 个即用模板**，覆盖 12 个行业类别。用户可以在 GUI 中浏览、搜索、插入模板，也可以通过 `/` 命令快速展开模板，或在 Chat 对话中由 AI agent 通过 MCP 自动调用。

> 相关文档：[GUI 使用指南](gui-guide.md) | [工作流配置](workflow-config.md)

---

## 目录结构

提示词模板采用分层目录结构组织：

```
prompts/
├── en.json           # 英文内置模板（默认）
├── zh-CN.json        # 简体中文内置模板
├── zh-TW.json        # 繁体中文内置模板
└── custom/
    ├── en.json       # 英文自定义模板
    ├── zh-CN.json    # 简体中文自定义模板
    └── zh-TW.json    # 繁体中文自定义模板
```

- `prompts/{lang}.json` — 内置模板，随系统发布，只读不可修改
- `prompts/custom/{lang}.json` — 自定义模板，用户可自由创建、编辑、删除
- 语言自动切换：GUI 界面语言变更时，系统自动加载对应语言的模板文件

### 支持的语言

| 语言代码 | 名称       |
|----------|------------|
| `en`     | English    |
| `zh-CN`  | 简体中文   |
| `zh-TW`  | 繁體中文   |

切换 GUI 语言时，提示词模板系统会自动加载对应语言的提示词文件，无需手动切换。

---

## 12 个行业类别

每个类别包含 7 个模板，共计 **84+ 个模板**，涵盖各行业的典型使用场景。

| 序号 | 类别         | 说明                                           | 代表模板                                                     |
|------|--------------|------------------------------------------------|--------------------------------------------------------------|
| 1    | 软件开发     | 代码生成、调试、重构、代码审查等               | `explain_code` 解释代码 / `review_code` 审查代码 / `generate_test` 生成单元测试 |
| 2    | 写作创作     | 文章撰写、创意写作、文案策划等                 | `write_article` 撰写文章 / `creative_story` 创意故事 / `copywriting` 文案策划 |
| 3    | 学术研究     | 论文写作、文献综述、数据分析等                 | `write_paper` 论文写作 / `literature_review` 文献综述 / `data_analysis` 数据分析 |
| 4    | 商业分析     | 市场分析、商业模式、竞争分析等                 | `market_analysis` 市场分析 / `business_model` 商业模式 / `competitive_analysis` 竞争分析 |
| 5    | 市场营销     | 营销方案、广告文案、社交媒体等                 | `marketing_plan` 营销方案 / `ad_copy` 广告文案 / `social_media` 社交媒体内容 |
| 6    | 法律合规     | 合同审查、法律咨询、合规检查等                 | `contract_review` 合同审查 / `legal_advice` 法律咨询 / `compliance_check` 合规检查 |
| 7    | 医疗健康     | 医学咨询、健康管理、药物说明等                 | `medical_consult` 医学咨询 / `health_plan` 健康管理 / `drug_info` 药物说明 |
| 8    | 教育培训     | 课程设计、教学计划、学习辅导等                 | `course_design` 课程设计 / `lesson_plan` 教学计划 / `tutoring` 学习辅导 |
| 9    | 金融投资     | 投资分析、风险评估、财务报告等                 | `investment_analysis` 投资分析 / `risk_assessment` 风险评估 / `financial_report` 财务报告 |
| 10   | 数据科学     | 数据分析、机器学习、数据可视化等               | `data_cleaning` 数据清洗 / `ml_model` 机器学习模型 / `data_viz` 数据可视化 |
| 11   | 设计创意     | UI/UX 设计、平面设计、创意构思等               | `ui_design` UI 设计 / `brand_design` 品牌设计 / `creative_brainstorm` 创意构思 |
| 12   | 系统运维     | 服务器管理、网络配置、监控告警等               | `server_setup` 服务器配置 / `network_config` 网络配置 / `monitor_setup` 监控告警设置 |

---

## GUI 提示词管理

在 GUI 的 **Prompts（提示词管理）** Tab 中，你可以：

- **浏览** — 按行业类别分组查看所有模板（内置 + 自定义）
- **搜索** — 按关键词搜索模板标题和内容
- **创建** — 创建新的自定义模板，选择所属类别和语言
- **编辑** — 修改已创建的自定义模板内容
- **删除** — 删除不需要的自定义模板

### 操作流程

1. 切换到 Prompts Tab
2. 左侧选择行业类别过滤
3. 使用搜索框输入关键词搜索
4. 点击模板卡片查看详情
5. 点击 **Insert to Chat** 按钮将模板内容插入到 Chat 输入框
6. 在 Chat 中对模板进行微调后发送

---

## Chat `/` 命令

在 Chat 输入框中输入 `/` 可触发命令补全。输入模板 id 后按 Enter 即可直接展开模板内容。

### 命令格式

```
/模板id 参数
```

例如：
- `/explain_code` — 解释选中的代码
- `/review_code` — 审查代码质量
- `/generate_test Rust` — 为 Rust 代码生成单元测试
- `/write_article 主题: AI 发展趋势` — 撰写指定主题的文章

### 常用命令列表

| 命令               | 功能             |
|--------------------|------------------|
| `/explain_code`    | 解释选中的代码   |
| `/review_code`     | 审查代码质量     |
| `/optimize_code`   | 优化代码性能     |
| `/generate_test`   | 生成单元测试     |
| `/refactor_code`   | 重构代码         |
| `/write_doc`       | 编写文档注释     |
| `/write_article`   | 撰写文章         |
| `/market_analysis` | 市场分析         |
| `/contract_review` | 合同审查         |
| `/data_analysis`   | 数据分析         |

输入 `/` 后系统会弹出自动补全列表，支持模糊搜索，可以按类别筛选，也可以直接输入模板 id 全称。

---

## 自定义模板

### 创建模板

1. 在 Prompts Tab 中点击 **创建新模板**
2. 填写模板信息：
   - **ID** — 唯一标识符，用于 `/` 命令调用（如 `my_custom_template`）
   - **标题** — 模板的显示名称
   - **类别** — 所属行业类别
   - **语言** — 模板使用的语言
   - **内容** — 模板的提示词文本，支持 `{{变量}}` 占位符
   - **描述** — 简短说明模板用途
3. 保存后模板将写入 `prompts/custom/{lang}.json`

### 编辑模板

在 Prompts Tab 中找到自定义模板，点击编辑按钮即可修改。编辑后保存，系统会更新 `prompts/custom/{lang}.json`。

### 删除模板

在 Prompts Tab 中找到自定义模板，点击删除按钮确认删除。

> ⚠️ 注意：只能删除自定义模板，内置模板为只读不可删除。

---

## 后端 RPC 接口

提示词管理系统提供以下 RPC 接口：

| RPC               | 方法   | 说明                                           |
|-------------------|--------|------------------------------------------------|
| `prompts.list`    | List   | 获取所有模板列表，支持按类别和语言过滤         |
| `prompts.search`  | Search | 按关键词搜索模板                               |
| `prompts.get`     | Get    | 获取单个模板详情                               |
| `prompts.create`  | Create | 创建新的自定义模板                             |
| `prompts.update`  | Update | 更新自定义模板                                 |
| `prompts.delete`  | Delete | 删除自定义模板                                 |

---

## MCP 工具

通过 MCP（Model Context Protocol），AI agent 可以自动发现和调用提示词模板：

| MCP 工具       | 功能                             |
|----------------|----------------------------------|
| `prompts_list` | 列出所有可用的提示词模板         |
| `prompts_get`  | 获取指定模板的详细内容           |

AI agent 通过 MCP 协议发现这些工具后，可以在对话中自动选择合适的模板并应用。例如在对话中，当用户说"帮我审查这段代码"时，AI agent 可以自动调用 `prompts_get` 获取 `review_code` 模板并应用到回答中。

---

## 设置中的开关

在 **Settings（设置） → 核心功能开关** 中，可以启用或禁用 **提示词管理** 功能模块。关闭后，Prompts Tab 将不再显示，相关 RPC 和 MCP 接口也将停止响应。

> 默认状态：**开启**
