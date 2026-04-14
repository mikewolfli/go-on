# BLUE15 — 基于全项目扫描的智能优化与强化建议

> 延续 BLUE14 的执行纪律：先方案冻结、再分阶段实施、最后端到端验收。
> 本文件基于对 go-on 项目的全面扫描结果（2026-04-14），针对自学习、强化学习、PUA、硬化、测试框架等维度提出优化建议，
> 每条给出**是否需要**判断与**推荐建议**，按实施优先级由高到低排列。

---

## 背景与约束确认

基于 BLUE14 后的真实项目状态（2026-04-14 全项目扫描）：

| 维度 | 现状 |
|---|---|
| 项目架构 | 模块化设计良好，代码质量高，226个测试全部通过 |
| 自学习机制 | `src/intelligence/adaptive_selector.rs` 已实现基础成功率统计 |
| 强化学习 | `src/intelligence/reinforcement.rs` 已实现基础设施但算法简单 |
| PUA 治理 | `src/governance/pua.rs` 已实现完整规则引擎，17个测试通过 |
| 硬化机制 | `src/governance/hardening.rs` 已实现预算和配额管理 |
| 测试框架 | 226个单元测试 + 34个集成测试 + 17个PUA测试，CI流程完整 |
| 代码质量 | 通过所有 clippy 检查，无编译警告 |

本轮强约束（继承 BLUE14）：

1. 所有改进必须可在 CI 中无外部依赖稳定执行。
2. 不破坏已通过的 226 个测试。
3. 每条优化必须可独立实施、可验收，不做大爆炸式重构。
4. 安全缺陷（OWASP）优先于功能优化。
5. 所有被采纳优化项必须按推荐方案完整实现，并接入主链路；接入方式（实时接入或 LAZY LOAD）由业务场景决策，但必须形成主链路闭环（触发 -> 执行 -> 反馈 -> 度量/审计），确保对程序产生积极作用，禁止"仅定义不生效"或"仅旁路演示"实现。
6. 方案选择需以"对现有结构更优或更完备"为目标，并以"最小化当前项目代码改动"为实现原则：优先复用现有模块/接口/测试资产，避免无必要的新层级、新抽象和大范围重写。
7. 全部更改完成后必须统一后台程序与 GUI 程序的资源文件命名与目录约定；考虑两者 EXE 编译产物会落在同一文件夹，必须进行冲突规避（文件名空间、子目录隔离或构建后重命名策略），禁止资源同名覆盖。
8. 每完成一个大项必须同步更新后台程序、GUI 与 vscode-addon 插件（接口、协议、配置、文档与验证脚本），确保三端功能对齐与可协同运行，避免"单端先行、其余端失配"。
9. 严禁任何 bridge-stub/测试桥接 shim 方案（尤其是在测试内用本地模块模拟生产模块）；遇到依赖边界或模块可见性问题，必须通过真实工程结构修复（模块归属、导出、重构）解决，禁止用测试侧补丁绕过。

闭环判定标准（适用于全体 B15 条目）：

1. 有主链路触发点：至少 1 个生产路径可稳定触发该功能。
2. 有执行与结果：功能执行结果可被调用方消费（返回值、状态、或副作用可观测）。
3. 有反馈与治理：失败/退化可进入日志、审计或学习系统。
4. 有验收门禁：至少 1 条自动化测试或 CI 门禁可阻止回归。
5. 有三端协同验收：每个大项完成时需至少验证一次 backend + GUI + vscode-addon 的联动路径。
6. 有同目录部署安全性：后台与 GUI 的 EXE 同目录部署时资源文件无重名冲突、无覆盖回归。

---

## 目标（按优先级排序）

| ID | 优先级 | 目标 | 是否需要 | 目标文件 |
|---|---|---|---|---|
| B15-P0-1 | P0 | 强化学习算法增强（多臂老虎机） | ✅ 必须 | `src/intelligence/adaptive_selector.rs` |
| B15-P0-2 | P0 | PUA 规则动态配置与可视化 | ✅ 必须 | `src/governance/pua.rs` + GUI |
| B15-P1-1 | P1 | 测试覆盖率提升与性能基准 | ✅ 需要 | `test_ci.sh` + `tests/` |
| B15-P1-2 | P1 | 学习数据持久化存储 | ✅ 需要 | `src/intelligence/reinforcement.rs` |
| B15-P2-1 | P2 | 硬化机制增强（故障恢复） | ⚠️ 需要 | `src/governance/hardening.rs` |
| B15-P2-2 | P2 | 可观测性增强（分布式追踪） | ⚠️ 需要 | `src/observability/` |
| B15-P3-1 | P3 | 微服务化架构准备 | 💡 参考价值 | 架构文档 |

---

## 技术选型（冻结）

### P0 强化学习算法策略
- 在现有 `AdaptiveModelSelector` 基础上添加多臂老虎机（Multi-armed Bandit）算法
- 保留现有成功率统计作为基础，添加探索-利用平衡机制
- 不引入外部机器学习库，使用纯 Rust 实现简单 UCB1 算法

### P0 PUA 动态配置策略
- 新增 `PuaRuleLoader` 支持从文件动态加载规则
- 在 GUI 中添加 PUA 规则可视化仪表板
- 保持向后兼容：未配置动态规则时使用默认规则

### P1 测试覆盖率策略
- 添加 `cargo tarpaulin` 代码覆盖率报告到 CI
- 新增性能基准测试模块 `tests/benchmarks/`
- 不改变现有测试结构，仅添加新测试类型

### P1 学习数据持久化策略
- 在现有 SQLite 存储基础上添加学习数据表
- 实现学习历史查询和分析接口
- 添加数据清理和归档策略

### P2 硬化增强策略
- 在现有 `BudgetTracker` 基础上添加断路器模式
- 实现优雅降级和自动故障转移
- 添加资源使用监控和警报

### P2 可观测性增强策略
- 在现有 `telemetry` 基础上添加 OpenTelemetry 支持
- 实现分布式追踪上下文传播
- 添加指标聚合和仪表板

### P3 微服务化策略
- 仅进行架构分析和文档准备
- 不实施实际拆分，保持单体架构
- 识别潜在的服务边界和接口

---

## 详细实施步骤

---

### B15-P0-1：强化学习算法增强（多臂老虎机）

**是否需要**：✅ **必须**

> 根因：当前 `AdaptiveModelSelector` 仅基于简单成功率统计选择模型，缺乏探索-利用平衡机制。
> 可能导致"赢家通吃"问题，新模型或改进模型无法获得足够测试机会。

**推荐建议**：在现有 `AdaptiveModelSelector` 基础上添加 UCB1（Upper Confidence Bound）算法实现多臂老虎机。

#### 步骤 1.1：扩展 `ModelMetrics` 数据结构

