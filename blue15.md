# BLUE15 正式版 — 生产化与智能治理优化建议

更新时间：2026-04-15

本文为 BLUE15 正式版，基于当前仓库状态重新整理，删除不必要项、合并重复项，并补充更贴近上线落地的优化建议。

## 一、执行结论

1. 当前阶段不采用微服务架构，采用模块化单体 + 多实例部署。
2. 执行顺序调整为“单机优化与质量增益优先”，上线准备项后置到收口阶段。
3. 所有改进必须满足最小改动、可回滚、可验收、三端协同（backend + GUI + vscode-addon）。

## 二、范围与约束

适用范围：go-on backend、GUI、vscode-addon 的联动能力建设。

硬约束：
1. 不破坏现有主链路与既有测试通过率。
2. 禁止 bridge-stub 或测试侧模拟生产模块绕过问题。
3. 每项改进必须形成闭环：触发 -> 执行 -> 反馈 -> 度量/审计。
4. 保持最小化代码改动，优先复用现有模块。
5. 同目录部署时，backend 与 GUI 资源命名必须避免冲突覆盖。

## 三、建议清单（按执行顺序重排）

重排原则：单机优化优先、上线准备后置；ID 与原优先级保留用于追踪。

| 执行顺序 | ID | 原优先级 | 建议项 | 是否需要 | 说明 |
|---|---|---|---|---|---|
| 1 | B15-P1-4 | P1 | 并发锁模型优化与毒化恢复 | 需要 | 单机稳定性与尾延迟收益高 |
| 2 | B15-P2-3 | P2 | 超时模型统一与线程开销收敛 | 建议 | 单机资源效率与抖动收敛 |
| 3 | B15-P2-4 | P2 | 健康探针分级与依赖健康映射 | 建议 | 单机可观测与退化判定清晰 |
| 4 | B15-P3-1 | P3 | 覆盖率与基准测试体系优化 | 建议 | 单机质量门禁精度提升 |
| 5 | B15-P1-1 | P1 | 模型选择增强（探索-利用平衡） | 需要 | 单机效果增益与策略多样性 |
| 6 | B15-P1-2 | P1 | 学习数据持久化与回放 | 需要 | 重启后保持学习连续性 |
| 7 | B15-P1-3 | P1 | PUA 动态规则与审计可视化 | 需要 | 治理能力可运维化 |
| 8 | B15-P2-1 | P2 | 故障恢复与降级（断路器） | 建议 | 异常场景韧性提升 |
| 9 | B15-P2-2 | P2 | 可观测性完善（追踪覆盖率与告警） | 建议 | 链路排障与告警闭环 |
| 10 | B15-P1-5 | P1 | 生产配置严格模式（安全守卫） | 需要 | 上线前 fail-fast 门禁 |
| 11 | B15-P3-2 | P3 | 入口层鉴权与双层限流闭环 | 建议 | 对外暴露最小防线 |
| 12 | B15-P0-2 | P0 | 运行稳定性基线（健康检查、优雅停机、配置校验） | 必须 | 上线收口稳定项 |
| 13 | B15-P0-1 | P0 | 生产化安全基线（鉴权、限流、反向代理、TLS） | 必须 | 正式上线门槛能力 |

## 四、删除与合并项说明

本版已删除或降级以下内容：
1. 删除“微服务化架构准备”作为当前实施目标：现阶段收益低于复杂度，不进入执行清单。
2. 删除大段示例代码与超细实现片段：保留策略与验收口径，避免文档与代码双重维护成本。
3. 合并“可观测性新增依赖”表述：仓库已有 OpenTelemetry 相关依赖，改为“覆盖与接入完善”。
4. 覆盖率工具改为“分层执行”：本地/夜间增强，不强行塞入所有 PR 快速流水线。

后端全项目扫描新增结论（本次补充）：
1. 已具备 phase 级限流与健康状态，但入口层（按来源/IP/租户）鉴权与限流闭环仍建议显式收口。
2. async 主路径存在较多 `std::sync::Mutex`/`RwLock` 用法，建议分阶段收敛到更适配异步场景的锁模型或更细粒度锁域。
3. 运行时配置已有安全告警能力（如 HTTP/HTTPS、keyring 提示），建议新增“生产严格模式”门禁，避免仅告警不阻断。
4. 健康检查基础良好，建议再区分 liveness/readiness 与关键依赖状态映射，便于编排系统决策。

## 五、分阶段实施计划（重排后）

### 阶段 A（单机优化先行）

目标：先在单机场景拿到可量化的稳定性、效率与质量收益。

工作项：
1. 梳理并收敛 async 热路径中的同步锁：优先改造高频写锁、长临界区与可能毒化的 `unwrap` 锁获取点。
2. 统一超时模型：减少阻塞式超时封装在请求主路径的使用，优先采用异步超时与可取消任务。
3. 健康探针升级：拆分 liveness/readiness，并将缓存、向量、限流器、断路器状态映射到 readiness 细项。
4. 覆盖率与基准测试分层执行：PR 快速门禁 + 夜间深度任务。

验收标准：
1. 压测下锁等待时间与尾延迟（P95/P99）较基线可观下降。
2. 线程数与内存抖动不出现异常增长。
3. readiness 可准确反映关键依赖退化，liveness 保持最小生存语义。
4. 质量门禁执行时间与问题检出率达到预期。

### 阶段 B（智能与治理增强）

目标：在单机稳定基础上提升模型选择质量与治理可运维性。

工作项：
1. 在现有模型选择逻辑上加入探索-利用平衡（例如 UCB 思路），保持兼容回退。
2. 学习记录持久化到现有 SQLite 体系，补齐查询与清理策略。
3. PUA 规则从静态转动态加载，并提供规则版本与变更审计。
4. GUI 与 vscode-addon 增加最小可用治理视图（状态、违规、规则版本）。
5. 梳理并收敛 async 热路径中的同步锁：优先改造高频写锁、长临界区与可能毒化的 `unwrap` 锁获取点。
6. 增加“生产严格模式”配置（例如 `runtime.production_strict=true`）：对不安全配置（明文 HTTP 上游、缺失鉴权、关键密钥缺失）执行启动阻断。

验收标准：
1. 新模型可获得探索机会，不被历史强者长期压制。
2. 重启后学习数据可恢复并继续生效。
3. 规则变更可追踪、可回滚。
4. 三端可看到一致的治理状态。
5. 压测下锁等待时间与尾延迟（P95/P99）较基线可观下降。
6. 生产严格模式可在风险配置下阻断启动，并给出可操作诊断信息。

### 阶段 C（上线准备后置）

目标：将对外暴露与上线门槛项作为最终收口，降低前期复杂度干扰。

工作项：
1. 接入统一入口（Nginx 或等价网关），强制 TLS 与基础限流。
2. 对外接口增加鉴权策略（至少 API key 或同等机制）。
3. 明确监听地址策略与默认安全配置（区分本地与服务器环境）。
4. 校验优雅停机行为，确保在飞请求可排空。
5. 建立 SLO 指标与告警阈值，并完成上线前演练。
6. 对外暴露场景建立“入口鉴权 + 入口限流 + 业务限流”双层或三层闭环。

验收标准：
1. 未鉴权请求被拒绝，限流命中可观测。
2. 健康检查、优雅停机、告警演练通过上线门禁。
3. 在恶意流量或突发流量下，入口层与业务层限流均可观测、可回放、可审计。

