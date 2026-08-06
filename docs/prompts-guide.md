# go-on 提示词模板系统

## 概述

提示词模板系统（Prompts System）是 go-on 内置的提示词模板管理功能，提供**149 个即用模板**，覆盖 16 个类别（以 `prompts/en.json` 为准）。用户可以在 GUI 中浏览、搜索、插入模板，也可以通过 `/` 命令快速展开模板，或在 Chat 对话中由 AI agent 通过 MCP 自动调用。

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

## 16 个类别

模板按类别组织，各类模板数量不一（合计 **149 个**）：

| # | 类别 | 模板数 | 代表模板 |
|----|------|--------|---------|
| 1 | 软件开发 | 10 | `explain_code` 解释代码 / `code_review` 代码审查 / `generate_unit_test` 生成单元测试 |
| 2 | 写作创作 | 12 | `blog_post_outline` 博客大纲 / `proofread_text` 校对 / `creative_story` 创意故事 |
| 3 | 学术研究 | 8 | `literature_review` 文献综述 / `abstract_generation` 摘要生成 / `peer_review` 同行评审 |
| 4 | 商业分析 | 9 | `swot_analysis` SWOT / `business_plan` 商业计划 / `competitive_analysis` 竞争分析 |
| 5 | 市场营销 | 13 | `marketing_strategy` 营销策略 / `ad_copy` 广告文案 / `social_media_content` 社媒内容 |
| 6 | 法律合规 | 11 | `contract_review` 合同审查 / `contract_clause` 合同条款 / `compliance_checklist` 合规清单 |
| 7 | 医疗健康 | 8 | `symptom_analysis` 症状分析 / `medication_guide` 用药说明 / `treatment_plan` 治疗方案 |
| 8 | 教育培训 | 10 | `lesson_plan` 教案 / `quiz_generation` 测验生成 / `explain_concept` 概念讲解 |
| 9 | 金融投资 | 11 | `investment_analysis` 投资分析 / `budget_planning` 预算规划 / `financial_report` 财务报告 |
| 10 | 数据科学 | 11 | `eda_plan` 探索性分析 / `model_selection` 模型选择 / `sql_query` SQL 查询 |
| 11 | 设计创意 | 11 | `ux_review` UX 评审 / `design_brief` 设计简报 / `accessibility_audit` 无障碍审计 |
| 12 | 系统运维 | 10 | `incident_response` 故障响应 / `monitoring_setup` 监控配置 / `security_hardening` 安全加固 |
| 13 | 效率提升 | 8 | `requirements_breakdown` 需求拆解 / `prd_draft` PRD 草稿 / `meeting_minutes` 会议纪要 |
| 14 | 工程交付 | 6 | `release_notes` 发布说明 / `rca_report` 根因报告 / `rollback_plan` 回滚计划 |
| 15 | 运营支持 | 6 | `customer_reply` 客服回复 / `kb_article` 知识库文章 / `faq_builder` FAQ 生成 |
| 16 | Go-On 代理技能 | 5 | `skill_discovery` 技能发现 / `tool_selection` 工具选择 / `best_practices` 代理最佳实践 |

---

## GUI 提示词管理

在 GUI 的 **Prompts（提示词管理）** Tab 中，你可以：

- **浏览** — 按类别分组查看所有模板（内置 + 自定义）
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
- `/code_review` — 审查代码质量
- `/generate_unit_test Rust` — 为 Rust 代码生成单元测试
- `/blog_post_outline 主题: AI 发展趋势` — 撰写指定主题的文章

### 常用命令列表

| 命令               | 功能             |
|--------------------|------------------|
| `/explain_code`    | 解释选中的代码   |
| `/code_review`     | 审查代码质量     |
| `/refactor_suggestion` | 重构建议     |
| `/generate_unit_test` | 生成单元测试  |
| `/debug_error`     | 调试错误         |
| `/generate_documentation` | 生成文档注释 |
| `/blog_post_outline` | 撰写文章大纲   |
| `/marketing_strategy` | 营销策略     |
| `/contract_review` | 合同审查         |
| `/literature_review` | 文献综述       |

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

AI agent 通过 MCP 协议发现这些工具后，可以在对话中自动选择合适的模板并应用。例如在对话中，当用户说"帮我审查这段代码"时，AI agent 可以自动调用 `prompts_get` 获取 `code_review` 模板并应用到回答中。

---

## 设置中的开关

在 **Settings（设置） → 核心功能开关** 中，可以启用或禁用 **提示词管理** 功能模块。关闭后，Prompts Tab 将不再显示，相关 RPC 和 MCP 接口也将停止响应。

> 默认状态：**开启**