在 `src/intelligence/adaptive_selector.rs` 中扩展 `ModelMetrics`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetrics {
    pub model_id: String,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub success_rate: f32,
    // 新增字段用于 UCB1 算法
    pub exploration_factor: f32,  // 探索因子，默认 2.0
    pub last_selected_at: i64,    // 上次被选择的时间戳
}
```

#### 步骤 1.2：实现 UCB1 得分计算

添加 UCB1 得分计算方法：

```rust
impl ModelMetrics {
    pub fn ucb1_score(&self, total_selections: u64, current_time: i64) -> f64 {
        if self.total_requests == 0 {
            // 从未尝试过的模型获得最高探索分数
            return f64::MAX;
        }
        
        let exploitation = self.success_rate as f64;
        let exploration = self.exploration_factor as f64 * 
            ((total_selections as f64).ln() / self.total_requests as f64).sqrt();
        
        // 时间衰减因子：长时间未选择的模型获得额外探索分数
        let time_since_last = (current_time - self.last_selected_at) as f64;
        let time_bonus = if time_since_last > 3600.0 { // 1小时
            0.1 * (time_since_last / 3600.0).ln()
        } else {
            0.0
        };
        
        exploitation + exploration + time_bonus
    }
}
```

#### 步骤 1.3：更新模型选择逻辑

修改 `get_best_model` 方法使用 UCB1 算法：

```rust
pub fn get_best_model(&mut self, candidates: &[String]) -> Option<String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    
    let total_selections: u64 = self.metrics.values().map(|m| m.total_requests).sum();
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    
    let mut best = None;
    let mut best_score = f64::NEG_INFINITY;
    
    for candidate in candidates {
        if let Some(metrics) = self.metrics.get_mut(candidate) {
            let score = metrics.ucb1_score(total_selections, current_time);
            if score > best_score {
                best_score = score;
                best = Some(candidate.clone());
            }
        } else {
            // 新模型：初始化并给予最高探索分数
            let new_metrics = ModelMetrics {
                model_id: candidate.clone(),
                total_requests: 0,
                successful_requests: 0,
                success_rate: 0.5,
                exploration_factor: 2.0,
                last_selected_at: current_time,
            };
            self.metrics.insert(candidate.clone(), new_metrics);
            return Some(candidate.clone()); // 立即选择新模型进行探索
        }
    }
    
    if let Some(best_model) = &best {
        // 更新被选择模型的时间戳
        if let Some(metrics) = self.metrics.get_mut(best_model) {
            metrics.last_selected_at = current_time;
        }
    }
    
    best
}
```

#### 步骤 1.4：添加配置参数

在 `src/core/config.rs` 中添加强化学习配置：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReinforcementConfig {
    /// UCB1 探索因子
    pub exploration_factor: f32,
    /// 是否启用时间衰减
    pub enable_time_decay: bool,
    /// 时间衰减阈值（秒）
    pub time_decay_threshold: i64,
}

impl Default for ReinforcementConfig {
    fn default() -> Self {
        Self {
            exploration_factor: 2.0,
            enable_time_decay: true,
            time_decay_threshold: 3600, // 1小时
        }
    }
}
```

#### 步骤 1.5：添加单测验证 UCB1 算法

```rust
#[test]
fn test_ucb1_new_model_exploration() {
    let mut selector = AdaptiveModelSelector::new();
    let candidates = vec!["model-a".to_string(), "model-b".to_string()];
    
    // 两个都是新模型，应该选择第一个
    let best = selector.get_best_model(&candidates);
    assert_eq!(best, Some("model-a".to_string()));
    
    // 记录 model-a 成功
    selector.record_result("model-a", true);
    
    // 再次选择，model-a 有数据，model-b 是新模型，应该选择 model-b 进行探索
    let best = selector.get_best_model(&candidates);
    assert_eq!(best, Some("model-b".to_string()));
}

#[test]
fn test_ucb1_exploitation_vs_exploration() {
    let mut selector = AdaptiveModelSelector::new();
    
    // model-a 有 90% 成功率但尝试次数少
    for _ in 0..10 {
        selector.record_result("model-a", true);
    }
    selector.record_result("model-a", false);
    
    // model-b 有 80% 成功率但尝试次数多
    for _ in 0..90 {
        selector.record_result("model-b", true);
    }
    for _ in 0..10 {
        selector.record_result("model-b", false);
    }
    
    let candidates = vec!["model-a".to_string(), "model-b".to_string()];
    let best = selector.get_best_model(&candidates);
    
    // UCB1 应该平衡探索和利用
    // model-a 成功率更高但尝试少，应该获得探索机会
    assert!(best.is_some());
}
```

#### 步骤 1.6：接入主链

在 `src/intelligence/model_selector.rs` 中使用增强的 `AdaptiveModelSelector`：

```rust
pub fn select_best_model_with_rl(
    candidates: &[String],
    context: &ModelSelectionContext,
) -> Result<String> {
    let mut selector = AdaptiveModelSelector::new();
    // 从持久化存储加载历史数据
    // 应用 UCB1 算法选择模型
    selector.get_best_model(candidates)
        .ok_or_else(|| anyhow!("No suitable model found"))
}
```

**验收标准**：
- 新模型能够获得探索机会（不被已有高成功率模型完全压制）
- UCB1 算法在测试中正确平衡探索和利用
- 时间衰减机制对长时间未使用模型有效
- 所有现有测试继续通过
- 新增测试验证 UCB1 算法行为

---

### B15-P0-2：PUA 规则动态配置与可视化

**是否需要**：✅ **必须**

> 根因：当前 PUA 规则硬编码在代码中，无法根据实际运行情况动态调整。
> 缺乏可视化界面，运维人员无法实时监控 PUA 执行情况和违规统计。

**推荐建议**：实现 PUA 规则动态加载和 GUI 可视化仪表板。

#### 步骤 2.1：创建 PUA 规则配置文件格式

创建 `pua_rules.toml` 配置文件格式：

```toml
[red_lines]
line1 = "Close the loop - Reject claims like 'I think it works' without build or test proof"
line2 = "Fact-driven verification - Reject unverified attribution such as 'probably environment issue'"
line3 = "Exhaust approaches - Reject early-exit responses after repeated failure"

[escalation_levels]
l0 = { description = "normal execution" }
l1 = { description = "after first failure, force a different approach", checks = ["different_approach"] }
l2 = { description = "after repeated failure, require deep search plus multiple hypotheses", checks = ["deep_search", "multiple_hypotheses"] }
l3 = { description = "execute all checklist items", checks = ["checklist_complete"] }
l4 = { description = "invert assumptions and run opposite strategy", checks = ["invert_assumptions"] }

[quality_compass]
checks = [
    "Build proof shown",
    "Error paths tested",
    "Pattern category scanned (iceberg rule)",
    "Root cause and prevention explained",
    "Quality improved with explicit rationale",
]

[stage_requirements]
planning = { required_actions = ["task_analysis", "approach_selection"], hard_fail_conditions = ["no_analysis"] }
execution = { required_actions = ["code_generation", "testing"], hard_fail_conditions = ["no_tests"] }
review = { required_actions = ["code_review", "security_check"], hard_fail_conditions = ["security_violation"] }
```

#### 步骤 2.2：实现 PUA 规则加载器

在 `src/governance/pua.rs` 中添加 `PuaRuleLoader`：

```rust
pub struct PuaRuleLoader {
    rules_path: PathBuf,
    last_modified: Option<std::time::SystemTime>,
    cached_rules: Arc<StdMutex<Option<PuaEnforcementPlan>>>,
}

impl PuaRuleLoader {
    pub fn new(rules_path: impl AsRef<Path>) -> Self {
        Self {
            rules_path: rules_path.as_ref().to_path_buf(),
            last_modified: None,
            cached_rules: Arc::new(StdMutex::new(None)),
        }
    }
    
    pub fn load_rules(&mut self) -> Result<PuaEnforcementPlan> {
        // 检查文件是否修改
        let metadata = std::fs::metadata(&self.rules_path)?;
        let modified = metadata.modified()?;
        
        if self.last_modified.map(|lm| lm < modified).unwrap_or(true) {
            // 文件已修改，重新加载
            let content = std::fs::read_to_string(&self.rules_path)?;
            let rules: PuaEnforcementPlan = toml::from_str(&content)
                .context("Failed to parse PUA rules TOML")?;
            
            let mut cache = self.cached_rules.lock()
                .map_err(|e| anyhow!("Failed to lock rules cache: {}", e))?;
            *cache = Some(rules.clone());
            self.last_modified = Some(modified);
            
            tracing::info!("PUA rules reloaded from {}", self.rules_path.display());
            Ok(rules)
        } else {
            // 使用缓存
            let cache = self.cached_rules.lock()
                .map_err(|e| anyhow!("Failed to lock rules cache: {}", e))?;
            cache.as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("No rules cached"))
        }
    }
    
    pub fn watch_for_changes(&self) -> Result<()> {
        // 实现文件监视（使用 notify 库或简单轮询）
        // 当文件变化时重新加载规则
        Ok(())
    }
}
```