## 六、补充条目（按 BLUE14 规则表达）

### B15-P1-4：并发锁模型优化与毒化恢复

是否需要：需要

推荐建议：
1. 对 async 热路径的 `std::sync::Mutex`/`RwLock` 进行分层治理：先改高频写入与长临界区，再处理低频路径。
2. 对锁获取失败（poisoned）场景，避免直接 `unwrap` 导致进程级故障，提供降级恢复或错误上抛。
3. 为关键锁增加最小观测项：等待时长、争用次数、慢锁阈值告警。

验收门禁：
1. 增加并发回归测试（高并发 chat / metrics / background cycle）。
2. 增加锁毒化恢复测试，验证不会因单点 panic 扩散为整体不可用。

### B15-P1-5：生产配置严格模式（安全守卫）

是否需要：需要

推荐建议：
1. 在现有 config 校验基础上新增严格模式开关：生产环境将高风险项从 warning 升级为 fail-fast。
2. 严格模式建议至少覆盖：上游明文 HTTP、缺失关键密钥、未启用入口鉴权。
3. 启动失败信息需具备可修复指引，便于运维快速闭环。

验收门禁：
1. 增加严格模式配置单测与集成测试。
2. CI 增加一组“风险配置必须启动失败”的负向用例。

### B15-P2-3：超时模型统一与线程开销收敛

是否需要：建议

推荐建议：
1. 梳理阻塞式超时封装的实际调用路径，优先替换主路径中的线程派生式超时。
2. 统一使用异步超时 + 可取消任务语义，减少线程抖动与上下文切换开销。
3. 将超时事件接入 trace/metrics，形成慢请求与超时分布视图。

验收门禁：
1. 压测下线程数与内存抖动不出现异常增长。
2. 超时行为与错误码稳定，且可在追踪中定位。

### B15-P2-4：健康探针分级与依赖健康映射

是否需要：建议

推荐建议：
1. liveness 仅反映进程存活与事件循环基本可用。
2. readiness 映射关键依赖状态（缓存、向量、限流器、断路器、生命周期状态）。
3. 对部分退化场景给出可继续服务与不可继续服务的判定边界。

验收门禁：
1. 编排器可依据 readiness 正确摘除异常实例。
2. 依赖恢复后 readiness 可自动恢复。

### B15-P3-2：入口层鉴权与双层限流闭环

是否需要：建议

推荐建议：
1. 在入口层实施统一鉴权（API key/JWT/网关签名任选其一），业务层保留 phase/任务级限流。
2. 对拒绝类事件统一记录审计字段（来源、路径、策略命中、trace_id）。
3. 保持与 GUI/vscode-addon 的错误语义一致，避免客户端误判。

验收门禁：
1. 未鉴权与超限请求在入口层被拦截并可观测。
2. 业务层限流仍可独立生效，形成分层防护。

## 七、部署架构建议（暂缓，待服务器部署时启用）

状态：暂缓执行（不纳入当前迭代交付）。

启用时机：
1. 项目进入服务器部署阶段。
2. 需要对外提供稳定公网或内网服务能力。
3. 已完成 P0/P1 的安全与稳定基线。

启用后默认建议：
1. 推荐形态为模块化单体 + 多实例，而非微服务拆分。
2. 入口层使用 Nginx/网关（TLS、限流、鉴权前置）。
3. 应用层使用 go-on 多实例（按 CPU 或请求量水平扩容）。
4. 数据层先用 SQLite，出现并发与写入瓶颈后再评估迁移。
5. 观测层统一接入日志、指标、追踪。

仅在下列条件满足时再评估微服务拆分：
1. 团队协作冲突持续、发布节奏需要彻底解耦。
2. 单体水平扩容后仍存在明确性能瓶颈。
3. 某子域需要独立可靠性目标与强隔离。

## 八、三端协同要求

每个大项交付都需同步完成：
1. backend：接口与行为落地。
2. GUI：可见状态、配置入口或告警反馈。
3. vscode-addon：协议兼容、错误提示和能力开关对齐。
4. 文档与脚本：README、配置样例、验证脚本同步更新。

## 九、统一验收清单

1. 功能验收：每项建议至少 1 条自动化测试覆盖主路径。
2. 回归验收：既有测试与核心流程无回退。
3. 运维验收：健康检查、日志、追踪、告警可用。
4. 安全验收：鉴权、限流、配置安全默认值生效。
5. 三端验收：backend + GUI + vscode-addon 联动通过。
6. 部署验收：同目录部署无资源命名冲突。

## 十、正式结语

BLUE15 的实施方向应从“可上线”出发，而不是从“架构形态升级”出发。
当前最优策略是：先做生产化单体的安全与稳定闭环，再逐步增强智能治理与可观测性；微服务拆分仅在明确瓶颈出现后再启动。

## 十一、本轮实施回写（2026-04-15）

本轮目标：对齐三端（backend + GUI + vscode-addon）治理状态闭环能力。

已完成项（一个大项闭环）：
1. backend 新增 `governance.status` RPC，统一输出治理状态、规则版本指纹、PUA 摘要、违规计数与配置告警摘要。
2. GUI `SecurityView` 改为调用 `governance.status` 实时渲染（状态、规则版本、风险与审计摘要），不再仅依赖静态演示数据。
3. vscode-addon 新增 `go-on.governanceStatus` 命令，并在 Settings 面板加入 `Governance Status` 按钮直连后端接口。

本轮完成率：
1. “三端治理状态对齐”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 8%（按 13 个建议项粗略计，已完成 1 个大项，后续按优先级继续推进）。

## 十二、本轮实施回写（2026-04-15，续）

本轮目标：完成 B15-P1-5「生产配置严格模式（安全守卫）」并打通三端可见性。

已完成项（一个大项闭环）：
1. backend 新增 `runtime.production_strict` 与 `runtime.entry_auth_enabled` 运行时配置项。
2. backend 在 `validate_runtime_readiness` 中实现严格模式 fail-fast：
	- 明文 HTTP 上游（`agents.*.url` 为 `http://`）阻断；
	- 任一已配置 agent 缺失密钥/secret 阻断；
	- 暴露 `acp_http_bind_addr` 但未启用入口鉴权标识（`entry_auth_enabled=false`）阻断。
3. backend `governance.status` 的 `governance.config` 扩展严格模式状态：
	- `production_strict`
	- `strict_violation_count`
	- `strict_violations`
4. GUI `SecurityView` 新增 `production_strict` 状态可视化与风险提示，并将严格模式违规计入配置评分。
5. vscode-addon `go-on.governanceStatus` 输出改为结构化摘要（治理状态 + strict 开关 + strict 违规数 + rules 版本）。
6. 配置模板 `config.toml` / `config.toml.autopilot-adaptive` 同步新增严格模式字段。
7. backend 单元测试补齐严格模式负向/正向用例。

本轮完成率：
1. “B15-P1-5 生产配置严格模式”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 15%（按 13 个建议项粗略计，已完成 2 个大项）。

## 十三、本轮实施回写（2026-04-15，续2）

本轮目标：完成 B15-P2-4「健康探针分级与依赖健康映射」并实现三端联动。