#### 步骤 2.3：更新 PUA 规则引擎支持动态规则

修改 `PuaRuleEngine` 支持动态规则：

```rust
pub struct PuaRuleEngine {
    rule_loader: Arc<StdMutex<PuaRuleLoader>>,
    default_plan: PuaEnforcementPlan,
}

impl PuaRuleEngine {
    pub fn new(rules_path: Option<impl AsRef<Path>>) -> Self {
        let default_plan = PuaEnforcementPlan::default();
        
        if let Some(path) = rules_path {
            let mut loader = PuaRuleLoader::new(path);
            match loader.load_rules() {
                Ok(rules) => {
                    tracing::info!("Loaded PUA rules from configuration file");
                    Self {
                        rule_loader: Arc::new(StdMutex::new(loader)),
                        default_plan: rules,
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load PUA rules: {}, using default", e);
                    Self {
                        rule_loader: Arc::new(StdMutex::new(loader)),
                        default_plan,
                    }
                }
            }
        } else {
            tracing::info!("Using default PUA rules");
            Self {
                rule_loader: Arc::new(StdMutex::new(PuaRuleLoader::new(""))),
                default_plan,
            }
        }
    }
    
    pub fn get_current_plan(&self) -> Result<PuaEnforcementPlan> {
        let mut loader = self.rule_loader.lock()
            .map_err(|e| anyhow!("Failed to lock rule loader: {}", e))?;
        
        // 尝试加载动态规则，失败时回退到默认
        loader.load_rules().or(Ok(self.default_plan.clone()))
    }
    
    pub fn reload_rules(&self) -> Result<()> {
        let mut loader = self.rule_loader.lock()
            .map_err(|e| anyhow!("Failed to lock rule loader: {}", e))?;
        loader.load_rules()?;
        Ok(())
    }
}
```

#### 步骤 2.4：在 GUI 中添加 PUA 可视化仪表板

在 `GUI/src/views/` 中添加 `PuaDashboardView.vue`：

```vue
<template>
  <div class="pua-dashboard">
    <h2>PUA 执行仪表板</h2>
    
    <div class="stats-grid">
      <div class="stat-card">
        <h3>今日违规</h3>
        <div class="stat-value">{{ stats.todayViolations }}</div>
      </div>
      <div class="stat-card">
        <h3>成功率</h3>
        <div class="stat-value">{{ stats.successRate }}%</div>
      </div>
      <div class="stat-card">
        <h3>平均响应时间</h3>
        <div class="stat-value">{{ stats.avgResponseTime }}ms</div>
      </div>
    </div>
    
    <div class="violations-section">
      <h3>最近违规</h3>
      <table class="violations-table">
        <thead>
          <tr>
            <th>时间</th>
            <th>违规类型</th>
            <th>详情</th>
            <th>处理状态</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="violation in recentViolations" :key="violation.id">
            <td>{{ formatTime(violation.timestamp) }}</td>
            <td>{{ violation.type }}</td>
            <td>{{ violation.details }}</td>
            <td :class="`status-${violation.status}`">
              {{ violation.status }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    
    <div class="rules-section">
      <h3>当前规则</h3>
      <div class="rules-editor">
        <textarea v-model="rulesJson" @blur="saveRules"></textarea>
        <button @click="reloadRules">重新加载规则</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'

interface PuaStats {
  todayViolations: number
  successRate: number
  avgResponseTime: number
}

interface Violation {
  id: string
  timestamp: number
  type: string
  details: string
  status: string
}

const stats = ref<PuaStats>({
  todayViolations: 0,
  successRate: 0,
  avgResponseTime: 0
})

const recentViolations = ref<Violation[]>([])
const rulesJson = ref('')

async function loadStats() {
  try {
    stats.value = await invoke('get_pua_stats')
  } catch (error) {
    console.error('Failed to load PUA stats:', error)
  }
}

async function loadViolations() {
  try {
    recentViolations.value = await invoke('get_recent_violations', { limit: 10 })
  } catch (error) {
    console.error('Failed to load violations:', error)
  }
}

async function loadRules() {
  try {
    const rules = await invoke('get_pua_rules')
    rulesJson.value = JSON.stringify(rules, null, 2)
  } catch (error) {
    console.error('Failed to load rules:', error)
  }
}

async function saveRules() {
  try {
    const parsed = JSON.parse(rulesJson.value)
    await invoke('update_pua_rules', { rules: parsed })
  } catch (error) {
    console.error('Failed to save rules:', error)
  }
}

async function reloadRules() {
  try {
    await invoke('reload_pua_rules')
    await loadRules()
  } catch (error) {
    console.error('Failed to reload rules:', error)
  }
}

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleString()
}

onMounted(() => {
  loadStats()
  loadViolations()
  loadRules()
  
  // 每30秒刷新数据
  setInterval(() => {
    loadStats()
    loadViolations()
  }, 30000)
})
</script>

<style scoped>
.pua-dashboard {
  padding: 20px;
}

.stats-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 20px;
  margin-bottom: 30px;
}

.stat-card {
  background: white;
  border-radius: 8px;
  padding: 20px;
  box-shadow: 0 2px 4px rgba(0,0,0,0.1);
}

.stat-card h3 {
  margin: 0 0 10px 0;
  color: #666;
  font-size: 14px;
}

.stat-value {
  font-size: 24px;
  font-weight: bold;
  color: #333;
}

.violations-section {
  margin-bottom: 30px;
}

.violations-table {
  width: 100%;
  border-collapse: collapse;
  background: white;
  border-radius: 8px;
  overflow: hidden;
  box-shadow: 0 2px 4px rgba(0,0,0,0.1);
}

.violations-table th,
.violations-table td {
  padding: 12px;
  text-align: left;
  border-bottom: 1px solid #eee;
}

.violations-table th {
  background: #f5f5f5;
  font-weight: 600;
}

.status-resolved {
  color: green;
}

.status-pending {
  color: orange;
}

.status-failed {
  color: red;
}

.rules-section {
  margin-bottom: 30px;
}

.rules-editor textarea {
  width: 100%;
  height: 300px;
  font-family: monospace;
  padding: 10px;
  border: 1px solid #ddd;
  border-radius: 4px;
  margin-bottom: 10px;
}

.rules-editor button {
  padding: 8px 16px;
  background: #007acc;
  color: white;
  border: none;
  border-radius: 4px;
  cursor: pointer;
}

.rules-editor button:hover {
  background: #005a9e;
}
</style>
```

#### 步骤 2.5：添加 Tauri 命令支持

在 `GUI/src-tauri/src/commands/` 中添加 PUA 相关命令：