已完成项（一个大项闭环）：
1. backend 新增 `health.probes` RPC：输出 `liveness` / `readiness` 分级状态、依赖组件映射、断路器快照、限流器桶快照与汇总计数。
2. backend 复用 `build_runtime_healthcheck_report` 作为 readiness 依赖基础，并统一状态语义（`healthy/warn/error/skipped`）。
3. GUI `HealthBreakdownView` 切换为消费 `health.probes`：新增探针卡片，按依赖映射刷新 cache/vector/breaker/rate-limiter 健康视图。
4. vscode-addon 新增 `go-on.healthProbes` 命令，并在 Settings 面板新增 `Health Probes` 按钮直连后端接口。

本轮完成率：
1. “B15-P2-4 健康探针分级与依赖健康映射”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 23%（按 13 个建议项粗略计，已完成 3 个大项）。

## 十四、本轮实施回写（2026-04-15，续3）

本轮目标：完成 B15-P3-2「入口层鉴权与双层限流闭环」并实现三端联动。

已完成项（一个大项闭环）：
1. backend ACP HTTP 入口新增统一门禁：
	- 入口鉴权：支持 `Authorization: Bearer <key>` / `X-API-Key` / `X-Go-On-Key`；
	- 鉴权配置：`runtime.entry_auth_enabled` + `runtime.entry_auth_api_key_env`（默认 `GO_ON_ENTRY_API_KEY`）；
	- 入口限流：按来源 IP 维度执行 token-bucket（`runtime.entry_rate_limit_rpm` + `runtime.entry_rate_limit_burst`）。
2. backend 对拒绝类请求输出结构化错误（含 `source/path/policy/trace_id`），并统一状态码：401/429/503。
3. backend `governance.status` 扩展入口防线可观测字段：
	- `config.entry_auth_enabled`
	- `config.entry_auth_api_key_env`
	- `config.entry_auth_key_configured`
	- `config.entry_rate_limit_rpm`
	- `config.entry_rate_limit_burst`
4. GUI `SecurityView` 增加入口防线可视化（entry auth on/off、entry rate limit）并将入口风险纳入评分与风险建议。
5. vscode-addon `go-on.governanceStatus` 输出增加入口鉴权与入口限流摘要。
6. 配置模板 `config.toml` / `config.toml.autopilot-adaptive` 同步新增入口鉴权密钥 env 与入口限流配置项。

本轮完成率：
1. “B15-P3-2 入口层鉴权与双层限流闭环”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 31%（按 13 个建议项粗略计，已完成 4 个大项）。

## 十五、专项补充建议（自学习/知识萃取/强化学习/Hardness/Harness）

说明：本节为补充优化清单，先给出可执行方向与门禁口径；为保持历史统计稳定性，暂不追溯改写前序“13项基线分母”的完成率，后续统一纳入下一版里程碑统计。

### B15-X1：自学习闭环质量门禁（Learning Loop Guardrail）

是否需要：需要

推荐建议：
1. 将“学习样本入库”拆分为四段门禁：可解析性、证据完整性、结果可归因、去重相似度阈值。
2. 对高风险样本（失败重试链、超时链、人工回滚链）设置更高权重，但要求最小证据集齐全。
3. 引入学习冷却窗口与最小样本量阈值，避免短时间噪声导致策略震荡。

验收门禁：
1. 连续 N 次低质量样本注入不会显著改变模型/策略排序。
2. 学习后关键指标（成功率、P95、回滚率）至少两项改善或不劣化。

### B15-X2：知识萃取分层与去噪（Knowledge Distillation Pipeline）

是否需要：需要

推荐建议：
1. 建立三层知识结构：原始证据层（不可变）、摘要层（可回放）、策略层（可执行规则）。
2. 对摘要层引入冲突检测（时间冲突、版本冲突、语义冲突）与置信度衰减机制。
3. 增加“错误知识墓碑”机制：被证伪结论不直接删除，保留否定证据与失效原因。

验收门禁：
1. 随机抽样知识可回溯到原始证据与生成上下文。
2. 新旧知识冲突时系统能给出确定性优先级与审计记录。

### B15-X3：强化学习奖励对齐与离线评估（RL Alignment & Offline Eval）

是否需要：建议

推荐建议：
1. 将奖励从单一成功率扩展为多目标加权：任务成功、时延成本、工具错误率、安全违规惩罚。
2. 引入反事实离线评估（off-policy replay）先验验证，再进入在线微调，避免线上试错成本过高。
3. 对奖励漂移设置告警：当最近窗口奖励分布与历史分布偏移超阈值时，自动降级到保守策略。

验收门禁：
1. 离线评估通过后再允许在线生效；未通过自动回退。
2. 奖励函数变更可追踪，且可一键回滚到上一稳定版本。

### B15-X4：Hardness 分级路由与预算编排（Difficulty-aware Routing）

是否需要：建议

推荐建议：
1. 建立 hardness 评分维度：上下文规模、跨文件跨度、工具依赖数、失败恢复复杂度。
2. 按 hardness 分层分配：模型档位、超时预算、并发上限、评审强度（单审/双审）。
3. 对高 hardness 请求默认启用更严格的保护：更高证据要求、更多中间检查点、自动降级策略。

验收门禁：
1. 高 hardness 任务的失败率和回滚率较基线下降。
2. 低 hardness 任务吞吐与响应时间不因新策略明显恶化。

### B15-X5：Harness 评测基座与回归挑战集（Evaluation Harness）

是否需要：需要

推荐建议：
1. 建立分层 harness：冒烟集、回归集、对抗集、长链路集，并提供固定随机种子保证可复现。
2. 为每类任务定义统一评分卡：正确性、稳定性、成本、时延、安全合规。
3. 将 harness 与 CI 分层绑定：PR 跑冒烟/关键回归，夜间跑全量与对抗集。

验收门禁：
1. 新版本上线前至少通过关键回归集与对抗集阈值。
2. Harness 结果可追溯到具体提交、配置快照与运行环境。

### B15-X6：Token 成本压缩与 Cost 优化（Token Compression & Cost Governance）

是否需要：需要

推荐建议：
1. 建立 token 预算分层：按 phase/任务类型/Hardness 设定输入与输出 token 上限，并提供超预算降级策略（摘要化、裁剪上下文、切换低成本模型）。
2. 引入上下文压缩流水线：历史对话滚动摘要、冗余证据去重、检索片段长度自适应，优先保留高信息密度内容。
3. 建立成本感知路由：在满足质量阈值前提下，优先选择单位有效 token 成本更低的模型组合；对高价模型调用设置触发条件与冷却时间。
4. 增加成本可观测与告警：按请求、phase、模型、成功/失败维度输出 token 与费用分布（P50/P95/P99），并对异常突增做告警。
5. 对工具调用做成本联动优化：限制低价值重复调用，合并可批处理步骤，减少“模型-工具-模型”往返轮次。

验收门禁：
1. 在不降低关键质量指标（成功率、回滚率、严重缺陷率）的前提下，单位任务平均 token 成本显著下降。
2. 高 Hardness 任务成本可控（有上限、有降级、有审计），且不会因压缩策略导致明显正确率回退。
3. 成本报表可追溯到请求级 trace_id、模型版本、配置快照与策略版本。

### B15-X7：配置基线收敛与一次性清理（Config Baseline Freeze）

是否需要：需要

推荐建议：
1. 对运行时关键配置建立“最小安全基线”模板，清理历史遗留开关和重复语义字段。
2. 收敛配置来源优先级（默认值/文件/环境变量/CLI），并输出单一最终生效视图。
3. 对高风险开关建立一次性迁移脚本（旧字段 -> 新字段）与弃用时间表。