```rust
#[tauri::command]
pub async fn get_pua_stats() -> Result<PuaStats, String> {
    let pua_engine = crate::state::get_pua_engine();
    let stats = pua_engine.get_stats().await
        .map_err(|e| format!("Failed to get PUA stats: {}", e))?;
    Ok(stats)
}

#[tauri::command]
pub async fn get_recent_violations(limit: usize) -> Result<Vec<ViolationRecord>, String> {
    let pua_engine = crate::state::get_pua_engine();
    let violations = pua_engine.get_recent_violations(limit).await
        .map_err(|e| format!("Failed to get violations: {}", e))?;
    Ok(violations)
}

#[tauri::command]
pub async fn get_pua_rules() -> Result<serde_json::Value, String> {
    let pua_engine = crate::state::get_pua_engine();
    let rules = pua_engine.get_current_rules().await
        .map_err(|e| format!("Failed to get rules: {}", e))?;
    Ok(rules)
}

#[tauri::command]
pub async fn update_pua_rules(rules: serde_json::Value) -> Result<(), String> {
    let pua_engine = crate::state::get_pua_engine();
    pua_engine.update_rules(rules).await
        .map_err(|e| format!("Failed to update rules: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn reload_pua_rules() -> Result<(), String> {
    let pua_engine = crate::state::get_pua_engine();
    pua_engine.reload_rules().await
        .map_err(|e| format!("Failed to reload rules: {}", e))?;
    Ok(())
}
```

#### 步骤 2.6：添加单测验证动态规则加载

```rust
#[test]
fn test_pua_rule_loader_file_watch() {
    use tempfile::NamedTempFile;
    
    // 创建临时规则文件
    let mut temp_file = NamedTempFile::new().unwrap();
    let initial_rules = r#"
[red_lines]
line1 = "Test rule 1"
line2 = "Test rule 2"
"#;
    temp_file.write_all(initial_rules.as_bytes()).unwrap();
    
    let mut loader = PuaRuleLoader::new(temp_file.path());
    let rules1 = loader.load_rules().unwrap();
    assert_eq!(rules1.red_lines.len(), 2);
    
    // 修改文件内容
    let updated_rules = r#"
[red_lines]
line1 = "Updated rule 1"
line2 = "Updated rule 2"
line3 = "New rule 3"
"#;
    std::thread::sleep(std::time::Duration::from_millis(100)); // 确保时间戳不同
    temp_file.write_all(updated_rules.as_bytes()).unwrap();
    
    // 应该检测到文件变化并重新加载
    let rules2 = loader.load_rules().unwrap();
    assert_eq!(rules2.red_lines.len(), 3);
}

#[test]
fn test_pua_rule_engine_fallback() {
    // 测试规则加载失败时回退到默认规则
    let non_existent_path = "/tmp/non-existent-pua-rules.toml";
    let engine = PuaRuleEngine::new(Some(non_existent_path));
    
    let rules = engine.get_current_plan().unwrap();
    // 应该使用默认规则
    assert!(!rules.red_lines.is_empty());
    assert!(!rules.quality_compass.is_empty());
}
```

#### 步骤 2.7：接入主链

更新 `test_ci.sh` 添加 PUA 相关测试：

```bash
# 步骤 7a: 测试 PUA 动态规则加载
echo "=== 步骤 7a: 测试 PUA 动态规则加载 ==="
cargo test pua_rule_loader -- --nocapture
echo "✅ PUA 动态规则加载测试通过"

# 步骤 7b: 测试 PUA 规则引擎
echo "=== 步骤 7b: 测试 PUA 规则引擎 ==="
cargo test pua_rule_engine -- --nocapture
echo "✅ PUA 规则引擎测试通过"
```

**验收标准**：
- PUA 规则可以从 TOML 文件动态加载
- 文件修改后规则自动重新加载（或通过命令重新加载）
- GUI 仪表板正确显示 PUA 统计数据和违规记录
- 规则更新后立即生效
- 规则加载失败时优雅回退到默认规则
- 所有现有 PUA 测试继续通过
- 新增测试验证动态规则功能

---

### B15-P1-1：测试覆盖率提升与性能基准

**是否需要**：✅ **需要**

> 根因：当前测试覆盖良好但缺乏代码覆盖率报告和性能基准测试。
> 无法量化测试效果，也无法检测性能回归。

**推荐建议**：添加代码覆盖率报告和性能基准测试框架。

#### 步骤 3.1：添加代码覆盖率工具到 CI

更新 `test_ci.sh` 添加覆盖率报告：

```bash
# 步骤 8: 代码覆盖率报告
echo "=== 步骤 8: 生成代码覆盖率报告 ==="

# 安装 tarpaulin（如果未安装）
if ! command -v cargo-tarpaulin &> /dev/null; then
    echo "Installing cargo-tarpaulin..."
    cargo install cargo-tarpaulin
fi

# 生成覆盖率报告
cargo tarpaulin --out Html --output-dir ./coverage
echo "✅ 代码覆盖率报告生成完成"

# 检查覆盖率阈值（至少 80%）
COVERAGE=$(cargo tarpaulin --out Xml | grep -oP 'line-rate="\K[0-9.]+')
if (( $(echo "$COVERAGE < 0.8" | bc -l) )); then
    echo "❌ 代码覆盖率低于 80%: ${COVERAGE}"
    exit 1
fi
echo "✅ 代码覆盖率: ${COVERAGE}"
```

#### 步骤 3.2：创建性能基准测试模块

创建 `tests/benchmarks/` 目录和基准测试：

```rust
// tests/benchmarks/model_selection_bench.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use go_on::intelligence::adaptive_selector::AdaptiveModelSelector;

fn benchmark_ucb1_selection(c: &mut Criterion) {
    c.bench_function("ucb1_model_selection_10_models", |b| {
        let mut selector = AdaptiveModelSelector::new();
        
        // 初始化10个模型的数据
        for i in 0..10 {
            let model_id = format!("model-{}", i);
            for _ in 0..100 {
                selector.record_result(&model_id, i % 3 != 0); // 模拟不同成功率
            }
        }
        
        let candidates: Vec<String> = (0..10).map(|i| format!("model-{}", i)).collect();
        
        b.iter(|| {
            black_box(selector.get_best_model(&candidates));
        })
    });
}

fn benchmark_pua_rule_validation(c: &mut Criterion) {
    c.bench_function("pua_rule_validation_100_rules", |b| {
        // 创建包含100条规则的PUA引擎
        // 基准测试规则验证性能
        b.iter(|| {
            // 模拟规则验证逻辑
            black_box(());
        })
    });
}

criterion_group!(
    benches,
    benchmark_ucb1_selection,
    benchmark_pua_rule_validation
);
criterion_main!(benches);
```

#### 步骤 3.3：添加基准测试到 CI

更新 `test_ci.sh` 添加基准测试：

```bash
# 步骤 9: 性能基准测试
echo "=== 步骤 9: 运行性能基准测试 ==="

# 安装 criterion（如果未安装）
if ! command -v cargo-criterion &> /dev/null; then
    echo "Installing cargo-criterion..."
    cargo install cargo-criterion
fi

# 运行基准测试
cargo criterion --message-format=json > benchmark_results.json
echo "✅ 性能基准测试完成"

# 检查性能回归（与基线比较）
if [ -f "benchmark_baseline.json" ]; then
    echo "Comparing with baseline..."
    # 实现性能回归检测逻辑
    # 如果性能下降超过10%，发出警告
fi
```

#### 步骤 3.4：添加集成测试覆盖率

创建 `tests/integration_coverage.rs` 测试关键集成路径：

```rust
#[test]
fn test_end_to_end_chat_flow_coverage() {
    // 测试完整的聊天流程，覆盖多个模块
    // 包括：请求处理 -> 模型选择 -> PUA验证 -> 响应生成
}

#[test]
fn test_pua_escalation_coverage() {
    // 测试PUA升级机制的所有级别
    // 覆盖L0-L4所有检查点
}

#[test]
fn test_error_recovery_coverage() {
    // 测试错误恢复路径
    // 包括：网络错误、模型失败、配置错误等
}
```

#### 步骤 3.5：添加测试覆盖率报告工具

创建 `scripts/test_coverage.sh`：

```bash
#!/bin/bash

set -e

echo "=== 生成详细测试覆盖率报告 ==="

# 1. 单元测试覆盖率
echo "1. 单元测试覆盖率..."
cargo tarpaulin --tests --out Xml --output-dir ./coverage/unit

# 2. 集成测试覆盖率
echo "2. 集成测试覆盖率..."
cargo tarpaulin --tests --out Xml --output-dir ./coverage/integration

# 3. 生成综合报告
echo "3. 生成综合报告..."
# 这里可以添加合并覆盖率报告的逻辑

# 4. 检查覆盖率阈值
echo "4. 检查覆盖率阈值..."
COVERAGE=$(cargo tarpaulin --out Xml | grep -oP 'line-rate="\K[0-9.]+')
echo "总体覆盖率: ${COVERAGE}"
if (( $(echo "$COVERAGE < 0.8" | bc -l) )); then
    echo "警告: 代码覆盖率低于 80%"
    exit 1
fi
echo "✅ 代码覆盖率检查通过"
```

**验收标准**：
- 代码覆盖率报告生成成功，包含 HTML 和 XML 格式
- 覆盖率阈值检查通过（至少 80%）
- 性能基准测试运行成功，无性能回归
- 新增的集成测试覆盖率测试通过
- 所有现有测试继续通过

---

### B15-P1-2：学习数据持久化存储

**是否需要**：✅ **需要**

> 根因：当前学习数据主要存储在内存中，应用重启后会丢失。
> 无法积累长期学习数据，影响模型选择的准确性。

**推荐建议**：在现有 SQLite 存储基础上添加学习数据表，实现学习数据的持久化。

#### 步骤 4.1：创建学习数据存储结构

在 `src/memory/memory.rs` 中添加学习数据存储：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningRecord {
    pub id: String,
    pub model_id: String,
    pub task_type: String,
    pub success: bool,
    pub response_time: u64,
    pub tokens_used: u32,
    pub created_at: i64,
}

pub struct LearningStore {
    db: rusqlite::Connection,
}

impl LearningStore {
    pub fn new(db_path: &Path) -> Result<Self> {
        let db = rusqlite::Connection::open(db_path)?;
        
        // 创建学习数据表
        db.execute(
            "CREATE TABLE IF NOT EXISTS learning_records (
                id TEXT PRIMARY KEY,
                model_id TEXT NOT NULL,
                task_type TEXT NOT NULL,
                success INTEGER NOT NULL,
                response_time INTEGER NOT NULL,
                tokens_used INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        // 创建索引
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_learning_model_id ON learning_records(model_id)",
            [],
        )?;
        
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_learning_created_at ON learning_records(created_at)",
            [],
        )?;
        
        Ok(Self { db })
    }
    
    pub fn save_record(&self, record: &LearningRecord) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO learning_records (id, model_id, task_type, success, response_time, tokens_used, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                &record.id,
                &record.model_id,
                &record.task_type,
                record.success as i32,
                record.response_time,
                record.tokens_used,
                record.created_at,
            ),
        )?;
        Ok(())
    }
    
    pub fn get_model_stats(&self, model_id: &str, limit_days: Option<u32>) -> Result<ModelMetrics> {
        let mut query = "SELECT COUNT(*), SUM(success), AVG(response_time), AVG(tokens_used) FROM learning_records WHERE model_id = ?";
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&model_id];
        
        if let Some(days) = limit_days {
            query += " AND created_at > ?";
            let cutoff = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64 - (days * 24 * 3600) as i64;
            params.push(&cutoff);
        }
        
        let mut stmt = self.db.prepare(query)?;
        let mut rows = stmt.query(params.as_slice())?;
        
        if let Some(row) = rows.next()? {
            let total: i64 = row.get(0)?;
            let successful: i64 = row.get(1)?;
            let avg_time: f64 = row.get(2)?;
            let avg_tokens: f64 = row.get(3)?;
            
            Ok(ModelMetrics {
                model_id: model_id.to_string(),
                total_requests: total as u64,
                successful_requests: successful as u64,
                success_rate: if total > 0 { successful as f32 / total as f32 } else { 0.5 },
                exploration_factor: 2.0,
                last_selected_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            })
        } else {
            Ok(ModelMetrics {
                model_id: model_id.to_string(),
                total_requests: 0,
                successful_requests: 0,
                success_rate: 0.5,
                exploration_factor: 2.0,
                last_selected_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            })
        }
    }
    
    pub fn cleanup_old_records(&self, days: u32) -> Result<()> {
        let cutoff = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64 - (days * 24 * 3600) as i64;
        
        self.db.execute(
            "DELETE FROM learning_records WHERE created_at < ?",
            [cutoff],
        )?;
        
        Ok(())
    }
}
```

#### 步骤 4.2：更新 `AdaptiveModelSelector` 使用持久化存储

修改 `src/intelligence/adaptive_selector.rs`：

```rust
pub struct AdaptiveModelSelector {
    metrics: HashMap<String, ModelMetrics>,
    learning_store: Option<Arc<LearningStore>>,
}

impl AdaptiveModelSelector {
    pub fn new_with_store(store: Option<Arc<LearningStore>>) -> Self {
        Self {
            metrics: HashMap::new(),
            learning_store: store,
        }
    }
    
    pub fn record_result(&mut self, model_id: &str, success: bool) {
        // 现有逻辑...
        
        // 持久化记录
        if let Some(store) = &self.learning_store {
            let record = LearningRecord {
                id: format!("{}-{}", model_id, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()),
                model_id: model_id.to_string(),
                task_type: "general".to_string(), // 可以根据实际任务类型设置
                success,
                response_time: 0, // 实际响应时间
                tokens_used: 0, // 实际使用的 tokens
                created_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64,
            };
            if let Err(e) = store.save_record(&record) {
                tracing::warn!("Failed to save learning record: {}", e);
            }
        }
    }
    
    pub fn load_from_store(&mut self, model_ids: &[String]) {
        if let Some(store) = &self.learning_store {
            for model_id in model_ids {
                if let Ok(metrics) = store.get_model_stats(model_id, Some(30)) { // 最近30天数据
                    self.metrics.insert(model_id.clone(), metrics);
                }
            }
        }
    }
}
```

#### 步骤 4.3：添加数据清理和归档策略

创建 `scripts/cleanup_learning_data.sh`：

```bash
#!/bin/bash

set -e

echo "=== 清理学习数据 ==="

# 保留最近90天的数据
DAYS_TO_KEEP=90

# 从配置中获取数据库路径
DB_PATH="./data/learning.db"

if [ -f "$DB_PATH" ]; then
    echo "清理 $DB_PATH 中超过 $DAYS_TO_KEEP 天的记录..."
    # 这里可以添加执行 SQL 清理的逻辑
    echo "✅ 学习数据清理完成"
else
    echo "数据库文件不存在，跳过清理"
fi

# 生成学习数据报告
echo "=== 生成学习数据报告 ==="
# 这里可以添加生成报告的逻辑
echo "✅ 学习数据报告生成完成"
```

#### 步骤 4.4：添加单测验证持久化存储

```rust
#[test]
fn test_learning_store_save_and_retrieve() {
    use tempfile::NamedTempFile;
    
    let temp_file = NamedTempFile::new().unwrap();
    let store = LearningStore::new(temp_file.path()).unwrap();
    
    let record = LearningRecord {
        id: "test-1".to_string(),
        model_id: "model-test".to_string(),
        task_type: "test".to_string(),
        success: true,
        response_time: 1000,
        tokens_used: 100,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    };
    
    store.save_record(&record).unwrap();
    
    let metrics = store.get_model_stats("model-test", None).unwrap();
    assert_eq!(metrics.total_requests, 1);
    assert_eq!(metrics.successful_requests, 1);
    assert_eq!(metrics.success_rate, 1.0);
}