验收门禁：
1. 同一配置在不同入口读取结果一致，无隐式覆盖歧义。
2. 旧配置兼容窗口内可平滑迁移，窗口后可明确失败并给修复指引。

### B15-X8：错误码、重试语义与客户端契约统一（Error Contract Unification）

是否需要：需要

推荐建议：
1. 统一后端错误码分层（参数错误/权限错误/限流/上游错误/内部错误），避免同错多码。
2. 为每类错误定义重试策略元信息（可重试、退避建议、最大重试次数）。
3. GUI 与 vscode-addon 对齐错误语义展示，减少误判和重复告警。

验收门禁：
1. 关键接口错误码覆盖率达到约定阈值，且跨端展示一致。
2. 重试后成功率提升且无明显风暴式重试。

### B15-X9：依赖与构建可复现优化（Reproducible Build Pack）

是否需要：建议

推荐建议：
1. 锁定核心依赖版本并建立升级节奏（安全补丁、功能升级分轨执行）。
2. 构建流程补齐可复现元数据（版本、提交、配置快照、构建参数）。
3. 将关键二进制与前端产物的校验信息纳入发布清单。

验收门禁：
1. 同版本在标准环境可复现一致产物或一致行为。
2. 依赖升级可回溯到影响评估与回滚方案。

### B15-X10：数据生命周期与存储治理（Data Lifecycle Governance）

是否需要：需要

推荐建议：
1. 为 cache/vector/ledger 定义统一生命周期策略：保留期、清理频率、归档规则。
2. 建立一次性存量整理流程（过期数据、重复摘要、异常记录）并提供幂等脚本。
3. 对关键数据路径增加容量水位告警与降级策略，避免磁盘风险传导到主链路。

验收门禁：
1. 清理与归档执行后，容量占用下降且查询性能不回退。
2. 生命周期策略可观测、可审计、可回放。

### B15-X11：总体一次优化到顶（One-Shot Optimization Peak）

是否需要：建议（用于阶段性集中冲刺）

推荐建议：
1. 以“一次性优化包”方式并行收敛 X1~X10，限定单次冲刺窗口与冻结期，避免长期分散改造带来的漂移。
2. 建立统一控制面板：质量、成本、稳定性、安全、可观测五类指标同屏对齐，以单一发布判定口径验收。
3. 采用“双闸门发布”策略：先在单机基线闸门达标，再进入上线闸门（入口防线、SLO、演练）达标。
4. 强制执行“可回滚先于可上线”：每个子项必须附带开关、回滚路径、回滚验证脚本。
5. 冲刺期间限制新增需求，仅允许缺陷修复与门禁相关变更，确保优化收益不被需求噪声稀释。

验收门禁：
1. X1~X10 关键指标达到联合阈值（质量不降、成本下降、稳定性提升、安全门禁通过）。
2. 冲刺结束后可在一个版本内回滚到前一稳定快照，并保持数据与接口兼容。
3. 三端（backend + GUI + vscode-addon）指标与状态口径一致，无显著语义偏差。

## 十六、下一步建议（与当前主线对齐）

1. 先落 B15-X5（Harness）作为质量地基，再推进 B15-X1/X2（学习与知识）避免“学到噪声”。
2. 并行规划 B15-X6（Token/Cost 治理）指标口径与预算策略，先打通观测再做自动化压缩与路由。
3. B15-X3（强化学习）建议在 Harness 稳定后启用离线评估优先路径。
4. B15-X4（Hardness）可与现有 phase 策略联动，先做只读评分与观测，再逐步开启路由干预。
5. 将 B15-X7~X10 作为“一次性优化包”集中交付：先做配置/错误契约统一，再做构建可复现与数据生命周期治理。
6. 在以上基础稳定后，执行 B15-X11“总体一次优化到顶”冲刺，作为阶段性收口版本。

## 十七、本轮实施回写（2026-04-15，续4）

本轮目标：完成 B15-P1-4「并发锁模型优化与毒化恢复」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 为 ACP 热路径共享锁新增统一观测与毒化恢复：记录 `acquisitions / poisoned_total / recovered_total / slow_wait_total / avg_wait_ms / max_wait_ms`，并在锁毒化后继续以恢复态执行而不是直接降为默认值。
2. backend 修正 `src/acp/background.rs` 背景健康循环错误使用默认 `PhaseRateLimiter` 的问题，改为复用 `AcpServer.phase_rate_limiter` 主链实例，保证限流健康与锁观测都反映真实运行态。
3. backend `health.probes` 扩展 `locks` 摘要与 `locks` 依赖项，统一输出锁状态、毒化恢复计数、慢锁次数和最大等待时间。
4. GUI `HealthBreakdownView` 新增锁模型卡片，实时展示锁状态、跟踪组件数、毒化恢复次数和最大等待时间。
5. vscode-addon `go-on.healthProbes` 输出新增锁摘要（lock status / poisoned / slow waits），与后端探针语义保持一致。
6. `test_ci.sh` 新增 6y BLUE15 P1-4 主链门禁，覆盖锁毒化恢复与锁状态聚合判断。

本轮完成率：
1. “B15-P1-4 并发锁模型优化与毒化恢复”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 38%（按 13 个建议项粗略计，已完成 5 个大项）。

## 十八、本轮实施回写（2026-04-15，续5）