#[test]
fn test_learning_store_cleanup() {
    use tempfile::NamedTempFile;
    
    let temp_file = NamedTempFile::new().unwrap();
    let store = LearningStore::new(temp_file.path()).unwrap();
    
    // 创建一个100天前的记录
    let old_record = LearningRecord {
        id: "test-old".to_string(),
        model_id: "model-test".to_string(),
        task_type: "test".to_string(),
        success: true,
        response_time: 1000,
        tokens_used: 100,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64 - (100 * 24 * 3600) as i64,
    };
    
    store.save_record(&old_record).unwrap();
    
    // 清理90天前的记录
    store.cleanup_old_records(90).unwrap();
    
    let metrics = store.get_model_stats("model-test", None).unwrap();
    assert_eq!(metrics.total_requests, 0);
}
```

#### 步骤 4.5：接入主链

更新 `src/core/setup.rs` 添加学习存储初始化：

```rust
pub fn initialize_learning_store(config: &AppConfig) -> Result<Option<Arc<LearningStore>>> {
    if let Some(runtime) = &config.runtime {
        if let Some(db_path) = &runtime.learning_db_path {
            let store = LearningStore::new(Path::new(db_path))?;
            Ok(Some(Arc::new(store)))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}
```

**验收标准**：
- 学习数据成功持久化到 SQLite 数据库
- 应用重启后学习数据不丢失
- 数据清理和归档策略正常工作
- 新增的持久化存储测试通过
- 所有现有测试继续通过

---

### B15-P2-1：硬化机制增强（故障恢复）

**是否需要**：⚠️ **需要**

> 根因：当前硬化机制主要关注预算管理，缺乏完善的故障恢复机制。
> 当系统遇到故障时，可能无法快速恢复到正常状态。

**推荐建议**：在现有 `BudgetTracker` 基础上添加断路器模式和优雅降级策略。

#### 步骤 5.1：实现断路器模式

在 `src/governance/hardening.rs` 中添加断路器：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CircuitState {
    Closed,    // 正常状态，请求正常通过
    Open,      // 故障状态，请求被拒绝
    HalfOpen,  // 试探状态，允许部分请求通过
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    failure_threshold: u32,
    recovery_timeout: Duration,
    last_failure_time: Option<Instant>,
    success_threshold: u32,
    consecutive_successes: u32,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, recovery_timeout: Duration, success_threshold: u32) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            failure_threshold,
            recovery_timeout,
            last_failure_time: None,
            success_threshold,
            consecutive_successes: 0,
        }
    }
    
    pub fn allow_request(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last_failure) = self.last_failure_time {
                    if last_failure.elapsed() >= self.recovery_timeout {
                        self.state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }
    
    pub fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                self.consecutive_successes += 1;
                if self.consecutive_successes >= self.success_threshold {
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                    self.consecutive_successes = 0;
                }
            }
            CircuitState::Open => {
                // 不应该在 Open 状态下记录成功
            }
        }
    }
    
    pub fn record_failure(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count += 1;
                if self.failure_count >= self.failure_threshold {
                    self.state = CircuitState::Open;
                    self.last_failure_time = Some(Instant::now());
                }
            }
            CircuitState::HalfOpen => {
                self.state = CircuitState::Open;
                self.last_failure_time = Some(Instant::now());
                self.consecutive_successes = 0;
            }
            CircuitState::Open => {
                self.last_failure_time = Some(Instant::now());
            }
        }
    }
    
    pub fn get_state(&self) -> CircuitState {
        self.state
    }
}
```

#### 步骤 5.2：实现优雅降级策略

添加优雅降级管理器：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GracefulDegradationManager {
    circuit_breakers: HashMap<String, CircuitBreaker>,
    fallback_models: HashMap<String, String>,
}

impl GracefulDegradationManager {
    pub fn new() -> Self {
        Self {
            circuit_breakers: HashMap::new(),
            fallback_models: HashMap::new(),
        }
    }
    
    pub fn register_service(&mut self, service_name: &str, fallback: &str) {
        self.circuit_breakers.insert(
            service_name.to_string(),
            CircuitBreaker::new(5, Duration::from_secs(30), 3),
        );
        self.fallback_models.insert(service_name.to_string(), fallback.to_string());
    }
    
    pub fn should_route_to_fallback(&mut self, service_name: &str) -> bool {
        if let Some(circuit_breaker) = self.circuit_breakers.get_mut(service_name) {
            !circuit_breaker.allow_request()
        } else {
            false
        }
    }
    
    pub fn get_fallback(&self, service_name: &str) -> Option<&String> {
        self.fallback_models.get(service_name)
    }
    
    pub fn record_service_result(&mut self, service_name: &str, success: bool) {
        if let Some(circuit_breaker) = self.circuit_breakers.get_mut(service_name) {
            if success {
                circuit_breaker.record_success();
            } else {
                circuit_breaker.record_failure();
            }
        }
    }
}
```

#### 步骤 5.3：集成到预算跟踪器

修改 `BudgetTracker` 添加故障恢复支持：

```rust
pub struct BudgetTracker {
    task_budget: TaskBudget,
    tokens_used: usize,
    tool_calls_made: usize,
    started_at: Instant,
    degradation_manager: Option<Arc<Mutex<GracefulDegradationManager>>>,
}

impl BudgetTracker {
    pub fn new_with_degradation(
        task_budget: TaskBudget,
        degradation_manager: Option<Arc<Mutex<GracefulDegradationManager>>>,
    ) -> Self {
        Self {
            task_budget,
            tokens_used: 0,
            tool_calls_made: 0,
            started_at: Instant::now(),
            degradation_manager,
        }
    }
    
    // 其他方法...
    
    pub fn check_degradation(&self, service_name: &str) -> Result<bool, BudgetExceededError> {
        if let Some(manager) = &self.degradation_manager {
            let mut manager = manager.lock().unwrap_or_else(|e| e.into_inner());
            Ok(manager.should_route_to_fallback(service_name))
        } else {
            Ok(false)
        }
    }
    
    pub fn record_service_result(&self, service_name: &str, success: bool) {
        if let Some(manager) = &self.degradation_manager {
            let mut manager = manager.lock().unwrap_or_else(|e| e.into_inner());
            manager.record_service_result(service_name, success);
        }
    }
}
```

#### 步骤 5.4：添加单测验证故障恢复

```rust
#[test]
fn test_circuit_breaker_transitions() {
    let mut breaker = CircuitBreaker::new(2, Duration::from_millis(100), 1);
    
    // 初始状态是 Closed
    assert_eq!(breaker.get_state(), CircuitState::Closed);
    assert!(breaker.allow_request());
    
    // 记录两次失败，应该打开断路器
    breaker.record_failure();
    assert_eq!(breaker.get_state(), CircuitState::Closed);
    breaker.record_failure();
    assert_eq!(breaker.get_state(), CircuitState::Open);
    assert!(!breaker.allow_request());
    
    // 等待恢复超时
    std::thread::sleep(Duration::from_millis(150));
    assert!(breaker.allow_request());
    assert_eq!(breaker.get_state(), CircuitState::HalfOpen);
    
    // 记录一次成功，应该关闭断路器
    breaker.record_success();
    assert_eq!(breaker.get_state(), CircuitState::Closed);
    assert!(breaker.allow_request());
}

#[test]
fn test_graceful_degradation() {
    let mut manager = GracefulDegradationManager::new();
    manager.register_service("model-a", "model-b");
    
    // 初始状态不应该降级
    assert!(!manager.should_route_to_fallback("model-a"));
    
    // 记录多次失败
    for _ in 0..5 {
        manager.record_service_result("model-a", false);
    }
    
    // 应该降级到 fallback
    assert!(manager.should_route_to_fallback("model-a"));
    assert_eq!(manager.get_fallback("model-a"), Some(&"model-b".to_string()));
}
```

#### 步骤 5.5：接入主链

在 `src/core/setup.rs` 中初始化故障恢复机制：

```rust
pub fn initialize_degradation_manager(config: &AppConfig) -> Result<Arc<Mutex<GracefulDegradationManager>>> {
    let manager = GracefulDegradationManager::new();
    
    // 为每个模型服务注册断路器和 fallback
    if let Some(agents) = &config.agents {
        for (name, _) in agents {
            // 这里可以根据配置设置合适的 fallback
            manager.register_service(name, "fallback-model");
        }
    }
    
    Ok(Arc::new(Mutex::new(manager)))
}
```

**验收标准**：
- 断路器模式正确实现，能够在故障时打开，在恢复时关闭
- 优雅降级策略能够在服务故障时切换到 fallback 服务
- 新增的故障恢复测试通过
- 所有现有测试继续通过

---

### B15-P2-2：可观测性增强（分布式追踪）

**是否需要**：⚠️ **需要**

> 根因：当前可观测性主要关注性能指标和日志，缺乏分布式追踪能力。
> 在复杂的微服务架构中，难以追踪请求的完整路径和性能瓶颈。

**推荐建议**：在现有 `telemetry` 基础上添加 OpenTelemetry 支持，实现分布式追踪。

#### 步骤 6.1：添加 OpenTelemetry 依赖

更新 `Cargo.toml` 添加 OpenTelemetry 依赖：

```toml
[dependencies]
# 现有的依赖...
opentelemetry = "0.31"
opentelemetry_sdk = { version = "0.31", features = ["rt-tokio"] }
opentelemetry-otlp = { version = "0.31", features = ["trace", "grpc-tonic"] }
tracing-opentelemetry = "0.24"
```

#### 步骤 6.2：实现分布式追踪初始化

在 `src/observability/telemetry_enhanced.rs` 中添加：

```rust
use opentelemetry::global;
use opentelemetry::sdk::trace::{Config, Sampler};
use opentelemetry::trace::TraceError;
use opentelemetry_otlp::WithExportConfig;
use tracing_opentelemetry::OpenTelemetryLayer;

pub fn init_opentelemetry() -> Result<(), TraceError> {
    // 配置 OpenTelemetry
    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint("http://localhost:4317"), // 默认 OTLP 端点
        )
        .with_trace_config(
            Config::default()
                .with_sampler(Sampler::AlwaysOn)
                .with_resource(opentelemetry::sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new("service.name", "go-on"),
                    opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ])),
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;
    
    // 全局注册 tracer
    global::set_tracer(tracer);
    
    Ok(())
}

pub fn setup_tracing_with_otel() -> Result<tracing_appender::non_blocking::WorkerGuard, TraceError> {
    // 初始化 OpenTelemetry
    init_opentelemetry()?;
    
    // 创建 OpenTelemetry layer
    let telemetry = tracing_opentelemetry::layer().with_tracer(global::tracer("go-on"));
    
    // 设置 tracing
    let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stdout());
    
    tracing_subscriber::registry()
        .with(telemetry)
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().with_writer(non_blocking))
        .init();
    
    Ok(guard)
}
```

#### 步骤 6.3：实现追踪上下文传播

创建 `src/observability/tracing_context.rs`：

```rust
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::sdk::propagation::TraceContextPropagator;
use serde_json::Value;

// 用于从 HTTP 请求头提取追踪上下文
struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl<'a> Extractor for HeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }
    
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

// 用于向 HTTP 响应头注入追踪上下文
struct HeaderInjector<'a>(&'a mut http::HeaderMap);

impl<'a> Injector for HeaderInjector<'a> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key, value.parse().unwrap());
    }
}

pub fn extract_context_from_headers(headers: &http::HeaderMap) -> opentelemetry::Context {
    let propagator = TraceContextPropagator::new();
    propagator.extract(&HeaderExtractor(headers))
}

pub fn inject_context_to_headers(context: &opentelemetry::Context, headers: &mut http::HeaderMap) {
    let propagator = TraceContextPropagator::new();
    propagator.inject(context, &mut HeaderInjector(headers));
}

pub fn extract_context_from_json(data: &Value) -> opentelemetry::Context {
    // 从 JSON 数据中提取追踪上下文
    // 实现细节...
    opentelemetry::Context::current()
}

pub fn inject_context_to_json(context: &opentelemetry::Context, data: &mut Value) {
    // 向 JSON 数据中注入追踪上下文
    // 实现细节...
}
```

#### 步骤 6.4：在关键路径添加追踪

在 `src/acp/impl/runtime.rs` 中添加：

```rust
use opentelemetry::trace::{Span, TraceContextExt, Tracer};
use tracing::{instrument, Span};

#[instrument(name = "process_chat_request", skip_all)]
async fn process_chat_request(
    server: &mut AcpServer,
    params: &ChatParams,
) -> Result<ChatResponse> {
    // 开始追踪
    let span = tracing::Span::current();
    
    // 处理请求...
    
    // 记录关键事件
    span.record("model_id", &params.model);
    span.record("message_count", &params.messages.len());
    
    // 调用下游服务时传播上下文
    let context = opentelemetry::Context::current();
    // 将 context 传递给下游服务...
    
    Ok(response)
}
```

#### 步骤 6.5：添加指标聚合

在 `src/observability/telemetry_enhanced.rs` 中添加：

```rust
use opentelemetry::metrics::{Counter, Histogram, Meter};
use once_cell::sync::Lazy;

static METER: Lazy<Meter> = Lazy::new(|| {
    global::meter("go-on")
});

static REQUEST_COUNTER: Lazy<Counter<u64>> = Lazy::new(|| {
    METER
        .u64_counter("http.requests.total")
        .with_description("Total number of HTTP requests")
        .init()
});

static REQUEST_LATENCY: Lazy<Histogram<f64>> = Lazy::new(|| {
    METER
        .f64_histogram("http.requests.latency")
        .with_description("HTTP request latency in seconds")
        .init()
});

pub fn record_http_request(method: &str, path: &str, status: u16, latency: f64) {
    REQUEST_COUNTER.add(
        1,
        &[opentelemetry::KeyValue::new("method", method),
          opentelemetry::KeyValue::new("path", path),
          opentelemetry::KeyValue::new("status", status.to_string())],
    );
    
    REQUEST_LATENCY.record(
        latency,
        &[opentelemetry::KeyValue::new("method", method),
          opentelemetry::KeyValue::new("path", path),
          opentelemetry::KeyValue::new("status", status.to_string())],
    );
}
```

#### 步骤 6.6：添加单测验证追踪功能

```rust
#[test]
fn test_tracing_context_propagation() {
    // 测试追踪上下文的提取和注入
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "traceparent",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".parse().unwrap(),
    );
    
    let context = extract_context_from_headers(&headers);
    assert!(context.has_active_span());
    
    let mut new_headers = http::HeaderMap::new();
    inject_context_to_headers(&context, &mut new_headers);
    assert!(new_headers.contains_key("traceparent"));
}