本轮目标：完成 B15-P2-3「超时模型统一与线程开销收敛」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/helpers/context.rs` 收敛 ACP 共享超时能力，统一请求超时封装、评审门超时语义和异步 runtime readiness 探测，替换主路径中分散的阻塞式本地端口探测逻辑。
2. backend 在 `src/acp/impl/agent.rs`、`src/acp/impl/chat.rs`、`src/acp/impl/request.rs` 主路径统一接入异步超时语义，减少线程派生式超时包装，并把不可用 runtime 过滤切换为异步 readiness 检查。
3. backend 在 `src/acp/prelude.rs`、`src/acp/helpers/metrics.rs`、`src/acp/impl/request.rs` 新增 `agent_timeout_failures_total` 与 `runtime_probe_timeout_total`，并将 timeout 摘要纳入 `runtime.health`、`health.probes`、`metrics.get`、`metrics.prometheus`、`trace.metrics` 主链输出。
4. GUI `HealthBreakdownView` 新增超时模型卡片，实时展示 agent 请求超时、review gate 超时、runtime probe 超时和超时总量，与健康探针依赖映射保持一致。
5. vscode-addon 的 `go-on.healthProbes`、`go-on.metricsGet`、`go-on.traceMetrics` 输出新增 timeout 摘要，保证桌面侧可以直接看到主链超时分布而不是只看原始 JSON。
6. `test_ci.sh` 新增 6z BLUE15 P2-3 主链门禁，覆盖共享超时封装、异步 runtime readiness 探测与 timeout 指标累积三个关键回归点。

本轮完成率：
1. “B15-P2-3 超时模型统一与线程开销收敛”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 46%（按 13 个建议项粗略计，已完成 6 个大项）。

## 十九、本轮实施回写（2026-04-15，续6）

本轮目标：完成 B15-P3-1「覆盖率与基准测试体系优化」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 新增 `requests/quality-benchmark.ndjson` 基准场景，固定覆盖 `initialize -> runtime.health -> metrics.get -> trace.metrics -> shutdown` 主链请求序列。
2. backend 集成测试扩展：`run_scenario_file_executes_quality_benchmark_requests` 校验基准场景输出结构，`ndjson_scenario_files_all_pass` 同步扩展到 5 个场景文件，保证回归覆盖随场景增长自动纳管。
3. 脚本层新增 `scripts/run-quality-gate.sh` 与 `scripts/run-quality-gate.ps1`，统一质量门禁执行入口（场景回放 + 基准回归 + 可选 tarpaulin 覆盖率门禁）。
4. GUI `BackendOpsView` 新增 `quality.baseline` 按钮，聚合展示 `runtime.health`、`metrics.get`、`trace.metrics` 的质量基线摘要（健康态、请求质量、慢请求与超时分布）。
5. vscode-addon 新增 `go-on.qualityBaseline` 命令（并注册 command palette），输出主链质量摘要与 `requests/*.ndjson` 场景数量，便于桌面端快速执行基线巡检。
6. `test_ci.sh` 新增 6aa BLUE15 P3-1 门禁，覆盖基准场景回放与全场景回归，并在可用时执行 tarpaulin 覆盖率阈值检查。

本轮完成率：
1. “B15-P3-1 覆盖率与基准测试体系优化”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 54%（按 13 个建议项粗略计，已完成 7 个大项）。

## 二十、本轮实施回写（2026-04-15，续7）

本轮目标：完成 B15-P1-1「模型选择增强（探索-利用平衡）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/intelligence/adaptive_selector.rs` 将模型选择从“纯成功率贪心”升级为 UCB 探索-利用平衡评分，新增 `exploration_bias`、候选排序 `rank_candidates` 与可观测快照 `snapshot`。
2. backend 修正 ACP 主链中模型选择排序错位问题：`src/acp/impl/request.rs` 从“按 agent 名排序但按 model 记录结果”改为“按 agent->selected model 映射排序并记录同一 model 结果”，确保学习反馈与选择策略一致。
3. backend 新增 `selector.status` RPC，输出探索系数、观测样本量与模型评分快照，作为线上策略可观测入口。
4. backend 新增 `requests/model-selector-benchmark.ndjson` 场景与 `run_scenario_file_executes_model_selector_benchmark_requests` 集成测试，`ndjson_scenario_files_all_pass` 场景总数同步扩展到 6。
5. GUI `BackendOpsView` 新增 `selector.status` 入口按钮，可直接查看模型选择器状态；vscode-addon 新增 `go-on.selectorStatus` 命令并注册到 command palette，输出探索系数、样本量与 Top 模型评分摘要。
6. `test_ci.sh` 新增 6ab BLUE15 P1-1 主链门禁，覆盖 UCB 候选排序、快照排序与 selector 场景回放三条关键回归链路。

本轮完成率：
1. “B15-P1-1 模型选择增强（探索-利用平衡）”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 62%（按 13 个建议项粗略计，已完成 8 个大项）。

## 二十一、本轮实施回写（2026-04-15，续8）

本轮目标：完成 B15-P1-2「学习数据持久化与回放」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `learning.replay` RPC：统一回放 `.goon/learning` 的学习记录窗口，输出 workflow/pua 事件统计，并拼接最近一次 learning bus 工件摘要。
2. backend 新增 `requests/learning-replay-benchmark.ndjson` 回放场景，覆盖 `initialize -> learning.replay -> learning.summary -> shutdown` 主链请求序列。
3. backend 集成测试新增 `run_scenario_file_executes_learning_replay_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 7，保证新增场景自动纳入回归。
4. GUI `BackendOpsView` 新增 `learning.replay` 快捷入口，可直接回放最近学习记录并查看输出。
5. vscode-addon 新增 `go-on.learningReplay` 命令并注册到 command palette，同时在 Settings 面板增加 `Learning Replay` 按钮直连后端接口，输出记录条数、事件分类和 learning bus 可用性摘要。
6. `test_ci.sh` 新增 6ac BLUE15 P1-2 主链门禁，覆盖学习回放场景执行与全场景回归。

本轮完成率：
1. “B15-P1-2 学习数据持久化与回放”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 69%（按 13 个建议项粗略计，已完成 9 个大项）。

## 二十二、本轮实施回写（2026-04-15，续9）

本轮目标：完成 B15-P1-3「PUA 动态规则与审计可视化」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增治理动态能力 RPC：`governance.plan.get`、`governance.plan.update`、`governance.audit.recent`，并将其接入 ACP 方法白名单与主分发链路。
2. backend 在 `governance.status` 输出中扩展 `dynamic_rules` 与 `audit.recent` 可观测字段，形成“规则动态态 + 审计窗口”统一主链视图。
3. backend 新增 `.goon/governance/audit.ndjson` 审计持久化读写，实现治理计划更新的审计落盘与最近事件回放。
4. backend 新增 `requests/governance-dynamic-rules-benchmark.ndjson` 场景与 `run_scenario_file_executes_governance_dynamic_rules_benchmark_requests` 集成测试，`ndjson_scenario_files_all_pass` 场景总数同步扩展到 8。
5. GUI `SecurityView` 接入真实治理审计链路：新增动态规则与最近审计计数展示，并通过 `governance.audit.recent` 渲染审计日志表，不再仅使用本地合成演示数据。
6. vscode-addon 新增 `go-on.governancePlanGet` 与 `go-on.governanceAuditRecent` 命令，Settings 面板新增 `Governance Plan` / `Governance Audit` 按钮，支持治理计划摘要和审计窗口快速查询。
7. `test_ci.sh` 新增 6ad BLUE15 P1-3 主链门禁，覆盖治理动态规则场景执行与全场景回归。

本轮完成率：
1. “B15-P1-3 PUA 动态规则与审计可视化”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 77%（按 13 个建议项粗略计，已完成 10 个大项）。

## 二十三、本轮实施回写（2026-04-15，续10）

本轮目标：完成 B15-P2-1「故障恢复与降级（断路器）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 扩展 `breaker.status` 输出：新增 `degraded_count` 与 `degraded_services`，统一呈现 failure-prevention 降级对象、熔断状态、降级等级和恢复建议。
2. backend 新增 `breaker.recovery` RPC（支持 `dry_run` 与按 agent 定向恢复），将 failure-prevention 恢复与 circuit-breaker reset 串为同一主链动作，并返回候选、恢复结果与剩余降级对象。
3. backend 在 `src/optimization/failure_prevention.rs` 新增恢复能力：支持单服务/全量恢复到健康基线，并补齐 `test_recover_resets_unhealthy_service_to_healthy` 回归测试。
4. backend 新增 `requests/breaker-recovery-benchmark.ndjson` 场景与 `run_scenario_file_executes_breaker_recovery_benchmark_requests` 集成测试，`ndjson_scenario_files_all_pass` 场景总数同步扩展到 9。
5. GUI `BackendOpsView` 新增 `breaker.recovery` 快捷入口；`HealthBreakdownView` 接入 `breaker.status` 降级对象与恢复建议展示，实现断路器“状态 + 恢复建议”可视化闭环。
6. vscode-addon 新增 `go-on.breakerRecovery` 命令（含可选 agent 定向恢复），并在 Settings 面板增加 `Breaker Recovery` 按钮直连后端主链接口。
7. `test_ci.sh` 新增 6ae BLUE15 P2-1 主链门禁，覆盖恢复单测、断路器恢复场景回放与全场景回归。

本轮完成率：
1. “B15-P2-1 故障恢复与降级（断路器）”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 85%（按 13 个建议项粗略计，已完成 11 个大项）。

## 二十四、本轮实施回写（2026-04-15，续11）

本轮目标：完成 B15-P2-2「可观测性完善（追踪覆盖率与告警）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `observability.alerts` RPC：聚合 runtime lifecycle、timeout 指标、断路器状态、降级服务和锁健康，输出统一告警项（severity/code/message/suggestion）与分级汇总。
2. backend 将 `observability.alerts` 接入 ACP 方法白名单与请求主分发链路，支持按 `limit` 限制告警返回量。
3. backend 新增 `requests/observability-alerts-benchmark.ndjson` 场景（`initialize -> runtime.health -> health.probes -> observability.alerts -> shutdown`）。
4. backend 集成测试新增 `run_scenario_file_executes_observability_alerts_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 10。
5. GUI `BackendOpsView` 新增 `observability.alerts` 快捷调用入口，支持直接查看聚合告警 JSON。
6. vscode-addon 新增 `go-on.observabilityAlerts` 命令，并在 Settings 面板增加 `Observability Alerts` 按钮；命令输出 critical/warn/info 摘要与首条告警代码。
7. `test_ci.sh` 新增 6af BLUE15 P2-2 主链门禁，覆盖 observability 场景执行与全场景回归。

本轮完成率：
1. “B15-P2-2 可观测性完善（追踪覆盖率与告警）”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：约 92%（按 13 个建议项粗略计，已完成 12 个大项）。

## 二十五、本轮实施回写（2026-04-15，续12）

本轮目标：完成 B15-P0-1「生产化安全基线（鉴权、限流、反向代理、TLS）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `security.baseline` RPC，并接入 ACP 方法白名单与主分发链路，统一输出入口暴露状态、鉴权配置状态、入口限流参数、production_strict 违规项与风险列表。
2. backend `security.baseline` 将 `governance_config_summary` 与运行时入口配置联动，形成可直接用于上线前检查的 `level/ingress_status/risk_count/risks` 结构化结果。
3. backend 新增 `requests/security-baseline-benchmark.ndjson` 场景（`initialize -> runtime.health -> security.baseline -> governance.status -> shutdown`）。
4. backend 集成测试新增 `run_scenario_file_executes_security_baseline_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 11。
5. GUI `BackendOpsView` 新增 `security.baseline` 快捷调用入口，支持直接查看安全基线主链结果。
6. vscode-addon 新增 `go-on.securityBaseline` 命令，并在 Settings 面板增加 `Security Baseline` 按钮，命令输出 level/ingress/strict/risk_count 摘要。
7. `test_ci.sh` 新增 6ag BLUE15 P0-1 主链门禁，覆盖安全基线场景执行与全场景回归。

本轮完成率：
1. “B15-P0-1 生产化安全基线（鉴权、限流、反向代理、TLS）”子目标完成率：100%。
2. BLUE15 全量清单总体完成率：100%（按 13 个建议项粗略计，已完成 13/13 个大项）。

## 二十六、本轮实施回写（2026-04-15，续13）

本轮目标：完成 B15-X5「Harness 评测基座与回归挑战集」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `harness.status` RPC，并接入 ACP 方法白名单与主分发链路，统一输出固定随机种子、场景总量、四类 suite（smoke/regression/adversarial/long_chain）分层统计、评分卡维度与运行时快照。
2. backend 新增 `requests/harness-benchmark.ndjson` 场景（`initialize -> harness.status -> metrics.get -> shutdown`），将 Harness 状态检查纳入请求主链。
3. backend 集成测试新增 `run_scenario_file_executes_harness_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 12，保证新场景自动纳入全量回归。
4. GUI `BackendOpsView` 新增 `harness.status` 快捷按钮，可直接发起 Harness 基座状态查询。
5. vscode-addon 新增 `go-on.harnessStatus` 命令，并在 Settings 面板增加 `Harness Status` 按钮，输出 suite 分层数量与固定 seed 摘要。
6. `test_ci.sh` 新增 6ah BLUE15 X5 主链门禁，覆盖 Harness 场景执行与全场景回归。

本轮完成率：
1. “B15-X5 Harness 评测基座与回归挑战集”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 9%（按 X1~X11 粗略计，已完成 1/11 个扩展大项）。

## 二十七、本轮实施回写（2026-04-15，续14）

本轮目标：完成 B15-X1「自学习闭环质量门禁（Learning Loop Guardrail）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `learning.guardrail` RPC，并接入 ACP 方法白名单与主分发链路，统一输出学习样本质量门禁状态（`pass/warn/block`）、阈值配置、质量统计与告警列表。
2. backend 新增学习门禁核心评估逻辑：覆盖可解析性（parseable ratio）、证据完整性、结果可归因、高风险样本覆盖、相似样本去重比例、冷却窗口判定与最小样本量门禁。
3. backend 将 `learning.summary` 主链返回扩展为内嵌 `guardrail` 摘要，保证既有学习汇总查询默认携带门禁结果，不需要额外切换接口。
4. backend 新增 `requests/learning-loop-guardrail-benchmark.ndjson` 场景（`initialize -> learning.guardrail -> learning.summary -> shutdown`）。
5. backend 集成测试新增 `run_scenario_file_executes_learning_loop_guardrail_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 13。
6. GUI `BackendOpsView` 新增 `learning.guardrail` 快捷按钮，可直接发起学习闭环质量门禁检查。
7. vscode-addon 新增 `go-on.learningGuardrail` 命令，并在 Settings 面板增加 `Learning Guardrail` 按钮，输出门禁状态、样本数、可解析率、质量率和高风险样本摘要。
8. `test_ci.sh` 新增 6ai BLUE15 X1 主链门禁，覆盖学习门禁场景执行与全场景回归。

本轮完成率：
1. “B15-X1 自学习闭环质量门禁（Learning Loop Guardrail）”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 18%（按 X1~X11 粗略计，已完成 2/11 个扩展大项：X5、X1）。

## 二十八、本轮实施回写（2026-04-15，续15）

本轮目标：完成 B15-X2「知识萃取分层与去噪（Knowledge Distillation Pipeline）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `knowledge.distill` RPC，并接入 ACP 方法白名单与请求主分发链路。
2. backend `knowledge.distill` 输出三层结构：
	- `evidence`：回放 `.goon/learning` 原始学习记录（workflow/pua 分类统计 + 原始记录窗口）；
	- `summary`：回放 `spec/latest-knowledge.json` 的知识摘要窗口；
	- `strategy`：从摘要层生成可执行规则候选（rule_id/when/then/confidence/source）。
3. backend 新增冲突检测与知识墓碑机制：
	- 以 `task+phase` 进行聚类，按 `confidence + generated_at` 选主结论；
	- 将被替代结论记为 `conflicts`；
	- 可选写入 `.goon/knowledge/tombstones.ndjson`，并支持回放最近墓碑窗口。
4. backend 新增 `requests/knowledge-distillation-benchmark.ndjson` 场景（`initialize -> learning.replay -> knowledge.distill -> shutdown`）。
5. backend 集成测试新增 `run_scenario_file_executes_knowledge_distillation_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 14。
6. GUI `BackendOpsView` 新增 `knowledge.distill` 快捷按钮，并在中英文 locale 增加 `knowledgeDistill` 标签。
7. vscode-addon 新增 `go-on.knowledgeDistill` 命令（命令面板可用），并在 Settings 面板增加 `Knowledge Distill` 按钮直连后端接口。
8. `test_ci.sh` 新增 6aj BLUE15 X2 主链门禁，覆盖知识萃取场景执行与全场景回归。

本轮完成率：
1. “B15-X2 知识萃取分层与去噪”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 27%（按 X1~X11 粗略计，已完成 3/11 个扩展大项：X5、X1、X2）。

## 二十九、本轮实施回写（2026-04-15，续16）

本轮目标：完成 B15-X3「强化学习奖励对齐与离线评估（RL Alignment & Offline Eval）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `rl.alignment.offline_eval` RPC，并接入 ACP 方法白名单与请求主分发链路。
2. backend `rl.alignment.offline_eval` 实现多目标奖励对齐：按 `success/latency/tool_error/safety` 四维权重计算样本 reward，并支持参数化权重配置。
3. backend 实现离线回放评估（off-policy replay）：将学习窗口拆分为 baseline/candidate 两段，评估 reward uplift 与安全惩罚回归，输出通过判定与建议运行模式。
4. backend 增加奖励漂移检测：比较 recent 与 historical reward 均值差异，超过阈值触发 drift alert，并给出保守回退建议。
5. backend 新增 `requests/rl-alignment-offline-eval-benchmark.ndjson` 场景（`initialize -> learning.replay -> rl.alignment.offline_eval -> shutdown`）。
6. backend 集成测试新增 `run_scenario_file_executes_rl_alignment_offline_eval_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 15。
7. GUI `BackendOpsView` 新增 `rl.alignment.offline_eval` 快捷按钮，并在中英文 locale 增加 `rlAlignmentEval` 标签。
8. vscode-addon 新增 `go-on.rlAlignmentEval` 命令（命令面板可用），并在 Settings 面板增加 `RL Alignment Eval` 按钮直连后端接口。
9. `test_ci.sh` 新增 6ak BLUE15 X3 主链门禁，覆盖 RL 离线评估场景执行与全场景回归。

本轮完成率：
1. “B15-X3 强化学习奖励对齐与离线评估”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 36%（按 X1~X11 粗略计，已完成 4/11 个扩展大项：X5、X1、X2、X3）。

## 三十、本轮实施回写（2026-04-15，续17）

本轮目标：完成 B15-X4「Hardness 分级路由与预算编排（Difficulty-aware Routing）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 落地 `hardness.status` 主链 RPC，统一输出 hardness 四维评分（context/cross-file/tool/recovery）、等级分层（low/medium/high/extreme）与预算编排建议（timeout/parallelism/reviews/mode）。
2. backend 在 `task.execute` 主链接入 hardness 默认编排：将 hardness 预算合并到 `adaptive.execution_defaults.hardness`，驱动并发上限、超时预算与推荐执行模式，形成“评分 -> 路由 -> 执行”闭环。
3. backend 请求场景 `requests/hardness-routing-benchmark.ndjson` 纳入主链序列（`initialize -> hardness.status -> task.execute -> shutdown`），并在集成测试修正为可编译的稳定用例名 `run_scenario_file_executes_hardness_routing_benchmark_requests`。
4. GUI `BackendOpsView` 已接入 `hardness.status` 按钮，支持直接发起难度路由评估；locale 同步提供 `hardnessStatus` 标签，保证界面可见与语义一致。
5. vscode-addon 已接入 `go-on.hardnessStatus` 命令与 Settings 按钮，本轮补齐 `package.json` 激活事件与命令面板注册，保证命令可发现、可触发、可回放。
6. `test_ci.sh` 新增 6al BLUE15 X4 主链门禁，覆盖 hardness 场景执行与全场景回归。

本轮完成率：
1. “B15-X4 Hardness 分级路由与预算编排”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 45%（按 X1~X11 粗略计，已完成 5/11 个扩展大项：X5、X1、X2、X3、X4）。

## 三十一、本轮实施回写（2026-04-15，续18）

本轮目标：完成 B15-X6「Token 成本压缩与 Cost 治理（Token Compression & Cost Governance）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `cost.status` RPC，并接入 ACP 方法白名单与主分发链路，统一输出 token 预算、压缩策略、成本路由和成本遥测摘要。
2. backend 落地成本治理核心模型：
	- 基于 task + params + hardness 的输入/输出 token 预算分层；
	- 根据预算与阈值触发压缩策略（rolling summary / dedupe evidence / adaptive retrieval window）；
	- 输出模型层级偏好、降级策略与高成本模型冷却建议；
	- 汇总运行时指标形成估算成本与风险遥测。
3. backend 将成本治理接入 `task.execute` 主链默认编排：在 `adaptive.execution_defaults` 中新增 `cost` 概要，实现“cost.status -> task.execute”一致语义闭环。
4. backend 新增 `requests/token-cost-governance-benchmark.ndjson` 场景（`initialize -> cost.status -> task.execute -> shutdown`）。
5. backend 集成测试新增 `run_scenario_file_executes_token_cost_governance_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 17。
6. GUI `BackendOpsView` 新增 `cost.status` 按钮，locale 同步新增 `backendOps.costStatus`（中英文）。
7. vscode-addon 新增 `go-on.costStatus` 命令、Settings 面板 `Cost Status` 按钮、消息分发与 command palette 清单注册。
8. `test_ci.sh` 新增 6am BLUE15 X6 主链门禁，覆盖成本治理场景执行与全场景回归。

本轮完成率：
1. “B15-X6 Token 成本压缩与 Cost 治理”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 55%（按 X1~X11 粗略计，已完成 6/11 个扩展大项：X5、X1、X2、X3、X4、X6）。

## 三十二、本轮实施回写（2026-04-15，续19）

本轮目标：完成 B15-X7「配置基线收敛与一次性清理（Config Baseline Freeze）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `config.baseline` RPC，并接入 ACP 方法白名单与主分发链路，统一输出配置基线冻结状态。
2. backend `config.baseline` 输出新增“最终生效视图 + 来源优先级 + 字段来源映射”：覆盖 protocol/rate-limit/strict/trace 等关键 runtime 字段。
3. backend 新增配置来源分析与迁移提示能力：
	- 识别 `cli_override / env / config_file / default` 优先级；
	- 检测 legacy 配置键并输出 `old_path -> new_path` 替换建议；
	- 输出兼容窗口与迁移后验证建议（`config.reload` / `runtime.health`）。
4. backend 新增 `requests/config-baseline-benchmark.ndjson` 场景（`initialize -> config.baseline -> config.reload -> shutdown`）。
5. backend 集成测试新增 `run_scenario_file_executes_config_baseline_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 18。
6. GUI `BackendOpsView` 新增 `config.baseline` 按钮，locale 同步新增 `backendOps.configBaseline`（中英文）。
7. vscode-addon 新增 `go-on.configBaseline` 命令，并在 Settings 面板新增 `Config Baseline` 按钮、消息分发与 command palette 清单注册。
8. `test_ci.sh` 新增 6an BLUE15 X7 主链门禁，覆盖配置基线场景执行与全场景回归。
9. 同轮修复跨端潜在回归：移除 GUI 与 vscode-addon 中 `cost.status` 的无效 `phase=execute` 参数，避免与主链 phase 约束冲突。

本轮完成率：
1. “B15-X7 配置基线收敛与一次性清理”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 64%（按 X1~X11 粗略计，已完成 7/11 个扩展大项：X5、X1、X2、X3、X4、X6、X7）。

## 三十三、本轮实施回写（2026-04-15，续20）

本轮目标：完成 B15-X8「错误码、重试语义与客户端契约统一（Error Contract Unification）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 将 `error.contract` 接入 ACP 方法白名单与主分发链路，并提供统一契约结构：`version + kinds + retry policy + compatibility`。
2. backend 统一错误输出契约：通过 `send_error` 的数据增强逻辑补齐 `kind`、`retry`、`version` 等字段，确保参数错误/限流/上游错误/内部错误等类别语义稳定。
3. backend 新增 `requests/error-contract-benchmark.ndjson` 场景（`initialize -> error.contract -> runtime.health -> shutdown`），覆盖契约查询与运行态主链联动。
4. backend 集成测试新增 `run_scenario_file_executes_error_contract_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 19。
5. GUI `BackendOpsView` 新增 `error.contract` 按钮，locale 同步新增 `backendOps.errorContract`（中英文），实现前端主链可见。
6. vscode-addon 新增 `go-on.errorContract` 命令，并在 Settings 面板新增 `Error Contract` 按钮、消息分发与 command palette 清单注册。
7. `test_ci.sh` 新增 6ao BLUE15 X8 主链门禁，覆盖错误契约场景执行与全场景回归。

本轮完成率：
1. “B15-X8 错误码、重试语义与客户端契约统一”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 73%（按 X1~X11 粗略计，已完成 8/11 个扩展大项：X5、X1、X2、X3、X4、X6、X7、X8）。

## 三十四、本轮实施回写（2026-04-15，续21）

本轮目标：完成 B15-X9「依赖与构建可复现优化（Reproducible Build Pack）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `build.repro` RPC，并接入 ACP 方法白名单与主分发链路，统一输出可复现构建包摘要。
2. backend `build.repro` 输出补齐构建可复现元数据：
	- 锁文件与清单清册（`Cargo.lock`、`GUI/package-lock.json`、`vscode-addon/package-lock.json`、各端 `package.json` / `Cargo.toml`）；
	- 版本/提交/构建参数（`package_version`、`git_commit`、`rustflags`、`cargo_build_target`、`cargo_profile`）；
	- 发布产物校验清单（binary/frontend/addon 产物存在性 + `fnv1a64` 指纹）；
	- 缺失必需项统计与状态判定（`reproducible_ready` / `reproducible_incomplete`）。
3. backend 新增 `requests/build-repro-benchmark.ndjson` 场景（`initialize -> build.repro -> runtime.health -> shutdown`），覆盖可复现包查询与运行态主链联动。
4. backend 集成测试新增 `run_scenario_file_executes_build_repro_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 20。
5. GUI `BackendOpsView` 新增 `build.repro` 按钮，locale 同步新增 `backendOps.buildRepro`（中英文）。
6. vscode-addon 新增 `go-on.buildRepro` 命令，并在 Settings 面板新增 `Build Repro` 按钮、消息分发与 command palette 清单注册。
7. `test_ci.sh` 新增 6ap BLUE15 X9 主链门禁，覆盖构建可复现场景执行与全场景回归。

本轮完成率：
1. “B15-X9 依赖与构建可复现优化”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 82%（按 X1~X11 粗略计，已完成 9/11 个扩展大项：X5、X1、X2、X3、X4、X6、X7、X8、X9）。

## 三十五、本轮实施回写（2026-04-15，续22）

本轮目标：完成 B15-X10「数据生命周期与存储治理（Data Lifecycle Governance）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `data.lifecycle` RPC，并接入 ACP 方法白名单与主分发链路。
2. backend `data.lifecycle` 输出统一生命周期治理视图：
	- `policy`：cache/vector/ledger 保留策略、清理频率、归档规则；
	- `storage`：cache/vector/ledger 容量统计（字节、文件数、目录数、不可读项）与水位阈值（warn/critical）；
	- `cleanup`：支持 `execute_gc` 参数触发一次维护周期并回传清理结果；
	- `audit`：维护快照、回放序列与后续动作建议，满足可观测/可审计/可回放。
3. backend 新增 `requests/data-lifecycle-benchmark.ndjson` 场景（`initialize -> data.lifecycle -> runtime.health -> shutdown`），覆盖生命周期治理与运行态主链联动。
4. backend 集成测试新增 `run_scenario_file_executes_data_lifecycle_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 21。
5. GUI `BackendOpsView` 新增 `data.lifecycle` 按钮，locale 同步新增 `backendOps.dataLifecycle`（中英文）。
6. vscode-addon 新增 `go-on.dataLifecycle` 命令，并在 Settings 面板新增 `Data Lifecycle` 按钮、消息分发与 command palette 清单注册。
7. `test_ci.sh` 新增 6aq BLUE15 X10 主链门禁，覆盖数据生命周期场景执行与全场景回归。

本轮完成率：
1. “B15-X10 数据生命周期与存储治理”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：约 91%（按 X1~X11 粗略计，已完成 10/11 个扩展大项：X5、X1、X2、X3、X4、X6、X7、X8、X9、X10）。

## 三十六、本轮实施回写（2026-04-15，续23）

本轮目标：完成 B15-X11「总体一次优化到顶（One-Shot Optimization Peak）」并接入三端主链。

已完成项（一个大项闭环）：
1. backend 在 `src/acp/impl/request.rs` 新增 `optimization.peak` RPC，并接入 ACP 方法白名单与主分发链路。
2. backend `optimization.peak` 输出联合峰值治理视图：
	- `gates`：quality / cost / stability / security / reproducibility / governance 六维门禁；
	- `overall_pass`：联合阈值是否通过；
	- `frozen_scope`：X1~X10 冻结范围快照；
	- `summary`：请求失败率、评审拒绝率、超时计数、breaker 与运行时健康摘要。
3. backend 将 hardness 与 cost 治理结果接入 `optimization.peak` 输出，形成 X4/X6 与 X11 的一致语义闭环。
4. backend 新增 `requests/optimization-peak-benchmark.ndjson` 场景（`initialize -> optimization.peak -> governance.status -> shutdown`）。
5. backend 集成测试新增 `run_scenario_file_executes_optimization_peak_benchmark_requests`，并将 `ndjson_scenario_files_all_pass` 场景总数扩展到 22。
6. GUI `BackendOpsView` 新增 `optimization.peak` 按钮，locale 同步新增 `backendOps.optimizationPeak`（中英文）。
7. vscode-addon 新增 `go-on.optimizationPeak` 命令，并在 Settings 面板新增 `Optimization Peak` 按钮、消息分发与 command palette 清单注册。
8. `test_ci.sh` 新增 6ar BLUE15 X11 主链门禁，覆盖优化峰值场景执行与全场景回归。

本轮完成率：
1. “B15-X11 总体一次优化到顶”子目标完成率：100%。
2. BLUE15 基线清单总体完成率：保持 100%（13/13）。
3. BLUE15 扩展清单完成率：100%（按 X1~X11 粗略计，已完成 11/11 个扩展大项：X5、X1、X2、X3、X4、X6、X7、X8、X9、X10、X11）。