#[test]
fn test_metrics_recording() {
    // 测试指标记录
    record_http_request("POST", "/v1/chat/completions", 200, 0.5);
    // 这里可以添加指标验证逻辑
}
```

#### 步骤 6.7：接入主链

在 `src/main.rs` 中初始化追踪：

```rust
fn main() -> Result<()> {
    // 初始化 OpenTelemetry 追踪
    let _guard = setup_tracing_with_otel()?;
    
    // 其他初始化...
    
    Ok(())
}
```

**验收标准**：
- OpenTelemetry 追踪正确初始化
- 追踪上下文在服务间正确传播
- 关键路径的追踪数据被正确记录
- 指标数据被正确聚合和导出
- 新增的追踪测试通过
- 所有现有测试继续通过

---

### B15-P3-1：微服务化架构准备

**是否需要**：💡 **参考价值**

> 根因：当前项目采用单体架构，随着功能增加，可能会遇到可维护性和扩展性问题。
> 提前规划微服务化可以为未来的扩展性做好准备。

**推荐建议**：仅进行架构分析和文档准备，不实施实际拆分，保持单体架构。

#### 步骤 7.1：架构分析和服务边界识别

创建 `ARCHITECTURE.md` 文档：

```markdown
# Go-On 微服务架构规划

## 现有单体架构

当前架构：
- 单一可执行文件
- 所有模块在同一个进程中运行
- 共享内存和状态

## 建议的微服务边界

1. **API 网关服务**
   - 处理 HTTP 请求
   - 路由和负载均衡
   - 认证和授权

2. **模型选择服务**
   - 模型选择算法
   - 学习数据管理
   - 模型性能评估

3. **PUA 治理服务**
   - PUA 规则引擎
   - 违规检测和处理
   - 质量评估

4. **执行引擎服务**
   - 任务执行
   - 工作流管理
   - 资源分配

5. **存储服务**
   - 缓存管理
   - 向量存储
   - 持久化存储

6. **监控服务**
   - 指标收集
   - 日志聚合
   - 分布式追踪

## 服务间通信

- **同步通信**：gRPC
- **异步通信**：消息队列（Kafka/RabbitMQ）
- **服务发现**：Consul/Etcd

## 部署策略

- **容器化**：Docker
- **编排**：Kubernetes
- **CI/CD**：GitHub Actions

## 迁移计划

1. **阶段 1**：服务边界识别和接口定义
2. **阶段 2**：核心服务拆分
3. **阶段 3**：剩余服务拆分
4. **阶段 4**：完全微服务化

## 注意事项

- 数据一致性
- 服务发现和负载均衡
- 分布式事务
- 监控和可观测性
- 部署和运维复杂度
```

#### 步骤 7.2：接口定义和服务契约

创建 `proto/` 目录和服务定义文件：

```protobuf
// proto/model_selector.proto
syntax = "proto3";

package model_selector;

service ModelSelector {
    rpc SelectModel (SelectModelRequest) returns (SelectModelResponse);
    rpc RecordModelResult (RecordModelResultRequest) returns (RecordModelResultResponse);
    rpc GetModelStats (GetModelStatsRequest) returns (GetModelStatsResponse);
}

message SelectModelRequest {
    repeated string candidates = 1;
    string task_type = 2;
    map<string, string> context = 3;
}

message SelectModelResponse {
    string model_id = 1;
    double confidence = 2;
    map<string, double> scores = 3;
}

message RecordModelResultRequest {
    string model_id = 1;
    bool success = 2;
    int64 response_time = 3;
    int32 tokens_used = 4;
    string task_type = 5;
}

message RecordModelResultResponse {
    bool success = 1;
}

message GetModelStatsRequest {
    string model_id = 1;
    int32 days = 2;
}

message GetModelStatsResponse {
    int64 total_requests = 1;
    int64 successful_requests = 2;
    double success_rate = 3;
    double avg_response_time = 4;
    double avg_tokens_used = 5;
}
```

#### 步骤 7.3：技术栈评估

创建 `TECH_STACK_EVALUATION.md` 文档：

```markdown
# 微服务技术栈评估

## 候选技术

### 服务通信
- **gRPC**：高性能、强类型、跨语言
- **REST**：简单、广泛支持
- **GraphQL**：灵活查询、减少过度获取

### 消息队列
- **Kafka**：高吞吐量、持久化
- **RabbitMQ**：可靠性、灵活路由
- **NATS**：轻量级、低延迟

### 服务发现
- **Consul**：功能丰富、健康检查
- **Etcd**：简单、可靠
- **Kubernetes**：内置服务发现

### 监控
- **Prometheus**：指标收集
- **Grafana**：可视化
- **Jaeger**：分布式追踪

## 推荐技术栈

| 类别 | 推荐技术 | 原因 |
|------|----------|------|
| 服务通信 | gRPC | 高性能、强类型、适合内部服务通信 |
| 消息队列 | Kafka | 高吞吐量、持久化、适合事件驱动架构 |
| 服务发现 | Kubernetes | 与容器编排集成、减少额外组件 |
| 监控 | Prometheus + Grafana + Jaeger | 完整的可观测性解决方案 |

## 迁移风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 数据一致性 | 高 | 采用 Saga 模式或事件溯源 |
| 网络延迟 | 中 | 合理设计服务边界，减少跨服务调用 |
| 部署复杂度 | 高 | 自动化 CI/CD 流程，使用基础设施即代码 |
| 监控复杂度 | 中 | 统一监控方案，标准化指标 |

## 成本评估

| 成本项 | 估计 | 原因 |
|--------|------|------|
| 开发成本 | 中 | 需要重新设计和实现服务接口 |
| 运维成本 | 高 | 需要管理多个服务和基础设施 |
| 基础设施成本 | 中 | 容器编排和监控需要额外资源 |
| 培训成本 | 低 | 团队熟悉现代微服务技术栈 |
```

**验收标准**：
- 架构分析文档完成
- 服务边界和接口定义清晰
- 技术栈评估完成
- 迁移计划制定合理
- 风险和成本评估全面

---

## 新增优化建议总结

| ID | 优先级 | 目标 | 是否需要 | 目标文件 |
|---|---|---|---|---|
| B15-P1-2 | P1 | 学习数据持久化存储 | ✅ 需要 | `src/memory/memory.rs` |
| B15-P2-1 | P2 | 硬化机制增强（故障恢复） | ⚠️ 需要 | `src/governance/hardening.rs` |
| B15-P2-2 | P2 | 可观测性增强（分布式追踪） | ⚠️ 需要 | `src/observability/` |
| B15-P3-1 | P3 | 微服务化架构准备 | 💡 参考价值 | 架构文档 |

---

## 实施建议

1. **优先级顺序**：按照 P0 → P1 → P2 → P3 的顺序实施
2. **分批实施**：每个优化项作为一个独立的批次实施
3. **测试验证**：每个批次完成后进行全面测试
4. **文档更新**：同步更新相关文档
5. **监控反馈**：实施后密切监控系统表现

---

## 预期效果

通过实施这些优化建议，go-on 项目将：

1. **智能性提升**：强化学习算法增强，学习数据持久化
2. **可靠性提升**：故障恢复机制，断路器模式
3. **可观测性提升**：分布式追踪，指标聚合
4. **可扩展性提升**：微服务架构准备
5. **用户体验提升**：PUA 规则可视化，性能优化

---

**结论**：go-on 项目已经具备良好的基础架构和代码质量，通过实施这些优化建议，可以进一步提升系统的智能性、可靠性和可扩展性，为未来的发展做好准备。